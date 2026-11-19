//! Logica di gioco: riceve i pacchetti decodificati dal trasporto e risponde.
mod biomes;
mod bot;
mod breaking;
mod chat;
mod chest;
pub mod config;
mod disk;
mod entity;
mod events;
mod furnace;
mod gen_client;
mod handlers;
mod interact;
mod inventory;
mod items;
mod join;
mod nbt;
mod placement;
mod playerdata;
mod recipes;
mod registry;
mod smelting;
mod storage;
mod streaming;
mod survival;
mod tables;
#[cfg(test)]
mod tests;
mod world;

use blackstone_net::{debug, log, warn, Config, Event, Sender};
use blackstone_protocol::{
    generated::{BlockActorData, BlockPos, UpdateBlock},
    wire::ServerPacket,
};
use blackstone_protocol::{login::PlayerInfo, Packet, Session};
use disk::{Disk, DiskEvent};
use events::{BlockChange, Hook, WorldAction};
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
        name: "Blackstone".into(),
        level: "Mondo".into(),
        version: blackstone_protocol::VERSION.into(),
        protocol: blackstone_protocol::PROTOCOL,
        max_players: 100,
        public_ip: std::env::var("PUBLIC_IP").ok().and_then(|v| v.parse().ok()),
    }
}

/// Modalità di gioco; il valore è quello che il protocollo chiama `GameType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GameMode {
    Survival = 0,
    #[default]
    Creative = 1,
    Adventure = 2,
    Spectator = 6,
}

impl GameMode {
    /// Dal numero salvato nei dati del giocatore; `None` per quelli che il server non conosce.
    pub(crate) fn from_i32(v: i32) -> Option<GameMode> {
        [GameMode::Survival, GameMode::Creative, GameMode::Adventure, GameMode::Spectator].into_iter().find(|&m| m as i32 == v)
    }

    /// Vero se il giocatore può prendere oggetti dal menu creativo e cestinarli (solo in creativa).
    pub(crate) fn creative_inventory(self) -> bool {
        self == GameMode::Creative
    }

    /// Dall'argomento di `/gamemode`: nome intero, abbreviazione o numero.
    pub(crate) fn parse(arg: &str) -> Option<GameMode> {
        match arg.to_lowercase().as_str() {
            "survival" | "s" | "0" => Some(GameMode::Survival),
            "creative" | "c" | "1" => Some(GameMode::Creative),
            "adventure" | "a" | "2" => Some(GameMode::Adventure),
            "spectator" | "sp" | "6" => Some(GameMode::Spectator),
            _ => None,
        }
    }

    /// Nome mostrato al giocatore.
    pub(crate) fn name(self) -> &'static str {
        match self {
            GameMode::Survival => "sopravvivenza",
            GameMode::Creative => "creativa",
            GameMode::Adventure => "avventura",
            GameMode::Spectator => "spettatore",
        }
    }
}

