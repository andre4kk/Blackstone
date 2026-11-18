//! Logica di gioco: riceve i pacchetti decodificati dal trasporto e risponde.
mod biomes;
mod chat;
pub mod config;
mod disk;
mod gen_client;
mod handlers;
mod join;
mod nbt;
mod storage;
mod streaming;
#[cfg(test)]
mod tests;
mod world;

use bedrock_net::{log, Config, Event, Sender};
use bedrock_protocol::{login::PlayerInfo, Packet, Session};
use disk::{Disk, DiskEvent};
use gen_client::{GenClient, GenConfig, GenEvent, Launch};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::Path,
    time::{Duration, Instant},
};
use tokio::{
    sync::mpsc::{unbounded_channel, UnboundedReceiver},
    time::{interval, MissedTickBehavior},
};
use vgen::Dim;

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
    /// Vero quando al giocatore si inviano i chunk (spento da `NO_CHUNKS=1`).
    streaming: bool,
    /// Posizione degli occhi: quella dello spawn, poi l'ultima di `PlayerAuthInput`.
    pos: [f32; 3],
    /// Chunk (x, z) in cui si trova.
    last_chunk: (i32, i32),
    /// Chunk già inviati e ancora nel raggio del giocatore: quando ne escono si tolgono, così al ritorno si rimandano.
    sent: HashSet<(i32, i32)>,
}

/// Il gioco gira a 20 tick al secondo.
const TICK: Duration = Duration::from_millis(50);
/// Quanti tick ricordare per calcolare i TPS reali.
const TPS_WINDOW: usize = 100;

/// Chiave di un chunk in memoria: (dimensione, x, z).
type ChunkKey = (u8, i32, i32);

struct Server {
    sender: Sender,
    players: HashMap<u64, Player>,
    /// Istanti degli ultimi tick, per misurare i TPS.
    ticks: VecDeque<Instant>,
    /// Durata media (media mobile) del lavoro di un tick, in millisecondi.
    mspt: f64,
    /// Mondo su disco, nel suo thread (assente nei test che non salvano e quando non c'è un mondo).
    disk: Option<Disk>,
    /// Generatore del mondo; assente in modalità piatta.
    gen: Option<GenClient>,
    /// Vero se i chunk nuovi sono `Chunk::flat` (generatore assente o mai partito).
    flat: bool,
    /// Il generatore ha già mandato `Ready`.
    gen_ready: bool,
    /// `Down` consecutivi del generatore senza un `Ready` in mezzo (dopo il primo `Ready`).
    gen_downs: u8,
    /// Spawn del mondo (piedi): dal `level.dat` o dal generatore; `None` finché non è noto.
    spawn: Option<(i32, i32, i32)>,
    /// Giocatori arrivati a `StartGame` prima che lo spawn fosse noto.
    waiting_start: Vec<u64>,
    /// Chunk in memoria.
    chunks: HashMap<ChunkKey, world::Chunk>,
    /// Chunk chiesti al generatore e non ancora arrivati.
    requested: HashSet<ChunkKey>,
    /// Chunk in lettura dal disco.
    loading: HashSet<ChunkKey>,
    /// Chunk da chiedere al generatore, in attesa che ci sia posto tra le richieste in volo (`MAX_GEN_IN_FLIGHT`).
    queue: HashSet<ChunkKey>,
    /// Richieste fallite per chunk (il generatore ha risposto con un errore).
    chunk_fails: HashMap<ChunkKey, u8>,
    /// Chunk falliti da richiedere di nuovo, con l'istante del nuovo tentativo.
    retry_after: HashMap<ChunkKey, Instant>,
    /// Richieste dello spawn fallite e istante del prossimo tentativo.
    spawn_fails: u8,
    spawn_retry_at: Option<Instant>,
    /// Raggio massimo di chunk concesso ai giocatori.
    max_radius: i32,
}

impl Server {
    /// Un server senza disco né generatore: mondo piatto e spawn piatto.
    fn new(sender: Sender) -> Self {
        Self {
            sender,
            players: HashMap::new(),
            ticks: VecDeque::new(),
            mspt: 0.0,
            disk: None,
            gen: None,
            flat: true,
            gen_ready: false,
            gen_downs: 0,
            spawn: Some(world::FLAT_SPAWN),
            waiting_start: Vec::new(),
            chunks: HashMap::new(),
            requested: HashSet::new(),
            loading: HashSet::new(),
            queue: HashSet::new(),
            chunk_fails: HashMap::new(),
            retry_after: HashMap::new(),
            spawn_fails: 0,
            spawn_retry_at: None,
            max_radius: join::MAX_RADIUS,
        }
    }

    /// Un server con il mondo su disco; gli eventi di lettura vanno a `events`.
    #[cfg(test)]
    fn with_world(sender: Sender, world: storage::World, events: tokio::sync::mpsc::UnboundedSender<DiskEvent>) -> Self {
        Self { disk: Some(Disk::start(world, events)), ..Self::new(sender) }
    }

