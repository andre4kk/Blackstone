//! Client del generatore del mondo: avvia (o raggiunge) `versmc-gen`, parla il protocollo `vgen` su TCP locale
//! e riavvia/riconnette il generatore se cade, rimandando le richieste rimaste senza risposta.

#![allow(dead_code)] // `Launch::Connect` serve ai test

use bedrock_net::{log, warn};
use std::{
    collections::{HashMap, VecDeque},
    io,
    net::SocketAddr,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{tcp::OwnedReadHalf, TcpStream},
    process::{Child, Command},
    sync::mpsc,
    time::{interval, sleep, timeout},
};
use vgen::{Dim, FromGen, ToGen};

/// Tempo massimo per `READY` e per l'handshake: il generatore carica il Translator prima di rispondere all'Hello.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
/// Una richiesta rimasta senza risposta per tanto tempo conta come fallita (rigenerazione bloccata nel generatore).
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Crolli del generatore con la stessa richiesta in volo dopo i quali la richiesta si dà per fallita invece di rimandarla.
const MAX_CRASHES_PER_REQUEST: u8 = 3;

/// Come ottenere la connessione al generatore.
pub(crate) enum Launch {
    /// Avvia `program serve --port 0 --seed S [args_extra]`, legge `READY <porta>` e si connette.
    Process { program: PathBuf, args_extra: Vec<String> },
    /// Si connette a un generatore già in ascolto (test); "riavvio" = riconnessione.
    Connect(SocketAddr),
}

pub(crate) struct GenConfig {
    pub launch: Launch,
    pub seed: u64,
    pub restart_delay: Duration,
    /// Dopo quanto una richiesta senza risposta diventa `ChunkFailed`/`SpawnFailed` (configurabile per i test).
    pub request_timeout: Duration,
}

pub(crate) enum GenEvent {
    Ready,
    Palette(Vec<(i32, Vec<u8>)>),
    Chunk(vgen::ChunkData),
    Spawn(Dim, i32, i32, i32),
    /// Il generatore ha risposto con un errore alla richiesta di quel chunk.
    ChunkFailed(Dim, i32, i32, String),
    /// Il generatore ha risposto con un errore alla richiesta dello spawn.
    SpawnFailed(Dim, String),
    Down(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pending {
    Chunk(Dim, i32, i32),
    Spawn(Dim),
}

pub(crate) struct GenClient {
    tx: mpsc::UnboundedSender<Pending>,
}

impl GenClient {
    pub(crate) fn spawn(cfg: GenConfig) -> (GenClient, mpsc::UnboundedReceiver<GenEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(run(cfg, rx, ev_tx));
        (GenClient { tx }, ev_rx)
    }

    /// Un client senza generatore: le richieste finiscono nel ricevitore restituito (senza eliminare i duplicati). Per i test.
    #[cfg(test)]
    pub(crate) fn for_tests() -> (GenClient, mpsc::UnboundedReceiver<Pending>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (GenClient { tx }, rx)
    }

    /// Non blocca; se lo stesso chunk è già in volo non fa niente.
    pub(crate) fn request_chunk(&self, dim: Dim, x: i32, z: i32) {
        let _ = self.tx.send(Pending::Chunk(dim, x, z));
    }

    pub(crate) fn request_spawn(&self, dim: Dim) {
        let _ = self.tx.send(Pending::Spawn(dim));
    }
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len > vgen::MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame troppo grande"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(body)
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, body: &[u8]) -> io::Result<()> {
    if body.len() > vgen::MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame troppo grande"));
    }
    let mut buf = Vec::with_capacity(body.len() + 4);
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(body);
    w.write_all(&buf).await
}

async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<FromGen> {
    let body = read_frame(r).await?;
    FromGen::decode(&body).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "messaggio del generatore illeggibile"))
}

