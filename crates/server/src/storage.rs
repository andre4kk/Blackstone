//! Salvataggio del mondo nel formato LevelDB di Bedrock (apribile dal gioco e da Amulet).
//!
//! Cartella del mondo: `level.dat` (NBT su disco con intestazione di 8 byte) e `db/` (LevelDB con compressione deflate raw, id 4).
//! Chiavi dei chunk: `x i32 LE`, `z i32 LE`, `[dimensione i32 LE se non è l'Overworld]`, tag, `[indice di sezione]`.
//!
//! Il formato è stato confrontato con PocketMine-MP (`src/world/format/io/leveldb/LevelDB.php`, commit
//! 6a7cc02e9dff59b69241aa0bcffdb9903ce86beb) e con Amulet-Level (`chunk_decode.cpp`, commit
//! 28e0674e1ac4f64717d8fd11ac16c6815ab583ae), in sola lettura: il codice qui è scritto da zero.
//!
//! Il LevelDB vive in un thread dedicato: `rusty-leveldb` non è `Send`, così `World` lo è. Le chiamate qui sono
//! sincrone: il server le fa dal thread del mondo (`disk.rs`), mai dal tick.
#![allow(dead_code)] // alcune funzioni (open, compact, block_hash_from_nbt) servono solo ai test
use crate::{
    nbt::{self, Enc, Reader, Tag},
    registry,
    world::{bits_for_palette, block_hash, block_nbt, pack_words, Chunk, State, BIT_WIDTHS, PLAINS},
};
use bedrock_net::log;
use rusty_leveldb::{Compressor, CompressorId, CompressorList, Options, Status, StatusCode, WriteBatch, DB};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    thread::{self, JoinHandle},
};
use vgen::{BlockEntity, ChunkData, Dim, Section};

/// Campo `version` delle voci di palette su disco. Viene da `canonical_block_states.nbt` di pmmp/BedrockData
/// (commit bdb44a48, "Updated from 1.26.30"): tutti i 16913 stati hanno `version` = 18168865 = 0x01153C21 (1.21.60.33).
/// Il gioco aggiorna da sé le versioni più vecchie al caricamento.
pub const BLOCK_STATE_VERSION: i32 = 18_168_865;
/// Inizio delle chiavi dei dati veri di un giocatore (il record `player_<id>` punta a una di queste).
const SERVER_PREFIX: &str = "player_server_";
/// Valore del tag `0x2C` (versione del chunk, 1.18.30+).
const CHUNK_VERSION: u8 = 40;
/// Versione dei sotto-chunk scritti (con indice di sezione nel corpo).
const SUBCHUNK_VERSION: u8 = 9;
/// Tag `0x36` = chunk completamente generato.
const FINALIZED: u8 = 2;
/// `StorageVersion` di `level.dat` e primo campo dell'intestazione del file.
const STORAGE_VERSION: u32 = 10;
/// Limite di sicurezza sulla dimensione di un blocco decompresso del database.
const MAX_BLOCK: usize = 64 << 20;

/// Tag delle chiavi di un chunk.
mod tag {
    pub const DATA_3D: u8 = 0x2b;
    pub const VERSION: u8 = 0x2c;
    pub const SUBCHUNK: u8 = 0x2f;
    pub const BLOCK_ENTITIES: u8 = 0x31;
    pub const FINALIZATION: u8 = 0x36;
}

// ---------- compressione ----------

/// Deflate raw (senza intestazione zlib), come i LevelDB di Bedrock: compressione di blocco con id 4.
#[derive(Debug, Clone, Copy, Default)]
pub struct BedrockDeflate;

impl CompressorId for BedrockDeflate {
    const ID: u8 = 4;
}

impl Compressor for BedrockDeflate {
    fn encode(&self, block: Vec<u8>) -> rusty_leveldb::Result<Vec<u8>> {
        Ok(miniz_oxide::deflate::compress_to_vec(&block, 6))
    }

    fn decode(&self, block: Vec<u8>) -> rusty_leveldb::Result<Vec<u8>> {
        miniz_oxide::inflate::decompress_to_vec_with_limit(&block, MAX_BLOCK).map_err(|e| Status::new(StatusCode::CompressionError, &format!("{e:?}")))
    }
}

/// Opzioni del database: scrive con deflate raw e legge sia quello sia i formati di default (nessuna, snappy).
fn db_options() -> Options {
    let mut list = CompressorList::default();
    list.set(BedrockDeflate);
    Options { create_if_missing: true, compressor: BedrockDeflate::ID, compressor_list: Rc::new(list), ..Options::default() }
}

// ---------- database in un thread ----------

enum Op {
    Get(Vec<u8>, mpsc::Sender<io::Result<Option<Vec<u8>>>>),
    /// Coppie chiave/valore; `None` cancella.
    Write(Vec<(Vec<u8>, Option<Vec<u8>>)>, mpsc::Sender<io::Result<()>>),
    Flush(mpsc::Sender<io::Result<()>>),
    Compact(mpsc::Sender<io::Result<()>>),
}

fn to_io(e: Status) -> io::Error {
    io::Error::other(e.to_string())
}

struct Db {
    tx: Option<mpsc::Sender<Op>>,
    thread: Option<JoinHandle<()>>,
}

impl Db {
    fn open(path: &Path) -> io::Result<Db> {
        let (tx, rx) = mpsc::channel::<Op>();
        let (ready_tx, ready_rx) = mpsc::channel();
        let path = path.to_path_buf();
        let thread = thread::Builder::new().name("leveldb".into()).spawn(move || {
            let mut db = match DB::open(&path, db_options()) {
                Ok(db) => db,
                Err(e) => return drop(ready_tx.send(Err(to_io(e)))),
            };
            let _ = ready_tx.send(Ok(()));
            while let Ok(op) = rx.recv() {
                match op {
                    Op::Get(key, reply) => {
                        let snapshot = db.get_snapshot();
                        let _ = reply.send(db.get_at(&snapshot, &key).map(|v| v.map(|b| b.to_vec())).map_err(to_io));
                    }
                    Op::Write(items, reply) => {
                        let mut batch = WriteBatch::default();
                        for (key, value) in &items {
                            match value {
                                Some(v) => batch.put(key, v),
                                None => batch.delete(key),
                            }
                        }
                        let _ = reply.send(db.write(batch, false).map_err(to_io));
                    }
                    Op::Flush(reply) => {
                        let _ = reply.send(db.flush().map_err(to_io));
                    }
                    Op::Compact(reply) => {
                        let _ = reply.send(db.compact_range(&[], &[0xff; 32]).map_err(to_io));
                    }
                }
            }
            // canale chiuso: `DB::drop` scarica il log e rilascia il lock
        })?;
        ready_rx.recv().map_err(|_| io::Error::other("thread del database terminato"))??;
        Ok(Db { tx: Some(tx), thread: Some(thread) })
    }

