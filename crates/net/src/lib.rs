//! Trasporto NetherNet (WebRTC) di Minecraft Bedrock: signaling HTTP + data channel.
//! Questo crate sa solo trasportare messaggi: connessioni che si aprono, messaggi in arrivo, messaggi da inviare.
//! Cosa c'è dentro i messaggi (pacchetti, compressione) è compito di `bedrock-protocol`.
mod identity;
mod media;
mod signaling;

use identity::Identity;
use std::{io, net::IpAddr, sync::{atomic::{AtomicU64, AtomicUsize}, Arc, OnceLock}, time::Instant};
use tokio::{net::{TcpListener, UdpSocket}, sync::mpsc};

pub use signaling::lan_ip;

/// Secondi dall'avvio del programma (si usa nei log per capire i tempi tra gli eventi).
pub fn uptime() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Come `println!`, ma con il tempo dall'avvio davanti.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { println!("{:>8.3}s  {}", $crate::uptime(), format_args!($($arg)*)) };
}

/// Configurazione del server, mostrata ai client nella lista server.
pub struct Config {
    /// Porta TCP (signaling) e UDP (gioco): la stessa.
    pub port: u16,
    pub name: String,
    pub level: String,
    pub version: String,
    pub protocol: u32,
    pub max_players: u32,
    /// IP pubblico da pubblicare come candidato ICE per chi gioca da Internet.
    pub public_ip: Option<IpAddr>,
}

/// Cosa succede su una connessione. `u64` è l'ID della connessione, assegnato dal trasporto.
pub enum Event {
    Open(u64),
    /// Un messaggio completo (frammenti già riassemblati).
    Message(u64, Vec<u8>),
    Close(u64),
}

pub(crate) enum Command {
    Send(u64, Vec<u8>),
    Kick(u64),
}

/// Handle clonabile per inviare dati o disconnettere. Non blocca mai.
#[derive(Clone)]
pub struct Sender(mpsc::UnboundedSender<Command>);

impl Sender {
    pub fn send(&self, id: u64, data: Vec<u8>) {
        let _ = self.0.send(Command::Send(id, data));
    }

    pub fn kick(&self, id: u64) {
        let _ = self.0.send(Command::Kick(id));
    }
}

pub struct Net {
    pub events: mpsc::UnboundedReceiver<Event>,
    pub sender: Sender,
}

/// Stato condiviso tra signaling e ciclo di rete.
pub(crate) struct Shared {
    cfg: Config,
    identity: Identity,
    players: AtomicUsize,
    next_id: AtomicU64,
}

/// Apre le porte e avvia il trasporto in background.
pub async fn start(cfg: Config) -> io::Result<Net> {
    uptime(); // avvia l'orologio
    str0m::crypto::from_feature_flags().install_process_default();
    let (udp, tcp) = (UdpSocket::bind(("0.0.0.0", cfg.port)).await?, TcpListener::bind(("0.0.0.0", cfg.port)).await?);
    log!("Server NetherNet in ascolto: TCP+UDP {} (protocollo {})", cfg.port, cfg.protocol);

    let shared = Arc::new(Shared { cfg, identity: Identity::load("identity.key"), players: AtomicUsize::new(0), next_id: AtomicU64::new(1) });
    let (event_tx, events) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (client_tx, client_rx) = mpsc::unbounded_channel();
    tokio::spawn(media::run(udp, shared.clone(), client_rx, command_rx, event_tx));
    tokio::spawn(signaling::serve(tcp, shared, client_tx));
    Ok(Net { events, sender: Sender(command_tx) })
}
