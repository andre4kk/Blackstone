//! `blackstone-gen serve`: risponde al server Bedrock con il protocollo `vgen` su 127.0.0.1.
//!
//! Una sola connessione. Dopo l'Hello invia la palette Bedrock, poi esegue `Generate` e `Spawn`
//! in parallelo su un pool rayon con stack ampio; le risposte escono appena pronte (ordine non garantito).
//! Ogni `Generate` genera l'intera regione 4×4 allineata che contiene il chunk e mette in cache
//! tutti i chunk tradotti: le richieste vicine non rigenerano nulla.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufWriter, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

use rayon::ThreadPool;
use steel_utils::BlockPos;
use vgen::{ChunkData, Dim, FromGen, ToGen, VERSION, read_frame, write_frame};

use crate::ms;
use crate::steel::Generator;
use crate::translate::{Translator, to_bedrock};

/// Stack dei thread che chiamano Steel (come `probe`).
const STACK_SIZE: usize = 64 * 1024 * 1024;
/// Lato (in chunk) della regione generata in un colpo solo.
const REGION: i32 = 4;
/// Chunk tradotti tenuti in cache per dimensione.
const CACHE_CHUNKS: usize = 1024;
/// Raggio (in chunk) della ricerca vanilla dello spawn attorno all'origine climatica.
const SPAWN_RADIUS: i32 = 5;

/// Opzioni del comando `serve`.
pub struct ServeArgs {
    pub port: u16,
    pub seed: u64,
}

pub fn parse_serve(args: &[String]) -> Result<ServeArgs, String> {
    let (mut port, mut seed) = (None, None);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("manca il valore di {flag}"))?;
        let bad = |_| format!("valore non valido per {flag}: {value}");
        match flag.as_str() {
            "--port" => port = Some(value.parse().map_err(bad)?),
            "--seed" => seed = Some(value.parse().map_err(bad)?),
            _ => return Err(format!("opzione sconosciuta: {flag}")),
        }
    }
    Ok(ServeArgs { port: port.ok_or("manca --port")?, seed: seed.ok_or("manca --seed")? })
}

