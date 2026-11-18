//! Tabelle di sopravvivenza (drop dei blocchi, strumenti, cibi) estratte da Dragonfly (MIT) da `tools/extract_survival.py`.
//!
//! I JSON stanno in `crates/server/data/`; qui si caricano una volta sola (`OnceLock`) e si interrogano per nome.
//!
//! Livelli di raccolta (`tier`): sono quelli di Dragonfly (`item.ToolTier.HarvestLevel`), che NON coincidono con la scala 0-5
//! pensata all'inizio: nessuno 0, legno e oro 1, pietra e rame 2, ferro 3, diamante 4, netherite 4 (come il diamante). Un blocco
//! con `min_tier` N si raccoglie con uno strumento del tipo giusto di livello >= N (ferro = 3, quindi il minerale di diamante vuole 3).
//!
//! Drop: ogni voce ha una probabilita'. Se le probabilita' di tutte le voci di un blocco sommano 1 (ghiaia: selce 0,1 + ghiaia 0,9)
//! sono alternative esclusive (un solo tiro); altrimenti sono tiri indipendenti.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
    Sword,
    Shears,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ToolInfo {
    pub kind: ToolKind,
    /// Livello di raccolta di Dragonfly (vedi il modulo): legno/oro 1, pietra 2, ferro 3, diamante e netherite 4.
    pub tier: u8,
    pub attack_damage: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Drop {
    pub item: String,
    pub min: u16,
    pub max: u16,
    pub chance: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BlockDrops {
    /// Strumento richiesto per ottenere i drop; `None` = qualsiasi mano o strumento.
    pub tool: Option<ToolKind>,
    /// Livello minimo di quello strumento.
    pub min_tier: u8,
    pub drops: Vec<Drop>,
}

#[allow(dead_code)] // lo usa un compito seguente (mangiare / danni)
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Food {
    pub points: u8,
    pub saturation: f32,
    pub always_edible: bool,
}

#[derive(Deserialize)]
struct ToolsFile {
    tools: HashMap<String, ToolInfo>,
}

#[derive(Deserialize)]
struct DropsFile {
    blocks: HashMap<String, BlockDrops>,
}

#[allow(dead_code)] // lo usa un compito seguente (mangiare / danni)
#[derive(Deserialize)]
struct FoodsFile {
    foods: HashMap<String, Food>,
}

/// Strumento (o arma) con questo nome, se lo e'.
pub(crate) fn tool(item_name: &str) -> Option<&'static ToolInfo> {
    static T: OnceLock<HashMap<String, ToolInfo>> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str::<ToolsFile>(include_str!("../data/tools.json")).expect("tools.json non valido").tools).get(item_name)
}

/// Regole di raccolta e drop di un blocco (nome senza stati, es. `minecraft:stone`). `None` = lascia se stesso (1) senza requisiti.
pub(crate) fn block_drops(block_name: &str) -> Option<&'static BlockDrops> {
    static T: OnceLock<HashMap<String, BlockDrops>> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str::<DropsFile>(include_str!("../data/block_drops.json")).expect("block_drops.json non valido").blocks).get(block_name)
}

/// Cibo con questo nome, se lo e'.
#[allow(dead_code)] // lo usa un compito seguente (mangiare / danni)
pub(crate) fn food(item_name: &str) -> Option<&'static Food> {
    static T: OnceLock<HashMap<String, Food>> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str::<FoodsFile>(include_str!("../data/foods.json")).expect("foods.json non valido").foods).get(item_name)
}

