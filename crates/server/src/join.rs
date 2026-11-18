//! Ingresso nel mondo: StartGame, registro degli oggetti e streaming dei chunk.
use crate::{biomes, streaming::chunk_of, Server};
use bedrock_net::log;
use bedrock_protocol::{
    generated::{
        enums, AvailableActorIdentifiers, BlockPos, ChunkRadiusUpdated, CreativeContent, ItemData, ItemRegistry, JigsawStructureData, LevelSettings,
        NetworkChunkPublisherUpdate, SpawnSettings, StartGame, Vec3, VoxelShapes,
    },
    id,
    packets::{play_status, text_type, PlayStatus, RequestChunkRadius, Text},
    varint,
    wire::ServerPacket,
    Packet, VERSION,
};
use std::sync::OnceLock;

/// Raggio massimo di chunk che il server accetta di inviare.
pub(crate) const MAX_RADIUS: i32 = 6;
/// NBT di un compound vuoto in formato di rete.
const EMPTY_NBT: [u8; 3] = [0x0a, 0, 0];

/// Interruttori di diagnostica (variabili d'ambiente), per isolare un problema senza ricompilare:
/// NO_CHUNKS=1 non invia chunk;
/// CHUNKS_EARLY=1 manda biomi e chunk subito dopo PlayerSpawn, senza aspettare SetLocalPlayerAsInitialized;
/// SKIP=jigsaw,voxel,items,creative,publisher non invia i pacchetti elencati;
/// SEND=actors invia anche i pacchetti spenti di default (vedi `send`).
fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

fn skip(packet: &str) -> bool {
    std::env::var("SKIP").is_ok_and(|v| v.split(',').any(|s| s.trim() == packet))
}

/// Pacchetti da abilitare esplicitamente con SEND=actors. Gli identificatori delle entità sono spenti di default: con i
/// dati minimi che inventiamo (nessuna entità) il client crolla; torneranno con dati completi.
fn send(packet: &str) -> bool {
    std::env::var("SEND").is_ok_and(|v| v.split(',').any(|s| s.trim() == packet))
}

/// NBT di rete: un compound che contiene solo liste vuote di compound, una per nome.
fn nbt_empty_lists(names: &[&str]) -> Vec<u8> {
    let mut b = vec![0x0a, 0]; // compound radice senza nome
    for name in names {
        b.push(0x09); // lista
        varint::write(&mut b, name.len() as u32);
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&[0x0a, 0]); // di compound, lunghezza 0
    }
    b.push(0);
    b
}

/// Oggetti vanilla (nome + ID) dai dati ufficiali di Mojang, già codificati: sono uguali per tutti i giocatori.
fn item_registry() -> &'static Packet {
    static PACKET: OnceLock<Packet> = OnceLock::new();
    PACKET.get_or_init(|| {
        let item_data = include_str!("../data/items.txt")
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(name, id)| ItemData {
                item_name: name.into(),
                item_id: id.parse().expect("ID oggetto non valido in items.txt"),
                item_version: enums::item_version::LEGACY,
                item_component_data: EMPTY_NBT.to_vec(),
                ..Default::default()
            })
            .collect();
        ItemRegistry { item_data }.packet()
    })
}

impl Server {
    /// Il client ha applicato i resource pack: gli diciamo che mondo è e dove si trova.
    pub(crate) fn start_game(&mut self, pid: u64) {
        let Some((sx, sy, sz)) = self.spawn else {
            log!("[#{pid}] lo spawn non è ancora noto: StartGame aspetta il generatore");
            self.waiting_start.push(pid);
            return;
        };
        let spawn = BlockPos { x: sx, y: sy, z: sz };
        // Gli occhi stanno 1.62 sopra i piedi, al centro del blocco di spawn.
        let eyes = [sx as f32 + 0.5, sy as f32 + 1.62, sz as f32 + 0.5];
        if let Some(p) = self.players.get_mut(&pid) {
            p.pos = eyes;
            p.last_chunk = chunk_of(eyes[0], eyes[2]);
        }
        let start = StartGame {
            entity_id: pid as i64,
            runtime_id: pid,
            game_type: enums::game_type::CREATIVE,
            position: Vec3 { x: eyes[0], y: eyes[1], z: eyes[2] }, // occhi del giocatore
            settings: LevelSettings {
                spawn_settings: SpawnSettings { dimension: 0, ..Default::default() },
                generator_type: if self.flat { enums::generator_type::FLAT } else { enums::generator_type::OVERWORLD },
                game_type: enums::game_type::CREATIVE,
                game_difficulty: enums::difficulty::NORMAL,
                default_spawn_block_position: spawn,
                achievements_disabled: true,
                multiplayer_game_intent: true,
                lan_broadcast_intent: true,
                xbox_live_broadcast_setting: enums::game_publish_setting::PUBLIC,
                platform_broadcast_setting: enums::game_publish_setting::PUBLIC,
                commands_enabled: true,
                player_permissions: enums::player_permission_level::MEMBER,
                server_chunk_tick_range: 4,
                base_game_version: VERSION.into(),
                ..Default::default()
            },
            level_name: "Mondo".into(),
            multiplayer_correlation_id: format!("rust-{pid}"),
            enable_item_stack_net_manager: true,
            server_version: VERSION.into(),
            player_property_data: EMPTY_NBT.to_vec(),
            block_network_ids_are_hashes: true,
            ..Default::default()
        };
        log!("[#{pid}] invio StartGame e registro oggetti");
        // Come i server di riferimento (gophertunnel/Dragonfly): dati di struttura e forme, StartGame, oggetti.
        // Biomi e identificatori arrivano dopo che il client si è inizializzato (`initialized`).
        let mut packets = Vec::new();
        if !skip("jigsaw") {
            packets.push(
                JigsawStructureData { jigsaw_structure_data_tag: nbt_empty_lists(&["processors", "template_pools", "jigsaws", "structure_sets"]) }.packet(),
            );
        }
        if !skip("voxel") {
            packets.push(VoxelShapes::default().packet());
        }
        packets.push(start.packet());
        if !skip("items") {
            packets.push(item_registry().clone());
        }
        self.send(pid, &packets);
    }

