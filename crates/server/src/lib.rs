//! Logica di gioco: riceve i pacchetti decodificati dal trasporto e risponde.
mod handlers;
mod join;
mod world;

use bedrock_net::{Config, Event, Sender};
use bedrock_protocol::{login::PlayerInfo, Packet, Session};
use std::{collections::HashMap, io};

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
}

struct Server {
    sender: Sender,
    players: HashMap<u64, Player>,
}

impl Server {
    /// Codifica i pacchetti per quel giocatore (con la sua compressione) e li invia in un solo messaggio.
    fn send(&self, id: u64, packets: &[Packet]) {
        if let Some(p) = self.players.get(&id) {
            self.sender.send(id, p.session.encode(packets));
        }
    }
}

/// Avvia il trasporto e gestisce gli eventi finché il programma gira.
pub async fn run(cfg: Config) -> io::Result<()> {
    let mut net = bedrock_net::start(cfg).await?;
    let mut server = Server { sender: net.sender.clone(), players: HashMap::new() };
    while let Some(event) = net.events.recv().await {
        match event {
            Event::Open(id) => server.open(id),
            Event::Message(id, data) => server.message(id, &data),
            Event::Close(id) => server.close(id),
        }
    }
    Ok(())
}
