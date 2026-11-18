//! Test del comando `versmc-gen serve`: lanciano il binario vero e parlano `vgen` su TCP.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use vgen::{Dim, FromGen, ToGen, VERSION, read_frame, write_frame};

const SEED: u64 = 13579;

/// Processo del generatore in ascolto e connessione già aperta.
struct Server {
    child: Child,
    stream: TcpStream,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Avvia `serve --port 0`, legge `READY <porta>` e si connette.
fn start() -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_versmc-gen"))
        .args(["serve", "--port", "0", "--seed", &SEED.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("il generatore deve avviarsi");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).expect("riga READY");
    let port: u16 = line.trim().strip_prefix("READY ").unwrap_or_else(|| panic!("attesa READY, arrivato {line:?}")).parse().expect("porta valida");
    let stream = TcpStream::connect(("127.0.0.1", port)).expect("connessione al generatore");
    Server { child, stream }
}

impl Server {
    fn send(&mut self, msg: &ToGen) {
        write_frame(&mut self.stream, &msg.encode()).expect("invio frame");
    }

    fn recv(&mut self) -> FromGen {
        let body = read_frame(&mut self.stream).expect("lettura frame");
        FromGen::decode(&body).expect("frame valido")
    }

    /// Handshake completo; restituisce la palette (hash -> nome Bedrock).
    fn hello(&mut self) -> HashMap<i32, String> {
        self.send(&ToGen::Hello { version: VERSION, seed: SEED });
        assert_eq!(self.recv(), FromGen::Hello { version: VERSION });
        match self.recv() {
            FromGen::Palette { entries } => entries.iter().map(|(hash, nbt)| (*hash, nbt_name(nbt))).collect(),
            other => panic!("attesa Palette, arrivato {other:?}"),
        }
    }

    fn chunk(&mut self, req: u32, x: i32, z: i32) -> vgen::ChunkData {
        self.send(&ToGen::Generate { req, dim: Dim::Overworld, x, z });
        match self.recv() {
            FromGen::Chunk { req: r, data } if r == req => data,
            other => panic!("atteso Chunk {req}, arrivato {other:?}"),
        }
    }
}

/// Legge il tag stringa `name` dall'NBT little-endian `{name, states}`.
fn nbt_name(nbt: &[u8]) -> String {
    let key = b"\x08\x04\x00name";
    let at = nbt.windows(key.len()).position(|w| w == key).expect("tag name") + key.len();
    let len = u16::from_le_bytes([nbt[at], nbt[at + 1]]) as usize;
    String::from_utf8(nbt[at + 2..at + 2 + len].to_vec()).unwrap()
}

/// Nome Bedrock del blocco in coordinate locali `(x, y, z)` del chunk (y assoluta).
fn block_name(chunk: &vgen::ChunkData, palette: &HashMap<i32, String>, x: usize, y: i32, z: usize) -> String {
    let index = y.div_euclid(16) as i8;
    let Some(section) = chunk.sections.iter().find(|s| s.index == index) else {
        return "minecraft:air".into();
    };
    let local = y.rem_euclid(16) as usize;
    let hash = section.palette[section.blocks[(x * 16 + z) * 16 + local] as usize];
    palette[&hash].clone()
}

#[test]
fn hello_then_chunk_0_0_has_bedrock_at_the_bottom() {
    let mut s = start();
    let palette = s.hello();
    let chunk = s.chunk(1, 0, 0);
    assert_eq!((chunk.dim, chunk.x, chunk.z), (Dim::Overworld, 0, 0));
    assert!(chunk.sections.iter().any(|sec| sec.index == -4), "manca la sezione -4");
    for x in 0..16 {
        for z in 0..16 {
            assert_eq!(block_name(&chunk, &palette, x, -64, z), "minecraft:bedrock", "colonna ({x}, {z})");
        }
    }
}

#[test]
fn ten_concurrent_requests_all_answer_with_their_req_id() {
    let mut s = start();
    s.hello();
    let mut expected = HashMap::new();
    for i in 0..10u32 {
        let (x, z) = (i as i32 % 5, i as i32 / 5 - 1);
        expected.insert(100 + i, (x, z));
        s.send(&ToGen::Generate { req: 100 + i, dim: Dim::Overworld, x, z });
    }
    let mut seen = HashSet::new();
    for _ in 0..10 {
        match s.recv() {
            FromGen::Chunk { req, data } => {
                assert_eq!(expected.get(&req), Some(&(data.x, data.z)), "req {req}");
                assert!(seen.insert(req), "req {req} ripetuta");
            }
            other => panic!("atteso Chunk, arrivato {other:?}"),
        }
    }
}

#[test]
fn spawn_is_on_solid_ground() {
    let mut s = start();
    let palette = s.hello();
    s.send(&ToGen::Spawn { req: 7, dim: Dim::Overworld });
    let (x, y, z) = match s.recv() {
        FromGen::Spawn { req: 7, x, y, z } => (x, y, z),
        other => panic!("atteso Spawn, arrivato {other:?}"),
    };
    let chunk = s.chunk(8, x >> 4, z >> 4);
    let below = block_name(&chunk, &palette, (x & 15) as usize, y - 1, (z & 15) as usize);
    assert!(!["minecraft:air", "minecraft:water", "minecraft:flowing_water"].contains(&below.as_str()), "sotto lo spawn ({x}, {y}, {z}) c'è {below}");
}

#[test]
fn wrong_version_gets_error_and_close() {
    let mut s = start();
    s.send(&ToGen::Hello { version: VERSION + 1, seed: SEED });
    assert!(matches!(s.recv(), FromGen::Error { req: 0, .. }));
    assert!(read_frame(&mut s.stream).is_err(), "la connessione doveva chiudersi");
}

#[test]
fn same_region_requests_generate_once() {
    let mut s = start();
    s.hello();
    let chunks = [(0, 0), (1, 0), (0, 1), (3, 3), (2, 1)];
    for (i, (x, z)) in chunks.iter().enumerate() {
        s.send(&ToGen::Generate { req: i as u32, dim: Dim::Overworld, x: *x, z: *z });
    }
    for _ in chunks {
        assert!(matches!(s.recv(), FromGen::Chunk { .. }));
    }
    // Chiudendo la connessione il generatore esce: lo stderr si legge fino in fondo.
    s.stream.shutdown(std::net::Shutdown::Both).unwrap();
    let mut log = String::new();
    s.child.stderr.take().unwrap().read_to_string(&mut log).unwrap();
    assert_eq!(log.matches("regione generata").count(), 1, "log del generatore:\n{log}");
}
