//! Traduzione dei chunk Java (Steel) in dati Bedrock con le mappature di Geyser (`data/geyser`, MIT).
//!
//! - Blocchi: `blocks.nbt` ha una voce per ogni stato Java, nello stesso ordine degli ID di stato di Steel
//!   (verificato dal numero totale di stati e dai test); il nome Bedrock manca quando coincide con quello Java.
//! - Biomi: `biomes.json` associa il nome Java all'ID numerico Bedrock.
//! - Block entity: si ricavano dallo stato di blocco (casse, letti, stendardi, teste, vasi, campane,
//!   cartelli, generatori di mostri) e si arricchiscono con l'NBT Java quando c'è (mostro del generatore, testo dei cartelli).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_registry::{REGISTRY, RegistryExt};
use steel_utils::{BlockStateId, Identifier};
use vgen::{BlockEntity, ChunkData, Section};

use crate::bedrock_hash::{State, block_hash, block_nbt, hash_nbt};
use crate::nbt::Tag;
use crate::steel::JavaChunk;

/// Voce di `blocks.nbt`: nome Bedrock (se diverso da quello Java) e stati Bedrock.
pub struct GeyserBlock {
    name: Option<&'static str>,
    states: &'static [(&'static str, State<'static>)],
}

include!(concat!(env!("OUT_DIR"), "/geyser_blocks.rs"));
include!(concat!(env!("OUT_DIR"), "/geyser_biomes.rs"));

/// ID Bedrock del bioma di ripiego (`minecraft:plains`).
const PLAINS: u32 = 1;
/// Indice "nessuna traduzione" in `Translator::java_to_bedrock`.
const NONE: u32 = u32::MAX;

/// Uno stato di blocco Bedrock che il traduttore può produrre.
struct BedrockState {
    name: String,
    states: &'static [(&'static str, State<'static>)],
    hash: i32,
}

/// Blocco Java che su Bedrock richiede un block entity.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    None,
    /// `pair`: scostamento (dx, dz) dell'altra metà di una cassa doppia; `lead`: metà che comanda.
    Chest {
        pair: Option<(i32, i32)>,
        lead: bool,
    },
    Bed(i8),
    /// Colore di base già nell'ordine Bedrock (15 - colore Java).
    Banner(i32),
    Skull {
        ty: i8,
        rotation: f32,
    },
    /// Pianta come indice in `Translator::bedrock`.
    FlowerPot(Option<u32>),
    /// Direzione Bedrock: sud 0, ovest 1, nord 2, est 3.
    Bell(i32),
    Spawner,
    Sign {
        hanging: bool,
    },
}

/// Tabelle di traduzione Java → Bedrock, costruite una volta all'avvio.
pub struct Translator {
    /// Per stato Java: indice in `bedrock`, o `NONE`.
    java_to_bedrock: Vec<u32>,
    bedrock: Vec<BedrockState>,
    /// Per stato Java: block entity Bedrock richiesto.
    kinds: Vec<Kind>,
    /// Per bioma Java (ID di registro): ID Bedrock.
    biomes: Vec<u32>,
    fallback: i32,
    air: i32,
    unmapped: AtomicU64,
}

/// Ordine dei colori Java (`DyeColor`), lo stesso dei letti Bedrock.
const DYES: [&str; 16] =
    ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black"];

fn dye(prefix: &str) -> Option<i8> {
    DYES.iter().position(|&d| d == prefix).map(|i| i as i8)
}

fn property(id: u16, name: &str) -> Option<&'static str> {
    REGISTRY.blocks.get_property_str(BlockStateId(id), name)
}

impl Translator {
    /// Costruisce le tabelle dal registro di Steel e dalle mappature Geyser incluse nel binario.
    pub fn new() -> Translator {
        steel_core::bootstrap::init_globals();
        let lookup = &REGISTRY.blocks.state_to_block_lookup;
        if lookup.len() != GEYSER_BLOCKS.len() {
            eprintln!(
                "attenzione: Steel ha {} stati di blocco, le mappature Geyser {}: gli stati in eccesso non sono tradotti",
                lookup.len(),
                GEYSER_BLOCKS.len()
            );
        }
        let mut bedrock: Vec<BedrockState> = Vec::new();
        let mut by_hash: HashMap<i32, u32> = HashMap::new();
        let mut intern = |name: String, states: &'static [(&'static str, State<'static>)]| -> u32 {
            let hash = block_hash(&name, states);
            *by_hash.entry(hash).or_insert_with(|| {
                bedrock.push(BedrockState { name, states, hash });
                (bedrock.len() - 1) as u32
            })
        };
        let java_to_bedrock: Vec<u32> = lookup
            .iter()
            .enumerate()
            .map(|(id, block)| match GEYSER_BLOCKS.get(id) {
                Some(g) => intern(g.name.map_or_else(|| block.key.to_string(), str::to_string), g.states),
                None => NONE,
            })
            .collect();
        let fallback_index = intern("minecraft:info_update".to_string(), &[]);
        let fallback = bedrock[fallback_index as usize].hash;