    /// Chiusura ordinata: salva i chunk modificati e scarica il database.
    fn shutdown(&mut self) {
        let Some(disk) = self.disk.as_ref() else { return };
        let mut saved = 0;
        for chunk in self.chunks.values_mut().filter(|c| c.dirty) {
            disk.save(chunk.clone());
            chunk.dirty = false;
            saved += 1;
        }
        disk.flush_blocking();
        log!("[mondo] chiusura: {saved} chunk salvati");
    }

    /// Codifica i pacchetti per quel giocatore (con la sua compressione) e li invia in un solo messaggio.
    fn send(&self, id: u64, packets: &[Packet]) {
        if let Some(p) = self.players.get(&id) {
            self.sender.send(id, p.session.encode(packets));
        }
    }

    /// Un tick di gioco: per ora lo streaming dei chunk. Qui andranno anche gli aggiornamenti di entità e mondo.
    fn tick(&mut self) {
        let start = Instant::now();
        self.stream_tick();
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

/// Come [`run`], ma con il mondo e le impostazioni di `server.toml`: apre `worlds/<world>`, avvia il generatore e salva alla chiusura.
pub async fn run_with(cfg: Config, server_cfg: &config::ServerConfig) -> io::Result<()> {
    let launch = Launch::Process { program: server_cfg.generator.clone(), args_extra: Vec::new() };
    run_world(cfg, server_cfg, &Path::new("worlds").join(&server_cfg.world), launch).await
}

/// Come [`run_with`], ma il mondo sta in `world_dir` e il generatore è già in ascolto su `generator` (nessun processo da avviare).
/// Serve ai test end-to-end, che mettono un generatore finto nello stesso processo.
pub async fn run_with_generator_at(cfg: Config, server_cfg: &config::ServerConfig, world_dir: &Path, generator: std::net::SocketAddr) -> io::Result<()> {
    run_world(cfg, server_cfg, world_dir, Launch::Connect(generator)).await
}

async fn run_world(mut cfg: Config, server_cfg: &config::ServerConfig, world_dir: &Path, launch: Launch) -> io::Result<()> {
    cfg.level = server_cfg.world.clone();
    let world = storage::World::open_with(world_dir, &server_cfg.world, server_cfg.seed)?;
    log!("[mondo] \"{}\" aperto, seme {}", server_cfg.world, world.seed());
    if let Some(seed) = server_cfg.seed.filter(|&s| s != world.seed()) {
        log!("[mondo] il seme di server.toml ({seed}) è diverso da quello del mondo esistente: vale quello del mondo ({})", world.seed());
    }
    let generator = GenConfig { launch, seed: world.seed(), restart_delay: Duration::from_secs(2), request_timeout: gen_client::REQUEST_TIMEOUT };
    serve(cfg, Some(world), Some(generator), server_cfg.view_distance.clamp(1, 32)).await
}

/// Avvia il trasporto e fa girare il gioco con il mondo piatto, senza salvare nulla.
pub async fn run(cfg: Config) -> io::Result<()> {
    serve(cfg, None, None, join::MAX_RADIUS).await
}

/// Prossimo evento del generatore; senza generatore non arriva mai.
async fn next_gen_event(rx: &mut Option<UnboundedReceiver<GenEvent>>) -> Option<GenEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Il gioco: gli eventi di rete arrivano quando capitano, il tick ogni 50 ms. Ctrl+C chiude in ordine salvando il mondo.
async fn serve(cfg: Config, world: Option<storage::World>, generator: Option<GenConfig>, max_radius: i32) -> io::Result<()> {
    let mut net = bedrock_net::start(cfg).await?;
    let mut server = Server::new(net.sender.clone());
    server.max_radius = max_radius;
    let (disk_tx, mut disk_rx) = unbounded_channel::<DiskEvent>();
    if generator.is_some() {
        server.spawn = world.as_ref().and_then(|w| w.spawn()); // se manca lo calcola il generatore
    }
    if let Some(world) = world {
        server.disk = Some(Disk::start(world, disk_tx));
    }
    let mut gen_rx = None;
    if let Some(generator) = generator {
        let (client, rx) = GenClient::spawn(generator);
        if server.spawn.is_none() {
            client.request_spawn(Dim::Overworld); // finché non arriva, i login aspettano
        }
        server.gen = Some(client);
        server.flat = false;
        gen_rx = Some(rx);
    }
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
            Some(event) = next_gen_event(&mut gen_rx) => {
                server.on_gen_event(event);
                if server.gen.is_none() {
                    gen_rx = None; // passati al mondo piatto: il generatore non serve più
                }
            }
            Some(event) = disk_rx.recv() => server.on_disk_event(event),
            _ = ticker.tick() => server.tick(),
            _ = tokio::signal::ctrl_c() => {
                log!("Ctrl+C: chiusura del server");
                server.shutdown();
                return Ok(());
            }
        }
    }
}