/// Ascolta, accetta una connessione e la serve finché non si chiude.
pub fn serve(args: &[String]) -> Result<(), String> {
    let a = parse_serve(args)?;
    let listener = TcpListener::bind(("127.0.0.1", a.port)).map_err(|e| format!("bind sulla porta {}: {e}", a.port))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    // Il traduttore serve già all'handshake (palette): si prepara mentre il server si connette.
    let translator =
        std::thread::Builder::new().name("blackstone-gen-traduttore".into()).stack_size(STACK_SIZE).spawn(Translator::new).map_err(|e| e.to_string())?;
    println!("READY {port}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;

    let (stream, peer) = listener.accept().map_err(|e| format!("accept: {e}"))?;
    eprintln!("connessione da {peer}");
    let translator = Arc::new(translator.join().map_err(|_| "il traduttore è andato in panico".to_string())?);
    let result = Service::new(a.seed, translator).run(stream);
    eprintln!("connessione chiusa");
    result
}

/// Chunk tradotto in cache, con il punto di spawn vanilla trovato nel chunk (se c'è).
struct Cached {
    data: ChunkData,
    spawn: Option<BlockPos>,
}

/// Cache LRU dei chunk di una dimensione più le regioni in generazione.
#[derive(Default)]
struct DimCache {
    chunks: HashMap<(i32, i32), (Arc<Cached>, u64)>,
    /// Ordine d'uso: tick -> chunk.
    order: BTreeMap<u64, (i32, i32)>,
    tick: u64,
    in_flight: HashSet<(i32, i32)>,
}

impl DimCache {
    fn get(&mut self, key: (i32, i32)) -> Option<Arc<Cached>> {
        self.tick += 1;
        let tick = self.tick;
        let (chunk, used) = self.chunks.get_mut(&key)?;
        self.order.remove(used);
        *used = tick;
        self.order.insert(tick, key);
        Some(Arc::clone(chunk))
    }

    fn insert(&mut self, key: (i32, i32), chunk: Arc<Cached>) {
        self.tick += 1;
        if let Some((_, old)) = self.chunks.insert(key, (chunk, self.tick)) {
            self.order.remove(&old);
        }
        self.order.insert(self.tick, key);
        while self.chunks.len() > CACHE_CHUNKS {
            let Some((_, oldest)) = self.order.pop_first() else { break };
            self.chunks.remove(&oldest);
        }
    }
}

/// Stato condiviso dai lavori di una connessione.
struct Service {
    seed: u64,
    translator: Arc<Translator>,
    /// Generatori creati alla prima richiesta della dimensione (avvio costoso).
    generators: [OnceLock<Generator>; 3],
    caches: [Mutex<DimCache>; 3],
    /// Svegliato quando una regione in generazione finisce (bene o male).
    region_done: Condvar,
    spawns: [OnceLock<Result<BlockPos, String>>; 3],
    pool: ThreadPool,
}

impl Service {
    fn new(seed: u64, translator: Arc<Translator>) -> Service {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).max(4);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .stack_size(STACK_SIZE)
            .thread_name(|i| format!("blackstone-gen-richieste-{i}"))
            .build()
            .expect("il pool delle richieste deve avviarsi");
        Service { seed, translator, generators: Default::default(), caches: Default::default(), region_done: Condvar::new(), spawns: Default::default(), pool }
    }

    /// Handshake, poi legge le richieste e le smista al pool finché la connessione resta aperta.
    fn run(self, stream: TcpStream) -> Result<(), String> {
        let mut reader = stream.try_clone().map_err(|e| e.to_string())?;
        let (tx, rx) = channel::<FromGen>();
        // Un solo thread possiede il lato di scrittura del socket.
        let writer_stream = stream.try_clone().map_err(|e| e.to_string())?;
        let writer = std::thread::Builder::new()
            .name("blackstone-gen-scrittura".into())
            .spawn(move || {
                let mut out = BufWriter::new(writer_stream);
                for msg in rx {
                    if write_frame(&mut out, &msg.encode()).and_then(|()| out.flush()).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        let hello = read_frame(&mut reader).map_err(|e| format!("lettura dell'Hello: {e}"))?;
        match ToGen::decode(&hello) {
            Some(ToGen::Hello { version, seed }) if version == VERSION && seed == self.seed => {
                let _ = tx.send(FromGen::Hello { version: VERSION });
                let _ = tx.send(FromGen::Palette { entries: self.translator.palette_entries() });
            }
            other => {
                let message = match other {
                    Some(ToGen::Hello { version, .. }) if version != VERSION => format!("versione del protocollo {version}, attesa {VERSION}"),
                    Some(ToGen::Hello { seed, .. }) => format!("seme {seed} diverso da quello del generatore ({})", self.seed),
                    _ => "atteso Hello come primo messaggio".to_string(),
                };
                eprintln!("handshake rifiutato: {message}");
                let _ = tx.send(FromGen::Error { req: 0, message: message.clone() });
                drop(tx);
                let _ = writer.join();
                let _ = stream.shutdown(Shutdown::Both);
                return Err(message);
            }
        }

        let service = Arc::new(self);
        loop {
            let frame = match read_frame(&mut reader) {
                Ok(frame) => frame,
                // Connessione chiusa (o rotta): si esce.
                Err(_) => return Ok(()),
            };
            let decoded = ToGen::decode(&frame);
            // Il generatore della dimensione si crea qui, fuori dal pool: se nascesse dentro un lavoro, gli altri lavori
            // della stessa dimensione occuperebbero tutti i thread aspettandolo e la creazione resterebbe bloccata.
            match &decoded {
                Some(ToGen::Generate { dim, .. }) | Some(ToGen::Spawn { dim: dim @ Dim::Overworld, .. }) => {
                    service.generator(*dim);
                }
                _ => {}
            }
            match decoded {
                Some(ToGen::Generate { req, dim, x, z }) => {
                    service.spawn_job(req, &tx, move |s| s.chunk(dim, x, z).map(|c| FromGen::Chunk { req, data: c.data.clone() }))
                }
                Some(ToGen::Spawn { req, dim }) => {
                    service.spawn_job(req, &tx, move |s| s.spawn(dim).map(|p| FromGen::Spawn { req, x: p.x(), y: p.y(), z: p.z() }))
                }
                Some(ToGen::Hello { .. }) => {
                    let _ = tx.send(FromGen::Error { req: 0, message: "Hello ripetuto".into() });
                }
                None => {
                    let _ = tx.send(FromGen::Error { req: 0, message: "frame non valido".into() });
                }
            }
        }
    }

    /// Esegue `job` sul pool; un errore o un panico diventano `FromGen::Error` con lo stesso `req`.
    fn spawn_job(self: &Arc<Self>, req: u32, tx: &Sender<FromGen>, job: impl FnOnce(&Service) -> Result<FromGen, String> + Send + 'static) {
        let (service, tx) = (Arc::clone(self), tx.clone());
        self.pool.spawn(move || {
            let reply = match catch_unwind(AssertUnwindSafe(|| job(&service))) {
                Ok(Ok(msg)) => msg,
                Ok(Err(message)) => FromGen::Error { req, message },
                Err(panic) => FromGen::Error { req, message: format!("panico: {}", panic_message(&panic)) },
            };
            let _ = tx.send(reply);
        });
    }

    fn generator(&self, dim: Dim) -> &Generator {
        self.generators[dim as usize].get_or_init(|| {
            let start = Instant::now();
            let generator = Generator::new(self.seed, dim);
            eprintln!("generatore {dim:?} pronto in {:.0} ms", ms(start));
            generator
        })
    }

    /// Chunk tradotto dalla cache; se manca genera (una volta sola) la sua regione 4×4.
    fn chunk(&self, dim: Dim, x: i32, z: i32) -> Result<Arc<Cached>, String> {
        let region = (x.div_euclid(REGION), z.div_euclid(REGION));
        let cache = &self.caches[dim as usize];
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(chunk) = guard.get((x, z)) {
                return Ok(chunk);
            }
            if !guard.in_flight.contains(&region) {
                break;
            }
            // Un altro lavoro sta generando la stessa regione: si aspetta il suo risultato.
            guard = self.region_done.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
        guard.in_flight.insert(region);
        drop(guard);

        let result = catch_unwind(AssertUnwindSafe(|| self.generate_region(dim, region)));
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.in_flight.remove(&region);
        let mut wanted = None;
        if let Ok(chunks) = &result {
            for chunk in chunks {
                let key = (chunk.data.x, chunk.data.z);
                if key == (x, z) {
                    wanted = Some(Arc::clone(chunk));
                }
                guard.insert(key, Arc::clone(chunk));
            }
        }
        drop(guard);
        self.region_done.notify_all();
        match result {
            Ok(_) => wanted.ok_or_else(|| format!("la regione {region:?} non contiene il chunk ({x}, {z})")),
            Err(panic) => Err(format!("generazione della regione {region:?} fallita: {}", panic_message(&panic))),
        }
    }

    /// Genera e traduce i 16 chunk della regione.
    fn generate_region(&self, dim: Dim, (rx, rz): (i32, i32)) -> Vec<Arc<Cached>> {
        let generator = self.generator(dim);
        let start = Instant::now();
        let (min_x, min_z) = (rx * REGION, rz * REGION);
        let java = generator.generate_region(min_x, min_z, min_x + REGION - 1, min_z + REGION - 1);
        let generated = ms(start);
        let chunks = java.iter().map(|c| Arc::new(Cached { data: to_bedrock(c, &self.translator), spawn: c.spawn })).collect();
        eprintln!("regione generata {dim:?} ({rx}, {rz}): {generated:.0} ms + traduzione {:.0} ms", ms(start) - generated);
        chunks
    }

    /// Spawn del mondo, calcolato una volta per dimensione.
    fn spawn(&self, dim: Dim) -> Result<BlockPos, String> {
        self.spawns[dim as usize].get_or_init(|| self.find_spawn(dim)).clone()
    }

    /// Ricerca vanilla (`World::initialize_spawn_if_needed`): origine climatica, poi spirale di 11×11 chunk.
    fn find_spawn(&self, dim: Dim) -> Result<BlockPos, String> {
        if dim != Dim::Overworld {
            return Err(format!("lo spawn esiste solo nell'overworld, non in {dim:?}"));
        }
        let start = Instant::now();
        let generator = self.generator(dim);
        let origin = generator.spawn_search_origin();
        let (cx, cz) = (origin.x() >> 4, origin.z() >> 4);
        let fallback = BlockPos::new((cx << 4) + 8, generator.spawn_height(), (cz << 4) + 8);
        let (mut ox, mut oz, mut dx, mut dz) = (0i32, 0i32, 0i32, -1i32);
        let mut found = None;
        for _ in 0..(2 * SPAWN_RADIUS + 1).pow(2) {
            if ox.abs() <= SPAWN_RADIUS
                && oz.abs() <= SPAWN_RADIUS
                && let Some(pos) = self.chunk(dim, cx + ox, cz + oz)?.spawn
            {
                found = Some(pos);
                break;
            }
            if ox == oz || (ox < 0 && ox == -oz) || (ox > 0 && ox == 1 - oz) {
                (dx, dz) = (-dz, dx);
            }
            ox += dx;
            oz += dz;
        }
        let spawn = found.unwrap_or(fallback);
        eprintln!("spawn {spawn:?} (origine {origin:?}) in {:.0} ms", ms(start));
        Ok(spawn)
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "sconosciuto".into())
}
