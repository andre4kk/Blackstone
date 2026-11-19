//! Ricette di fusione e combustibili della fornace, caricati da `data/smelting.json` (estratto da Dragonfly, MIT, con
//! `tools/extract_smelting.py`). Un oggetto si riconosce per il suo ID (il valore ausiliario non conta: nei nomi di questa versione le
//! varianti hanno nomi diversi).
use crate::{inventory::Item, registry};
use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

/// Tick per cuocere un oggetto nella fornace (Dragonfly `Furnace.Tick`: `tickSmelting(10s, ...)`).
pub(crate) const COOK_TICKS: i32 = 200;

/// Cosa dà la fusione di un oggetto.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Smelt {
    pub output: Item,
    pub xp: f64,
}

/// Quanto dura un combustibile e cosa resta al suo posto (se non si toglie semplicemente un pezzo).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fuel {
    pub ticks: i32,
    pub residue: Option<Item>,
}

#[derive(Deserialize)]
struct SmeltRow {
    input: String,
    output: String,
    xp: f64,
}

#[derive(Deserialize)]
struct FuelRow {
    item: String,
    ticks: i32,
    residue: Option<String>,
}

#[derive(Deserialize)]
struct File {
    smelting: Vec<SmeltRow>,
    fuel: Vec<FuelRow>,
}

pub(crate) struct Tables {
    smelts: HashMap<i16, Smelt>,
    fuels: HashMap<i16, Fuel>,
}

impl Tables {
    /// La fusione di `item` (`None` se non si può cuocere).
    pub fn smelt(&self, item: &Item) -> Option<&Smelt> {
        self.smelts.get(&item.id)
    }

    /// Il combustibile di `item` (`None` se non brucia).
    pub fn fuel(&self, item: &Item) -> Option<&Fuel> {
        self.fuels.get(&item.id)
    }
}

pub(crate) fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let file: File = serde_json::from_str(include_str!("../data/smelting.json")).expect("smelting.json non valido");
        let items = registry::items();
        let smelts = file.smelting.iter().filter_map(|r| Some((items.id(&r.input)?, Smelt { output: Item::from_name(&r.output, 1)?, xp: r.xp }))).collect();
        let fuels = file
            .fuel
            .iter()
            .filter_map(|r| {
                let residue = match &r.residue {
                    Some(name) => Some(Item::from_name(name, 1)?),
                    None => None,
                };
                Some((items.id(&r.item)?, Fuel { ticks: r.ticks, residue }))
            })
            .collect();
        Tables { smelts, fuels }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> Item {
        Item::from_name(name, 1).unwrap()
    }

    #[test]
    fn iron_and_logs_smelt_and_coal_burns() {
        let t = tables();
        let smelt = t.smelt(&item("minecraft:raw_iron")).expect("il ferro grezzo cuoce");
        assert_eq!(smelt.output.id, item("minecraft:iron_ingot").id);
        assert!((smelt.xp - 0.7).abs() < 1e-9);
        assert!(t.smelt(&item("minecraft:oak_log")).is_some());
        assert!(t.smelt(&item("minecraft:dirt")).is_none());
        assert_eq!(t.fuel(&item("minecraft:coal")).map(|f| f.ticks), Some(1600));
        assert_eq!(t.fuel(&item("minecraft:stick")).map(|f| f.ticks), Some(100));
        let lava = t.fuel(&item("minecraft:lava_bucket")).unwrap();
        assert_eq!((lava.ticks, lava.residue.as_ref().map(|r| r.id)), (20000, Some(item("minecraft:bucket").id)));
        assert!(t.fuel(&item("minecraft:stone")).is_none());
    }
}