/// Avvia il processo e attende `READY <porta>`. Restituisce il processo (da tenere vivo) e l'indirizzo.
async fn start_process(program: &PathBuf, extra: &[String], seed: u64) -> io::Result<(Child, SocketAddr)> {
    let mut child = Command::new(program)
        .args(["serve", "--port", "0", "--seed", &seed.to_string()])
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("stdout non disponibile"))?;
    let mut lines = BufReader::new(stdout).lines();
    let port = timeout(STARTUP_TIMEOUT, async {
        while let Some(line) = lines.next_line().await? {
            if let Some(p) = line.trim().strip_prefix("READY ") {
                return p.trim().parse::<u16>().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "porta READY non valida"));
            }
        }
        Err(io::Error::new(io::ErrorKind::UnexpectedEof, "il generatore è uscito prima di READY"))
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "READY non ricevuto"))??;
    // Il resto dello stdout viene scaricato per non bloccare il generatore.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    Ok((child, SocketAddr::from(([127, 0, 0, 1], port))))
}

fn to_msg(req: u32, p: Pending) -> ToGen {
    match p {
        Pending::Chunk(dim, x, z) => ToGen::Generate { req, dim, x, z },
        Pending::Spawn(dim) => ToGen::Spawn { req, dim },
    }
}

/// Una richiesta in volo.
struct Entry {
    p: Pending,
    /// Crolli del generatore mentre questa richiesta era stata inviata e senza risposta.
    crashes: u8,
    /// Quando è stata scritta nella connessione attuale (`None` = non ancora inviata a questo generatore).
    sent_at: Option<Instant>,
}

struct State {
    inflight: HashMap<u32, Entry>,
    next_req: u32,
    /// Dopo un crollo le richieste rimaste in volo si rimandano una alla volta (prima quelle con meno crolli), così
    /// il crollo successivo si attribuisce a una sola richiesta e non a tutte quelle che le stavano accanto.
    probation: VecDeque<u32>,
    /// La richiesta di prova ora in volo.
    probe: Option<u32>,
}

impl State {
    /// Fine di una connessione finita male: ogni richiesta che il generatore aveva ricevuto conta un crollo; a
    /// `MAX_CRASHES_PER_REQUEST` la richiesta si dà per fallita (ritenta o ripiega il server) invece di rimandarla.
    fn end_session(&mut self, ev: &mpsc::UnboundedSender<GenEvent>) {
        let mut give_up = Vec::new();
        for (&id, e) in self.inflight.iter_mut() {
            if e.sent_at.take().is_some() {
                e.crashes += 1;
                if e.crashes >= MAX_CRASHES_PER_REQUEST {
                    give_up.push(id);
                }
            }
        }
        give_up.sort_unstable();
        for id in give_up {
            if let Some(e) = self.inflight.remove(&id) {
                warn!("[gen] la richiesta {:?} ha fatto cadere il generatore {} volte: la do per fallita", e.p, e.crashes);
                fail(ev, e.p, format!("il generatore è caduto {} volte con questa richiesta", e.crashes));
            }
        }
        self.probe = None;
        self.probation.clear();
    }

    /// All'inizio di una connessione: le richieste rimaste in volo vanno rimandate (stessi id), una alla volta.
    fn start_probation(&mut self) {
        let mut ids: Vec<(u8, u32)> = self.inflight.iter().map(|(&id, e)| (e.crashes, id)).collect();
        ids.sort_unstable();
        self.probation = ids.into_iter().map(|(_, id)| id).collect();
        self.probe = None;
    }

    /// Se non c'è una richiesta di prova in volo, manda la prossima. `Err` se la scrittura fallisce.
    async fn pump_probation<W: AsyncWrite + Unpin>(&mut self, wr: &mut W) -> io::Result<()> {
        if self.probe.is_some_and(|id| !self.inflight.contains_key(&id)) {
            self.probe = None; // risposta, errore o scaduta
        }
        while self.probe.is_none() {
            let Some(id) = self.probation.pop_front() else { return Ok(()) };
            let Some(e) = self.inflight.get_mut(&id) else { continue };
            write_frame(wr, &to_msg(id, e.p).encode()).await?;
            e.sent_at = Some(Instant::now());
            self.probe = Some(id);
        }
        Ok(())
    }

