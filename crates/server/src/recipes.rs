//! Ricette di crafting (banco da lavoro e griglia 2x2), caricate da `data/recipes.json` (estratto da Dragonfly, MIT, con
//! `tools/extract_recipes.py`) e il pacchetto `CraftingData` che le manda al client.
//!
//! ID di rete: come Dragonfly (`session/player.go`, `sendRecipes`) sono l'indice + 1 nell'ordine di registrazione, mai 0. Qui
//! l'ordine è quello del file: prima le ricette `shapeless`, poi le `shaped`, contando solo quelle tenute (le scartate dallo
//! script non consumano ID). Il client le conosce solo dal pacchetto di ingresso e le rimanda nelle azioni `CraftRecipe` e
//! `CraftRecipeAuto` (`RecipeNetworkID`): `Recipes::by_network_id` le ritrova. Gli ID non si salvano su disco.
//!
//! Corrispondenza di un oggetto con un ingrediente (`Ingredient::matches`): per nome e variante, dove la variante 32767
//! accetta qualunque valore ausiliario; un tag accetta gli oggetti elencati in `Recipes::tag_contains`.
#![allow(dead_code)] // i consumatori (azioni di crafting) arrivano nel compito seguente
use crate::registry::{self, state_value, BlockState};
use bedrock_protocol::crafting::{CraftingData, Descriptor, Ingredient as WireIngredient, RecipeOutput, ShapedRecipe, ShapelessRecipe};
use bedrock_protocol::{wire::ServerPacket, Packet};
use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

/// Variante che accetta qualunque valore ausiliario.
pub(crate) const ANY_META: i32 = 32767;

/// Un ingrediente di una casella della ricetta.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ingredient {
    /// Casella che deve restare vuota.
    Empty,
    /// Un oggetto per nome (`meta` = `ANY_META` per qualunque variante).
    Item { name: String, meta: i32, count: u32 },
    /// Qualunque oggetto di un tag (`minecraft:planks`...).
    Tag { tag: String, count: u32 },
}

impl Ingredient {
    /// Quantità richiesta (0 per la casella vuota).
    pub fn count(&self) -> u32 {
        match self {
            Ingredient::Empty => 0,
            Ingredient::Item { count, .. } | Ingredient::Tag { count, .. } => *count,
        }
    }
}

/// Il risultato di una ricetta.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Output {
    pub name: String,
    /// ID di rete dell'oggetto (`items.txt`).
    pub item_id: i16,
    /// Valore ausiliario (variante).
    pub meta: u32,
    pub count: u16,
    /// Hash dello stato del blocco se il risultato è un blocco con stato esatto.
    pub block: Option<i32>,
}

/// Forma della ricetta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// `width x height` caselle, per righe, senza ruotare.
    Shaped { width: u32, height: u32 },
    /// Ingredienti in qualunque posizione.
    Shapeless,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Recipe {
    /// ID di rete (1..), vedi la documentazione del modulo.
    pub id: u32,
    pub shape: Shape,
    pub priority: i32,
    /// Blocco che serve (sempre `crafting_table`: anche la griglia 2x2 dell'inventario vale per le ricette che ci stanno).
    pub block: String,
    /// Una voce per casella (shaped) o per ingrediente (shapeless).
    pub input: Vec<Ingredient>,
    pub output: Vec<Output>,
}

impl Recipe {
    /// Vero se la ricetta non entra nella griglia 2x2 dell'inventario e serve il banco da lavoro.
    pub fn needs_table(&self) -> bool {
        match self.shape {
            Shape::Shaped { width, height } => width > 2 || height > 2,
            Shape::Shapeless => self.input.iter().filter(|i| **i != Ingredient::Empty).count() > 4,
        }
    }
}

#[derive(Deserialize)]
struct File {
    tags: HashMap<String, Vec<String>>,
    recipes: Vec<RawRecipe>,
}

#[derive(Deserialize)]
struct RawRecipe {
    id: u32,
    kind: String,
    priority: i32,
    block: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    input: Vec<Option<RawIngredient>>,
    output: Vec<RawOutput>,
}

#[derive(Deserialize)]
struct RawIngredient {
    name: Option<String>,
    #[serde(default)]
    meta: i32,
    tag: Option<String>,
    count: u32,
}

#[derive(Deserialize)]
struct RawOutput {
    name: String,
    meta: u32,
    count: u16,
    block_states: Option<Vec<RawState>>,
}

#[derive(Deserialize)]
struct RawState {
    key: String,
    #[serde(rename = "type")]
    kind: String,
    value: serde_json::Value,
}

