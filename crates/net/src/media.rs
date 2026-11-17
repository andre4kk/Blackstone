//! Ciclo di rete WebRTC: una sola socket UDP per tutti i giocatori, smistata da str0m (sans-I/O).
use crate::{Command, Event, Shared};
use std::{
    net::SocketAddr,
    sync::{atomic::Ordering::Relaxed, Arc},
    time::{Duration, Instant},
};
use str0m::{
    channel::ChannelId,
    net::{Protocol, Receive},
    Event as RtcEvent, IceConnectionState, Input, Output, Rtc,
};
use tokio::{
    net::UdpSocket,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    time::{sleep_until, Instant as TInstant},
};

const RELIABLE: &str = "ReliableDataChannel";
const FRAGMENT: usize = 10_000; // dimensione massima di un frammento (header di 1 byte escluso)
/// Messaggio più grande ammesso: l'header di un byte conta al massimo 256 frammenti.
const MAX_MESSAGE: usize = 256 * FRAGMENT;

/// Il messaggio in arrivo supera `MAX_MESSAGE`: il client sta mandando dati non validi.
#[derive(Debug, PartialEq)]
struct TooLarge;

/// Riassembla i frammenti del data channel. Header di 1 byte: N>0 = ne mancano ancora N; 0 = ultimo (o unico).
#[derive(Default)]
struct Reassembler(Vec<u8>);

impl Reassembler {
    /// Aggiunge un frammento; ritorna il messaggio quando è completo. Oltre `MAX_MESSAGE` scarta tutto.
    fn push(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>, TooLarge> {
        let Some((&header, payload)) = data.split_first() else { return Ok(None) };
        if self.0.len() + payload.len() > MAX_MESSAGE {
            self.0 = Vec::new();
            return Err(TooLarge);
        }
        self.0.extend_from_slice(payload);
        Ok((header == 0).then(|| std::mem::take(&mut self.0)))
    }
}

/// Una connessione WebRTC (un giocatore) con il suo data channel affidabile.
pub(crate) struct Client {
    id: u64,
    rtc: Rtc,
    addr: SocketAddr,
    rel: Option<ChannelId>,
    opened: bool,
    frag: Reassembler,
    next: Instant,
}

impl Client {
    pub fn new(id: u64, rtc: Rtc, addr: SocketAddr) -> Self {
        Self { id, rtc, addr, rel: None, opened: false, frag: Reassembler::default(), next: Instant::now() }
    }

    /// Svuota gli output di str0m (pacchetti UDP da inviare, eventi) e memorizza la prossima scadenza.
    async fn drive(&mut self, sock: &UdpSocket, events: &UnboundedSender<Event>) {
        loop {
            match self.rtc.poll_output() {
                Ok(Output::Timeout(t)) => {
                    self.next = t;
                    return;
                }
                Ok(Output::Transmit(t)) => {
                    let _ = sock.send_to(&t.contents, t.destination).await;
                }
                Ok(Output::Event(e)) => self.on_rtc_event(e, events),
                Err(_) => {
                    self.rtc.disconnect();
                    return;
                }
            }
        }
    }

    fn on_rtc_event(&mut self, e: RtcEvent, events: &UnboundedSender<Event>) {
        match e {
            RtcEvent::ChannelOpen(id, label) if label == RELIABLE => {
                self.rel = Some(id);
                self.opened = true;
                let _ = events.send(Event::Open(self.id));
            }
            RtcEvent::ChannelData(d) => match self.frag.push(&d.data) {
                Ok(Some(message)) => {
                    let _ = events.send(Event::Message(self.id, message));
                }
                Ok(None) => {}
                Err(TooLarge) => {
                    crate::log!("[#{}] messaggio oltre {MAX_MESSAGE} byte: disconnetto", self.id);
                    self.rtc.disconnect();
                }
            },
            RtcEvent::ChannelClose(_) => crate::log!("[#{}] data channel chiuso", self.id),
            RtcEvent::IceConnectionStateChange(state) => {
                crate::log!("[#{}] ICE: {state:?}", self.id);
                if state == IceConnectionState::Disconnected {
                    self.rtc.disconnect();
                }
            }
            _ => {}
        }
    }