    /// Il client chiede quanti chunk vuole vedere: si risponde col raggio concesso e con PlayerSpawn. I chunk
    /// non partono ancora: come nei server di riferimento si aspetta `SetLocalPlayerAsInitialized`.
    pub(crate) fn request_chunk_radius(&mut self, pid: u64, body: &[u8]) {
        let Some(request) = RequestChunkRadius::decode(body) else { return };
        let radius = request.chunk_radius.clamp(1, self.max_radius);
        log!("[#{pid}] chunk richiesti: raggio {} -> concesso {radius}", request.chunk_radius);
        if let Some(p) = self.players.get_mut(&pid) {
            p.radius = radius;
        }
        let mut packets =
            vec![ChunkRadiusUpdated { chunk_radius: radius }.packet(), Packet::new(id::PLAY_STATUS, PlayStatus { status: play_status::PLAYER_SPAWN }.encode())];
        if !skip("creative") {
            packets.push(CreativeContent::default().packet());
        }
        self.send(pid, &packets);
        if flag("CHUNKS_EARLY") {
            self.initialized(pid); // diagnostica: il vecchio ordine, senza aspettare il client
        }
    }

    /// Il client ha finito di caricare ed è nel mondo: ora si mandano biomi, identificatori e chunk.
    pub(crate) fn initialized(&mut self, pid: u64) {
        if self.players.get(&pid).is_some_and(|p| p.spawned) {
            return log!("[#{pid}] SetLocalPlayerAsInitialized ripetuto: ignorato");
        }
        let Some(radius) = self.players.get(&pid).filter(|p| p.info.is_some()).map(|p| p.radius).filter(|&r| r > 0) else {
            return log!("[#{pid}] SetLocalPlayerAsInitialized prima della richiesta del raggio: ignorato");
        };
        log!("[#{pid}] il client è inizializzato: invio i chunk");
        let streaming = !flag("NO_CHUNKS");
        let Some(p) = self.players.get_mut(&pid) else { return };
        p.spawned = true;
        p.streaming = streaming;
        p.last_chunk = chunk_of(p.pos[0], p.pos[2]);
        let eyes = p.pos;
        let name = self.name(pid);
        self.tell(pid, "§aBenvenuto sul server Rust! Scrivi /help per i comandi.");
        self.broadcast(Text::new(text_type::RAW, format!("§e{name} è entrato nel gioco")));
        let mut packets = vec![biomes::packet().clone()];
        if send("actors") {
            packets.push(AvailableActorIdentifiers { identifier_list: nbt_empty_lists(&["idlist"]) }.packet());
        }
        if !skip("publisher") {
            packets.push(self.publisher_update(eyes, radius));
        }
        self.send(pid, &packets);
        if !streaming {
            log!("[#{pid}] NO_CHUNKS attivo: non invio chunk");
        }
        // I chunk partono dal tick successivo (`stream_tick`), dal più vicino, al massimo CHUNKS_PER_TICK per tick.
    }

    /// `NetworkChunkPublisherUpdate`: il centro della vista (blocco del giocatore) e il raggio in blocchi.
    pub(crate) fn publisher_update(&self, eyes: [f32; 3], radius: i32) -> Packet {
        NetworkChunkPublisherUpdate {
            new_position_for_view: BlockPos { x: eyes[0].floor() as i32, y: eyes[1].floor() as i32, z: eyes[2].floor() as i32 },
            new_radius_for_view: radius as u32 * 16,
            ..Default::default()
        }
        .packet()
    }
}