/// Danno di un colpo con questo oggetto in mano (1,0 a mani nude o con un oggetto che non e' uno strumento).
#[allow(dead_code)] // lo usa un compito seguente (mangiare / danni)
pub(crate) fn attack_damage(item_name: &str) -> f32 {
    tool(item_name).map_or(1.0, |t| t.attack_damage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stone_needs_a_pickaxe_and_drops_cobblestone() {
        let d = block_drops("minecraft:stone").unwrap();
        assert_eq!(d.tool, Some(ToolKind::Pickaxe));
        assert!(d.min_tier >= 1);
        assert_eq!(d.drops.len(), 1);
        assert_eq!((d.drops[0].item.as_str(), d.drops[0].min, d.drops[0].max), ("minecraft:cobblestone", 1, 1));
    }

    #[test]
    fn iron_ore_needs_stone_tier() {
        let d = block_drops("minecraft:iron_ore").unwrap();
        assert_eq!((d.tool, d.min_tier), (Some(ToolKind::Pickaxe), 2));
        assert_eq!(d.drops[0].item, "minecraft:raw_iron");
        // Dragonfly: il diamante si raccoglie con il ferro (livello 3), l'ossidiana con il diamante (4)
        assert_eq!(block_drops("minecraft:diamond_ore").unwrap().min_tier, 3);
        assert_eq!(block_drops("minecraft:obsidian").unwrap().min_tier, 4);
    }

    #[test]
    fn grass_block_drops_dirt_by_hand() {
        let d = block_drops("minecraft:grass_block").unwrap();
        assert_eq!((d.tool, d.min_tier), (None, 0));
        assert_eq!(d.drops[0].item, "minecraft:dirt");
    }

    #[test]
    fn dirt_has_no_entry_and_drops_itself() {
        assert!(block_drops("minecraft:dirt").is_none());
        assert!(block_drops("minecraft:oak_planks").is_none());
    }

    #[test]
    fn glass_drops_nothing() {
        let d = block_drops("minecraft:glass").unwrap();
        assert!(d.drops.is_empty());
    }

    #[test]
    fn gravel_and_tall_grass_have_chances() {
        let g = block_drops("minecraft:gravel").unwrap();
        let flint = g.drops.iter().find(|d| d.item == "minecraft:flint").unwrap();
        assert!((flint.chance - 0.1).abs() < 1e-6);
        assert!((g.drops.iter().map(|d| d.chance).sum::<f32>() - 1.0).abs() < 1e-6);
        let s = block_drops("minecraft:short_grass").unwrap();
        assert_eq!(s.drops[0].item, "minecraft:wheat_seeds");
        assert!((s.drops[0].chance - 0.125).abs() < 1e-6);
        assert_eq!(block_drops("minecraft:oak_leaves").unwrap().tool, Some(ToolKind::Shears));
    }

    #[test]
    fn diamond_sword_damage() {
        // Dragonfly: BaseAttackDamage del diamante (4) + 3 = 7 (il brief diceva 8, che e' la netherite)
        assert_eq!(attack_damage("minecraft:diamond_sword"), 7.0);
        assert_eq!(attack_damage("minecraft:netherite_sword"), 8.0);
        assert_eq!(attack_damage("minecraft:wooden_sword"), 4.0);
    }

    #[test]
    fn fist_damage_is_one() {
        assert_eq!(attack_damage("minecraft:air"), 1.0);
        assert_eq!(attack_damage("minecraft:stick"), 1.0);
    }

    #[test]
    fn bread_food() {
        let f = food("minecraft:bread").unwrap();
        assert_eq!((f.points, f.saturation, f.always_edible), (5, 6.0, false));
        assert_eq!(food("minecraft:cooked_beef").unwrap().points, 8);
        assert!(food("minecraft:stick").is_none());
    }

    #[test]
    fn golden_apple_always_edible() {
        assert!(food("minecraft:golden_apple").unwrap().always_edible);
        assert!(!food("minecraft:apple").unwrap().always_edible);
    }

    #[test]
    fn wooden_pickaxe_tier_1() {
        let t = tool("minecraft:wooden_pickaxe").unwrap();
        assert_eq!((t.kind, t.tier), (ToolKind::Pickaxe, 1));
        assert_eq!(tool("minecraft:stone_pickaxe").unwrap().tier, 2);
        assert_eq!(tool("minecraft:iron_pickaxe").unwrap().tier, 3);
        assert_eq!(tool("minecraft:diamond_pickaxe").unwrap().tier, 4);
        assert_eq!(tool("minecraft:netherite_pickaxe").unwrap().tier, 4);
        assert_eq!(tool("minecraft:shears").unwrap().kind, ToolKind::Shears);
    }
}
