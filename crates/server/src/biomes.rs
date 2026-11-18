//! Definizione dei biomi (`BiomeDefinitionList`), costruita da `data/biomes.json` (vedi `tools/extract_biomes.py`).
use bedrock_protocol::wire::ServerPacket;
use bedrock_protocol::{
    generated::{BiomeDefinitionData, BiomeDefinitionList, BiomeStringList, BiomeTagsData},
    Packet,
};
use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

#[derive(Deserialize)]
struct File {
    biomes: Vec<Biome>,
}

#[derive(Deserialize)]
struct Biome {
    name: String,
    id: u16,
    temperature: f32,
    downfall: f32,
    foliage_snow: f32,
    depth: f32,
    scale: f32,
    /// Colore dell'acqua sulla mappa, ARGB.
    water_argb: u32,
    rain: bool,
    tags: Vec<String>,
}

/// Tutti i biomi di `biomes.json`. La lista di stringhe contiene prima i nomi dei biomi (la chiave della mappa è l'indice del
/// nome) e poi i tag, senza ripetizioni; i tag di un bioma sono indici in quella lista.
pub(crate) fn definition_list() -> BiomeDefinitionList {
    let file: File = serde_json::from_str(include_str!("../data/biomes.json")).expect("biomes.json non valido");
    let mut strings: Vec<String> = file.biomes.iter().map(|b| b.name.clone()).collect();
    let mut tag_index: HashMap<String, u16> = HashMap::new();
    let mut map = Vec::with_capacity(file.biomes.len());
    for (key, b) in file.biomes.iter().enumerate() {
        let tags = b
            .tags
            .iter()
            .map(|t| {
                *tag_index.entry(t.clone()).or_insert_with(|| {
                    strings.push(t.clone());
                    (strings.len() - 1) as u16
                })
            })
            .collect();
        let data = BiomeDefinitionData {
            id: b.id,
            temperature: b.temperature,
            downfall: b.downfall,
            foliage_snow: b.foliage_snow,
            depth: b.depth,
            scale: b.scale,
            map_water_color_argb: b.water_argb as i32,
            rain: b.rain,
            tags: Some(BiomeTagsData { tags }),
            chunk_gen_data: None,
        };
        map.push((key as u16, data));
    }
    BiomeDefinitionList { map_of_biome_names_to_data: map, string_list: BiomeStringList { strings } }
}

/// Il pacchetto già codificato: è uguale per tutti i giocatori.
pub(crate) fn packet() -> &'static Packet {
    static PACKET: OnceLock<Packet> = OnceLock::new();
    PACKET.get_or_init(|| definition_list().packet())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Ogni bioma che il generatore può produrre (tabella di Geyser: ID Bedrock) ha una definizione.
    #[test]
    fn biomes_json_covers_every_geyser_biome() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../generator/data/geyser/biomes.json");
        let geyser: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("tabella di Geyser")).unwrap();
        let defined: HashSet<u16> = definition_list().map_of_biome_names_to_data.iter().map(|(_, d)| d.id).collect();
        let geyser = geyser.as_object().unwrap();
        assert!(geyser.len() > 60);
        for (name, entry) in geyser {
            let id = entry["bedrock_id"].as_u64().unwrap() as u16;
            assert!(defined.contains(&id), "il bioma {name} (ID {id}) manca in biomes.json");
        }
        assert!(defined.contains(&195), "dappled_forest");
    }

    #[test]
    fn definition_list_is_consistent() {
        let list = definition_list();
        let strings = &list.string_list.strings;
        let mut ids = HashSet::new();
        for (key, d) in &list.map_of_biome_names_to_data {
            let name = &strings[*key as usize];
            assert!(name.starts_with("minecraft:"), "{name}");
            assert!(ids.insert(d.id), "ID {} duplicato", d.id);
            for t in &d.tags.as_ref().expect("tag").tags {
                assert!(!strings[*t as usize].is_empty());
            }
        }
        let plains = list.map_of_biome_names_to_data.iter().find(|(k, _)| strings[*k as usize] == "minecraft:plains").unwrap();
        assert_eq!((plains.1.id, plains.1.temperature, plains.1.downfall), (crate::world::PLAINS, 0.8, 0.4));
        assert!(plains.1.rain);
        let tag_names: Vec<&str> = plains.1.tags.as_ref().unwrap().tags.iter().map(|t| strings[*t as usize].as_str()).collect();
        assert!(tag_names.contains(&"overworld") && tag_names.contains(&"plains"), "{tag_names:?}");
        assert!(packet().body.len() > 1000);
    }
}
