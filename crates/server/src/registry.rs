//! Registro di blocchi, oggetti e menu creativo, costruito dai dati committati in `data/` (vedi `tools/extract_blocks.py` e
//! `tools/extract_creative.py`; ID degli oggetti da `items.txt`). Gli stati dei blocchi e il menu creativo vengono da Dragonfly (MIT, 1.26.60).
#![allow(dead_code)] // i consumatori (piazzamento, creativo) arrivano nei task seguenti
use crate::world::{block_hash, block_nbt, State};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

/// Valore di uno stato di blocco, con il tipo di tag NBT che ha su disco (byte, int, stringa).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StateValue {
    Byte(u8),
    Int(i32),
    Str(String),
}

impl StateValue {
    fn as_state(&self) -> State<'_> {
        match self {
            StateValue::Byte(v) => State::Byte(*v),
            StateValue::Int(v) => State::Int(*v),
            StateValue::Str(v) => State::Str(v),
        }
    }
}

/// Uno stato di blocco: nome e stati in ordine alfabetico di chiave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlockState {
    pub name: String,
    pub states: Vec<(String, StateValue)>,
}

impl BlockState {
    fn borrowed(&self) -> Vec<(&str, State<'_>)> {
        self.states.iter().map(|(k, v)| (k.as_str(), v.as_state())).collect()
    }

    /// ID di rete dello stato (uguale a `world::block_hash`).
    pub fn hash(&self) -> i32 {
        block_hash(&self.name, &self.borrowed())
    }

    pub fn get(&self, key: &str) -> Option<&StateValue> {
        self.states.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Copia con `key` impostata a `value` (aggiunta se manca, tenendo l'ordine alfabetico). Non controlla che lo stato esista: vedi `Blocks::is_valid`.
    pub fn with(&self, key: &str, value: StateValue) -> BlockState {
        let mut next = self.clone();
        match next.states.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => {
                next.states.push((key.to_string(), value));
                next.states.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        next
    }

    /// NBT su disco little-endian `{name, states}`.
    pub fn nbt(&self) -> Vec<u8> {
        block_nbt(&self.name, &self.borrowed())
    }
}

/// Tutti gli stati dei blocchi di `blocks.json`.
pub(crate) struct Blocks {
    states: HashMap<i32, BlockState>,
    defaults: HashMap<String, i32>,
    /// Per blocco: chiavi (alfabetiche) con i loro valori ammessi.
    domains: HashMap<String, Vec<(String, Vec<StateValue>)>>,
}

impl Blocks {
    /// Nomi di tutti i blocchi del registro.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.defaults.keys().map(String::as_str)
    }

    pub fn state(&self, hash: i32) -> Option<&BlockState> {
        self.states.get(&hash)
    }

    /// Hash dello stato di default del blocco `name`.
    pub fn default_hash(&self, name: &str) -> Option<i32> {
        self.defaults.get(name).copied()
    }

    /// Valori ammessi per la chiave `key` del blocco `name`.
    pub fn values(&self, name: &str, key: &str) -> Option<&[StateValue]> {
        self.domains.get(name)?.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_slice())
    }

    /// Vero se lo stato esiste esattamente così (blocco, chiavi, tipi e valori).
    pub fn is_valid(&self, s: &BlockState) -> bool {
        self.state(s.hash()) == Some(s)
    }
}

#[derive(Deserialize)]
struct BlocksFile {
    blocks: Vec<BlockDef>,
}

#[derive(Deserialize)]
struct BlockDef {
    name: String,
    states: Vec<StateDef>,
    default: i32,
}

#[derive(Deserialize)]
struct StateDef {
    key: String,
    #[serde(rename = "type")]
    kind: String,
    values: Vec<serde_json::Value>,
}

pub(crate) fn state_value(kind: &str, v: &serde_json::Value) -> StateValue {
    match kind {
        "byte" => StateValue::Byte(v.as_u64().and_then(|n| u8::try_from(n).ok()).expect("valore byte non valido")),
        "int" => StateValue::Int(v.as_i64().and_then(|n| i32::try_from(n).ok()).expect("valore int non valido")),
        "string" => StateValue::Str(v.as_str().expect("valore stringa non valido").to_string()),
        other => panic!("tipo di stato {other} sconosciuto"),
    }
}

fn load_blocks() -> Blocks {
    let file: BlocksFile = serde_json::from_str(include_str!("../data/blocks.json")).expect("blocks.json non valido");
    let mut blocks = Blocks { states: HashMap::new(), defaults: HashMap::new(), domains: HashMap::new() };
    for def in file.blocks {
        let domain: Vec<(String, Vec<StateValue>)> =
            def.states.iter().map(|s| (s.key.clone(), s.values.iter().map(|v| state_value(&s.kind, v)).collect())).collect();
        // prodotto cartesiano dei valori di tutte le chiavi
        let mut combos: Vec<Vec<(String, StateValue)>> = vec![Vec::new()];
        for (key, values) in &domain {
            combos = combos.iter().flat_map(|c| values.iter().map(move |v| c.iter().cloned().chain([(key.clone(), v.clone())]).collect())).collect();
        }
        for states in combos {
            let state = BlockState { name: def.name.clone(), states };
            blocks.states.insert(state.hash(), state);
        }
        assert!(blocks.states.contains_key(&def.default), "{}: lo stato di default manca", def.name);
        blocks.defaults.insert(def.name.clone(), def.default);
        blocks.domains.insert(def.name, domain);
    }
    blocks
}

pub(crate) fn blocks() -> &'static Blocks {
    static BLOCKS: OnceLock<Blocks> = OnceLock::new();
    BLOCKS.get_or_init(load_blocks)
}