/// Tutte le ricette, per ID di rete e per tag.
pub(crate) struct Recipes {
    list: Vec<Recipe>,
    tags: HashMap<String, Vec<String>>,
}

impl Recipes {
    /// Tutte le ricette, nell'ordine degli ID (la ricetta con ID `n` sta all'indice `n - 1`).
    pub fn all(&self) -> &[Recipe] {
        &self.list
    }

    /// La ricetta con quell'ID di rete (0 e gli ID sconosciuti non esistono).
    pub fn by_network_id(&self, id: u32) -> Option<&Recipe> {
        self.list.get((id as usize).checked_sub(1)?)
    }

    /// Vero se l'oggetto `name` appartiene al tag.
    pub fn tag_contains(&self, tag: &str, name: &str) -> bool {
        self.tags.get(tag).is_some_and(|names| names.iter().any(|n| n == name))
    }

    /// Vero se un oggetto con quel nome e valore ausiliario soddisfa l'ingrediente (la quantità non si controlla qui).
    pub fn matches(&self, ingredient: &Ingredient, name: &str, meta: i32) -> bool {
        match ingredient {
            Ingredient::Empty => false,
            Ingredient::Item { name: n, meta: m, .. } => n == name && (*m == ANY_META || *m == meta),
            Ingredient::Tag { tag, .. } => self.tag_contains(tag, name),
        }
    }
}

fn load() -> Recipes {
    let file: File = serde_json::from_str(include_str!("../data/recipes.json")).expect("recipes.json non valido");
    let list: Vec<Recipe> = file
        .recipes
        .into_iter()
        .enumerate()
        .map(|(index, r)| {
            assert_eq!(r.id as usize, index + 1, "ID di rete non consecutivi in recipes.json");
            let shape = match r.kind.as_str() {
                "shaped" => Shape::Shaped { width: r.width, height: r.height },
                "shapeless" => Shape::Shapeless,
                other => panic!("tipo di ricetta {other} sconosciuto"),
            };
            let input = r
                .input
                .into_iter()
                .map(|i| match i {
                    None => Ingredient::Empty,
                    Some(RawIngredient { tag: Some(tag), count, .. }) => Ingredient::Tag { tag, count },
                    Some(RawIngredient { name: Some(name), meta, count, .. }) => Ingredient::Item { name, meta, count },
                    Some(_) => panic!("ingrediente senza nome né tag"),
                })
                .collect();
            let output = r
                .output
                .into_iter()
                .map(|o| {
                    let block = o.block_states.map(|list| {
                        let states = list.iter().map(|s| (s.key.clone(), state_value(&s.kind, &s.value))).collect();
                        BlockState { name: o.name.clone(), states }.hash()
                    });
                    let item_id = registry::items().id(&o.name).unwrap_or_else(|| panic!("{}: oggetto senza ID", o.name));
                    Output { name: o.name, item_id, meta: o.meta, count: o.count, block }
                })
                .collect();
            Recipe { id: r.id, shape, priority: r.priority, block: r.block, input, output }
        })
        .collect();
    Recipes { list, tags: file.tags }
}

pub(crate) fn recipes() -> &'static Recipes {
    static RECIPES: OnceLock<Recipes> = OnceLock::new();
    RECIPES.get_or_init(load)
}

fn wire_input(i: &Ingredient) -> WireIngredient {
    match i {
        Ingredient::Empty => WireIngredient::empty(),
        Ingredient::Item { name, meta, count } => WireIngredient { descriptor: Descriptor::Item { name: name.clone(), meta: *meta }, count: *count as i32 },
        Ingredient::Tag { tag, count } => WireIngredient { descriptor: Descriptor::Tag(tag.clone()), count: *count as i32 },
    }
}

fn wire_output(o: &Output) -> RecipeOutput {
    RecipeOutput { id: i32::from(o.item_id), count: o.count, aux: o.meta, block_runtime_id: o.block.unwrap_or(0) }
}