    fn call<T>(&self, op: impl FnOnce(mpsc::Sender<io::Result<T>>) -> Op) -> io::Result<T> {
        let closed = || io::Error::other("database chiuso");
        let (tx, rx) = mpsc::channel();
        self.tx.as_ref().ok_or_else(closed)?.send(op(tx)).map_err(|_| closed())?;
        rx.recv().map_err(|_| closed())?
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        self.tx = None; // il thread esce dal ciclo e chiude il database
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---------- chiavi, hash e palette ----------

/// Chiave di un chunk: `x`, `z`, la dimensione solo se non è l'Overworld, poi il tag.
fn chunk_key(dim: Dim, x: i32, z: i32, tag: u8) -> Vec<u8> {
    let mut key = Vec::with_capacity(14);
    key.extend_from_slice(&x.to_le_bytes());
    key.extend_from_slice(&z.to_le_bytes());
    if dim != Dim::Overworld {
        key.extend_from_slice(&(dim as i32).to_le_bytes());
    }
    key.push(tag);
    key
}

/// Nome e hash di rete di un compound `{name, states, ...}` (il resto, `version` compresa, è ignorato). Gli stati sono
/// ordinati da `block_hash`, quindi l'ordine su disco non conta.
pub(crate) fn block_from_tag(tag: &Tag) -> Option<(String, i32)> {
    let name = tag.get("name")?.as_str()?;
    let Tag::Compound(items) = tag.get("states")? else { return None };
    let mut states = Vec::with_capacity(items.len());
    for (key, value) in items {
        let state = match value {
            Tag::Byte(v) => State::Byte(*v as u8),
            Tag::Int(v) => State::Int(*v),
            Tag::String(s) => State::Str(s),
            _ => return None,
        };
        states.push((key.as_str(), state));
    }
    Some((name.to_string(), block_hash(name, &states)))
}

/// Hash di rete di uno stato di blocco a partire dal suo NBT su disco `{name, states, ...}`: lo stesso valore di
/// `world::block_hash`.
pub fn block_hash_from_nbt(nbt: &[u8]) -> Option<i32> {
    let (_, tag) = Reader::new(nbt, Enc::Disk).root()?;
    block_from_tag(&tag).map(|(_, hash)| hash)
}

fn air() -> i32 {
    block_hash("minecraft:air", &[])
}

fn water() -> i32 {
    block_hash("minecraft:water", &[("liquid_depth", State::Int(0))])
}

fn info_update() -> i32 {
    block_hash("minecraft:info_update", &[])
}

// ---------- lettura dei byte ----------

/// Cursore sui byte di un record.
struct Cur<'a>(&'a [u8]);

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if n > self.0.len() {
            return Err("dati troncati".into());
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// Una radice NBT su disco.
    fn nbt(&mut self) -> Result<Tag, String> {
        let mut r = Reader::new(self.0, Enc::Disk);
        let (_, tag) = r.root().ok_or("NBT non valido")?;
        self.0 = r.rest();
        Ok(tag)
    }

    /// Intestazione, parole e numero di voci di uno storage a palette; le voci restano da leggere.
    /// Restituisce gli indici delle 4096 celle e il numero di voci della palette.
    fn storage(&mut self) -> Result<(Box<[u16; 4096]>, usize), String> {
        let header = self.u8()?;
        if header & 1 != 0 {
            return Err("storage non persistente".into());
        }
        let bits = header >> 1;
        let mut indices = Box::new([0u16; 4096]);
        if bits == 0 {
            return Ok((indices, 1));
        }
        if !BIT_WIDTHS.contains(&bits) {
            return Err(format!("{bits} bit per voce non ammessi"));
        }
        let bits = bits as usize;
        let per_word = 32 / bits;
        let words = self.take(4096usize.div_ceil(per_word) * 4)?;
        let mask = (1u32 << bits) - 1;
        for (i, slot) in indices.iter_mut().enumerate() {
            let word = u32::from_le_bytes(words[i / per_word * 4..][..4].try_into().unwrap());
            *slot = ((word >> (i % per_word * bits)) & mask) as u16;
        }
        let count = self.i32()?;
        if !(1..=65536).contains(&count) {
            return Err(format!("palette di {count} voci"));
        }
        let count = count as usize;
        if indices.iter().any(|&i| i as usize >= count) {
            return Err("indice fuori dalla palette".into());
        }
        Ok((indices, count))
    }
}

/// UUID casuale (versione 4) in forma testuale.
fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("generatore casuale non disponibile");
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Scritture chiave/valore per il database (`None` cancella).
type WriteOps = Vec<(Vec<u8>, Option<Vec<u8>>)>;

/// Il record `player_<identity>` com'è sul disco.
#[derive(Debug, PartialEq)]
enum Lookup {
    /// Il giocatore non ha mai salvato nulla.
    Missing,
    /// Il record è buono e punta a questa chiave dei dati.
    Found(String),
    /// Il record c'è ma non è NBT, o non ha un `ServerId` che cominci per `player_server_`: i suoi byte originali.
    Unreadable(Vec<u8>),
}

/// Interpreta i byte del record `player_<identity>`. La chiave dei dati deve essere una `player_server_*`: un record alterato
/// non può puntare ai chunk o ad altro.
fn classify_record(bytes: Option<Vec<u8>>) -> Lookup {
    let Some(bytes) = bytes else { return Lookup::Missing };
    let id = Reader::new(&bytes, Enc::Disk).root().and_then(|(_, record)| record.get("ServerId").and_then(Tag::as_str).map(str::to_string));
    match id {
        Some(id) if id.starts_with(SERVER_PREFIX) => Lookup::Found(id),
        _ => Lookup::Unreadable(bytes),
    }
}

/// Cosa serve scrivere, oltre ai dati, per salvare `identity` data la lettura del suo record: la chiave dei dati e le scritture
/// del record. `Err` se la lettura è fallita (errore del database: non si inventa un `ServerId`). Un record illeggibile si copia
/// prima, tale e quale, sotto `player_server_<uuid>_illeggibile` (con un UUID nuovo) e si rifà; uno buono si lascia com'è.
fn save_plan(identity: &str, lookup: Result<Lookup, String>, new_uuid: &mut dyn FnMut() -> String) -> io::Result<(String, WriteOps)> {
    let lookup = lookup.map_err(|e| io::Error::other(format!("lettura del record di {identity}: {e}")))?;
    let mut ops = Vec::with_capacity(3);
    let server_id = match lookup {
        Lookup::Found(id) => return Ok((id, ops)),
        Lookup::Missing => format!("{SERVER_PREFIX}{}", new_uuid()),
        Lookup::Unreadable(bytes) => {
            let backup = format!("{SERVER_PREFIX}{}_illeggibile", new_uuid());
            log!("[mondo] record di {identity} illeggibile: copia in {backup}, poi lo rifaccio");
            ops.push((backup.into_bytes(), Some(bytes)));
            format!("{SERVER_PREFIX}{}", new_uuid())
        }
    };
    // L'XUID è l'`MsaId`; senza account Xbox l'identità è una `name:<nome>` e va in `SelfSignedId` (non verificato su un mondo vero).
    let (msa, self_signed) = if identity.starts_with("name:") { ("", identity) } else { (identity, "") };
    let record = Tag::Compound(vec![
        ("MsaId".into(), Tag::String(msa.into())),
        ("SelfSignedId".into(), Tag::String(self_signed.into())),
        ("ServerId".into(), Tag::String(server_id.clone())),
    ]);
    ops.push((format!("player_{identity}").into_bytes(), Some(nbt::write_root("", &record, Enc::Disk))));
    Ok((server_id, ops))
}

/// Secondi dall'epoca Unix, per `LastPlayed`.
fn unix_time() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

// ---------- mondo ----------

pub struct World {
    dir: PathBuf,
    db: Db,
    /// Contenuto di `level.dat`.
    level: Tag,
    /// Tabella inversa: hash di rete -> NBT su disco `{name, states, version}`.
    palette: HashMap<i32, Vec<u8>>,
    /// Hash sconosciuti già segnalati.
    warned: HashSet<i32>,
}

impl World {
    /// Apre il mondo in `dir` (creandolo se manca, con il nome della cartella e un seme casuale).
    pub fn open(dir: &Path) -> io::Result<World> {
        let name = dir.file_name().map_or_else(|| "Mondo".to_string(), |n| n.to_string_lossy().into_owned());
        World::open_with(dir, &name, None)
    }