/// Stato di un giocatore connesso.
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
    /// Vero dopo aver loggato un `PlayerAuthInput` illeggibile: si logga solo il primo, a 20 Hz sarebbe un diluvio.
    pai_logged: bool,
    /// Chunk (x, z) in cui si trova.
    last_chunk: (i32, i32),
    /// Chunk già inviati e ancora nel raggio del giocatore: quando ne escono si tolgono, così al ritorno si rimandano.
    sent: HashSet<(i32, i32)>,
    /// Entità (ID) che questo giocatore vede in questo momento: la differenza con quelle nei chunk `sent` dice cosa mostrare o togliere.
    visible: HashSet<u64>,
    mode: GameMode,
    inventory: inventory::Inventory,
    /// Quante richieste di pila e quanti `PlayerAuthInput` con azioni su oggetti o blocchi sono già stati loggati in esadecimale
    /// (al massimo `HEX_LOGS`): servono a verificare nel gioco le codifiche non sicure.
    stack_logs: u8,
    /// Quante richieste di pila con azioni di crafting sono già state loggate in esadecimale (al massimo `HEX_LOGS`).
    craft_logs: u8,
    action_logs: u8,
    /// ID dei pacchetti sconosciuti già registrati nel log: ognuno si scrive una volta sola per giocatore.
    unknown_logged: std::collections::HashSet<u32>,
    /// Quanti `PlayerSkin` sono già stati loggati in esadecimale (al massimo `HEX_LOGS`).
    skin_logs: u8,
    /// Blocco che il giocatore ha cominciato a rompere (Start/Continue) e non ha ancora rotto né abbandonato.
    breaking: Option<[i32; 3]>,
    /// Il blocco di cui gli spettatori vedono le crepe e la velocita' mandata (`LevelEvent` 3600/3602, dato `65535 / tick`): solo in
    /// sopravvivenza e avventura. Serve a mandare l'aggiornamento solo se il tempo di rottura cambia e a fermare le crepe a fine rottura.
    crack: Option<([i32; 3], u32)>,
    /// Rotazione dall'ultimo `PlayerAuthInput` (gradi): 0 = sud, 90 = ovest, -90 = est, 180 = nord. Serve a orientare i blocchi piazzati.
    yaw: f32,
    pitch: f32,
    /// Vero se l'ultimo `PlayerAuthInput` completo aveva il flag di accovacciamento.
    sneaking: bool,
    /// Corsa, nuoto e planata dichiarati dal client con i flag di inizio e fine di `PlayerAuthInput`: li vedono gli spettatori.
    sprinting: bool,
    swimming: bool,
    gliding: bool,
    /// Ultimo blocco di cui si è tentata la rottura, col tick: la stessa rottura chiesta due volte nello stesso tick vale una.
    last_break: Option<(u64, [i32; 3])>,
    /// A che punto sono i dati salvati del giocatore (vedi [`DataState`]).
    data: DataState,
    /// Posizione dei piedi salvata nel mondo, in attesa di `StartGame`.
    saved_feet: Option<[f32; 3]>,
    /// Vero da quando `StartGame` ha fissato posizione e inventario: solo allora i dati sono quelli veri e si possono salvare.
    started: bool,
    /// Vero tra l'apertura dell'inventario (`Interact` OpenInventory) e la sua chiusura: un secondo `ContainerOpen` farebbe crashare il client.
    inventory_open: bool,
    /// Volo confermato dal server (`StartFlying`/`StopFlying`): entra nel livello di abilità mandato al client.
    flying: bool,
    /// Vita (0..=20, mezzo cuore = 1): vedi `survival`.
    health: f32,
    /// Fame (0..=20), saturazione (0..=20) ed esaurimento (0..5): i valori di Dragonfly `hunger`.
    food: u8,
    saturation: f32,
    exhaustion: f32,
    /// Contatore della fame (1..=80, Dragonfly `foodTick`): a 1 si rigenera o si muore di fame, ogni 20 la rigenerazione rapida.
    food_tick: u32,
    /// Fino a questo tick (escluso) il giocatore è nel periodo di invulnerabilità dopo un colpo; `last_damage` è il danno di quel colpo.
    invulnerable_until: u64,
    last_damage: f32,
    /// Vero da quando la vita è arrivata a 0 e finché il giocatore non rinasce.
    dead: bool,
    /// Il tick della morte: dopo `survival::BODY_TICKS` il corpo sparisce a chi lo vedeva.
    death_tick: u64,
    /// Quanti `Respawn` ricevuti sono già stati loggati in esadecimale (al massimo `HEX_LOGS`).
    respawn_logs: u8,
    /// Altezza dei piedi da cui è iniziata la caduta in corso (il punto più alto prima della discesa); `None` se non sta cadendo.
    fall_start_y: Option<f32>,
    /// Il punto (occhi) a cui il server ha teletrasportato il client e che il client non ha ancora raggiunto: i `PlayerAuthInput` lontani da lì
    /// partono da prima del teletrasporto e si ignorano (Dragonfly `teleportPos`). `None` se non c'è un teletrasporto in attesa.
    teleport_to: Option<[f32; 3]>,
    /// Il tick del teletrasporto in attesa: dopo `TELEPORT_TIMEOUT` tick il movimento torna valido comunque (vedi `player_auth_input`).
    teleport_at: u64,
    /// Il tick in cui ha cominciato a mangiare il cibo in mano (Dragonfly `usingItem`/`usingSince`); `None` se non mangia.
    eating_since: Option<u64>,
    /// Quanti usi e rilasci di oggetti (`InventoryTransaction` ClickAir e ReleaseItem) sono già stati loggati in esadecimale (al massimo `HEX_LOGS`).
    use_logs: u8,
    /// Quanti colpi (`InventoryTransaction` UseItemOnEntity) sono già stati loggati in esadecimale (al massimo `HEX_LOGS`).
    attack_logs: u8,
    /// Quante transazioni Normal (Q per buttare un oggetto) sono già state loggate in esadecimale (al massimo `HEX_LOGS`).
    drop_logs: u8,
    /// Se si è già loggata una transazione Normal non valida (si logga una volta sola per giocatore).
    drop_rejected_logged: bool,
}

