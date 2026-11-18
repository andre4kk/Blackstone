//! Ingresso nel mondo: StartGame, registro degli oggetti e streaming dei chunk.
use crate::{
    biomes,
    inventory::window,
    playerdata::{self, PlayerData, EYE_HEIGHT},
    registry::{self, CreativeItem},
    streaming::chunk_of,
    DataState, GameMode, Server,
};
use bedrock_net::log;
use bedrock_protocol::{
    generated::{
        enums::{self, container_enum_name},
        AttributeData, AvailableActorIdentifiers, BlockPos, ChunkRadiusUpdated, CreativeContent, CreativeGroupInfoPayload, CreativeItemEntryPayload,
        FullContainerName, InventoryContent, ItemData, ItemRegistry, JigsawStructureData, LevelSettings, NetworkChunkPublisherUpdate,
        NetworkItemInstanceDescriptorData, PlayerHotbar, SerializedAbilitiesData, SerializedAbilitiesDataSerializedLayer, SetPlayerGameType, SpawnSettings,
        StartGame, SyncedPlayerMovementSettings, UpdateAbilities, UpdateAttributes, Vec2, Vec3, VoxelShapes,
    },
    id,
    item::ItemStack,
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
/// SEND=biomes,actors invia anche i pacchetti spenti di default (vedi `send`).
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
/// Voci di `ItemRegistry`: nome e ID da `items.txt`, versione, `component_based` e dati dei componenti da Dragonfly
/// (`items_meta.json`, estratto con `tools/extract_item_meta.py`). Il client collega un oggetto al suo blocco solo con la
/// versione giusta: mandando tutto come "legacy" i blocchi nuovi (assi di warped…) restavano oggetti senza texture.
/// Gli oggetti che Dragonfly non conosce prendono la versione 2 (None), quella di quasi tutti gli oggetti vanilla.
pub(crate) fn item_entries() -> Vec<ItemData> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let meta: serde_json::Value = serde_json::from_str(include_str!("../data/items_meta.json")).expect("items_meta.json non valido");
    let meta = &meta["items"];
    include_str!("../data/items.txt")
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(name, id)| {
            let m = &meta[name];
            ItemData {
                item_name: name.into(),
                item_id: id.parse().expect("ID oggetto non valido in items.txt"),
                is_component_based: m[1].as_bool().unwrap_or(false),
                item_version: m[0].as_i64().unwrap_or(enums::item_version::NONE as i64) as i32,
                item_component_data: m[2].as_str().map_or_else(|| EMPTY_NBT.to_vec(), |b| STANDARD.decode(b).expect("dati dei componenti non validi")),
            }
        })
        .collect()
}

fn item_registry() -> &'static Packet {
    static PACKET: OnceLock<Packet> = OnceLock::new();
    PACKET.get_or_init(|| ItemRegistry { item_data: item_entries() }.packet())
}

/// Bit di `UpdateAbilities` (P§7.2).
mod ability {
    pub const BUILD: u32 = 1 << 0;
    pub const MINE: u32 = 1 << 1;
    pub const DOORS_AND_SWITCHES: u32 = 1 << 2;
    pub const OPEN_CONTAINERS: u32 = 1 << 3;
    pub const ATTACK_PLAYERS: u32 = 1 << 4;
    pub const ATTACK_MOBS: u32 = 1 << 5;
    pub const INVULNERABLE: u32 = 1 << 8;
    pub const FLYING: u32 = 1 << 9;
    pub const MAY_FLY: u32 = 1 << 10;
    pub const INSTANT_BUILD: u32 = 1 << 11;
    pub const NO_CLIP: u32 = 1 << 17;
    /// Tutte le abilità che il client conosce: il valore di ognuna lo decide il server.
    pub const ALL: u32 = 0xF_FFFF;
    /// Tipi di livello.
    pub const LAYER_BASE: u16 = 1;
    pub const LAYER_SPECTATOR: u16 = 2;
}