        let mut t = Translator {
            java_to_bedrock,
            bedrock,
            kinds: Vec::new(),
            biomes: Vec::new(),
            fallback,
            air: block_hash("minecraft:air", &[]),
            unmapped: AtomicU64::new(0),
        };
        t.kinds = (0..lookup.len()).map(|id| t.kind_of(id as u16)).collect();
        t.biomes = (0..REGISTRY.biomes.len())
            .map(|id| {
                let key = REGISTRY.biomes.by_id(id).expect("bioma registrato").key.to_string();
                GEYSER_BIOMES.iter().find(|(name, _)| *name == key).map_or(PLAINS, |&(_, b)| b)
            })
            .collect();
        t
    }

    /// Hash Bedrock dello stato Java; `minecraft:info_update` (contato) se non c'è corrispondenza.
    pub fn block(&self, java_state: u16) -> i32 {
        match self.java_to_bedrock.get(java_state as usize) {
            Some(&i) if i != NONE => self.bedrock[i as usize].hash,
            _ => {
                self.unmapped.fetch_add(1, Ordering::Relaxed);
                self.fallback
            }
        }
    }

    /// ID Bedrock del bioma Java; `minecraft:plains` (1) se sconosciuto.
    pub fn biome(&self, java_biome: u16) -> u32 {
        self.biomes.get(java_biome as usize).copied().unwrap_or(PLAINS)
    }

    /// Quante traduzioni di blocco sono ricadute sul ripiego.
    pub fn unmapped(&self) -> u64 {
        self.unmapped.load(Ordering::Relaxed)
    }

    /// Per ogni stato Bedrock che il traduttore può produrre: (hash, NBT su disco little-endian `{name, states}`).
    pub fn palette_entries(&self) -> Vec<(i32, Vec<u8>)> {
        self.bedrock
            .iter()
            .map(|b| {
                let nbt = block_nbt(&b.name, b.states);
                debug_assert_eq!(hash_nbt(&nbt), b.hash);
                (b.hash, nbt)
            })
            .collect()
    }

    fn kind(&self, java_state: u16) -> Kind {
        self.kinds.get(java_state as usize).copied().unwrap_or(Kind::None)
    }

    /// Indice Bedrock dello stato predefinito di un blocco Java (per le piante nei vasi).
    fn default_bedrock(&self, java_name: &str) -> Option<u32> {
        let key: Identifier = java_name.parse().ok()?;
        let block = REGISTRY.blocks.by_key(&key)?;
        let id = REGISTRY.blocks.get_default_state_id(block).0;
        self.java_to_bedrock.get(id as usize).copied().filter(|&i| i != NONE)
    }

    fn kind_of(&self, id: u16) -> Kind {
        let block = REGISTRY.blocks.state_to_block_lookup[id as usize];
        if block.key.namespace != "minecraft" {
            return Kind::None;
        }
        let path: &str = &block.key.path;
        match path {
            "chest" | "trapped_chest" => {
                let ty = property(id, "type").unwrap_or("single");
                if ty == "single" {
                    return Kind::Chest { pair: None, lead: false };
                }
                // Come `ChestBlock.getConnectedDirection`: la metà sinistra ha l'altra in senso orario.
                let left = ty == "left";
                let s = if left { 1 } else { -1 };
                let pair = match property(id, "facing").unwrap_or("north") {
                    "north" => (s, 0),
                    "south" => (-s, 0),
                    "east" => (0, s),
                    _ => (0, -s),
                };
                Kind::Chest { pair: Some(pair), lead: !left }
            }
            "bell" => Kind::Bell(match property(id, "facing").unwrap_or("north") {
                "south" => 0,
                "west" => 1,
                "north" => 2,
                _ => 3,
            }),
            "spawner" => Kind::Spawner,
            "flower_pot" => Kind::FlowerPot(None),
            _ => {
                if let Some(plant) = path.strip_prefix("potted_") {
                    let plant = match plant {
                        "azalea_bush" => "azalea",
                        "flowering_azalea_bush" => "flowering_azalea",
                        p => p,
                    };
                    return Kind::FlowerPot(self.default_bedrock(&format!("minecraft:{plant}")));
                }
                if let Some(color) = path.strip_suffix("_bed").and_then(dye) {
                    return Kind::Bed(color);
                }
                if let Some(color) = path.strip_suffix("_wall_banner").or_else(|| path.strip_suffix("_banner")).and_then(dye) {
                    return Kind::Banner(15 - i32::from(color));
                }
                if path.ends_with("_hanging_sign") {
                    return Kind::Sign { hanging: true };
                }
                if path.ends_with("_sign") {
                    return Kind::Sign { hanging: false };
                }
                let skull = match path {
                    "skeleton_skull" | "skeleton_wall_skull" => 0,
                    "wither_skeleton_skull" | "wither_skeleton_wall_skull" => 1,
                    "zombie_head" | "zombie_wall_head" => 2,
                    "player_head" | "player_wall_head" => 3,
                    "creeper_head" | "creeper_wall_head" => 4,
                    "dragon_head" | "dragon_wall_head" => 5,
                    "piglin_head" | "piglin_wall_head" => 6,
                    _ => return Kind::None,
                };
                let rotation = property(id, "rotation").and_then(|r| r.parse::<f32>().ok()).map_or(0.0, |r| r * 22.5);
                Kind::Skull { ty: skull, rotation }
            }
        }
    }

    /// NBT di rete Bedrock del block entity in `(x, y, z)`; `java` è l'NBT Java, se Steel ne ha uno.
    fn block_entity(&self, kind: Kind, x: i32, y: i32, z: i32, java: Option<&NbtCompound>) -> Option<Vec<u8>> {
        let s = |v: &str| Tag::Str(v.to_string());
        let id = match kind {
            Kind::None => return None,
            Kind::Chest { .. } => "Chest",
            Kind::Bed(_) => "Bed",
            Kind::Banner(_) => "Banner",
            Kind::Skull { .. } => "Skull",
            Kind::FlowerPot(_) => "FlowerPot",
            Kind::Bell(_) => "Bell",
            Kind::Spawner => "MobSpawner",
            Kind::Sign { hanging: false } => "Sign",
            Kind::Sign { hanging: true } => "HangingSign",
        };
        let mut nbt = vec![
            ("id".to_string(), s(id)),
            ("x".to_string(), Tag::Int(x)),
            ("y".to_string(), Tag::Int(y)),
            ("z".to_string(), Tag::Int(z)),
            ("isMovable".to_string(), Tag::Byte(1)),
        ];
        let mut put = |k: &str, v: Tag| nbt.push((k.to_string(), v));
        match kind {
            Kind::None => unreachable!(),
            Kind::Chest { pair, lead } => {
                put("Findable", Tag::Byte(0));
                if let Some((dx, dz)) = pair {
                    put("pairx", Tag::Int(x + dx));
                    put("pairz", Tag::Int(z + dz));
                    if lead {
                        put("pairlead", Tag::Byte(1));
                    }
                }
            }
            Kind::Bed(color) => put("color", Tag::Byte(color)),
            Kind::Banner(base) => {
                put("Base", Tag::Int(base));
                put("Type", Tag::Int(0));
            }
            Kind::Skull { ty, rotation } => {
                put("SkullType", Tag::Byte(ty));
                put("Rotation", Tag::Float(rotation));
                put("MouthMoving", Tag::Byte(0));
                put("MouthTickCount", Tag::Int(0));
            }
            Kind::FlowerPot(plant) => {
                if let Some(b) = plant.map(|i| &self.bedrock[i as usize]) {
                    let mut states: Vec<(String, Tag)> = b
                        .states
                        .iter()
                        .map(|&(k, v)| {
                            let v = match v {
                                State::Byte(b) => Tag::Byte(b as i8),
                                State::Int(i) => Tag::Int(i),
                                State::Str(t) => s(t),
                            };
                            (k.to_string(), v)
                        })
                        .collect();
                    states.sort_by(|a, b| a.0.cmp(&b.0));
                    put("PlantBlock", Tag::Compound(vec![("name".to_string(), s(&b.name)), ("states".to_string(), Tag::Compound(states))]));
                }
            }
            Kind::Bell(direction) => {
                put("Direction", Tag::Int(direction));
                put("Ringing", Tag::Byte(0));
                put("Ticks", Tag::Int(0));
            }
            Kind::Spawner => {
                let entity = java.and_then(|j| j.compound("SpawnData")).and_then(|d| d.compound("entity")).and_then(|e| e.string("id"));
                if let Some(entity) = entity {
                    put("EntityIdentifier", s(&entity.to_str()));
                }
            }
            Kind::Sign { .. } => {
                let text = |side: &str| -> Tag {
                    let lines: Vec<String> = java
                        .and_then(|j| j.compound(side))
                        .and_then(|t| t.list("messages"))
                        .map(|m| m.as_nbt_tags().iter().map(plain_text).collect())
                        .unwrap_or_default();
                    let text = lines.join("\n").trim_end_matches('\n').to_string();
                    Tag::Compound(vec![
                        ("Text".to_string(), Tag::Str(text)),
                        ("TextOwner".to_string(), Tag::Str(String::new())),
                        ("SignTextColor".to_string(), Tag::Int(0xFF00_0000u32 as i32)),
                        ("GlowIgnited".to_string(), Tag::Byte(0)),
                        ("HideGlowOutline".to_string(), Tag::Byte(0)),
                        ("PersistFormatting".to_string(), Tag::Byte(1)),
                    ])
                };
                put("FrontText", text("front_text"));
                put("BackText", text("back_text"));
                put("IsWaxed", Tag::Byte(0));
            }
        }
        Some(Tag::Compound(nbt).to_network())
    }
}