impl Default for Player {
    /// Un giocatore appena arrivato: vita, fame e saturazione di partenza (`survival::MAX_HEALTH`, `MAX_FOOD`, `START_SATURATION`).
    fn default() -> Self {
        Player {
            session: Session::default(),
            info: None,
            radius: 0,
            spawned: false,
            streaming: false,
            pos: [0.0; 3],
            pai_logged: false,
            last_chunk: (0, 0),
            sent: HashSet::new(),
            visible: HashSet::new(),
            mode: GameMode::default(),
            inventory: inventory::Inventory::default(),
            stack_logs: 0,
            craft_logs: 0,
            action_logs: 0,
            unknown_logged: Default::default(),
            skin_logs: 0,
            breaking: None,
            crack: None,
            yaw: 0.0,
            pitch: 0.0,
            sneaking: false,
            sprinting: false,
            swimming: false,
            gliding: false,
            last_break: None,
            data: DataState::default(),
            saved_feet: None,
            started: false,
            inventory_open: false,
            flying: false,
            health: survival::MAX_HEALTH,
            food: survival::MAX_FOOD,
            saturation: survival::START_SATURATION,
            exhaustion: 0.0,
            food_tick: 1,
            invulnerable_until: 0,
            last_damage: 0.0,
            dead: false,
            death_tick: 0,
            respawn_logs: 0,
            fall_start_y: None,
            teleport_to: None,
            teleport_at: 0,
            eating_since: None,
            use_logs: 0,
            attack_logs: 0,
            drop_logs: 0,
            drop_rejected_logged: false,
        }
    }
}

/// Il caricamento dei dati salvati di un giocatore (vedi `playerdata`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DataState {
    /// Non ancora chiesti al disco.
    #[default]
    Unrequested,
    /// Chiesti al disco, risposta non arrivata: l'inventario in memoria è vuoto e non va mai salvato.
    Loading,
    /// Arrivati (o non c'è un disco da cui leggerli).
    Loaded,
}

/// Quanti pacchetti per giocatore si registrano in esadecimale (richieste di pila e input con azioni).
const HEX_LOGS: u8 = 3;

/// Il gioco gira a 20 tick al secondo.
const TICK: Duration = Duration::from_millis(50);
/// Quanti tick ricordare per calcolare i TPS reali.
const TPS_WINDOW: usize = 100;

/// Quanti giri di aggiornamento dei vicini dopo una modifica: la porta senza pavimento perde la metà bassa al primo,
/// quella alta al secondo. Oltre non si va, così nessuna catena di reazioni può durare per sempre.
const NEIGHBOR_ROUNDS: usize = 2;