/// Quali abilità ha il giocatore in una modalità (come Dragonfly, `SendAbilities`). Il volo lo chiede il client
/// (`StartFlying`/`StopFlying` in `PlayerAuthInput`) e il server lo conferma qui.
pub(crate) fn abilities_for(mode: GameMode, flying: bool) -> u32 {
    use ability::*;
    if mode == GameMode::Spectator {
        // Lo spettatore ha come livello base quello di un giocatore normale: il resto lo aggiunge il livello sopra.
        return BUILD | MINE | DOORS_AND_SWITCHES | OPEN_CONTAINERS | ATTACK_PLAYERS | ATTACK_MOBS;
    }
    let mut values = DOORS_AND_SWITCHES | OPEN_CONTAINERS | ATTACK_PLAYERS | ATTACK_MOBS;
    if mode == GameMode::Creative {
        values |= MAY_FLY | INVULNERABLE | INSTANT_BUILD;
        if flying {
            values |= FLYING;
        }
    }
    if matches!(mode, GameMode::Survival | GameMode::Creative) {
        values |= BUILD | MINE;
    }
    values
}

/// Livelli di abilità per la modalità: il livello base e, per lo spettatore, quello che lo fa volare senza collisioni.
pub(crate) fn ability_layers(mode: GameMode, flying: bool) -> Vec<SerializedAbilitiesDataSerializedLayer> {
    let layer = |serialized_layer, abilities_set, ability_values| SerializedAbilitiesDataSerializedLayer {
        serialized_layer,
        abilities_set,
        ability_values,
        fly_speed: 0.05,
        vertical_fly_speed: 1.0,
        walk_speed: 0.1,
    };
    let mut layers = Vec::new();
    if mode == GameMode::Spectator {
        use ability::*;
        let set =
            BUILD | MINE | DOORS_AND_SWITCHES | OPEN_CONTAINERS | ATTACK_PLAYERS | ATTACK_MOBS | INVULNERABLE | FLYING | MAY_FLY | INSTANT_BUILD | NO_CLIP;
        layers.push(layer(LAYER_SPECTATOR, set, INVULNERABLE | FLYING | NO_CLIP));
    }
    layers.push(layer(ability::LAYER_BASE, ability::ALL, abilities_for(mode, flying)));
    layers
}

/// Una voce del menu creativo come la vuole `CreativeContent`.
fn creative_descriptor(c: &CreativeItem) -> NetworkItemInstanceDescriptorData {
    NetworkItemInstanceDescriptorData {
        id: registry::items().id(&c.name).map_or(0, i32::from),
        stack_size: 1,
        aux_value: c.meta,
        block_runtime_id: c.block.unwrap_or(0),
        user_data_buffer: match &c.nbt {
            Some(nbt) => ItemStack::user_data_with_nbt(nbt),
            None => ItemStack::empty_user_data(),
        },
    }
}

/// Attributi del giocatore che il client usa per prevedere il movimento (velocità) e per vita e fame, con i valori vanilla.
/// Senza questo pacchetto il client non riceve mai la velocità di camminata: Dragonfly lo manda subito dopo `StartGame`.
pub(crate) fn spawn_attributes(runtime_id: u64) -> UpdateAttributes {
    let attribute = |name: &str, value: f32, max: f32, default: f32| AttributeData {
        min_value: 0.0,
        max_value: max,
        current_value: value,
        default_min_value: 0.0,
        default_max_value: max,
        default_value: default,
        name: name.into(),
        modifiers: Vec::new(),
    };
    UpdateAttributes {
        target_runtime_id: runtime_id,
        attribute_list: vec![
            attribute("minecraft:movement", 0.1, f32::MAX, 0.1),
            attribute("minecraft:health", 20.0, 20.0, 20.0),
            attribute("minecraft:player.hunger", 20.0, 20.0, 20.0),
            attribute("minecraft:player.saturation", 5.0, 20.0, 5.0),
            attribute("minecraft:player.exhaustion", 0.0, 5.0, 0.0),
        ],
        tick: 0,
    }
}

