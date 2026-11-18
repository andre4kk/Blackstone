//! Logica di gioco: riceve i pacchetti decodificati dal trasporto e risponde.
mod chat;
mod gen_client;
mod handlers;
mod join;
#[cfg(test)]
mod tests;
mod world;

use bedrock_net::{Config, Event, Sender};
use bedrock_protocol::{login::PlayerInfo, Packet, Session};
use std::{
    collections::{HashMap, VecDeque},
    io,
    time::{Duration, Instant},
};
use tokio::time::{interval, MissedTickBehavior};

/// Configurazione di default. L'IP pubblico (per giocare da Internet) si imposta con la variabile PUBLIC_IP.
pub fn config(port: u16) -> Config {
    Config {
        port,
        name: "Rust NetherNet Server".into(),
        level: "Mondo".into(),
        version: bedrock_protocol::VERSION.into(),
        protocol: bedrock_protocol::PROTOCOL,
        max_players: 100,
        public_ip: std::env::var("PUBLIC_IP").ok().and_then(|v| v.parse().ok()),
    }
}

/// Stato di un giocatore connesso.
#[derive(Default)]
struct Player {
    session: Session,
    /// Compilato dopo il login.
    info: Option<PlayerInfo>,
    /// Raggio di chunk concesso (0 finché il client non lo chiede).
    radius: i32,
    /// Vero quando il client si è inizializzato e vede il mondo.
    spawned: bool,
}

/// Il gioco gira a 20 tick al secondo.
const TICK: Duration = Duration::from_millis(50);
/// Quanti tick ricordare per calcolare i TPS reali.
const TPS_WINDOW: usize = 100;

struct Server {
    sender: Sender,
    players: HashMap<u64, Player>,
    /// Istanti degli ultimi tick, per misurare i TPS.
    ticks: VecDeque<Instant>,
    /// Durata media (media mobile) del lavoro di un tick, in millisecondi.
    mspt: f64,
}

impl Server {
    fn new(sender: Sender) -> Self {
        Self { sender, players: HashMap::new(), ticks: VecDeque::new(), mspt: 0.0 }
    }

    /// Codifica i pacchetti per quel giocatore (con la sua compressione) e li invia in un solo messaggio.
    fn send(&self, id: u64, packets: &[Packet]) {
        if let Some(p) = self.players.get(&id) {
            self.sender.send(id, p.session.encode(packets));
        }
    }

    /// Un tick di gioco. Qui andranno gli aggiornamenti di giocatori, entità e mondo.
    fn tick(&mut self) {
        let start = Instant::now();
        // ... (nulla da aggiornare per ora)
        self.mspt = self.mspt * 0.9 + start.elapsed().as_secs_f64() * 1000.0 * 0.1;
        self.ticks.push_back(start);
        if self.ticks.len() > TPS_WINDOW {
            self.ticks.pop_front();
        }
    }

    /// Tick al secondo reali (al massimo 20), sugli ultimi `TPS_WINDOW` tick.
    fn tps(&self) -> f64 {
        match (self.ticks.front(), self.ticks.back()) {
            (Some(first), Some(last)) if self.ticks.len() > 1 => ((self.ticks.len() - 1) as f64 / last.duration_since(*first).as_secs_f64()).min(20.0),
            _ => 20.0,
        }
    }
}

/// Avvia il trasporto e fa girare il gioco: gli eventi di rete arrivano quando capitano, il tick ogni 50 ms.
pub async fn run(cfg: Config) -> io::Result<()> {
    let mut net = bedrock_net::start(cfg).await?;
    let mut server = Server::new(net.sender.clone());
    let mut ticker = interval(TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip); // se un tick tarda, non si "recupera" a raffica
    loop {
        tokio::select! {
            event = net.events.recv() => match event {
                Some(Event::Open(id)) => server.open(id),
                Some(Event::Message(id, data)) => server.message(id, &data),
                Some(Event::Close(id)) => server.close(id),
                None => return Ok(()),
            },
            _ = ticker.tick() => server.tick(),
        }
    }
}
