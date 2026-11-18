//! Logica di gioco: riceve i pacchetti decodificati dal trasporto e risponde.
mod chat;
pub mod config;
mod gen_client;
mod handlers;
mod join;
mod nbt;
mod storage;
#[cfg(test)]
mod tests;
mod world;

use bedrock_net::{log, Config, Event, Sender};
use bedrock_protocol::{login::PlayerInfo, Packet, Session};
use std::{
    collections::{HashMap, VecDeque},
    io,
    path::Path,
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
    /// Mondo su disco (assente nei test che non salvano).
    world: Option<storage::World>,
    /// Chunk in memoria, per (dimensione, x, z). Lo streaming li riempirà; per ora serve a salvarli alla chiusura.
    #[allow(dead_code)]
    chunks: HashMap<(u8, i32, i32), world::Chunk>,
}

impl Server {
    fn new(sender: Sender) -> Self {
        Self { sender, players: HashMap::new(), ticks: VecDeque::new(), mspt: 0.0, world: None, chunks: HashMap::new() }
    }

    #[allow(dead_code)] // usato dai test; lo streaming lo userà per i chunk caricati
    fn with_world(sender: Sender, world: storage::World) -> Self {
        Self { world: Some(world), ..Self::new(sender) }
    }

    /// Chiusura ordinata: salva i chunk modificati e scarica il database.
    fn shutdown(&mut self) {
        let Some(world) = self.world.as_mut() else { return };
        let mut saved = 0;
        for chunk in self.chunks.values_mut().filter(|c| c.dirty) {
            match world.save(chunk) {
                Ok(()) => {
                    chunk.dirty = false;
                    saved += 1;
                }
                Err(e) => log!("[mondo] salvataggio del chunk {},{} fallito: {e}", chunk.x, chunk.z),
            }
        }
        match world.flush() {
            Ok(()) => log!("[mondo] chiusura: {saved} chunk salvati"),
            Err(e) => log!("[mondo] flush fallito: {e}"),
        }
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

/// Come [`run`], ma con il mondo e le impostazioni di `server.toml`: apre `worlds/<world>` e lo salva alla chiusura.
pub async fn run_with(mut cfg: Config, server_cfg: &config::ServerConfig) -> io::Result<()> {
    cfg.level = server_cfg.world.clone();
    let world = storage::World::open_with(&Path::new("worlds").join(&server_cfg.world), &server_cfg.world, server_cfg.seed)?;
    log!("[mondo] \"{}\" aperto, seme {}", server_cfg.world, world.seed());
    serve(cfg, Some(world)).await
}

/// Avvia il trasporto e fa girare il gioco senza salvare nulla.
pub async fn run(cfg: Config) -> io::Result<()> {
    serve(cfg, None).await
}

/// Il gioco: gli eventi di rete arrivano quando capitano, il tick ogni 50 ms. Ctrl+C chiude in ordine salvando il mondo.
async fn serve(cfg: Config, world: Option<storage::World>) -> io::Result<()> {
    let mut net = bedrock_net::start(cfg).await?;
    let mut server = Server::new(net.sender.clone());
    server.world = world;
    let mut ticker = interval(TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip); // se un tick tarda, non si "recupera" a raffica
    loop {
        tokio::select! {
            event = net.events.recv() => match event {
                Some(Event::Open(id)) => server.open(id),
                Some(Event::Message(id, data)) => server.message(id, &data),
                Some(Event::Close(id)) => server.close(id),
                None => {
                    server.shutdown();
                    return Ok(());
                }
            },
            _ = ticker.tick() => server.tick(),
            _ = tokio::signal::ctrl_c() => {
                log!("Ctrl+C: chiusura del server");
                server.shutdown();
                return Ok(());
            }
        }
    }
}