    /// Le richieste senza risposta da almeno `limit` diventano fallite e escono dalle richieste in volo.
    fn expire(&mut self, limit: Duration, ev: &mpsc::UnboundedSender<GenEvent>) {
        let now = Instant::now();
        let mut late: Vec<u32> = self.inflight.iter().filter(|(_, e)| e.sent_at.is_some_and(|t| now.duration_since(t) >= limit)).map(|(&id, _)| id).collect();
        late.sort_unstable();
        for id in late {
            if let Some(e) = self.inflight.remove(&id) {
                warn!("[gen] nessuna risposta alla richiesta {:?} dopo {:.1} s: la do per fallita", e.p, limit.as_secs_f32());
                fail(ev, e.p, format!("nessuna risposta dal generatore entro {:.1} s", limit.as_secs_f32()));
            }
        }
    }
}

/// Segnala il fallimento di una richiesta (il server ritenta o ripiega).
fn fail(ev: &mpsc::UnboundedSender<GenEvent>, p: Pending, message: String) {
    let _ = ev.send(match p {
        Pending::Chunk(dim, x, z) => GenEvent::ChunkFailed(dim, x, z, message),
        Pending::Spawn(dim) => GenEvent::SpawnFailed(dim, message),
    });
}

async fn run(cfg: GenConfig, mut rx: mpsc::UnboundedReceiver<Pending>, ev: mpsc::UnboundedSender<GenEvent>) {
    let mut st = State { inflight: HashMap::new(), next_req: 1, probation: VecDeque::new(), probe: None };
    loop {
        let reason = match session(&cfg, &mut rx, &ev, &mut st).await {
            Ok(()) => return, // il client è stato lasciato cadere
            Err(e) => e.to_string(),
        };
        warn!("[gen] generatore non disponibile: {reason}");
        st.end_session(&ev);
        if ev.send(GenEvent::Down(reason)).is_err() {
            return;
        }
        sleep(cfg.restart_delay).await;
    }
}