/// Il pacchetto `CraftingData` con tutte le ricette (uguale per tutti): come Dragonfly lo manda subito dopo il menu creativo,
/// con `ClearRecipes` vero. L'ID di ricetta (testo) è `versmcbe:<ID di rete>`, unico.
pub(crate) fn crafting_data_packet() -> &'static Packet {
    static PACKET: OnceLock<Packet> = OnceLock::new();
    PACKET.get_or_init(|| {
        let mut data = CraftingData { clear_recipes: true, ..Default::default() };
        for r in recipes().all() {
            let recipe_id = format!("versmcbe:{}", r.id);
            let input: Vec<_> = r.input.iter().map(wire_input).collect();
            let output: Vec<_> = r.output.iter().map(wire_output).collect();
            match r.shape {
                Shape::Shaped { width, height } => data.shaped.push(ShapedRecipe {
                    recipe_id,
                    width: width as i32,
                    height: height as i32,
                    input,
                    output,
                    block: r.block.clone(),
                    priority: r.priority,
                    network_id: r.id,
                }),
                Shape::Shapeless => {
                    data.shapeless.push(ShapelessRecipe { recipe_id, input, output, block: r.block.clone(), priority: r.priority, network_id: r.id })
                }
            }
        }
        data.packet()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(name: &str, count: u16) -> &'static Recipe {
        recipes()
            .all()
            .iter()
            .find(|r| r.output.len() == 1 && r.output[0].name == name && r.output[0].count == count)
            .unwrap_or_else(|| panic!("{name} x{count}"))
    }

    #[test]
    fn ids_are_consecutive_from_one_and_lookup_works() {
        let all = recipes().all();
        assert!(all.len() > 2000, "{} ricette", all.len());
        for (i, r) in all.iter().enumerate() {
            assert_eq!(r.id as usize, i + 1);
            assert_eq!(recipes().by_network_id(r.id), Some(r));
        }
        assert_eq!(recipes().by_network_id(0), None);
        assert_eq!(recipes().by_network_id(all.len() as u32 + 1), None);
        assert_eq!(recipes().by_network_id(u32::MAX), None);
    }

    #[test]
    fn every_recipe_is_well_formed() {
        for r in recipes().all() {
            assert_eq!(r.block, "crafting_table");
            assert!(!r.output.is_empty());
            match r.shape {
                Shape::Shaped { width, height } => assert_eq!(r.input.len() as u32, width * height, "ricetta {}", r.id),
                Shape::Shapeless => assert!(r.input.iter().any(|i| *i != Ingredient::Empty), "ricetta {}", r.id),
            }
            for o in &r.output {
                assert_eq!(registry::items().id(&o.name), Some(o.item_id));
            }
            for i in &r.input {
                if let Ingredient::Tag { tag, .. } = i {
                    assert!(recipes().tags.contains_key(tag), "tag {tag} senza elenco");
                }
            }
        }
    }

    #[test]
    fn sticks_are_two_planks_in_a_column_and_fit_the_2x2_grid() {
        let r = find("minecraft:stick", 4);
        assert_eq!(r.shape, Shape::Shaped { width: 1, height: 2 });
        assert!(!r.needs_table());
        assert!(r
            .input
            .iter()
            .all(|i| matches!(i, Ingredient::Tag { tag, count: 1 } if tag == "minecraft:planks") || matches!(i, Ingredient::Item { count: 1, .. })));
    }

    #[test]
    fn crafting_table_needs_four_planks_and_a_wooden_pickaxe_needs_the_table() {
        let t = find("minecraft:crafting_table", 1);
        assert_eq!(t.shape, Shape::Shaped { width: 2, height: 2 });
        assert!(!t.needs_table());
        let p = find("minecraft:wooden_pickaxe", 1);
        assert_eq!(p.shape, Shape::Shaped { width: 3, height: 3 });
        assert!(p.needs_table());
    }

    #[test]
    fn tags_and_variants_match() {
        let planks = Ingredient::Tag { tag: "minecraft:planks".into(), count: 1 };
        assert!(recipes().matches(&planks, "minecraft:oak_planks", 0));
        assert!(!recipes().matches(&planks, "minecraft:stone", 0));
        let any = Ingredient::Item { name: "minecraft:stick".into(), meta: ANY_META, count: 1 };
        assert!(recipes().matches(&any, "minecraft:stick", 7));
        let exact = Ingredient::Item { name: "minecraft:stick".into(), meta: 0, count: 1 };
        assert!(!recipes().matches(&exact, "minecraft:stick", 7));
        assert!(!recipes().matches(&Ingredient::Empty, "minecraft:stick", 0));
    }

    #[test]
    fn crafting_data_packet_has_every_recipe() {
        let p = crafting_data_packet();
        assert_eq!(p.id, 52);
        let shaped = recipes().all().iter().filter(|r| matches!(r.shape, Shape::Shaped { .. })).count();
        let shapeless = recipes().all().len() - shaped;
        // la prima lista del pacchetto è quella delle ricette shaped
        let mut r = bedrock_protocol::reader::Reader(&p.body);
        assert_eq!(r.var_u32(), Some(shaped as u32));
        assert!(p.body.len() > shaped + shapeless);
    }
}