/// Nomi e ID numerici degli oggetti (`items.txt`).
pub(crate) struct Items {
    ids: HashMap<String, i16>,
    names: HashMap<i16, String>,
}

impl Items {
    pub fn id(&self, name: &str) -> Option<i16> {
        self.ids.get(name).copied()
    }

    pub fn name(&self, id: i16) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// Hash dello stato di default del blocco con lo stesso nome dell'oggetto; `None` per i non-blocchi.
    pub fn block_of(&self, name: &str) -> Option<i32> {
        blocks().default_hash(name)
    }
}

fn load_items() -> Items {
    let mut items = Items { ids: HashMap::new(), names: HashMap::new() };
    for line in include_str!("../data/items.txt").lines().filter(|l| !l.is_empty()) {
        let (name, id) = line.split_once('\t').expect("riga di items.txt senza tabulazione");
        let id: i16 = id.parse().expect("ID di items.txt non valido");
        items.ids.insert(name.to_string(), id);
        items.names.insert(id, name.to_string());
    }
    items
}

pub(crate) fn items() -> &'static Items {
    static ITEMS: OnceLock<Items> = OnceLock::new();
    ITEMS.get_or_init(load_items)
}

/// Una voce del menu creativo.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CreativeItem {
    pub name: String,
    pub meta: u32,
    /// Hash dello stato del blocco che l'oggetto piazza, se la voce porta uno stato.
    pub block: Option<i32>,
    /// NBT su disco dell'oggetto (incantesimi, fuochi d'artificio...).
    pub nbt: Option<Vec<u8>>,
    /// Indice del gruppo in `creative().0`.
    pub group: u32,
}

/// Un gruppo del menu creativo; `icon` manca per i gruppi anonimi.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CreativeGroup {
    /// 1 costruzione, 2 natura, 3 equipaggiamento, 4 oggetti.
    pub category: u8,
    pub name: String,
    pub icon: Option<CreativeItem>,
}

#[derive(Deserialize)]
struct CreativeFile {
    groups: Vec<GroupDef>,
    items: Vec<ItemDef>,
}

#[derive(Deserialize)]
struct GroupDef {
    category: u8,
    name: String,
    icon: Option<ItemDef>,
}

#[derive(Deserialize)]
struct ItemDef {
    name: String,
    meta: u32,
    block_states: Option<Vec<CreativeState>>,
    nbt_b64: Option<String>,
    #[serde(default)]
    group: u32,
}

#[derive(Deserialize)]
struct CreativeState {
    key: String,
    #[serde(rename = "type")]
    kind: String,
    value: serde_json::Value,
}

impl ItemDef {
    fn build(self) -> CreativeItem {
        let block = self.block_states.map(|list| {
            let states = list.iter().map(|s| (s.key.clone(), state_value(&s.kind, &s.value))).collect();
            let hash = BlockState { name: self.name.clone(), states }.hash();
            assert!(blocks().state(hash).is_some(), "{}: stato di blocco sconosciuto", self.name);
            hash
        });
        let nbt = self.nbt_b64.map(|b| STANDARD.decode(b).expect("nbt_b64 non valido"));
        CreativeItem { name: self.name, meta: self.meta, block, nbt, group: self.group }
    }
}

fn load_creative() -> (Vec<CreativeGroup>, Vec<CreativeItem>) {
    let file: CreativeFile = serde_json::from_str(include_str!("../data/creative.json")).expect("creative.json non valido");
    let groups = file.groups.into_iter().map(|g| CreativeGroup { category: g.category, name: g.name, icon: g.icon.map(ItemDef::build) }).collect();
    (groups, file.items.into_iter().filter(|i| !hidden_from_creative(&i.name)).map(ItemDef::build).collect())
}