/// Ogni quanti tick i chunk modificati vanno al disco (1200 tick = 60 s).
const AUTOSAVE_TICKS: u64 = 1200;

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
    /// Posizioni delle fornaci nei chunk in memoria (Overworld): `furnaces_tick` percorre solo queste.
    furnaces: HashSet<[i32; 3]>,
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
    /// Ganci che possono annullare le azioni dei giocatori sul mondo (vedi [`Server::allow`]).
    hooks: Vec<Hook>,
    /// Tick eseguiti finora: scandiscono il salvataggio automatico e le modifiche programmate.
    tick_count: u64,
    /// Modifiche di blocchi programmate per un tick futuro (i pulsanti che risalgono): vedi [`Server::schedule`].
    scheduled: Vec<interact::Scheduled>,
    /// Già detto nel log che le modifiche al mondo piatto di ripiego non si salvano.
    flat_warned: bool,
    /// Entità del mondo (giocatori visti dagli altri, oggetti a terra).
    entities: entity::Entities,
    /// Giocatori finti (`/bot`), con ciò che serve a mostrarli e animarli.
    bots: Vec<bot::Bot>,
    /// Se vero, chi muore tiene l'inventario (`keep_inventory` di `server.toml`).
    keep_inventory: bool,
    /// Se falso, i giocatori non si fanno danno tra loro (`pvp` di `server.toml`).
    pvp: bool,
    /// Stato del generatore casuale del gioco (drop dei blocchi...): xorshift64*, seminato da `getrandom`.
    rng: u64,
}