    /// Invia un messaggio sul canale affidabile, frammentandolo se serve.
    fn send(&mut self, data: &[u8]) {
        let Some(ch) = self.rel else { return };
        let chunks: Vec<_> = data.chunks(FRAGMENT).collect();
        if data.len() > MAX_MESSAGE {
            // L'header conta al massimo 256 frammenti: è un errore del chiamante, che deve dividere i batch.
            return crate::log!("[#{}] messaggio di {} byte non inviato: il limite è {MAX_MESSAGE}", self.id, data.len());
        }
        for (i, c) in chunks.iter().enumerate() {
            let msg = [&[(chunks.len() - 1 - i) as u8][..], c].concat();
            match self.rtc.channel(ch) {
                Some(mut ch) => {
                    if let Err(e) = ch.write(true, &msg) {
                        return crate::log!("[#{}] scrittura sul data channel fallita ({} byte): {e}", self.id, msg.len());
                    }
                }
                None => return crate::log!("[#{}] data channel non più disponibile", self.id),
            }
        }
    }
}

/// Ciclo unico per tutti i giocatori. Dopo ogni evento si "guida" solo la connessione coinvolta.
pub(crate) async fn run(
    sock: UdpSocket,
    shared: Arc<Shared>,
    mut new_clients: UnboundedReceiver<Client>,
    mut commands: UnboundedReceiver<Command>,
    events: UnboundedSender<Event>,
) {
    let (mut clients, mut buf): (Vec<Client>, _) = (Vec::new(), vec![0u8; 2048]);
    loop {
        let next = clients.iter().map(|c| c.next).min().unwrap_or_else(|| Instant::now() + Duration::from_secs(1));
        tokio::select! {
            Ok((n, source)) = sock.recv_from(&mut buf) => receive(&mut clients, &sock, &events, &buf[..n], source).await,
            Some(mut c) = new_clients.recv() => {
                c.drive(&sock, &events).await;
                clients.push(c);
            }
            Some(cmd) = commands.recv() => command(&mut clients, cmd, &sock, &events).await,
            _ = sleep_until(TInstant::from_std(next)) => {
                let now = Instant::now();
                for c in clients.iter_mut().filter(|c| c.next <= now) {
                    let _ = c.rtc.handle_input(Input::Timeout(now));
                    c.drive(&sock, &events).await;
                }
            }
        }
        clients.retain(|c| {
            let alive = c.rtc.is_alive();
            if !alive && c.opened {
                let _ = events.send(Event::Close(c.id));
            }
            alive
        });
        shared.players.store(clients.len(), Relaxed);
    }
}

/// Smista un datagramma UDP alla connessione che lo accetta (stesso ufrag ICE o stesso indirizzo).
async fn receive(clients: &mut [Client], sock: &UdpSocket, events: &UnboundedSender<Event>, data: &[u8], source: SocketAddr) {
    let now = Instant::now();
    for c in clients.iter_mut() {
        let Ok(contents) = data.try_into() else { return };
        let input = Input::Receive(now, Receive { proto: Protocol::Udp, source, destination: c.addr, contents });
        if c.rtc.accepts(&input) {
            let _ = c.rtc.handle_input(input);
            c.drive(sock, events).await;
            return;
        }
    }
}

async fn command(clients: &mut [Client], cmd: Command, sock: &UdpSocket, events: &UnboundedSender<Event>) {
    let (Command::Send(id, _) | Command::Kick(id)) = &cmd;
    let Some(c) = clients.iter_mut().find(|c| c.id == *id) else { return };
    match cmd {
        Command::Send(_, data) => c.send(&data),
        Command::Kick(_) => c.rtc.disconnect(),
    }
    c.drive(sock, events).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reassembles_fragments_in_order() {
        let mut r = Reassembler::default();
        assert_eq!(r.push(&[1, b'a', b'b']), Ok(None));
        assert_eq!(r.push(&[0, b'c']), Ok(Some(b"abc".to_vec())));
        assert_eq!(r.push(&[0, b'x']), Ok(Some(b"x".to_vec())), "dopo un messaggio completo si riparte da zero");
    }

    #[test]
    fn rejects_messages_larger_than_the_limit() {
        let mut r = Reassembler::default();
        let fragment = [&[255u8][..], &[0u8; FRAGMENT]].concat();
        let mut result = Ok(None);
        for _ in 0..=MAX_MESSAGE / FRAGMENT {
            result = r.push(&fragment);
            if result.is_err() {
                break;
            }
        }
        assert_eq!(result, Err(TooLarge), "un client non deve poter riempire la memoria con frammenti infiniti");
        assert_eq!(r.push(&[0, 1]), Ok(Some(vec![1])), "dopo l'errore il buffer è vuoto");
    }

    #[test]
    fn ignores_empty_datagrams() {
        assert_eq!(Reassembler::default().push(&[]), Ok(None));
    }
}