    /// Come `open`, con il nome e il seme da usare se il mondo va creato. Un mondo esistente tiene il proprio seme.
    pub fn open_with(dir: &Path, name: &str, seed: Option<u64>) -> io::Result<World> {
        fs::create_dir_all(dir)?;
        let level = match fs::read(dir.join("level.dat")) {
            Ok(raw) => Self::parse_level(&raw)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let seed = seed.unwrap_or_else(|| {
                    let mut b = [0u8; 8];
                    getrandom::fill(&mut b).expect("generatore casuale non disponibile");
                    u64::from_le_bytes(b) & i64::MAX as u64 // resta esprimibile in server.toml
                });
                let _ = fs::write(dir.join("levelname.txt"), name);
                Self::new_level(name, seed)
            }
            Err(e) => return Err(e),
        };
        let db = Db::open(&dir.join("db"))?;
        let mut world = World { dir: dir.to_path_buf(), db, level, palette: HashMap::new(), warned: HashSet::new() };
        world.write_level()?;
        // Blocchi del mondo piatto di ripiego e dello strato dell'acqua: si salvano anche senza la tabella del generatore.
        let known: [(&str, &[(&str, State)]); 6] = [
            ("minecraft:air", &[]),
            ("minecraft:water", &[("liquid_depth", State::Int(0))]),
            ("minecraft:bedrock", &[("infiniburn_bit", State::Byte(0))]),
            ("minecraft:dirt", &[]),
            ("minecraft:grass_block", &[]),
            ("minecraft:info_update", &[]),
        ];
        for (name, states) in known {
            world.register(block_hash(name, states), &block_nbt(name, states));
        }
        Ok(world)
    }

    fn new_level(name: &str, seed: u64) -> Tag {
        // versione del client come lista di 5 interi, da "1.26.63"
        let mut version: Vec<i32> = bedrock_protocol::VERSION.split('.').filter_map(|p| p.parse().ok()).collect();
        version.resize(5, 0);
        Tag::Compound(vec![
            ("LevelName".into(), Tag::String(name.into())),
            ("RandomSeed".into(), Tag::Long(seed as i64)),
            ("StorageVersion".into(), Tag::Int(STORAGE_VERSION as i32)),
            ("NetworkVersion".into(), Tag::Int(bedrock_protocol::PROTOCOL as i32)),
            ("Generator".into(), Tag::Int(1)), // infinito
            ("GameType".into(), Tag::Int(0)),
            ("Difficulty".into(), Tag::Int(2)),
            ("LastPlayed".into(), Tag::Long(unix_time())),
            ("lastOpenedWithVersion".into(), Tag::List(3, version.into_iter().map(Tag::Int).collect())),
            ("commandsEnabled".into(), Tag::Byte(1)),
        ])
    }