impl Server {
    /// Prossimo numero casuale in [0, 1).
    pub(crate) fn roll(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let x = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (x >> 40) as f32 / (1u32 << 24) as f32 // 24 bit: esatti in un f32, mai 1.0
    }

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
            furnaces: HashSet::new(),
            requested: HashSet::new(),
            loading: HashSet::new(),
            queue: HashSet::new(),
            chunk_fails: HashMap::new(),
            retry_after: HashMap::new(),
            spawn_fails: 0,
            spawn_retry_at: None,
            max_radius: join::MAX_RADIUS,
            hooks: Vec::new(),
            tick_count: 0,
            scheduled: Vec::new(),
            flat_warned: false,
            entities: entity::Entities::default(),
            bots: Vec::new(),
            keep_inventory: false,
            pvp: true,
            rng: {
                let mut b = [0u8; 8];
                getrandom::fill(&mut b).expect("generatore casuale non disponibile");
                u64::from_le_bytes(b) | 1 // xorshift non parte da zero
            },
        }
    }

    /// Un server con il mondo su disco; gli eventi di lettura vanno a `events`.
    #[cfg(test)]
    fn with_world(sender: Sender, world: storage::World, events: tokio::sync::mpsc::UnboundedSender<DiskEvent>) -> Self {
        Self { disk: Some(Disk::start(world, events)), ..Self::new(sender) }
    }

    /// Manda al disco i chunk modificati e li segna puliti; restituisce quanti. Senza disco non fa nulla.
    fn save_dirty(&mut self) -> usize {
        let Some(disk) = self.disk.as_ref() else { return 0 };
        let mut saved = 0;
        for chunk in self.chunks.values_mut().filter(|c| c.dirty) {
            disk.save(chunk.clone());
            chunk.dirty = false;
            saved += 1;
        }
        saved
    }

    /// Chiusura ordinata: salva i giocatori ancora connessi e i chunk modificati, poi scarica il database.
    fn shutdown(&mut self) {
        // griglia di crafting e cursore tornano nell'inventario prima di salvare (come alla disconnessione): il salvataggio
        // contiene solo inventario, mano e armatura
        let ids: Vec<u64> = self.players.keys().copied().collect();
        for pid in ids {
            self.close_chest_view(pid);
            self.return_ui_items(pid);
        }
        if self.disk.is_none() {
            return;
        }
        let spawn = self.spawn;
        for p in self.players.values_mut() {
            survival::settle_dead(p, spawn);
            playerdata::persist(self.disk.as_ref(), p);
        }
        let saved = self.save_dirty();
        if let Some(disk) = &self.disk {
            disk.flush_blocking();
        }
        log!("[mondo] chiusura: {saved} chunk salvati");
        log!("[server] spento");
    }

    /// Codifica i pacchetti per quel giocatore (con la sua compressione) e li invia in un solo messaggio.
    fn send(&self, id: u64, packets: &[Packet]) {
        if let Some(p) = self.players.get(&id) {
            self.sender.send(id, p.session.encode(packets));
        }
    }

    /// Un tick di gioco: lo streaming dei chunk e, ogni `AUTOSAVE_TICKS`, il salvataggio dei chunk modificati.
    fn tick(&mut self) {
        let start = Instant::now();
        self.stream_tick();
        self.bots_tick();
        self.visibility_tick();
        self.items_tick();
        self.pickup_tick();
        self.movement_tick();
        self.void_tick();
        self.hunger_tick();
        self.windows_tick();
        self.furnaces_tick();
        self.eating_tick();
        self.tick_count += 1;
        self.run_scheduled();
        if self.tick_count.is_multiple_of(AUTOSAVE_TICKS) {
            let saved = self.save_dirty();
            if saved > 0 {
                debug!("[mondo] salvataggio automatico: {saved} chunk");
            }
        }
        self.mspt = self.mspt * 0.9 + start.elapsed().as_secs_f64() * 1000.0 * 0.1;
        self.ticks.push_back(start);
        if self.ticks.len() > TPS_WINDOW {
            self.ticks.pop_front();
        }
    }

    /// Blocco alla posizione `pos` (x, y, z) della dimensione: hash dello stato e se nel secondo strato c'è acqua.
    /// `None` se il chunk non è in memoria o `y` è fuori dall'altezza del mondo.
    pub(crate) fn block_at(&self, dim: Dim, pos: [i32; 3]) -> Option<(i32, bool)> {
        let (min, max) = dim.height_range();
        if !(min..max).contains(&pos[1]) {
            return None;
        }
        let chunk = self.chunks.get(&(dim as u8, pos[0].div_euclid(16), pos[2].div_euclid(16)))?;
        let (lx, lz) = (pos[0].rem_euclid(16) as usize, pos[2].rem_euclid(16) as usize);
        Some((chunk.block(lx, pos[1], lz), chunk.water(lx, pos[1], lz)))
    }

    /// Applica le modifiche ai chunk in memoria (quelle fuori dal mondo o in chunk non caricati si scartano) e le manda
    /// a chi ha quel chunk in `sent`: `UpdateBlock` per lo strato 0 (blocco) e per lo strato 1 (acqua o aria). Poi fa reagire
    /// i vicini (vedi `placement::neighbors`): chi perde l'appoggio sparisce, muri e scale si ricalcolano, fino a `NEIGHBOR_ROUNDS` giri
    /// (le conseguenze delle conseguenze). Tutte le modifiche del mondo (piazzamento, rottura, interazioni) passano da qui.
    pub(crate) fn set_blocks(&mut self, dim: Dim, changes: &[BlockChange]) {
        let mut applied = self.apply_blocks(dim, changes);
        for _ in 0..NEIGHBOR_ROUNDS {
            let cells: Vec<[i32; 3]> = applied.iter().map(|c| c.pos).collect();
            let extra = placement::neighbors::updates(&cells, &placement::WorldView { server: self, dim });
            if extra.is_empty() {
                break;
            }
            applied = self.apply_blocks(dim, &extra);
        }
    }

    /// Scrive le modifiche nei chunk e le manda ai giocatori, senza far reagire i vicini. Restituisce quelle applicate
    /// (le altre erano fuori dal mondo o in chunk non caricati). Un blocco che cambia nome perde il suo block entity.
    fn apply_blocks(&mut self, dim: Dim, changes: &[BlockChange]) -> Vec<BlockChange> {
        let (min, max) = dim.height_range();
        let mut applied = Vec::with_capacity(changes.len());
        // Celle il cui blocco ha cambiato nome (chi aveva aperto una finestra lì la perde) e block entity di forziere perduti (il contenuto cade a terra).
        let mut gone: Vec<[i32; 3]> = Vec::new();
        let mut lost_chests: Vec<([i32; 3], Vec<u8>)> = Vec::new();
        let mut lost_furnaces: Vec<([i32; 3], Vec<u8>)> = Vec::new();
        for c in changes.iter().filter(|c| (min..max).contains(&c.pos[1])) {
            let (cx, cz) = (c.pos[0].div_euclid(16), c.pos[2].div_euclid(16));
            let Some(chunk) = self.chunks.get_mut(&(dim as u8, cx, cz)) else { continue };
            let (lx, lz) = (c.pos[0].rem_euclid(16) as usize, c.pos[2].rem_euclid(16) as usize);
            let old = chunk.block(lx, c.pos[1], lz);
            chunk.set_block(lx, c.pos[1], lz, c.hash, c.water);
            // la fornace che si accende o si spegne cambia nome ma resta la stessa: il suo block entity e le finestre aperte restano
            if old != c.hash
                && registry::blocks().state(old).map(|s| &s.name) != registry::blocks().state(c.hash).map(|s| &s.name)
                && !crate::furnace::is_lit_swap(old, c.hash)
            {
                if crate::chest::is_chest(old) {
                    if let Some(be) = chunk.block_entities.iter().find(|b| [b.x, b.y, b.z] == c.pos) {
                        lost_chests.push((c.pos, be.nbt.clone()));
                    }
                }
                if crate::furnace::is_furnace(old) {
                    if let Some(be) = chunk.block_entities.iter().find(|b| [b.x, b.y, b.z] == c.pos) {
                        lost_furnaces.push((c.pos, be.nbt.clone()));
                    }
                }
                chunk.remove_block_entity(c.pos);
                gone.push(c.pos);
            }
            if chunk.fallback {
                // un chunk di ripiego (mondo piatto, o generazione fallita) non è il mondo vero: salvarlo ne cancellerebbe la
                // versione generata la prossima volta. La modifica resta in memoria ma il chunk non si salva, in qualunque modalità.
                chunk.dirty = false;
                if !std::mem::replace(&mut self.flat_warned, true) {
                    warn!("[mondo] chunk di ripiego (mondo piatto o generazione fallita): le modifiche non si salvano");
                }
            }
            applied.push(((cx, cz), *c));
        }
        for (&pid, player) in &self.players {
            let packets: Vec<Packet> =
                applied.iter().filter(|(chunk, _)| player.sent.contains(chunk)).flat_map(|(_, c)| update_packets(c.pos, c.hash, c.water)).collect();
            if !packets.is_empty() {
                self.send(pid, &packets);
            }
        }
        for pos in gone {
            self.close_windows_at(pos);
        }
        for (pos, nbt) in lost_chests {
            self.chest_removed(pos, &nbt);
        }
        for (pos, nbt) in lost_furnaces {
            self.furnace_removed(pos, &nbt);
        }
        applied.into_iter().map(|(_, c)| c).collect()
    }

    /// Mette (o sostituisce) il block entity di una cella e lo manda a chi vede il chunk (`BlockActorData`). Fuori dai chunk
    /// in memoria non fa nulla.
    pub(crate) fn put_block_entity(&mut self, dim: Dim, entity: vgen::BlockEntity) {
        self.store_block_entity(dim, entity, true);
    }

    /// Come `put_block_entity`; con `notify` falso non manda `BlockActorData` (per i cambi che il client non deve vedere, come il contenuto di un forziere).
    pub(crate) fn store_block_entity(&mut self, dim: Dim, entity: vgen::BlockEntity, notify: bool) {
        let (cx, cz) = (entity.x.div_euclid(16), entity.z.div_euclid(16));
        let Some(chunk) = self.chunks.get_mut(&(dim as u8, cx, cz)) else { return };
        let packet = BlockActorData { block_position: BlockPos { x: entity.x, y: entity.y, z: entity.z }, actor_data_tags: entity.nbt.clone() }.packet();
        let pos = [entity.x, entity.y, entity.z];
        chunk.put_block_entity(entity);
        if chunk.fallback {
            chunk.dirty = false; // vedi `apply_blocks`: i chunk di ripiego non si salvano mai
        }
        if dim == Dim::Overworld && self.block_at(dim, pos).is_some_and(|(hash, _)| crate::furnace::is_furnace(hash)) {
            self.furnaces.insert(pos);
        }
        if !notify {
            return;
        }
        for (&pid, _) in self.players.iter().filter(|(_, p)| p.sent.contains(&(cx, cz))) {
            self.send(pid, std::slice::from_ref(&packet));
        }
    }

    /// Rimanda a un solo giocatore lo stato vero dei blocchi in `positions` (per annullare ciò che il suo client ha già fatto).
    pub(crate) fn resend_blocks(&self, pid: u64, dim: Dim, positions: &[[i32; 3]]) {
        let packets: Vec<Packet> =
            positions.iter().filter_map(|&pos| self.block_at(dim, pos).map(|(hash, water)| update_packets(pos, hash, water))).flatten().collect();
        if !packets.is_empty() {
            self.send(pid, &packets);
        }
    }

    /// Come [`Server::resend_blocks`] per le celle `cells` e per i loro sei vicini. Il client prevede porte, letti e piante alte
    /// per intero, e a volte un blocco diverso da quello che il server decide: dopo un rifiuto (o un esito diverso) la seconda
    /// metà o la cella prevista resterebbero sullo schermo come celle fantasma.
    pub(crate) fn resend_around(&self, pid: u64, dim: Dim, cells: &[[i32; 3]]) {
        self.resend_blocks(pid, dim, &placement::with_neighbors(cells));
    }

    /// Mostra l'azione ai ganci: `false` se almeno uno la annulla (allora chi chiama non deve cambiare nulla).
    pub(crate) fn allow(&mut self, action: &WorldAction) -> bool {
        self.hooks.iter_mut().all(|hook| hook(action))
    }

    /// Tick al secondo reali (al massimo 20), sugli ultimi `TPS_WINDOW` tick.
    fn tps(&self) -> f64 {
        match (self.ticks.front(), self.ticks.back()) {
            (Some(first), Some(last)) if self.ticks.len() > 1 => ((self.ticks.len() - 1) as f64 / last.duration_since(*first).as_secs_f64()).min(20.0),
            _ => 20.0,
        }
    }
}

