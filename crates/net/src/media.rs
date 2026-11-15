//! Ciclo di rete WebRTC: una sola socket UDP per tutti i giocatori, smistata da str0m (sans-I/O).
use crate::{Command, Event, Shared};
use std::{net::SocketAddr, sync::{atomic::Ordering::Relaxed, Arc}, time::{Duration, Instant}};
use str0m::{channel::ChannelId, net::{Protocol, Receive}, Event as RtcEvent, IceConnectionState, Input, Output, Rtc};
use tokio::{net::UdpSocket, sync::mpsc::{UnboundedReceiver, UnboundedSender}, time::{sleep_until, Instant as TInstant}};

const RELIABLE: &str = "ReliableDataChannel";
const FRAGMENT: usize = 10_000; // dimensione massima di un frammento (header di 1 byte escluso)

/// Una connessione WebRTC (un giocatore) con il suo data channel affidabile.
pub(crate) struct Client {
    id: u64,
    rtc: Rtc,
    addr: SocketAddr,
    rel: Option<ChannelId>,
    opened: bool,
    frag: Vec<u8>,
    next: Instant,
}

impl Client {
    pub fn new(id: u64, rtc: Rtc, addr: SocketAddr) -> Self {
        Self { id, rtc, addr, rel: None, opened: false, frag: Vec::new(), next: Instant::now() }
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
            RtcEvent::ChannelData(d) => {
                // Header di 1 byte: N>0 = frammento, ne mancano ancora N; 0 = messaggio completo (o ultimo frammento).
                let Some((&header, payload)) = d.data.split_first() else { return };
                self.frag.extend_from_slice(payload);
                if header == 0 {
                    let _ = events.send(Event::Message(self.id, std::mem::take(&mut self.frag)));
                }
            }
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
        if chunks.len() > 256 {
            return; // l'header conta fino a 255 frammenti: oltre è un errore del chiamante
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