/// Menu creativo completo (uguale per tutti: net id = indice + 1, come Dragonfly). Le icone dei gruppi non possono essere aria
/// (il client scarta gli oggetti non validi): ai gruppi anonimi si dà come icona la loro prima voce.
fn creative_packet() -> &'static Packet {
    static PACKET: OnceLock<Packet> = OnceLock::new();
    PACKET.get_or_init(|| {
        let (groups, items) = registry::creative();
        let groups = groups
            .iter()
            .enumerate()
            .map(|(index, g)| {
                let icon = g.icon.as_ref().or_else(|| items.iter().find(|i| i.group as usize == index)).or_else(|| items.first());
                CreativeGroupInfoPayload {
                    creative_category: g.category,
                    name: g.name.clone(),
                    group_icon_item: icon.map(creative_descriptor).unwrap_or_default(),
                }
            })
            .collect();
        let entries = items
            .iter()
            .enumerate()
            .map(|(index, i)| CreativeItemEntryPayload { creative_net_id: index as u32 + 1, item_instance: creative_descriptor(i), group_index: i.group })
            .collect();
        CreativeContent { groups, entries }.packet()
    })
}

/// Prepara i dati pesanti da costruire (stati dei blocchi, menu creativo) prima che entri il primo giocatore.
pub(crate) fn warm_up() {
    registry::blocks();
    creative_packet();
}