/// `UpdateBlock` per lo strato 0 (il blocco) e per lo strato 1 (acqua se `water`, altrimenti aria); flag 3 = vicini + rete.
fn update_packets(pos: [i32; 3], hash: i32, water: bool) -> [Packet; 2] {
    let layer = |runtime_id: i32, layer: u32| {
        UpdateBlock { block_position: BlockPos { x: pos[0], y: pos[1], z: pos[2] }, block_runtime_id: runtime_id as u32, flags: 3, layer }.packet()
    };
    let liquid = if water { world::block_hash("minecraft:water", &[("liquid_depth", world::State::Int(0))]) } else { world::block_hash("minecraft:air", &[]) };
    [layer(hash, 0), layer(liquid, 1)]
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
        warn!("[mondo] il seme di server.toml ({seed}) è diverso da quello del mondo esistente: vale quello del mondo ({})", world.seed());
    }
    let generator = GenConfig { launch, seed: world.seed(), restart_delay: Duration::from_secs(2), request_timeout: gen_client::REQUEST_TIMEOUT };
    serve(cfg, Some(world), Some(generator), server_cfg.view_distance.clamp(1, 32), server_cfg.keep_inventory, server_cfg.pvp).await
}

/// Avvia il trasporto e fa girare il gioco con il mondo piatto, senza salvare nulla.
pub async fn run(cfg: Config) -> io::Result<()> {
    serve(cfg, None, None, join::MAX_RADIUS, false, true).await
}