/// Una connessione al generatore. `Ok` solo quando il client è stato lasciato cadere.
async fn session(cfg: &GenConfig, rx: &mut mpsc::UnboundedReceiver<Pending>, ev: &mpsc::UnboundedSender<GenEvent>, st: &mut State) -> io::Result<()> {
    let mut child = None;
    let addr = match &cfg.launch {
        Launch::Process { program, args_extra } => {
            let (c, a) = start_process(program, args_extra, cfg.seed).await?;
            child = Some(c);
            a
        }
        Launch::Connect(a) => *a,
    };
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true).ok();
    let (mut rd, mut wr) = stream.into_split();

    write_frame(&mut wr, &ToGen::Hello { version: vgen::VERSION, seed: cfg.seed }.encode()).await?;
    let palette = timeout(STARTUP_TIMEOUT, async {
        match read_msg(&mut rd).await? {
            FromGen::Hello { version } if version == vgen::VERSION => {}
            FromGen::Hello { version } => return Err(io::Error::other(format!("versione del generatore {version} diversa da {}", vgen::VERSION))),
            FromGen::Error { message, .. } => return Err(io::Error::other(format!("handshake rifiutato: {message}"))),
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "risposta inattesa all'Hello")),
        }
        match read_msg(&mut rd).await? {
            FromGen::Palette { entries } => Ok(entries),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "palette attesa dopo l'Hello")),
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake scaduto"))??;
    log!("[gen] generatore pronto su {addr}");
    let _ = ev.send(GenEvent::Ready);
    let _ = ev.send(GenEvent::Palette(palette));

    // Rimanda le richieste rimaste senza risposta (stessi id), una alla volta.
    st.start_probation();
    st.pump_probation(&mut wr).await?;

    // La lettura sta in un task a parte: `read_exact` non è cancellabile dentro `select!`.
    let (in_tx, mut in_rx) = mpsc::unbounded_channel();
    let reader = tokio::spawn(read_loop(rd, in_tx));
    let mut check = interval((cfg.request_timeout / 20).clamp(Duration::from_millis(10), Duration::from_secs(1)));
    let err = loop {
        tokio::select! {
            // finché si rimandano le richieste di prova, le nuove aspettano nel canale
            r = rx.recv(), if st.probe.is_none() && st.probation.is_empty() => {
                let Some(p) = r else {
                    reader.abort();
                    return Ok(());
                };
                if st.inflight.values().any(|q| q.p == p) {
                    continue;
                }
                let req = st.next_req;
                st.next_req = st.next_req.wrapping_add(1).max(1);
                st.inflight.insert(req, Entry { p, crashes: 0, sent_at: None });
                if let Err(e) = write_frame(&mut wr, &to_msg(req, p).encode()).await {
                    break e;
                }
                if let Some(e) = st.inflight.get_mut(&req) {
                    e.sent_at = Some(Instant::now());
                }
            }
            m = in_rx.recv() => {
                match m {
                    Some(Ok(msg)) => handle(msg, st, ev),
                    Some(Err(e)) => break e,
                    None => break io::Error::new(io::ErrorKind::UnexpectedEof, "connessione chiusa"),
                }
                if let Err(e) = st.pump_probation(&mut wr).await {
                    break e;
                }
            }
            _ = check.tick() => {
                st.expire(cfg.request_timeout, ev);
                if let Err(e) = st.pump_probation(&mut wr).await {
                    break e;
                }
            }
            _ = async {
                match child.as_mut() {
                    Some(c) => {
                        let _ = c.wait().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {
                break io::Error::new(io::ErrorKind::UnexpectedEof, "il processo del generatore è uscito");
            }
        }
    };
    reader.abort();
    Err(err)
}

async fn read_loop(mut rd: OwnedReadHalf, tx: mpsc::UnboundedSender<io::Result<FromGen>>) {
    loop {
        let m = read_msg(&mut rd).await;
        let stop = m.is_err();
        if tx.send(m).is_err() || stop {
            return;
        }
    }
}

fn handle(msg: FromGen, st: &mut State, ev: &mpsc::UnboundedSender<GenEvent>) {
    match msg {
        FromGen::Chunk { req, data } => {
            if st.inflight.remove(&req).is_some() {
                let _ = ev.send(GenEvent::Chunk(data));
            }
        }
        FromGen::Spawn { req, x, y, z } => {
            if let Some(Entry { p: Pending::Spawn(dim), .. }) = st.inflight.remove(&req) {
                let _ = ev.send(GenEvent::Spawn(dim, x, y, z));
            }
        }
        FromGen::Error { req, message } => {
            let what = st.inflight.remove(&req).map(|e| e.p);
            warn!("[gen] errore dal generatore (richiesta {req}, {what:?}): {message}");
            match what {
                Some(Pending::Chunk(dim, x, z)) => {
                    let _ = ev.send(GenEvent::ChunkFailed(dim, x, z, message));
                }
                Some(Pending::Spawn(dim)) => {
                    let _ = ev.send(GenEvent::SpawnFailed(dim, message));
                }
                None => {}
            }
        }
        FromGen::Hello { .. } | FromGen::Palette { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn chunk(dim: Dim, x: i32, z: i32) -> vgen::ChunkData {
        vgen::ChunkData { dim, x, z, sections: vec![], block_entities: vec![] }
    }

    /// Accetta una connessione, fa l'handshake e restituisce lo stream.
    async fn accept(l: &TcpListener) -> TcpStream {
        let (mut s, _) = timeout(Duration::from_secs(5), l.accept()).await.expect("timeout accept").unwrap();
        let hello = ToGen::decode(&read_frame(&mut s).await.unwrap()).unwrap();
        assert!(matches!(hello, ToGen::Hello { version: vgen::VERSION, seed: 7 }));
        write_frame(&mut s, &FromGen::Hello { version: vgen::VERSION }.encode()).await.unwrap();
        write_frame(&mut s, &FromGen::Palette { entries: vec![(5, vec![1, 2])] }.encode()).await.unwrap();
        s
    }

    async fn next_req(s: &mut TcpStream) -> ToGen {
        let body = timeout(Duration::from_secs(5), read_frame(s)).await.expect("timeout richiesta").unwrap();
        ToGen::decode(&body).unwrap()
    }

    fn cfg(addr: SocketAddr) -> GenConfig {
        GenConfig { launch: Launch::Connect(addr), seed: 7, restart_delay: Duration::from_millis(50), request_timeout: REQUEST_TIMEOUT }
    }

    async fn ev(rx: &mut mpsc::UnboundedReceiver<GenEvent>) -> GenEvent {
        timeout(Duration::from_secs(5), rx.recv()).await.expect("timeout").expect("canale chiuso")
    }

    #[tokio::test]
    async fn chunk_request_reaches_generator_and_comes_back() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, mut rx) = GenClient::spawn(cfg(l.local_addr().unwrap()));
        let mut s = accept(&l).await;
        assert!(matches!(ev(&mut rx).await, GenEvent::Ready));
        match ev(&mut rx).await {
            GenEvent::Palette(p) => assert_eq!(p, vec![(5, vec![1, 2])]),
            _ => panic!("palette attesa"),
        }
        client.request_chunk(Dim::Overworld, 3, -4);
        let ToGen::Generate { req, dim, x, z } = next_req(&mut s).await else { panic!() };
        assert_eq!((dim, x, z), (Dim::Overworld, 3, -4));
        write_frame(&mut s, &FromGen::Chunk { req, data: chunk(dim, x, z) }.encode()).await.unwrap();
        match ev(&mut rx).await {
            GenEvent::Chunk(c) => assert_eq!((c.x, c.z), (3, -4)),
            _ => panic!("chunk atteso"),
        }
        client.request_spawn(Dim::Nether);
        let ToGen::Spawn { req, .. } = next_req(&mut s).await else { panic!() };
        write_frame(&mut s, &FromGen::Spawn { req, x: 1, y: 2, z: 3 }.encode()).await.unwrap();
        assert!(matches!(ev(&mut rx).await, GenEvent::Spawn(Dim::Nether, 1, 2, 3)));
    }

    #[tokio::test]
    async fn generator_errors_are_reported_per_request() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, mut rx) = GenClient::spawn(cfg(l.local_addr().unwrap()));
        let mut s = accept(&l).await;
        client.request_chunk(Dim::Overworld, 3, -4);
        let ToGen::Generate { req, .. } = next_req(&mut s).await else { panic!() };
        write_frame(&mut s, &FromGen::Error { req, message: "boom".into() }.encode()).await.unwrap();
        client.request_spawn(Dim::Overworld);
        let ToGen::Spawn { req, .. } = next_req(&mut s).await else { panic!() };
        write_frame(&mut s, &FromGen::Error { req, message: "no spawn".into() }.encode()).await.unwrap();
        let mut seen = Vec::new();
        while seen.len() < 2 {
            match ev(&mut rx).await {
                GenEvent::ChunkFailed(dim, x, z, m) => seen.push(format!("chunk {dim:?} {x} {z} {m}")),
                GenEvent::SpawnFailed(dim, m) => seen.push(format!("spawn {dim:?} {m}")),
                _ => {}
            }
        }
        assert_eq!(seen, ["chunk Overworld 3 -4 boom", "spawn Overworld no spawn"]);
    }

    #[tokio::test]
    async fn duplicate_requests_are_sent_once() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, _rx) = GenClient::spawn(cfg(l.local_addr().unwrap()));
        let mut s = accept(&l).await;
        client.request_chunk(Dim::Overworld, 1, 1);
        client.request_chunk(Dim::Overworld, 1, 1);
        client.request_chunk(Dim::Overworld, 2, 2);
        let a = next_req(&mut s).await;
        let b = next_req(&mut s).await;
        assert!(matches!(a, ToGen::Generate { x: 1, z: 1, .. }));
        assert!(matches!(b, ToGen::Generate { x: 2, z: 2, .. }), "il duplicato non va inviato: {b:?}");
    }

    #[tokio::test]
    async fn pending_requests_are_resent_after_generator_restart() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, mut rx) = GenClient::spawn(cfg(l.local_addr().unwrap()));
        let mut s = accept(&l).await;
        client.request_chunk(Dim::Overworld, 9, 9);
        assert!(matches!(next_req(&mut s).await, ToGen::Generate { .. }));
        drop(s); // il generatore cade dopo la prima richiesta
        loop {
            if matches!(ev(&mut rx).await, GenEvent::Down(_)) {
                break;
            }
        }
        let mut s = accept(&l).await;
        let ToGen::Generate { req, dim, x, z } = next_req(&mut s).await else { panic!() };
        assert_eq!((x, z), (9, 9));
        write_frame(&mut s, &FromGen::Chunk { req, data: chunk(dim, x, z) }.encode()).await.unwrap();
        loop {
            if let GenEvent::Chunk(c) = ev(&mut rx).await {
                assert_eq!((c.x, c.z), (9, 9));
                break;
            }
        }
    }

    #[tokio::test]
    async fn missing_program_reports_down_without_panicking() {
        let cfg = GenConfig {
            launch: Launch::Process { program: PathBuf::from("questo-programma-non-esiste-xyz"), args_extra: vec![] },
            seed: 1,
            restart_delay: Duration::from_millis(50),
            request_timeout: REQUEST_TIMEOUT,
        };
        let (_client, mut rx) = GenClient::spawn(cfg);
        assert!(matches!(ev(&mut rx).await, GenEvent::Down(_)));
        assert!(matches!(ev(&mut rx).await, GenEvent::Down(_))); // continua a riprovare
    }

    #[tokio::test]
    #[ignore]
    async fn real_generator_process_serves_chunk_0_0() {
        let Ok(bin) = std::env::var("VERSMC_GEN") else {
            println!("VERSMC_GEN non impostata: test saltato");
            return;
        };
        let cfg = GenConfig {
            launch: Launch::Process { program: PathBuf::from(bin), args_extra: vec![] },
            seed: 13579,
            restart_delay: Duration::from_millis(500),
            request_timeout: REQUEST_TIMEOUT,
        };
        let (client, mut rx) = GenClient::spawn(cfg);
        let long = Duration::from_secs(120);
        assert!(matches!(timeout(long, rx.recv()).await.unwrap().unwrap(), GenEvent::Ready));
        assert!(matches!(timeout(long, rx.recv()).await.unwrap().unwrap(), GenEvent::Palette(_)));
        client.request_chunk(Dim::Overworld, 0, 0);
        let c = loop {
            if let GenEvent::Chunk(c) = timeout(long, rx.recv()).await.unwrap().unwrap() {
                break c;
            }
        };
        assert_eq!(c.sections.iter().map(|s| s.index).min(), Some(-4));
        drop(client);
        drop(rx);
        sleep(Duration::from_secs(2)).await;
        #[cfg(windows)]
        {
            let out = std::process::Command::new("tasklist").args(["/FI", "IMAGENAME eq versmc-gen.exe"]).output().unwrap();
            let out = String::from_utf8_lossy(&out.stdout).to_lowercase();
            assert!(!out.contains("versmc-gen.exe"), "processo del generatore ancora vivo: {out}");
        }
    }

    /// Generatore finto che muore (chiude la connessione) ogni volta che riceve il chunk (5, 5); gli altri li serve.
    async fn poison_generator(l: TcpListener, crashes: std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        loop {
            let mut s = accept(&l).await;
            loop {
                let body = match read_frame(&mut s).await {
                    Ok(b) => b,
                    Err(_) => break,
                };
                match ToGen::decode(&body).unwrap() {
                    ToGen::Generate { x: 5, z: 5, .. } => {
                        crashes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        break; // come un abort: la connessione cade
                    }
                    ToGen::Generate { req, dim, x, z } => write_frame(&mut s, &FromGen::Chunk { req, data: chunk(dim, x, z) }.encode()).await.unwrap(),
                    _ => {}
                }
            }
        }
    }

    #[tokio::test]
    async fn request_that_kills_the_generator_is_failed_after_three_crashes() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let crashes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gen = tokio::spawn(poison_generator(l, crashes.clone()));
        let (client, mut rx) = GenClient::spawn(cfg(addr));
        client.request_chunk(Dim::Overworld, 1, 1);
        client.request_chunk(Dim::Overworld, 5, 5);
        client.request_chunk(Dim::Overworld, 2, 2);
        let (mut served, mut failed, mut downs) = (Vec::new(), Vec::new(), 0);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while failed.is_empty() || served.len() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "il generatore è stato riavviato {downs} volte e la richiesta non è mai fallita");
            match ev(&mut rx).await {
                GenEvent::Chunk(c) => served.push((c.x, c.z)),
                GenEvent::ChunkFailed(_, x, z, _) => failed.push((x, z)),
                GenEvent::Down(_) => downs += 1,
                _ => {}
            }
        }
        served.sort();
        assert_eq!(failed, [(5, 5)], "la richiesta che uccide il generatore fallisce");
        assert_eq!(served, [(1, 1), (2, 2)], "le altre richieste vengono servite lo stesso");
        assert_eq!(crashes.load(std::sync::atomic::Ordering::SeqCst), MAX_CRASHES_PER_REQUEST as usize, "non viene più rimandata");
        // il client continua a servire richieste nuove
        client.request_chunk(Dim::Overworld, 7, 7);
        loop {
            if let GenEvent::Chunk(c) = ev(&mut rx).await {
                assert_eq!((c.x, c.z), (7, 7));
                break;
            }
        }
        gen.abort();
    }

    #[tokio::test]
    async fn unanswered_request_times_out_and_leaves_the_in_flight_set() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut c = cfg(l.local_addr().unwrap());
        c.request_timeout = Duration::from_millis(200);
        let (client, mut rx) = GenClient::spawn(c);
        let mut s = accept(&l).await; // il generatore c'è ma non risponde mai
        client.request_chunk(Dim::Overworld, 3, 3);
        client.request_spawn(Dim::Overworld);
        let ToGen::Generate { req: late, .. } = next_req(&mut s).await else { panic!() };
        assert!(matches!(next_req(&mut s).await, ToGen::Spawn { .. }));
        let (mut chunk_failed, mut spawn_failed) = (false, false);
        while !(chunk_failed && spawn_failed) {
            match ev(&mut rx).await {
                GenEvent::ChunkFailed(Dim::Overworld, 3, 3, _) => chunk_failed = true,
                GenEvent::SpawnFailed(Dim::Overworld, _) => spawn_failed = true,
                GenEvent::Down(_) => panic!("il generatore non è caduto"),
                _ => {}
            }
        }
        // la risposta tardiva viene ignorata
        write_frame(&mut s, &FromGen::Chunk { req: late, data: chunk(Dim::Overworld, 3, 3) }.encode()).await.unwrap();
        // e la stessa richiesta riparte (non è più "in volo")
        client.request_chunk(Dim::Overworld, 3, 3);
        assert!(matches!(next_req(&mut s).await, ToGen::Generate { x: 3, z: 3, .. }));
        loop {
            match ev(&mut rx).await {
                GenEvent::Chunk(_) => panic!("la risposta tardiva va ignorata"),
                GenEvent::ChunkFailed(Dim::Overworld, 3, 3, _) => break, // la seconda scade a sua volta
                _ => {}
            }
        }
    }
}