    /// `level.dat`: `[versione u32 LE][lunghezza u32 LE]` e NBT su disco.
    fn parse_level(raw: &[u8]) -> io::Result<Tag> {
        let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, format!("level.dat: {m}"));
        if raw.len() < 8 {
            return Err(bad("troncato"));
        }
        let len = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
        let body = raw[8..].get(..len).ok_or_else(|| bad("lunghezza oltre il file"))?;
        match Reader::new(body, Enc::Disk).root() {
            Some((_, tag @ Tag::Compound(_))) => Ok(tag),
            _ => Err(bad("NBT non valido")),
        }
    }

    fn write_level(&self) -> io::Result<()> {
        let body = nbt::write_root("", &self.level, Enc::Disk);
        let mut file = Vec::with_capacity(body.len() + 8);
        file.extend_from_slice(&STORAGE_VERSION.to_le_bytes());
        file.extend_from_slice(&(body.len() as u32).to_le_bytes());
        file.extend_from_slice(&body);
        let tmp = self.dir.join("level.dat.tmp");
        fs::write(&tmp, file)?;
        fs::rename(tmp, self.dir.join("level.dat"))
    }

    pub fn seed(&self) -> u64 {
        self.level.get("RandomSeed").and_then(Tag::as_i64).unwrap_or(0) as u64
    }

    pub fn spawn(&self) -> Option<(i32, i32, i32)> {
        let get = |k: &str| self.level.get(k).and_then(Tag::as_i32);
        Some((get("SpawnX")?, get("SpawnY")?, get("SpawnZ")?))
    }

    pub fn set_spawn(&mut self, x: i32, y: i32, z: i32) {
        for (key, v) in [("SpawnX", x), ("SpawnY", y), ("SpawnZ", z)] {
            self.level.set(key, Tag::Int(v));
        }
        if let Err(e) = self.write_level() {
            log!("[mondo] impossibile salvare level.dat: {e}");
        }
    }

    /// Imposta la tabella inversa hash -> `{name, states}` mandata dal generatore (si aggiunge a quella già nota).
    /// Alle voci si aggiunge il campo `version`.
    pub fn set_palette(&mut self, entries: Vec<(i32, Vec<u8>)>) {
        for (hash, nbt) in entries {
            self.register(hash, &nbt);
        }
    }

    fn register(&mut self, hash: i32, nbt: &[u8]) {
        match Reader::new(nbt, Enc::Disk).root() {
            Some((_, mut tag @ Tag::Compound(_))) => {
                if tag.get("version").is_none() {
                    tag.set("version", Tag::Int(BLOCK_STATE_VERSION));
                }
                self.palette.insert(hash, nbt::write_root("", &tag, Enc::Disk));
            }
            _ => log!("[mondo] voce di palette {hash} non valida, ignorata"),
        }
    }

    /// NBT su disco di uno stato (dalla tabella del generatore o, in mancanza, dal registro dei blocchi). Un hash sconosciuto a entrambi
    /// diventa `minecraft:info_update` (segnalato una volta per hash).
    fn entry(&mut self, hash: i32) -> &[u8] {
        if !self.palette.contains_key(&hash) {
            // non è nella tabella del generatore: ripiego sul registro dei blocchi
            if let Some(state) = registry::blocks().state(hash) {
                self.register(hash, &state.nbt());
            }
        }
        let known = self.palette.contains_key(&hash);
        if !known && self.warned.insert(hash) {
            log!("[mondo] blocco sconosciuto (hash {hash}): salvato come minecraft:info_update");
        }
        &self.palette[&if known { hash } else { info_update() }]
    }

    fn db_get(&self, key: &[u8]) -> io::Result<Option<Vec<u8>>> {
        self.db.call(|reply| Op::Get(key.to_vec(), reply))
    }

    fn db_put(&mut self, key: &[u8], value: &[u8]) -> io::Result<()> {
        self.db.call(|reply| Op::Write(vec![(key.to_vec(), Some(value.to_vec()))], reply))
    }

    /// Porta il contenuto in memoria in file `.ldb` compressi.
    fn compact(&mut self) -> io::Result<()> {
        self.db.call(Op::Compact)
    }

    /// Scarica le scritture sul disco.
    pub fn flush(&mut self) -> io::Result<()> {
        self.db.call(Op::Flush)
    }

    // ---------- dati dei giocatori ----------

    /// Il record `player_<identity>` com'è sul disco. `Err` solo se il database non risponde (un errore che può passare: non dice
    /// nulla sul record). Un record presente ma che non si interpreta non è un errore: è `Lookup::Unreadable`, coi suoi byte.
    fn player_lookup(&self, identity: &str) -> Result<Lookup, String> {
        let bytes = self.db_get(format!("player_{identity}").as_bytes()).map_err(|e| format!("database: {e}"))?;
        Ok(classify_record(bytes))
    }

    /// Dati salvati di un giocatore (NBT su disco, tali e quali). `None` se non ne ha mai salvati o non si leggono (con una riga di log).
    pub fn load_player(&mut self, identity: &str) -> Option<Vec<u8>> {
        let result = self.player_lookup(identity).and_then(|lookup| match lookup {
            Lookup::Found(id) => self.db_get(id.as_bytes()).map_err(|e| format!("database: {e}")),
            Lookup::Missing => Ok(None),
            Lookup::Unreadable(_) => Err("record del giocatore illeggibile o senza ServerId valido".into()),
        });
        result.unwrap_or_else(|e| {
            log!("[mondo] dati di {identity} illeggibili, si parte da capo: {e}");
            None
        })
    }

    /// Salva i dati del giocatore come i server dedicati: il record `player_<identity>` (`MsaId`, `SelfSignedId`, `ServerId`) punta a
    /// `player_server_<uuid>` con i dati veri. L'UUID si genera la prima volta e poi si riusa. Se il database non risponde alla
    /// lettura del record il salvataggio fallisce senza toccare nulla (un nuovo `ServerId` staccherebbe il giocatore dai suoi dati).
    /// Se il record c'è ma non si interpreta, prima di rifarlo se ne copiano i byte sotto la chiave di riserva
    /// `player_server_<uuid>_illeggibile` (vedi [`save_plan`]), così nulla va perso.
    pub fn save_player(&mut self, identity: &str, nbt: &[u8]) -> io::Result<()> {
        self.save_player_with(identity, nbt, &mut uuid_v4)
    }

    /// Come [`World::save_player`], con il generatore di UUID a scelta (i test lo fissano).
    fn save_player_with(&mut self, identity: &str, nbt: &[u8], new_uuid: &mut dyn FnMut() -> String) -> io::Result<()> {
        let lookup = self.player_lookup(identity);
        let (server_id, mut ops) = save_plan(identity, lookup, new_uuid)?;
        ops.push((server_id.into_bytes(), Some(nbt.to_vec())));
        self.db.call(|reply| Op::Write(ops, reply))
    }

    // ---------- scrittura dei chunk ----------

    /// Scrive uno storage a palette su disco: intestazione `bits << 1` (bit basso 0 = persistente), parole, numero di voci
    /// come `i32` LE (solo con più di una voce) e le voci. Con 0 bit segue direttamente la voce unica.
    fn put_storage(out: &mut Vec<u8>, n: usize, index_at: impl Fn(usize) -> usize, mut entry: impl FnMut(&mut Vec<u8>, usize)) {
        let bits = bits_for_palette(n);
        out.push(bits << 1);
        pack_words(bits, index_at).into_iter().for_each(|w| out.extend_from_slice(&w.to_le_bytes()));
        if bits > 0 {
            out.extend_from_slice(&(n as i32).to_le_bytes());
        }
        (0..n).for_each(|i| entry(out, i));
    }

    /// Sotto-chunk v9: blocchi e, se c'è, strato dell'acqua (palette aria/acqua).
    fn encode_section(&mut self, s: &Section) -> Vec<u8> {
        let mut out = vec![SUBCHUNK_VERSION, if s.water.is_some() { 2 } else { 1 }, s.index as u8];
        let palette = if s.palette.is_empty() { vec![air()] } else { s.palette.clone() };
        Self::put_storage(&mut out, palette.len(), |n| s.blocks[n] as usize, |out, i| out.extend_from_slice(self.entry(palette[i])));
        if let Some(mask) = &s.water {
            let pair = [air(), water()];
            Self::put_storage(&mut out, 2, |n| (mask[n / 64] >> (n % 64) & 1) as usize, |out, i| out.extend_from_slice(self.entry(pair[i])));
        }
        out
    }

    /// Altezza del blocco più alto non d'aria (acqua compresa) di ogni colonna, relativa al fondo della dimensione;
    /// 0 se la colonna è vuota. Indice `z * 16 + x`.
    fn heightmap(chunk: &Chunk) -> [i16; 256] {
        let (min_y, _) = chunk.dim.height_range();
        let air = air();
        let mut heights = [0i16; 256];
        for x in 0..16 {
            for z in 0..16 {
                'column: for s in chunk.sections.iter().rev().flatten() {
                    for y in (0..16).rev() {
                        let n = (x * 16 + z) * 16 + y;
                        let wet = s.water.as_ref().is_some_and(|m| m[n / 64] >> (n % 64) & 1 == 1);
                        if wet || s.palette.get(s.blocks[n] as usize).is_some_and(|&h| h != air) {
                            heights[z * 16 + x] = (s.index as i32 * 16 + y as i32 - min_y) as i16;
                            break 'column;
                        }
                    }
                }
            }
        }
        heights
    }

    /// Data3D: mappa delle altezze (256 `i16` LE) e uno storage di biomi per ogni sezione della dimensione, con id `i32` LE.
    /// Le sezioni mancanti ripetono il bioma dell'ultima cella della presente più vicina sotto (come sulla rete).
    fn encode_data3d(chunk: &Chunk) -> Vec<u8> {
        let mut out: Vec<u8> = Self::heightmap(chunk).iter().flat_map(|h| h.to_le_bytes()).collect();
        let mut below = PLAINS as u32;
        for slot in &chunk.sections {
            match slot {
                Some(s) => {
                    let mut palette: Vec<u32> = Vec::new();
                    let idx: Vec<usize> =
                        s.biomes.iter().map(|&b| palette.iter().position(|&p| p == b).unwrap_or_else(|| (palette.push(b), palette.len() - 1).1)).collect();
                    let cell = |n: usize| {
                        let (x, z, y) = (n / 256, n / 16 % 16, n % 16);
                        idx[(x / 4 * 4 + z / 4) * 4 + y / 4]
                    };
                    Self::put_storage(&mut out, palette.len(), cell, |out, i| out.extend_from_slice(&(palette[i] as i32).to_le_bytes()));
                    below = s.biomes[3];
                }
                None => Self::put_storage(&mut out, 1, |_| 0, |out, _| out.extend_from_slice(&(below as i32).to_le_bytes())),
            }
        }
        out
    }

    /// Block entity: NBT su disco concatenati (quelle con NBT di rete illeggibile si saltano).
    fn encode_block_entities(chunk: &Chunk) -> Vec<u8> {
        let mut out = Vec::new();
        for be in &chunk.block_entities {
            match Reader::new(&be.nbt, Enc::Network).root() {
                Some((name, tag)) => out.extend(nbt::write_root(&name, &tag, Enc::Disk)),
                None => log!("[mondo] block entity in {},{},{} con NBT illeggibile: non salvata", be.x, be.y, be.z),
            }
        }
        out
    }

    pub fn save(&mut self, chunk: &Chunk) -> io::Result<()> {
        let (dim, x, z) = (chunk.dim, chunk.x, chunk.z);
        let key = |t: u8| chunk_key(dim, x, z, t);
        let mut ops = vec![(key(tag::VERSION), Some(vec![CHUNK_VERSION]))];
        let (lowest, _) = Chunk::section_range(dim);
        for (slot, section) in chunk.sections.iter().enumerate() {
            let mut k = key(tag::SUBCHUNK);
            match section {
                Some(s) => {
                    k.push(s.index as u8);
                    ops.push((k, Some(self.encode_section(s))));
                }
                None => {
                    k.push((lowest + slot as i32) as i8 as u8);
                    ops.push((k, None)); // una sezione svuotata non deve lasciare dati vecchi
                }
            }
        }
        ops.push((key(tag::DATA_3D), Some(Self::encode_data3d(chunk))));
        let entities = Self::encode_block_entities(chunk);
        ops.push((key(tag::BLOCK_ENTITIES), (!entities.is_empty()).then_some(entities)));
        ops.push((key(tag::FINALIZATION), Some(vec![FINALIZED])));
        self.db.call(|reply| Op::Write(ops, reply))
    }

    // ---------- lettura dei chunk ----------

    /// Sotto-chunk v1, v8 o v9. Dal primo strato i blocchi; dal secondo (se c'è) i blocchi d'acqua.
    fn decode_section(bytes: &[u8], index: i8) -> Result<Section, String> {
        let mut c = Cur(bytes);
        let version = c.u8()?;
        let layers = match version {
            1 => 1,
            8 | 9 => c.u8()?,
            v => return Err(format!("versione {v} non supportata")),
        };
        if version == 9 {
            c.u8()?; // indice di sezione, già nella chiave
        }
        let mut section = Section { index, palette: vec![air()], blocks: Box::new([0; 4096]), biomes: Box::new([PLAINS as u32; 64]), water: None };
        for layer in 0..layers.min(2) {
            let (indices, count) = c.storage()?;
            let entries = (0..count)
                .map(|_| c.nbt().and_then(|t| block_from_tag(&t).ok_or_else(|| "voce di palette non valida".to_string())))
                .collect::<Result<Vec<_>, _>>()?;
            if layer == 0 {
                section.palette = entries.iter().map(|e| e.1).collect();
                section.blocks = indices;
            } else {
                let mut mask = Box::new([0u64; 64]);
                for (n, &i) in indices.iter().enumerate() {
                    if entries[i as usize].0 == "minecraft:water" {
                        mask[n / 64] |= 1 << (n % 64);
                    }
                }
                section.water = mask.iter().any(|&w| w != 0).then_some(mask);
            }
        }
        Ok(section)
    }

    /// Biomi di Data3D: per ogni sezione una griglia di 64 celle (4x4x4), campionando l'angolo di ogni cella.
    fn decode_biomes(bytes: &[u8], sections: usize) -> Result<Vec<[u32; 64]>, String> {
        let mut c = Cur(bytes);
        c.take(512)?; // mappa delle altezze
        let mut out: Vec<[u32; 64]> = Vec::new();
        while out.len() < sections && !c.0.is_empty() {
            if c.0[0] >> 1 == 127 {
                // 0xFF: uguale alla sezione precedente
                c.u8()?;
                out.push(*out.last().ok_or("nessuna sezione precedente da copiare")?);
                continue;
            }
            let (indices, count) = c.storage()?;
            let palette = (0..count).map(|_| c.i32()).collect::<Result<Vec<_>, _>>()?;
            let mut cells = [0u32; 64];
            for (cell, slot) in cells.iter_mut().enumerate() {
                let (cx, cz, cy) = (cell / 16, cell / 4 % 4, cell % 4);
                *slot = palette[indices[(cx * 4 * 16 + cz * 4) * 16 + cy * 4] as usize] as u32;
            }
            out.push(cells);
        }
        Ok(out)
    }

    /// Block entity: NBT su disco concatenati; `x`, `y`, `z` dal compound.
    fn decode_block_entities(bytes: &[u8]) -> Result<Vec<BlockEntity>, String> {
        let mut c = Cur(bytes);
        let mut out = Vec::new();
        while !c.0.is_empty() {
            let tag = c.nbt()?;
            let coord = |k: &str| tag.get(k).and_then(Tag::as_i32);
            let (Some(x), Some(y), Some(z)) = (coord("x"), coord("y"), coord("z")) else {
                return Err("block entity senza coordinate".into());
            };
            out.push(BlockEntity { x, y, z, nbt: nbt::write_root("", &tag, Enc::Network) });
        }
        Ok(out)
    }

    /// Carica un chunk: `Ok(None)` se non è mai stato salvato, `Err` (con la causa) se i dati sono illeggibili.
    fn try_load(&mut self, dim: Dim, x: i32, z: i32) -> Result<Option<Chunk>, String> {
        let db_err = |e: io::Error| format!("database: {e}");
        let key = |t: u8| chunk_key(dim, x, z, t);
        if self.db_get(&key(tag::VERSION)).map_err(db_err)?.is_none() {
            return Ok(None);
        }
        let (lowest, count) = Chunk::section_range(dim);
        let mut sections = Vec::new();
        for slot in 0..count as i32 {
            let index = (lowest + slot) as i8;
            let mut k = key(tag::SUBCHUNK);
            k.push(index as u8);
            if let Some(bytes) = self.db_get(&k).map_err(db_err)? {
                sections.push(Self::decode_section(&bytes, index).map_err(|e| format!("sotto-chunk {index}: {e}"))?);
            }
        }
        if let Some(bytes) = self.db_get(&key(tag::DATA_3D)).map_err(db_err)? {
            let biomes = Self::decode_biomes(&bytes, count).map_err(|e| format!("Data3D: {e}"))?;
            for s in &mut sections {
                if let Some(cells) = biomes.get((s.index as i32 - lowest) as usize) {
                    *s.biomes = *cells;
                }
            }
        }
        let block_entities = match self.db_get(&key(tag::BLOCK_ENTITIES)).map_err(db_err)? {
            Some(bytes) => Self::decode_block_entities(&bytes).map_err(|e| format!("block entity: {e}"))?,
            None => Vec::new(),
        };
        Ok(Some(Chunk::from_gen(ChunkData { dim, x, z, sections, block_entities })))
    }

    /// Carica un chunk salvato. `None` se manca o è illeggibile (in quel caso con una riga di log).
    pub fn load(&mut self, dim: Dim, x: i32, z: i32) -> Option<Chunk> {
        self.try_load(dim, x, z).unwrap_or_else(|e| {
            log!("[mondo] chunk {x},{z} ({dim:?}) illeggibile, verrà rigenerato: {e}");
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };
    use vgen::{BlockEntity, ChunkData, Section};

    /// Cartella temporanea di un test, rimossa alla fine.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!("versmc-test-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn air() -> i32 {
        block_hash("minecraft:air", &[])
    }

    fn stone() -> i32 {
        block_hash("minecraft:stone", &[])
    }

    fn log_y() -> i32 {
        block_hash("minecraft:oak_log", &[("pillar_axis", State::Str("y"))])
    }

    fn furnace() -> i32 {
        block_hash("minecraft:lit_furnace", &[("facing_direction", State::Int(2))])
    }

    /// Palette inversa come la manderebbe il generatore, con NBT `{name, states}` senza `version`.
    fn generator_palette() -> Vec<(i32, Vec<u8>)> {
        vec![
            (stone(), block_nbt("minecraft:stone", &[])),
            (log_y(), block_nbt("minecraft:oak_log", &[("pillar_axis", State::Str("y"))])),
            (furnace(), block_nbt("minecraft:lit_furnace", &[("facing_direction", State::Int(2))])),
        ]
    }

    fn section(index: i8, palette: Vec<i32>, f: impl Fn(usize) -> u16) -> Section {
        let mut blocks = Box::new([0u16; 4096]);
        blocks.iter_mut().enumerate().for_each(|(i, b)| *b = f(i));
        Section { index, palette, blocks, biomes: Box::new([PLAINS as u32; 64]), water: None }
    }

    /// Chunk vario: sezioni -4, -3 e 0 (le altre vuote), biomi, acqua, block entity con NBT di rete.
    fn sample_chunk(dim: Dim, x: i32, z: i32) -> Chunk {
        use crate::nbt::{write_root, Enc, Tag};
        let mut low = section(-4, vec![air(), stone(), log_y()], |i| (i % 3) as u16);
        low.biomes.iter_mut().enumerate().for_each(|(i, b)| *b = (i % 5) as u32 + 1);
        let mut wet = section(-3, vec![stone()], |_| 0);
        let mut mask = Box::new([0u64; 64]);
        mask[3] = 0xf0f0;
        wet.water = Some(mask);
        let top = section(0, vec![air(), furnace()], |i| (i % 7 == 0) as u16);
        let sign = Tag::Compound(vec![
            ("id".into(), Tag::String("Sign".into())),
            ("x".into(), Tag::Int(x * 16 + 3)),
            ("y".into(), Tag::Int(-60)),
            ("z".into(), Tag::Int(z * 16 + 4)),
            ("Text".into(), Tag::String("ciao".into())),
        ]);
        let nbt = write_root("", &sign, Enc::Network);
        Chunk::from_gen(ChunkData { dim, x, z, sections: vec![low, wet, top], block_entities: vec![BlockEntity { x: x * 16 + 3, y: -60, z: z * 16 + 4, nbt }] })
    }

    fn open(dir: &TempDir) -> World {
        let mut w = World::open_with(&dir.0, "Prova", Some(42)).unwrap();
        w.set_palette(generator_palette());
        w
    }

    #[test]
    fn chest_content_survives_save_and_reopen() {
        use crate::{chest::ChestData, inventory::Item};
        let dir = TempDir::new("chest");
        let mut chunk = sample_chunk(Dim::Overworld, 1, 2);
        let mut data = ChestData::empty();
        data.items[0] = Item::from_name("minecraft:cobblestone", 33).unwrap();
        data.items[26] = Item::from_name("minecraft:iron_pickaxe", 1).unwrap();
        data.name = Some("Tesoro".into());
        data.pair = Some([17, 33]);
        chunk.put_block_entity(data.entity([16, 5, 32]));
        let mut w = open(&dir);
        w.save(&chunk).unwrap();
        w.flush().unwrap();
        drop(w);
        let mut w = open(&dir);
        let loaded = w.load(Dim::Overworld, 1, 2).expect("chunk salvato");
        let be = loaded.block_entities.iter().find(|b| [b.x, b.y, b.z] == [16, 5, 32]).expect("il forziere c'è ancora");
        assert_eq!(ChestData::parse(&be.nbt), data, "oggetti, nome e abbinamento tornano uguali");
    }

    #[test]
    fn furnace_content_and_times_survive_save_and_reopen() {
        use crate::{furnace::FurnaceData, inventory::Item};
        let dir = TempDir::new("furnace");
        let mut chunk = sample_chunk(Dim::Overworld, 1, 2);
        let mut data = FurnaceData::empty();
        data.items[0] = Item::from_name("minecraft:raw_iron", 7).unwrap();
        data.items[1] = Item::from_name("minecraft:coal", 3).unwrap();
        data.items[2] = Item::from_name("minecraft:iron_ingot", 2).unwrap();
        (data.burn_time, data.burn_duration, data.cook_time, data.xp) = (1234, 1600, 57, 4);
        chunk.put_block_entity(data.entity([16, 5, 32]));
        let mut w = open(&dir);
        w.save(&chunk).unwrap();
        w.flush().unwrap();
        drop(w);
        let mut w = open(&dir);
        let loaded = w.load(Dim::Overworld, 1, 2).expect("chunk salvato");
        let be = loaded.block_entities.iter().find(|b| [b.x, b.y, b.z] == [16, 5, 32]).expect("la fornace c'è ancora");
        assert_eq!(FurnaceData::parse(&be.nbt), data, "oggetti e tempi tornano uguali");
    }

    #[test]
    fn chunk_survives_save_and_reopen() {
        let dir = TempDir::new("survive");
        let original = sample_chunk(Dim::Overworld, 5, -7);
        let nether = sample_chunk(Dim::Nether, 5, -7);
        let mut w = open(&dir);
        w.save(&original).unwrap();
        w.save(&nether).unwrap();
        w.flush().unwrap();
        drop(w);

        let mut w = open(&dir);
        let loaded = w.load(Dim::Overworld, 5, -7).expect("chunk del Overworld");
        assert_eq!(loaded.network_bytes(), original.network_bytes());
        assert_eq!(loaded.sub_chunk_count(), 5, "fino alla sezione 0 inclusa");
        assert_eq!(loaded.block_entities, original.block_entities);
        assert!(!loaded.dirty);
        let other = w.load(Dim::Nether, 5, -7).expect("chunk del Nether");
        assert_eq!(other.dim, Dim::Nether);
        assert!(w.load(Dim::End, 5, -7).is_none(), "mai salvato");
        assert!(w.load(Dim::Overworld, 6, -7).is_none());
    }

    #[test]
    fn flat_chunk_survives_without_generator_palette() {
        let dir = TempDir::new("flat");
        let flat = Chunk::flat(Dim::Overworld, 1, 1);
        let mut w = World::open_with(&dir.0, "Prova", None).unwrap();
        w.save(&flat).unwrap();
        drop(w);
        let mut w = World::open(&dir.0).unwrap();
        assert_eq!(w.load(Dim::Overworld, 1, 1).unwrap().network_bytes(), flat.network_bytes());
    }

    #[test]
    fn save_and_load_block_not_in_generator_palette() {
        use crate::registry::{blocks, StateValue};
        let dir = TempDir::new("fallback");
        let stairs = blocks().state(blocks().default_hash("minecraft:oak_stairs").unwrap()).unwrap().with("weirdo_direction", StateValue::Int(3));
        assert!(blocks().is_valid(&stairs));
        let hash = stairs.hash();
        let chunk = Chunk::from_gen(ChunkData {
            dim: Dim::Overworld,
            x: 2,
            z: 3,
            sections: vec![section(0, vec![air(), hash], |i| (i % 2) as u16)],
            block_entities: Vec::new(),
        });
        let mut w = World::open_with(&dir.0, "Prova", None).unwrap(); // nessuna palette del generatore
        w.save(&chunk).unwrap();
        drop(w);
        let mut w = World::open(&dir.0).unwrap();
        let loaded = w.load(Dim::Overworld, 2, 3).expect("chunk salvato");
        assert_eq!(loaded.sections.iter().flatten().next().unwrap().palette, vec![air(), hash], "la scala non diventa info_update");
        assert_eq!(loaded.network_bytes(), chunk.network_bytes());
        assert!(w.warned.is_empty());
    }

    #[test]
    fn corrupt_chunk_record_loads_as_none_and_logs() {
        let dir = TempDir::new("corrupt");
        let mut w = open(&dir);
        w.save(&sample_chunk(Dim::Overworld, 0, 0)).unwrap();
        assert!(w.try_load(Dim::Overworld, 0, 0).unwrap().is_some());
        let mut key = chunk_key(Dim::Overworld, 0, 0, 0x2f);
        key.push(0xfc); // sezione -4
        w.db_put(&key, &[0x13, 0x37, 0xff, 0x00, 0x42]).unwrap();
        let err = w.try_load(Dim::Overworld, 0, 0).expect_err("sotto-chunk illeggibile");
        assert!(err.contains("sotto-chunk"), "{err}");
        assert!(w.load(Dim::Overworld, 0, 0).is_none());
        // un chunk mai salvato non è un errore
        assert!(matches!(w.try_load(Dim::Overworld, 9, 9), Ok(None)));
    }

    #[test]
    fn level_dat_roundtrip_keeps_seed_and_spawn() {
        let dir = TempDir::new("leveldat");
        let mut w = World::open_with(&dir.0, "Prova", Some(0x1234_5678_9abc)).unwrap();
        assert_eq!(w.seed(), 0x1234_5678_9abc);
        assert_eq!(w.spawn(), None);
        w.set_spawn(10, 70, -20);
        drop(w);
        let raw = fs::read(dir.0.join("level.dat")).unwrap();
        assert_eq!(u32::from_le_bytes(raw[..4].try_into().unwrap()), 10, "versione di archiviazione");
        assert_eq!(u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize, raw.len() - 8);
        let w = World::open(&dir.0).unwrap();
        assert_eq!((w.seed(), w.spawn()), (0x1234_5678_9abc, Some((10, 70, -20))));
        drop(w);
        // il seme di un mondo esistente vince su quello richiesto
        assert_eq!(World::open_with(&dir.0, "Prova", Some(1)).unwrap().seed(), 0x1234_5678_9abc);
    }

    #[test]
    fn new_world_without_seed_picks_one_and_keeps_it() {
        let dir = TempDir::new("randomseed");
        let first = World::open_with(&dir.0, "Prova", None).unwrap().seed();
        assert_eq!(World::open(&dir.0).unwrap().seed(), first);
    }

    #[test]
    fn dimension_key_includes_dim_only_when_not_overworld() {
        assert_eq!(chunk_key(Dim::Overworld, 1, -2, 0x2c), [1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 0x2c]);
        assert_eq!(chunk_key(Dim::Nether, 1, -2, 0x2c), [1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 1, 0, 0, 0, 0x2c]);
        assert_eq!(chunk_key(Dim::End, 1, -2, 0x2f), [1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 2, 0, 0, 0, 0x2f]);
    }

    #[test]
    fn air_nbt_roundtrips_to_the_same_hash() {
        let air_nbt = block_nbt("minecraft:air", &[]);
        assert_eq!(block_hash_from_nbt(&air_nbt), Some(air()));
        assert_eq!(block_hash_from_nbt(&air_nbt), Some(-604_749_536));
    }

    #[test]
    fn nbt_hash_ignores_version_and_state_order() {
        use crate::nbt::{write_root, Enc, Tag};
        let states = [("a", State::Int(2)), ("b", State::Byte(1)), ("c", State::Str("x"))];
        let expected = block_hash("minecraft:test", &states);
        // stati in ordine sparso e `version` in fondo, come li scrive il gioco
        let tag = Tag::Compound(vec![
            ("name".into(), Tag::String("minecraft:test".into())),
            ("states".into(), Tag::Compound(vec![("c".into(), Tag::String("x".into())), ("a".into(), Tag::Int(2)), ("b".into(), Tag::Byte(1))])),
            ("version".into(), Tag::Int(18_168_865)),
        ]);
        assert_eq!(block_hash_from_nbt(&write_root("", &tag, Enc::Disk)), Some(expected));
        assert_eq!(block_hash_from_nbt(&[1, 2, 3]), None);
        let no_states = Tag::Compound(vec![("name".into(), Tag::String("minecraft:test".into()))]);
        assert_eq!(block_hash_from_nbt(&write_root("", &no_states, Enc::Disk)), None);
    }

    #[test]
    fn unknown_hash_is_saved_as_info_update() {
        let dir = TempDir::new("unknown");
        let mut w = open(&dir);
        let mut c = sample_chunk(Dim::Overworld, 0, 0);
        c.sections[0].as_mut().unwrap().palette[1] = 123_456; // non è nella tabella
        w.save(&c).unwrap();
        w.save(&c).unwrap();
        let loaded = w.load(Dim::Overworld, 0, 0).unwrap();
        assert_eq!(loaded.sections[0].as_ref().unwrap().palette[1], block_hash("minecraft:info_update", &[]));
        assert_eq!(w.warned.len(), 1, "una sola segnalazione per hash");
    }

    #[test]
    fn disk_palette_entries_carry_name_states_and_version() {
        let dir = TempDir::new("diskpal");
        let mut w = open(&dir);
        w.save(&sample_chunk(Dim::Overworld, 0, 0)).unwrap();
        let mut key = chunk_key(Dim::Overworld, 0, 0, 0x2f);
        key.push(0xfc);
        let raw = w.db_get(&key).unwrap().unwrap();
        assert_eq!(&raw[..2], &[9, 1], "v9, un solo strato");
        assert_eq!(raw[2], 0xfc);
        assert_eq!(raw[3], 2 << 1, "3 voci -> 2 bit, bit basso 0 = persistente");
        let words = 4096 / 16 * 4;
        let mut at = 4 + words;
        assert_eq!(i32::from_le_bytes(raw[at..at + 4].try_into().unwrap()), 3, "lunghezza della palette: i32 LE");
        at += 4;
        let mut r = crate::nbt::Reader::new(&raw[at..], crate::nbt::Enc::Disk);
        let (_, first) = r.root().unwrap();
        assert_eq!(first.get("name").and_then(|t| t.as_str()), Some("minecraft:air"));
        assert_eq!(first.get("version").and_then(|t| t.as_i32()), Some(BLOCK_STATE_VERSION));
        let (_, second) = r.root().unwrap();
        assert_eq!(second.get("name").and_then(|t| t.as_str()), Some("minecraft:stone"));
        let (_, third) = r.root().unwrap();
        assert_eq!(third.get("states").and_then(|s| s.get("pillar_axis")).and_then(|t| t.as_str()), Some("y"));
        assert!(r.rest().is_empty());
    }

    #[test]
    fn data3d_has_heightmap_and_disk_biomes() {
        let dir = TempDir::new("data3d");
        let mut w = open(&dir);
        w.save(&Chunk::flat(Dim::Overworld, 0, 0)).unwrap();
        let raw = w.db_get(&chunk_key(Dim::Overworld, 0, 0, 0x2b)).unwrap().unwrap();
        // Overworld: 24 sezioni di bioma, ciascuna con una sola voce (pianura): 1 byte + i32
        assert_eq!(raw.len(), 512 + 24 * 5);
        let height = |i: usize| i16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]);
        assert!((0..256).all(|i| height(i) == 3), "erba a y=-61, 3 sopra il fondo (-64)");
        assert_eq!(&raw[512..517], &[0, PLAINS as u8, 0, 0, 0], "0 bit, persistente, una voce i32 LE");
        assert_eq!(w.db_get(&chunk_key(Dim::Overworld, 0, 0, 0x2c)).unwrap().unwrap(), [40]);
        assert_eq!(w.db_get(&chunk_key(Dim::Overworld, 0, 0, 0x36)).unwrap().unwrap(), [2]);
    }

    #[test]
    fn heightmap_follows_the_highest_non_air_block_of_each_column() {
        let dir = TempDir::new("height");
        let mut w = open(&dir);
        // sezione 0 (y 0..16): colonna (x=2, z=3) piena fino a y=5, colonna (x=7, z=1) con un blocco a y=15
        let top = section(0, vec![air(), stone()], |i| {
            let (x, z, y) = (i / 256, i / 16 % 16, i % 16);
            ((x == 2 && z == 3 && y <= 5) || (x == 7 && z == 1 && y == 15)) as u16
        });
        let c = Chunk::from_gen(ChunkData { dim: Dim::Overworld, x: 0, z: 0, sections: vec![top], block_entities: vec![] });
        w.save(&c).unwrap();
        let raw = w.db_get(&chunk_key(Dim::Overworld, 0, 0, 0x2b)).unwrap().unwrap();
        let height = |x: usize, z: usize| i16::from_le_bytes([raw[(z * 16 + x) * 2], raw[(z * 16 + x) * 2 + 1]]);
        assert_eq!(height(2, 3), 64 + 5);
        assert_eq!(height(7, 1), 64 + 15);
        assert_eq!(height(0, 0), 0, "colonna vuota");
    }

    #[test]
    fn deflate_compressor_roundtrips_and_is_raw() {
        use rusty_leveldb::Compressor;
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 13) as u8).collect();
        let packed = BedrockDeflate.encode(data.clone()).unwrap();
        assert!(packed.len() < data.len());
        assert_ne!(packed[0], 0x78, "deflate raw: niente intestazione zlib");
        assert_eq!(BedrockDeflate.decode(packed).unwrap(), data);
        assert!(BedrockDeflate.decode(vec![0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn reopen_with_only_the_deflate_compressor_reads_chunk_keys() {
        use rusty_leveldb::{CompressorId, CompressorList, Options, DB};
        let dir = TempDir::new("reopen");
        let mut w = open(&dir);
        w.save(&sample_chunk(Dim::Overworld, 2, 3)).unwrap();
        w.compact().unwrap(); // porta il memtable in un file .ldb compresso
        drop(w);
        assert!(fs::read_dir(dir.0.join("db")).unwrap().any(|e| e.unwrap().path().extension().is_some_and(|x| x == "ldb")), "nessun file .ldb");

        let mut list = CompressorList::new();
        list.set(BedrockDeflate);
        let opt = Options { compressor_list: std::rc::Rc::new(list), compressor: BedrockDeflate::ID, create_if_missing: false, ..Options::default() };
        let mut db = DB::open(dir.0.join("db"), opt).unwrap();
        let mut key = chunk_key(Dim::Overworld, 2, 3, 0x2f);
        key.push(0xfc);
        let raw = db.get(&key).expect("sotto-chunk presente");
        assert_eq!(raw[0], 9);
        assert_eq!(db.get(&chunk_key(Dim::Overworld, 2, 3, 0x2c)).unwrap().to_vec(), [40]);
    }

    #[test]
    fn save_load_player_in_world() {
        let dir = TempDir::new("player");
        let mut w = open(&dir);
        assert_eq!(w.load_player("2535"), None, "mai salvato");
        let nbt = nbt::write_root("", &Tag::Compound(vec![("Pos".into(), Tag::Int(1))]), Enc::Disk);
        w.save_player("2535", &nbt).unwrap();
        assert_eq!(w.load_player("2535"), Some(nbt.clone()));

        let map = w.db_get(b"player_2535").unwrap().expect("chiave player_<id>");
        let (_, map) = Reader::new(&map, Enc::Disk).root().unwrap();
        let server_id = map.get("ServerId").and_then(Tag::as_str).unwrap().to_string();
        assert!(server_id.starts_with("player_server_"), "{server_id}");
        assert_eq!(map.get("MsaId").and_then(Tag::as_str), Some("2535"));
        assert!(map.get("SelfSignedId").is_some());
        assert_eq!(w.db_get(server_id.as_bytes()).unwrap(), Some(nbt.clone()), "dati in player_server_<uuid>");
        let uuid = &server_id["player_server_".len()..];
        assert_eq!(uuid.len(), 36, "UUID testuale");
        assert_eq!(uuid.matches('-').count(), 4);

        // sopravvive alla riapertura
        w.flush().unwrap();
        drop(w);
        let mut w = open(&dir);
        assert_eq!(w.load_player("2535"), Some(nbt));
    }

    #[test]
    fn same_identity_reuses_server_id() {
        let dir = TempDir::new("player-id");
        let mut w = open(&dir);
        let server_id = |w: &World, id: &str| {
            let map = w.db_get(format!("player_{id}").as_bytes()).unwrap().unwrap();
            Reader::new(&map, Enc::Disk).root().unwrap().1.get("ServerId").and_then(Tag::as_str).unwrap().to_string()
        };
        w.save_player("2535", &[0x0a, 0, 0]).unwrap();
        let first = server_id(&w, "2535");
        w.save_player("2535", &nbt::write_root("", &Tag::Compound(vec![("A".into(), Tag::Int(1))]), Enc::Disk)).unwrap();
        assert_eq!(server_id(&w, "2535"), first, "stesso giocatore, stesso ServerId");
        assert_eq!(w.load_player("2535").map(|b| b.len()), Some(nbt::write_root("", &Tag::Compound(vec![("A".into(), Tag::Int(1))]), Enc::Disk).len()));
        w.save_player("name:Bob", &[0x0a, 0, 0]).unwrap();
        assert_ne!(server_id(&w, "name:Bob"), first, "un altro giocatore ha il suo");
        assert_eq!(w.load_player("2535").map(|b| b.len()), Some(nbt::write_root("", &Tag::Compound(vec![("A".into(), Tag::Int(1))]), Enc::Disk).len()));
    }

    #[test]
    fn broken_player_record_is_ignored() {
        let dir = TempDir::new("player-bad");
        let mut w = open(&dir);
        w.db_put(b"player_7", b"non nbt").unwrap();
        assert_eq!(w.load_player("7"), None, "mappatura illeggibile: si parte da capo");
        w.save_player("7", &[0x0a, 0, 0]).unwrap(); // e si può risalvare
        assert!(w.load_player("7").is_some());
    }

    #[test]
    fn unreadable_player_record_is_backed_up_before_being_replaced() {
        let dir = TempDir::new("player-backup");
        let mut w = open(&dir);
        w.db_put(b"player_7", b"non nbt").unwrap();
        let n = std::cell::Cell::new(0);
        let mut uuid = || {
            n.set(n.get() + 1);
            format!("uuid-{}", n.get())
        };
        let nbt = [0x0a, 0, 0];
        w.save_player_with("7", &nbt, &mut uuid).unwrap();
        assert_eq!(w.db_get(b"player_server_uuid-1_illeggibile").unwrap(), Some(b"non nbt".to_vec()), "i byte originali sotto la chiave di riserva");
        assert_eq!(w.load_player("7"), Some(nbt.to_vec()), "il record rifatto punta ai dati nuovi");
        assert_eq!(w.db_get(b"player_server_uuid-2").unwrap(), Some(nbt.to_vec()));
        // un record buono non si copia e non cambia ServerId
        w.save_player_with("7", &nbt, &mut uuid).unwrap();
        assert_eq!(n.get(), 2, "nessun UUID nuovo");
        assert_eq!(w.db_get(b"player_server_uuid-3_illeggibile").unwrap(), None);
    }

    #[test]
    fn a_failed_read_never_regenerates_the_server_id() {
        let n = std::cell::Cell::new(0);
        let mut uuid = || {
            n.set(n.get() + 1);
            format!("uuid-{}", n.get())
        };
        // il database non risponde: il salvataggio fallisce e non si inventa nulla
        let err = save_plan("7", Err("database: occupato".into()), &mut uuid).unwrap_err();
        assert!(err.to_string().contains("lettura del record"), "{err}");
        assert_eq!(n.get(), 0, "nessun UUID generato");
        // un record buono mantiene il suo ServerId e non viene riscritto
        let (id, ops) = save_plan("7", Ok(Lookup::Found("player_server_x".into())), &mut uuid).unwrap();
        assert_eq!((id.as_str(), ops.len(), n.get()), ("player_server_x", 0, 0));
        // solo un record presente ma non interpretabile si rifà (dopo la copia)
        let (_, ops) = save_plan("7", Ok(Lookup::Unreadable(b"zz".to_vec())), &mut uuid).unwrap();
        assert_eq!(ops.len(), 2, "copia di riserva e record nuovo");
        assert_eq!(classify_record(None), Lookup::Missing);
        assert_eq!(classify_record(Some(b"zz".to_vec())), Lookup::Unreadable(b"zz".to_vec()));
        // senza ServerId `player_server_*` il record è illeggibile (non può puntare ai chunk)
        let alien = nbt::write_root("", &Tag::Compound(vec![("ServerId".into(), Tag::String("chunk".into()))]), Enc::Disk);
        assert_eq!(classify_record(Some(alien.clone())), Lookup::Unreadable(alien));
    }

    #[test]
    fn a_closed_database_fails_the_save() {
        let dir = TempDir::new("player-closed");
        let mut w = open(&dir);
        w.db.tx = None;
        assert!(w.save_player("7", &[0x0a, 0, 0]).is_err());
    }
}