/// Prossimo evento del generatore; senza generatore non arriva mai.
async fn next_gen_event(rx: &mut Option<UnboundedReceiver<GenEvent>>) -> Option<GenEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Il gioco: gli eventi di rete arrivano quando capitano, il tick ogni 50 ms. Ctrl+C chiude in ordine salvando il mondo.
async fn serve(cfg: Config, world: Option<storage::World>, generator: Option<GenConfig>, max_radius: i32, keep_inventory: bool, pvp: bool) -> io::Result<()> {
    // Gli stati dei blocchi e il menu creativo si costruiscono ora, non al pacchetto del primo giocatore.
    join::warm_up();
    let address = blackstone_net::lan_ip().map_or_else(|| "localhost".to_owned(), |ip| ip.to_string());
    log!("Blackstone v{} - Minecraft Bedrock {} (protocollo {}) su {address}:{}", env!("CARGO_PKG_VERSION"), cfg.version, cfg.protocol, cfg.port);
    let mut net = blackstone_net::start(cfg).await?;
    let mut server = Server::new(net.sender.clone());
    server.max_radius = max_radius;
    server.keep_inventory = keep_inventory;
    server.pvp = pvp;
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
                log!("[server] Ctrl+C: chiusura in corso");
                server.shutdown();
                return Ok(());
            }
        }
    }
}