impl Default for Translator {
    fn default() -> Self {
        Translator::new()
    }
}

/// Testo semplice di un componente di testo Java in NBT (stringa o compound con `text`).
fn plain_text(tag: &NbtTag) -> String {
    match tag {
        NbtTag::String(s) => s.to_str().into_owned(),
        NbtTag::Compound(c) => c.string("text").map(|t| t.to_str().into_owned()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Traduce un chunk Java in dati Bedrock: palette di hash, ordine dei blocchi `(x*16 + z)*16 + y`,
/// biomi `(x*4 + z)*4 + y`, sezioni di sola aria omesse, block entity in NBT di rete.
pub fn to_bedrock(chunk: &JavaChunk, t: &Translator) -> ChunkData {
    let java_entities: HashMap<(i32, i32, i32), &NbtCompound> = chunk.block_entities.iter().map(|(pos, _, nbt)| ((pos.x(), pos.y(), pos.z()), nbt)).collect();
    let first_section = chunk.min_y.div_euclid(16);
    // Stato Java → indice nella palette della sezione corrente (u16::MAX = non ancora visto).
    let mut local = vec![u16::MAX; t.java_to_bedrock.len()];
    let mut sections = Vec::new();
    let mut block_entities = Vec::new();
    for (i, java) in chunk.sections.iter().enumerate() {
        let mut palette: Vec<i32> = Vec::new();
        let mut seen: Vec<u16> = Vec::new();
        let mut has_entities = false;
        let mut blocks = Box::new([0u16; 4096]);
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let state = java.states[(y * 16 + z) * 16 + x];
                    let index = match local.get(state as usize) {
                        Some(&index) if index != u16::MAX => index,
                        _ => {
                            let hash = t.block(state);
                            has_entities |= t.kind(state) != Kind::None;
                            let index = palette.iter().position(|&h| h == hash).unwrap_or_else(|| {
                                palette.push(hash);
                                palette.len() - 1
                            }) as u16;
                            if let Some(slot) = local.get_mut(state as usize) {
                                *slot = index;
                                seen.push(state);
                            }
                            index
                        }
                    };
                    blocks[(x * 16 + z) * 16 + y] = index;
                }
            }
        }
        for state in seen {
            local[state as usize] = u16::MAX;
        }
        let base_y = chunk.min_y + 16 * i as i32;
        if has_entities {
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        let state = java.states[(y * 16 + z) * 16 + x];
                        let kind = t.kind(state);
                        if kind == Kind::None {
                            continue;
                        }
                        let (wx, wy, wz) = (chunk.x * 16 + x as i32, base_y + y as i32, chunk.z * 16 + z as i32);
                        let java_nbt = java_entities.get(&(wx, wy, wz)).copied();
                        if let Some(nbt) = t.block_entity(kind, wx, wy, wz, java_nbt) {
                            block_entities.push(BlockEntity { x: wx, y: wy, z: wz, nbt });
                        }
                    }
                }
            }
        }
        if palette.iter().all(|&h| h == t.air) {
            continue;
        }
        let mut biomes = Box::new([0u32; 64]);
        for y in 0..4 {
            for z in 0..4 {
                for x in 0..4 {
                    biomes[(x * 4 + z) * 4 + y] = t.biome(java.biomes[(y * 4 + z) * 4 + x]);
                }
            }
        }
        sections.push(Section { index: (first_section + i as i32) as i8, palette, blocks, biomes });
    }
    ChunkData { dim: chunk.dim, x: chunk.x, z: chunk.z, sections, block_entities }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bedrock_hash::block_hash;
    use crate::nbt::{Tag, read_disk, read_network};
    use crate::steel::{JavaChunk, JavaSection};
    use steel_registry::REGISTRY;
    use steel_registry::RegistryExt;
    use steel_utils::Identifier;

    fn java_id(name: &str) -> u16 {
        steel_core::bootstrap::init_globals();
        let key: Identifier = name.parse().unwrap();
        let block = REGISTRY.blocks.by_key(&key).expect("blocco sconosciuto");
        REGISTRY.blocks.get_default_state_id(block).0
    }

    fn java_biome(name: &str) -> u16 {
        steel_core::bootstrap::init_globals();
        let key: Identifier = name.parse().unwrap();
        REGISTRY.biomes.id_from_key(&key).expect("bioma sconosciuto") as u16
    }

    fn empty_chunk() -> JavaChunk {
        JavaChunk {
            dim: vgen::Dim::Overworld,
            x: 0,
            z: 0,
            min_y: -64,
            sections: (0..24).map(|_| JavaSection { states: Box::new([0; 4096]), biomes: Box::new([0; 64]) }).collect(),
            block_entities: Vec::new(),
        }
    }

    #[test]
    fn stone_maps_to_stone() {
        let t = Translator::new();
        assert_eq!(t.block(java_id("minecraft:stone")), block_hash("minecraft:stone", &[]));
    }

    #[test]
    fn air_maps_to_air() {
        assert_eq!(Translator::new().block(0), -604_749_536);
    }

    #[test]
    fn every_java_state_has_a_translation_or_counts_as_unmapped() {
        let t = Translator::new();
        let total = REGISTRY.blocks.state_to_block_lookup.len();
        let before = t.unmapped();
        for id in 0..total {
            t.block(id as u16);
        }
        let unmapped = t.unmapped() - before;
        assert!(unmapped * 100 < total as u64, "{unmapped} stati non mappati su {total}");
    }

    #[test]
    fn plains_and_dappled_forest_map_to_bedrock_ids() {
        let t = Translator::new();
        assert_eq!(t.biome(java_biome("minecraft:plains")), 1);
        assert_eq!(t.biome(java_biome("minecraft:dappled_forest")), 195);
    }

    #[test]
    fn red_bed_and_chest_get_bedrock_block_entities() {
        let t = Translator::new();
        let mut chunk = empty_chunk();
        // Sezione 4 = y da 0 a 15; indice Java (y*16 + z)*16 + x.
        chunk.sections[4].states[(16 + 1) * 16 + 1] = java_id("minecraft:red_bed");
        chunk.sections[4].states[(16 + 3) * 16 + 3] = java_id("minecraft:chest");
        let data = to_bedrock(&chunk, &t);
        let find = |x, y, z| {
            let e = data.block_entities.iter().find(|e| (e.x, e.y, e.z) == (x, y, z)).expect("block entity mancante");
            read_network(&e.nbt).expect("NBT di rete valido")
        };
        let bed = find(1, 1, 1);
        assert_eq!(bed.get("id"), Some(&Tag::Str("Bed".into())));
        assert_eq!(bed.get("color"), Some(&Tag::Byte(14)));
        assert_eq!(bed.get("x"), Some(&Tag::Int(1)));
        assert_eq!(bed.get("y"), Some(&Tag::Int(1)));
        assert_eq!(bed.get("z"), Some(&Tag::Int(1)));
        let chest = find(3, 1, 3);
        assert_eq!(chest.get("id"), Some(&Tag::Str("Chest".into())));
        assert_eq!(data.block_entities.len(), 2);
    }

    #[test]
    fn unknown_state_falls_back_and_is_counted() {
        let t = Translator::new();
        let before = t.unmapped();
        t.block(u16::MAX);
        assert_eq!(t.unmapped(), before + 1);
    }

    #[test]
    fn double_chest_gets_pair_coordinates() {
        let t = Translator::new();
        steel_core::bootstrap::init_globals();
        let key: Identifier = "minecraft:chest".parse().unwrap();
        let left = REGISTRY.blocks.state_id_from_properties(&key, &[("facing", "north"), ("type", "left")]).unwrap().0;
        let right = REGISTRY.blocks.state_id_from_properties(&key, &[("facing", "north"), ("type", "right")]).unwrap().0;
        let mut chunk = empty_chunk();
        chunk.sections[4].states[5 * 16 + 4] = left;
        chunk.sections[4].states[5 * 16 + 5] = right;
        let data = to_bedrock(&chunk, &t);
        let left = data.block_entities.iter().find(|e| e.x == 4).map(|e| read_network(&e.nbt).unwrap()).unwrap();
        assert_eq!(left.get("pairx"), Some(&Tag::Int(5)));
        assert_eq!(left.get("pairz"), Some(&Tag::Int(5)));
    }

    #[test]
    fn sections_use_bedrock_order_and_skip_air() {
        let t = Translator::new();
        let mut chunk = empty_chunk();
        let stone = java_id("minecraft:stone");
        // x=1, y=2, z=3 nella sezione 5 (y da 16 a 31).
        chunk.sections[5].states[(2 * 16 + 3) * 16 + 1] = stone;
        let plains = java_biome("minecraft:plains");
        chunk.sections[5].biomes[(2 * 4 + 1) * 4 + 3] = plains; // x=3, y=2, z=1
        let data = to_bedrock(&chunk, &t);
        assert_eq!(data.sections.len(), 1, "le sezioni di sola aria vanno omesse");
        let s = &data.sections[0];
        assert_eq!(s.index, 1);
        let stone_hash = block_hash("minecraft:stone", &[]);
        let i = s.palette.iter().position(|&h| h == stone_hash).unwrap() as u16;
        assert_eq!(s.blocks[(16 + 3) * 16 + 2], i);
        assert_eq!(s.blocks.iter().filter(|&&b| b == i).count(), 1);
        assert_eq!(s.biomes[(3 * 4 + 1) * 4 + 2], 1);
    }

    #[test]
    fn geyser_order_matches_steel_state_ids() {
        steel_core::bootstrap::init_globals();
        assert_eq!(GEYSER_BLOCKS.len(), REGISTRY.blocks.state_to_block_lookup.len());
        let t = Translator::new();
        // Nomi diversi tra Java e Bedrock, con stati, all'inizio, a metà e in fondo al registro.
        let key: Identifier = "minecraft:oak_log".parse().unwrap();
        let log = REGISTRY.blocks.state_id_from_properties(&key, &[("axis", "x")]).unwrap().0;
        assert_eq!(t.block(log), block_hash("minecraft:oak_log", &[("pillar_axis", State::Str("x"))]));
        let red_bed = t.block(java_id("minecraft:red_bed"));
        let bed = t.bedrock.iter().find(|b| b.hash == red_bed).unwrap();
        assert_eq!(bed.name, "minecraft:bed");
        assert_eq!(t.block(java_id("minecraft:flower_pot")), block_hash("minecraft:flower_pot", &[("update_bit", State::Byte(0))]));
        assert!(t.bedrock.iter().all(|b| b.name.starts_with("minecraft:")), "nomi Bedrock senza spazio dei nomi");
    }

    #[test]
    fn generated_chunk_translates_without_fallbacks() {
        let t = Translator::new();
        // In debug la generazione chiede più stack del thread di test predefinito.
        let chunk = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| crate::steel::Generator::new(13579, vgen::Dim::Overworld).generate(0, 0))
            .unwrap()
            .join()
            .unwrap();
        let data = to_bedrock(&chunk, &t);
        assert_eq!((data.x, data.z), (0, 0));
        assert!(data.sections.len() >= 5, "{} sezioni", data.sections.len());
        assert!(data.sections.iter().all(|s| (-4..20).contains(&s.index)));
        assert_eq!(t.unmapped(), 0);
        let known: std::collections::HashSet<i32> = t.palette_entries().into_iter().map(|(h, _)| h).collect();
        assert!(data.sections.iter().flat_map(|s| &s.palette).all(|h| known.contains(h)));
    }

    #[test]
    fn palette_entries_contain_air() {
        let t = Translator::new();
        let entries = t.palette_entries();
        let (_, nbt) = entries.iter().find(|(h, _)| *h == -604_749_536).expect("aria nella palette");
        let tag = read_disk(nbt).expect("NBT su disco valido");
        assert_eq!(tag.get("name"), Some(&Tag::Str("minecraft:air".into())));
    }
}
