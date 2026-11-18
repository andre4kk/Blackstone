//! Client del generatore del mondo: avvia (o raggiunge) `versmc-gen`, parla il protocollo `vgen` su TCP locale
//! e riavvia/riconnette il generatore se cade, rimandando le richieste rimaste senza risposta.

#![allow(dead_code)] // `Launch::Connect` serve ai test

use bedrock_net::log;
use std::{collections::HashMap, io, net::SocketAddr, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{tcp::OwnedReadHalf, TcpStream},
    process::{Child, Command},
    sync::mpsc,
    time::{sleep, timeout},
};
use vgen::{Dim, FromGen, ToGen};

/// Tempo massimo per `READY` e per l'handshake: il generatore carica il Translator prima di rispondere all'Hello.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

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

struct State {
    inflight: HashMap<u32, Pending>,
    next_req: u32,
}

async fn run(cfg: GenConfig, mut rx: mpsc::UnboundedReceiver<Pending>, ev: mpsc::UnboundedSender<GenEvent>) {
    let mut st = State { inflight: HashMap::new(), next_req: 1 };
    loop {
        let reason = match session(&cfg, &mut rx, &ev, &mut st).await {
            Ok(()) => return, // il client è stato lasciato cadere
            Err(e) => e.to_string(),
        };
        log!("[gen] generatore non disponibile: {reason}");
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

    // Rimanda le richieste rimaste senza risposta (stessi id).
    let mut resend: Vec<(u32, Pending)> = st.inflight.iter().map(|(k, v)| (*k, *v)).collect();
    resend.sort_by_key(|(k, _)| *k);
    for (req, p) in resend {
        write_frame(&mut wr, &to_msg(req, p).encode()).await?;
    }

    // La lettura sta in un task a parte: `read_exact` non è cancellabile dentro `select!`.
    let (in_tx, mut in_rx) = mpsc::unbounded_channel();
    let reader = tokio::spawn(read_loop(rd, in_tx));
    let err = loop {
        tokio::select! {
            r = rx.recv() => {
                let Some(p) = r else {
                    reader.abort();
                    return Ok(());
                };
                if st.inflight.values().any(|q| *q == p) {
                    continue;
                }
                let req = st.next_req;
                st.next_req = st.next_req.wrapping_add(1).max(1);
                st.inflight.insert(req, p);
                if let Err(e) = write_frame(&mut wr, &to_msg(req, p).encode()).await {
                    break e;
                }
            }
            m = in_rx.recv() => match m {
                Some(Ok(msg)) => handle(msg, st, ev),
                Some(Err(e)) => break e,
                None => break io::Error::new(io::ErrorKind::UnexpectedEof, "connessione chiusa"),
            },
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
            if let Some(Pending::Spawn(dim)) = st.inflight.remove(&req) {
                let _ = ev.send(GenEvent::Spawn(dim, x, y, z));
            }
        }
        FromGen::Error { req, message } => {
            let what = st.inflight.remove(&req);
            log!("[gen] errore dal generatore (richiesta {req}, {what:?}): {message}");
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
        GenConfig { launch: Launch::Connect(addr), seed: 7, restart_delay: Duration::from_millis(50) }
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
        let cfg =
            GenConfig { launch: Launch::Process { program: PathBuf::from(bin), args_extra: vec![] }, seed: 13579, restart_delay: Duration::from_millis(500) };
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
}