/// Oggetti che Dragonfly mette nel menu creativo ma che il client 1.26.6x mostra come oggetti sconosciuti (nome in
/// codice, niente texture): scale e lastre di lana, provate nel gioco il 18/11/2026. Si tolgono dal menu.
fn hidden_from_creative(name: &str) -> bool {
    name.ends_with("_wool_stairs") || name.ends_with("_wool_slab")
}

pub(crate) fn creative() -> &'static (Vec<CreativeGroup>, Vec<CreativeItem>) {
    static CREATIVE: OnceLock<(Vec<CreativeGroup>, Vec<CreativeItem>)> = OnceLock::new();
    CREATIVE.get_or_init(load_creative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wool_stairs_and_slabs_are_not_in_the_creative_menu() {
        let (_, items) = creative();
        assert!(items.iter().all(|i| !hidden_from_creative(&i.name)));
        assert!(items.iter().any(|i| i.name == "minecraft:white_wool"), "la lana normale resta");
    }

    #[test]
    fn air_and_stone_hashes() {
        for (name, expected) in [("minecraft:air", Some(-604_749_536)), ("minecraft:stone", Some(-2_144_268_767))] {
            let h = blocks().default_hash(name);
            assert_eq!(h, expected, "{name}");
            assert_eq!(blocks().state(h.unwrap()).unwrap().hash(), h.unwrap());
        }
    }

    #[test]
    fn stairs_have_corner_state() {
        let values = blocks().values("minecraft:oak_stairs", "minecraft:corner").expect("chiave dello stato");
        assert!(values.contains(&StateValue::Str("outer_left".into())));
    }

    #[test]
    fn every_creative_block_item_places_a_known_state() {
        let (_, items) = creative();
        let with_block: Vec<_> = items.iter().filter_map(|i| i.block.map(|h| (i, h))).collect();
        assert!(with_block.len() > 900, "{} voci con blocco", with_block.len());
        for (item, hash) in with_block {
            assert!(blocks().state(hash).is_some(), "{}: stato {hash} sconosciuto", item.name);
        }
    }

    #[test]
    fn creative_items_all_have_ids() {
        let (groups, items) = creative();
        assert!(items.len() > 1900 && groups.len() > 100);
        for item in items {
            assert!(self::items().id(&item.name).is_some(), "{} senza ID", item.name);
            assert!((item.group as usize) < groups.len());
        }
    }

    #[test]
    fn block_state_helpers() {
        let stairs = blocks().state(blocks().default_hash("minecraft:oak_stairs").unwrap()).unwrap();
        assert_eq!(stairs.get("weirdo_direction"), Some(&StateValue::Int(0)));
        assert_eq!(stairs.get("nessuna"), None);
        let turned = stairs.with("weirdo_direction", StateValue::Int(3)).with("upside_down_bit", StateValue::Byte(1));
        assert_eq!(turned.get("weirdo_direction"), Some(&StateValue::Int(3)));
        assert_eq!(stairs.get("weirdo_direction"), Some(&StateValue::Int(0)), "with non modifica l'originale");
        assert!(blocks().is_valid(&turned));
        assert_ne!(turned.hash(), stairs.hash());
        assert_eq!(blocks().state(turned.hash()), Some(&turned));
        assert!(!blocks().is_valid(&stairs.with("weirdo_direction", StateValue::Int(9))));
        assert!(!blocks().is_valid(&stairs.with("inventata", StateValue::Byte(0))));
        // l'NBT su disco è quello che dà l'hash di rete
        assert_eq!(crate::storage::block_hash_from_nbt(&turned.nbt()), Some(turned.hash()));
    }

    #[test]
    fn items_map_names_to_ids_and_blocks() {
        let items = items();
        let id = items.id("minecraft:stone").expect("pietra");
        assert_eq!(items.name(id), Some("minecraft:stone"));
        assert_eq!(items.block_of("minecraft:stone"), blocks().default_hash("minecraft:stone"));
        assert_eq!(items.block_of("minecraft:stick"), None, "non è un blocco");
        assert_eq!(items.id("minecraft:non_esiste"), None);
        assert_eq!(items.name(i16::MAX), None);
    }

    #[test]
    fn creative_entries_keep_states_and_nbt() {
        let (groups, items) = creative();
        let walls = items.iter().find(|i| i.name == "minecraft:cobblestone_wall").unwrap();
        assert_eq!(walls.block, blocks().default_hash("minecraft:cobblestone_wall"));
        let book = items.iter().find(|i| i.name == "minecraft:enchanted_book").unwrap();
        assert!(book.nbt.as_ref().is_some_and(|n| n[0] == 0x0a), "NBT su disco con radice compound");
        assert!(groups.iter().any(|g| g.icon.is_none()), "i gruppi anonimi non hanno icona");
        assert!(groups[0].icon.is_some() && (1..=4).contains(&groups[0].category));
    }
}
