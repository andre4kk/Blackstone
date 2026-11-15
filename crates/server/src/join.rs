//! Ingresso nel mondo: StartGame, registro degli oggetti e streaming dei chunk.
use crate::{world, Server};
use bedrock_net::log;
use bedrock_protocol::{
    generated::{
        enums, AvailableActorIdentifiers, BiomeDefinitionData, BiomeDefinitionList, BiomeStringList, BlockPos, ChunkPos, ChunkRadiusUpdated, CreativeContent,
        ItemData, ItemRegistry, JigsawStructureData, LevelChunk, LevelSettings, NetworkChunkPublisherUpdate, SpawnSettings, StartGame, Vec3, VoxelShapes,
    },
    id,
    packets::{play_status, PlayStatus, RequestChunkRadius},
    varint,
    wire::ServerPacket,
    Packet, VERSION,
};
use std::sync::OnceLock;

/// Raggio massimo di chunk che il server accetta di inviare.
const MAX_RADIUS: i32 = 6;
/// Sezioni di chunk per messaggio: i batch troppo grandi rallentano la compressione e il client.
const CHUNKS_PER_MESSAGE: usize = 16;
/// NBT di un compound vuoto in formato di rete.
const EMPTY_NBT: [u8; 3] = [0x0a, 0, 0];

/// Interruttori di diagnostica (variabili d'ambiente), per isolare un problema senza ricompilare:
/// NO_CHUNKS=1 non invia chunk; SUBCHUNK_V8=1 sotto-chunk in versione 8 invece della 9;
/// CHUNKS_EARLY=1 manda biomi e chunk subito dopo PlayerSpawn, senza aspettare SetLocalPlayerAsInitialized;
/// SKIP=jigsaw,voxel,items,biomes,actors,creative non invia i pacchetti elencati.
fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

fn skip(packet: &str) -> bool {
    std::env::var("SKIP").is_ok_and(|v| v.split(',').any(|s| s.trim() == packet))
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

/// Biomi: per ora solo la pianura, con i valori dei dati ufficiali (clima 0.8/0.4, acqua #44AFF5).
/// La chiave della mappa è l'indice del nome nella lista di stringhe.
fn biome_definitions() -> BiomeDefinitionList {
    let plains = BiomeDefinitionData {
        id: world::PLAINS,
        temperature: 0.8,
        downfall: 0.4,
        depth: 0.125,
        scale: 0.05,
        map_water_color_argb: 0xFF44_AFF5u32 as i32,
        rain: true,
        ..Default::default()
    };
    BiomeDefinitionList { map_of_biome_names_to_data: vec![(0, plains)], string_list: BiomeStringList { strings: vec!["minecraft:plains".into()] } }
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
    pub(crate) fn start_game(&self, pid: u64) {
        let spawn = BlockPos { x: 0, y: world::GROUND_Y, z: 0 };
        let start = StartGame {
            entity_id: pid as i64,
            runtime_id: pid,
            game_type: enums::game_type::CREATIVE,
            position: Vec3 { x: 0.5, y: world::GROUND_Y as f32 + 1.62, z: 0.5 }, // occhi del giocatore
            settings: LevelSettings {
                spawn_settings: SpawnSettings { dimension: 0, ..Default::default() },
                generator_type: enums::generator_type::FLAT,
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
            packets.push(JigsawStructureData { jigsaw_structure_data_tag: nbt_empty_lists(&["processors", "template_pools", "jigsaws", "structure_sets"]) }.packet());
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
        let radius = request.chunk_radius.clamp(1, MAX_RADIUS);
        log!("[#{pid}] chunk richiesti: raggio {} -> concesso {radius}", request.chunk_radius);
        if let Some(p) = self.players.get_mut(&pid) {
            p.radius = radius;
        }
        let mut packets = vec![
            ChunkRadiusUpdated { chunk_radius: radius }.packet(),
            Packet::new(id::PLAY_STATUS, PlayStatus { status: play_status::PLAYER_SPAWN }.encode()),
        ];
        if !skip("creative") {
            packets.push(CreativeContent::default().packet());
        }
        self.send(pid, &packets);
        if flag("CHUNKS_EARLY") {
            self.initialized(pid); // diagnostica: il vecchio ordine, senza aspettare il client
        }
    }

    /// Il client ha finito di caricare ed è nel mondo: ora si mandano biomi, identificatori e chunk.
    pub(crate) fn initialized(&self, pid: u64) {
        let Some(radius) = self.players.get(&pid).map(|p| p.radius).filter(|&r| r > 0) else {
            return log!("[#{pid}] SetLocalPlayerAsInitialized prima della richiesta del raggio: ignorato");
        };
        log!("[#{pid}] il client è inizializzato: invio biomi e chunk");
        let mut packets = Vec::new();
        if !skip("biomes") {
            packets.push(biome_definitions().packet());
        }
        if !skip("actors") {
            packets.push(AvailableActorIdentifiers { identifier_list: nbt_empty_lists(&["idlist"]) }.packet());
        }
        packets.push(NetworkChunkPublisherUpdate {
            new_position_for_view: BlockPos { x: 0, y: world::GROUND_Y, z: 0 },
            new_radius_for_view: radius as u32 * 16,
            ..Default::default()
        }
        .packet());
        self.send(pid, &packets);

        if flag("NO_CHUNKS") {
            return log!("[#{pid}] NO_CHUNKS attivo: non invio chunk");
        }
        let v9 = !flag("SUBCHUNK_V8"); // la v9 è quella dei server di riferimento
        let chunks: Vec<Packet> = (-radius..=radius)
            .flat_map(|x| (-radius..=radius).map(move |z| (x, z)))
            .filter(|(x, z)| x * x + z * z <= radius * radius + radius) // area quasi circolare
            .map(|(x, z)| {
                LevelChunk { chunk_position: ChunkPos { x, z }, sub_chunks_count: 1, serialized_chunk_data: world::flat_chunk(v9).to_vec(), ..Default::default() }.packet()
            })
            .collect();
        for group in chunks.chunks(CHUNKS_PER_MESSAGE) {
            self.send(pid, group);
        }
        log!("[#{pid}] inviati {} chunk", chunks.len());
    }
}