impl Server {
    /// Il disco ha risposto con i dati salvati del giocatore (o senza): si applicano e il login riprende da `StartGame`.
    pub(crate) fn on_player_loaded(&mut self, pid: u64, data: Option<Vec<u8>>) {
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.data == DataState::Loading) else {
            return log!("[#{pid}] dati del giocatore arrivati per chi non li aspetta più: scartati");
        };
        p.data = DataState::Loaded;
        match data.as_deref().map(PlayerData::from_nbt) {
            None => log!("[#{pid}] nessun dato salvato: giocatore nuovo"),
            Some(None) => log!("[#{pid}] dati salvati illeggibili: si parte da capo"),
            Some(Some(d)) => {
                log!("[#{pid}] dati salvati caricati: modalità {:?}, posizione {:?}", d.mode, d.feet);
                // un'altra dimensione o una posizione impossibile non si possono usare: si parte dallo spawn, il resto resta
                if d.dim == 0 && d.feet.iter().all(|v| v.abs() <= 30_000_000.0) {
                    p.saved_feet = Some(d.feet);
                } else {
                    log!("[#{pid}] posizione salvata non utilizzabile (dimensione {}): parte dallo spawn", d.dim);
                }
                (p.mode, p.yaw, p.pitch, p.inventory) = (d.mode, d.yaw, d.pitch, d.inventory);
            }
        }
        self.start_game(pid);
    }

    /// Prima di `StartGame` servono i dati salvati del giocatore: si chiedono al disco e il login aspetta la risposta
    /// (`on_player_loaded`). Vero se ci sono già (o non c'è un disco da cui leggerli).
    fn player_data_ready(&mut self, pid: u64) -> bool {
        let Some(p) = self.players.get_mut(&pid) else { return false };
        match (p.data, &self.disk, &p.info) {
            (DataState::Loaded, ..) => true,
            (DataState::Loading, ..) => false, // già chiesti: arriveranno
            (DataState::Unrequested, Some(disk), Some(info)) => {
                log!("[#{pid}] StartGame aspetta i dati salvati del giocatore");
                disk.load_player(pid, playerdata::identity(info));
                p.data = DataState::Loading;
                false
            }
            (DataState::Unrequested, ..) => {
                p.data = DataState::Loaded; // senza mondo su disco si parte vuoti
                true
            }
        }
    }

    /// Il client ha applicato i resource pack: gli diciamo che mondo è e dove si trova.
    pub(crate) fn start_game(&mut self, pid: u64) {
        if !self.player_data_ready(pid) {
            return;
        }
        let Some((sx, sy, sz)) = self.spawn else {
            log!("[#{pid}] lo spawn non è ancora noto: StartGame aspetta il generatore");
            self.waiting_start.push(pid);
            return;
        };
        let spawn = BlockPos { x: sx, y: sy, z: sz };
        // Gli occhi stanno 1.62 sopra i piedi, al centro del blocco di spawn.
        let mut eyes = [sx as f32 + 0.5, sy as f32 + EYE_HEIGHT, sz as f32 + 0.5];
        let Some(p) = self.players.get_mut(&pid) else { return };
        if let Some(feet) = p.saved_feet.take() {
            eyes = [feet[0], feet[1] + EYE_HEIGHT, feet[2]]; // la posizione salvata vale più dello spawn
        }
        p.pos = eyes;
        p.last_chunk = chunk_of(eyes[0], eyes[2]);
        p.started = true; // da qui posizione e inventario sono quelli veri: all'uscita si salvano
        let (mode, rotation) = (p.mode, Vec2 { x: p.pitch, y: p.yaw });
        let start = StartGame {
            entity_id: pid as i64,
            runtime_id: pid,
            game_type: mode as i32,
            position: Vec3 { x: eyes[0], y: eyes[1], z: eyes[2] }, // occhi del giocatore
            rotation,
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
            // La rottura dei blocchi la decide il client (azioni di rottura + UseItem Destroy) e il server la controlla: P§8.
            movement_settings: SyncedPlayerMovementSettings { rewind_history_size: 0, server_authoritative_block_breaking: false },
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
            packets.push(creative_packet().clone());
        }
        packets.push(spawn_attributes(pid).packet());
        if !skip("metadata") {
            packets.push(bedrock_protocol::actor::set_actor_data(pid, &self.entity_data(pid)));
            // gravità, collisioni, arrampicata: senza, il giocatore non cade
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
        self.add_player_entity(pid);
        self.tell(pid, "§aBenvenuto sul server Rust! Scrivi /help per i comandi.");
        self.broadcast(Text::new(text_type::RAW, format!("§e{name} è entrato nel gioco")));
        self.send_abilities(pid);
        self.send_inventory(pid);
        let mut packets = Vec::new();
        // La nostra BiomeDefinitionList fa chiudere il client (provato il 18/11/2026): spenta finché non è completa.
        // Senza, il client usa le sue definizioni interne e gli ID dei biomi nei chunk funzionano lo stesso.
        if send("biomes") {
            packets.push(biomes::packet().clone());
        }
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

    /// Modalità di gioco e abilità del giocatore (`SetPlayerGameType` e `UpdateAbilities`, livello base).
    pub(crate) fn send_abilities(&self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let data = SerializedAbilitiesData {
            target_player_raw_id: pid as i64,
            player_permissions: enums::player_permission_level::MEMBER,
            command_permissions: 0,
            layers: ability_layers(p.mode, p.flying),
        };
        self.send(pid, &[SetPlayerGameType { player_game_type: p.mode as i32 }.packet(), UpdateAbilities { data }.packet()]);
    }

    /// Solo le abilità (`UpdateAbilities`): la risposta a `StartFlying`/`StopFlying`, come Dragonfly `SendAbilities`.
    pub(crate) fn send_only_abilities(&self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let data = SerializedAbilitiesData {
            target_player_raw_id: pid as i64,
            player_permissions: enums::player_permission_level::MEMBER,
            command_permissions: 0,
            layers: ability_layers(p.mode, p.flying),
        };
        self.send(pid, &[UpdateAbilities { data }.packet()]);
    }

    /// Inventario, mano secondaria e armatura, e quale casella della barra è in mano.
    pub(crate) fn send_inventory(&self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let content = |window_id: u32, container: u8, slots: Vec<ItemStack>| {
            InventoryContent {
                container_id: window_id,
                slots,
                full_container_name: FullContainerName { container_name: container, dynamic_id: None },
                storage_item: ItemStack::air(),
            }
            .packet()
        };
        let inv = &p.inventory;
        self.send(
            pid,
            &[
                content(window::INVENTORY, container_enum_name::INVENTORY_CONTAINER, inv.main_stacks()),
                content(window::OFFHAND, container_enum_name::OFFHAND_CONTAINER, inv.offhand_stacks()),
                content(window::ARMOR, container_enum_name::ARMOR_CONTAINER, inv.armor_stacks()),
                PlayerHotbar { selected_slot: inv.selected as u32, container_id: window::INVENTORY as u8, should_select_slot: true }.packet(),
            ],
        );
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
