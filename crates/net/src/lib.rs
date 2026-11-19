//! Trasporto NetherNet (WebRTC) di Minecraft Bedrock: signaling HTTP + data channel.
//! Questo crate sa solo trasportare messaggi: connessioni che si aprono, messaggi in arrivo, messaggi da inviare.
//! Cosa c'è dentro i messaggi (pacchetti, compressione) è compito di `blackstone-protocol`.
mod identity;
mod media;
mod signaling;

use identity::Identity;
use std::{
    io,
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, AtomicUsize},
        Arc, OnceLock,
    },
    time::Instant,
};
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::mpsc,
};

pub use signaling::lan_ip;

/// Secondi dall'avvio del programma (si usa nei log per capire i tempi tra gli eventi).
pub fn uptime() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Livello di un messaggio di log, dal più rumoroso al più importante.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
}

impl Level {
    /// Legge "debug", "info" o "warn" (anche "warning"), senza badare alle maiuscole.
    pub fn parse(text: &str) -> Option<Level> {
        match text.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            _ => None,
        }
    }

    /// Etichetta di quattro lettere e codice colore ANSI.
    fn tag(self) -> (&'static str, &'static str) {
        match self {
            Level::Debug => ("DBG ", "36"),
            Level::Info => ("INFO", "32"),
            Level::Warn => ("WARN", "33;1"),
        }
    }
}

/// Livello minimo mostrato; 255 = non ancora deciso (si legge `BLACKSTONE_LOG`, altrimenti info).
static LEVEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(255);

/// Imposta il livello minimo dalla configurazione. La variabile d'ambiente `BLACKSTONE_LOG` ha la precedenza.
pub fn set_level(configured: Level) {
    let level = std::env::var("BLACKSTONE_LOG")
        .or_else(|_| std::env::var("VERSMC_LOG"))
        .ok() /* VERSMC_LOG: vecchio nome, ancora letto */
        .and_then(|v| Level::parse(&v))
        .unwrap_or(configured);
    LEVEL.store(level as u8, std::sync::atomic::Ordering::Relaxed);
}

/// Il livello minimo attuale.
pub fn level() -> Level {
    let current = LEVEL.load(std::sync::atomic::Ordering::Relaxed);
    if current == 255 {
        set_level(Level::Info);
        return level();
    }
    match current {
        0 => Level::Debug,
        1 => Level::Info,
        _ => Level::Warn,
    }
}

/// Vero se un messaggio di questo livello va mostrato.
pub fn enabled(level_of_message: Level) -> bool {
    level_of_message >= level()
}

/// Colori solo se l'output è un terminale (e `NO_COLOR` non c'è); su Windows serve attivare le sequenze ANSI.
fn use_color() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| {
        use std::io::IsTerminal;
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() && enable_ansi()
    })
}

#[cfg(windows)]
fn enable_ansi() -> bool {
    extern "system" {
        fn GetStdHandle(id: u32) -> *mut core::ffi::c_void;
        fn GetConsoleMode(handle: *mut core::ffi::c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut core::ffi::c_void, mode: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 4;
    // SAFETY: chiamate Win32 senza puntatori oltre a `mode`, che è una variabile locale valida.
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0u32;
        GetConsoleMode(handle, &mut mode) != 0 && SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn enable_ansi() -> bool {
    true
}

/// Formatta una riga di log: tempo dall'avvio, etichetta del livello, messaggio.
pub fn format_line(time: f64, level_of_message: Level, colored: bool, message: &std::fmt::Arguments) -> String {
    let (tag, color) = level_of_message.tag();
    if colored {
        format!("[90m{time:>8.3}s[0m [{color}m{tag}[0m {message}")
    } else {
        format!("{time:>8.3}s {tag} {message}")
    }
}

/// Scrive la riga se il livello è abilitato. Usata dalle macro `log!`, `warn!` e `debug!`.
pub fn emit(level_of_message: Level, message: std::fmt::Arguments) {
    if enabled(level_of_message) {
        println!("{}", format_line(uptime(), level_of_message, use_color(), &message));
    }
}

/// Messaggio di livello info (quello normale), con tempo e etichetta davanti.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::emit($crate::Level::Info, format_args!($($arg)*)) };
}

/// Messaggio di avvertimento: qualcosa è andato storto ma il server continua.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::emit($crate::Level::Warn, format_args!($($arg)*)) };
}

/// Dettagli tecnici, mostrati solo con `log_level = "debug"`.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => { $crate::emit($crate::Level::Debug, format_args!($($arg)*)) };
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

/// Cosa il gioco chiede al trasporto. Pubblico solo per i test di chi usa `Sender::for_tests`.
#[derive(Debug, PartialEq)]
pub enum Command {
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

    /// Un `Sender` senza rete: i comandi finiscono nel ricevitore restituito. Serve ai test della logica di gioco.
    pub fn for_tests() -> (Self, mpsc::UnboundedReceiver<Command>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
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
    log!("[rete] in ascolto su TCP+UDP {}", cfg.port);

    let shared = Arc::new(Shared { cfg, identity: Identity::load("identity.key"), players: AtomicUsize::new(0), next_id: AtomicU64::new(1) });
    let (event_tx, events) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (client_tx, client_rx) = mpsc::unbounded_channel();
    tokio::spawn(media::run(udp, shared.clone(), client_rx, command_rx, event_tx));
    tokio::spawn(signaling::serve(tcp, shared, client_tx));
    Ok(Net { events, sender: Sender(command_tx) })
}

#[cfg(test)]
mod log_tests {
    use super::*;

    #[test]
    fn levels_are_ordered_and_parsed() {
        assert!(Level::Debug < Level::Info && Level::Info < Level::Warn);
        assert_eq!(Level::parse(" DEBUG "), Some(Level::Debug));
        assert_eq!(Level::parse("warning"), Some(Level::Warn));
        assert_eq!(Level::parse("rumore"), None);
    }

    #[test]
    fn filtering_follows_the_minimum_level() {
        // Unico test che tocca il livello globale (i test girano in parallelo).
        std::env::remove_var("BLACKSTONE_LOG");
        std::env::remove_var("VERSMC_LOG");
        set_level(Level::Info);
        assert!(!enabled(Level::Debug) && enabled(Level::Info) && enabled(Level::Warn));
        set_level(Level::Warn);
        assert!(!enabled(Level::Info) && enabled(Level::Warn));
        set_level(Level::Debug);
        assert!(enabled(Level::Debug));
        set_level(Level::Info);
    }

    #[test]
    fn line_format_plain_and_colored() {
        assert_eq!(format_line(1.5, Level::Warn, false, &format_args!("[x] ciao")), "   1.500s WARN [x] ciao");
        let colored = format_line(1.5, Level::Info, true, &format_args!("m"));
        assert!(colored.contains("[32mINFO[0m m"));
    }
}
