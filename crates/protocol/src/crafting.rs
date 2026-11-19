//! Pacchetto `CraftingData` (ricette mandate al client). Byte da gophertunnel v1.63.0: `packet/crafting_data.go` e
//! `protocol/recipe.go` (`marshalShaped`, `marshalShapeless`), `protocol/item_descriptor.go`, `protocol/writer.go`
//! (`ItemDescriptorCount`, `Item`). Si scrivono solo le ricette `shaped` e `shapeless`: le altre liste vanno vuote.
use crate::item::ItemStack;
use crate::wire::{Encode, ServerPacket, Writer};

/// Come un ingrediente descrive l'oggetto che accetta.
#[derive(Clone, Debug, PartialEq)]
pub enum Descriptor {
    /// Casella vuota (`InvalidItemDescriptor`).
    Empty,
    /// Un oggetto per nome; `meta` 32767 accetta qualunque variante.
    Item { name: String, meta: i32 },
    /// Un tag di oggetti (`minecraft:planks`...).
    Tag(String),
}

/// `ItemDescriptorCount`: descrittore e quantità.
#[derive(Clone, Debug, PartialEq)]
pub struct Ingredient {
    pub descriptor: Descriptor,
    pub count: i32,
}

impl Ingredient {
    pub fn empty() -> Self {
        Self { descriptor: Descriptor::Empty, count: 0 }
    }
}

impl Encode for Ingredient {
    fn encode(&self, w: &mut Writer) {
        match &self.descriptor {
            // variante 0, poi il valore ausiliario 32767 (come scrive gophertunnel)
            Descriptor::Empty => {
                w.var_u32(0);
                w.var_i32(32767);
            }
            // variante 1 (Default), l'etichetta del tipo, i campi; il tag ha in più l'ausiliario 32767
            Descriptor::Item { name, meta } => {
                w.var_u32(1);
                w.string("name");
                w.string(name);
                w.var_i32(*meta);
            }
            Descriptor::Tag(tag) => {
                w.var_u32(1);
                w.string("item_tag");
                w.string(tag);
                w.var_i32(32767);
            }
        }
        w.var_i32(self.count);
    }
}

/// Risultato di una ricetta (`protocol.ItemStack` di gophertunnel, senza NBT: il danno si toglie come fa Dragonfly).
#[derive(Clone, Debug, PartialEq)]
pub struct RecipeOutput {
    /// ID di rete dell'oggetto (`items.txt`).
    pub id: i32,
    pub count: u16,
    pub aux: u32,
    /// Hash dello stato del blocco, 0 se non è un blocco.
    pub block_runtime_id: i32,
}

impl Encode for RecipeOutput {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.id);
        w.u16(self.count);
        w.var_u32(self.aux);
        w.var_i32(self.block_runtime_id);
        if self.id != 0 {
            w.bytes(&ItemStack::empty_user_data());
        } else {
            w.var_u32(0);
        }
    }
}

fn write_list<T: Encode>(w: &mut Writer, list: &[T]) {
    w.var_u32(list.len() as u32);
    for v in list {
        v.encode(w);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapelessRecipe {
    pub recipe_id: String,
    pub input: Vec<Ingredient>,
    pub output: Vec<RecipeOutput>,
    pub block: String,
    pub priority: i32,
    pub network_id: u32,
}

impl Encode for ShapelessRecipe {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.recipe_id);
        write_list(w, &self.input);
        write_list(w, &self.output);
        w.raw(&[0; 16]); // UUID vuoto, come Dragonfly
        w.string(&self.block);
        w.var_i32(self.priority);
        w.bool(false); // requisito di sblocco assente (come Dragonfly)
        w.var_u32(self.network_id);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapedRecipe {
    pub recipe_id: String,
    pub width: i32,
    pub height: i32,
    /// `width * height` caselle, per righe.
    pub input: Vec<Ingredient>,
    pub output: Vec<RecipeOutput>,
    pub block: String,
    pub priority: i32,
    pub network_id: u32,
}

impl Encode for ShapedRecipe {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.recipe_id);
        w.var_i32(self.width);
        w.var_i32(self.height);
        write_list(w, &self.input);
        write_list(w, &self.output);
        w.raw(&[0; 16]); // UUID vuoto
        w.string(&self.block);
        w.var_i32(self.priority);
        w.bool(true); // AssumeSymmetry, come Dragonfly
        w.bool(false); // requisito di sblocco assente
        w.var_u32(self.network_id);
    }
}

/// `CraftingData` (ID 52). Le liste che non si usano (multi, dati utente, chimica, fucina, pozioni, riduttori) vanno vuote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CraftingData {
    pub shaped: Vec<ShapedRecipe>,
    pub shapeless: Vec<ShapelessRecipe>,
    /// Se vero il client dimentica le ricette ricevute prima.
    pub clear_recipes: bool,
}

impl Encode for CraftingData {
    fn encode(&self, w: &mut Writer) {
        write_list(w, &self.shaped);
        write_list(w, &self.shapeless);
        for _ in 0..9 {
            // multi, dati utente, 2 di chimica, 2 di fucina, 2 di pozioni, riduttori di materiali
            w.var_u32(0);
        }
        w.bool(self.clear_recipes);
    }
}

impl ServerPacket for CraftingData {
    const ID: u32 = 52;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_52() {
        assert_eq!(CraftingData::default().packet().id, 52);
    }

    #[test]
    fn empty_packet_has_eleven_empty_lists_and_the_clear_flag() {
        let b = CraftingData { clear_recipes: true, ..Default::default() }.to_bytes();
        assert_eq!(b, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn shapeless_recipe_bytes() {
        let r = ShapelessRecipe {
            recipe_id: "r".into(),
            input: vec![Ingredient { descriptor: Descriptor::Item { name: "minecraft:oak_planks".into(), meta: 32767 }, count: 1 }],
            output: vec![RecipeOutput { id: 5, count: 4, aux: 0, block_runtime_id: -1 }],
            block: "crafting_table".into(),
            priority: 0,
            network_id: 7,
        };
        let mut exp: Vec<u8> = vec![1, b'r'];
        exp.push(1); // un ingrediente
        exp.extend([1, 4]); // variante Default, etichetta "name"
        exp.extend(b"name");
        exp.push(20);
        exp.extend(b"minecraft:oak_planks");
        exp.extend([0xfe, 0xff, 0x03]); // meta 32767, zigzag
        exp.push(2); // quantità 1, zigzag
        exp.push(1); // un risultato
        exp.extend([10, 4, 0, 0, 1]); // id zigzag(5), quantità u16, ausiliario, blocco zigzag(-1)
        exp.push(10);
        exp.extend([0; 10]); // dati utente vuoti
        exp.extend([0; 16]); // UUID
        exp.push(14);
        exp.extend(b"crafting_table");
        exp.extend([0, 0, 7]); // priorità, nessun requisito, ID di rete
        assert_eq!(r.to_bytes(), exp);
    }

    #[test]
    fn shaped_recipe_bytes_with_empty_slot_and_tag() {
        let r = ShapedRecipe {
            recipe_id: "s".into(),
            width: 2,
            height: 1,
            input: vec![Ingredient { descriptor: Descriptor::Tag("minecraft:planks".into()), count: 1 }, Ingredient::empty()],
            output: vec![RecipeOutput { id: 1, count: 1, aux: 0, block_runtime_id: 0 }],
            block: "crafting_table".into(),
            priority: 50,
            network_id: 300,
        };
        let mut exp: Vec<u8> = vec![1, b's', 4, 2]; // ID; larghezza 2 e altezza 1 (zigzag)
        exp.push(2); // due caselle
        exp.extend([1, 8]);
        exp.extend(b"item_tag");
        exp.push(16);
        exp.extend(b"minecraft:planks");
        exp.extend([0xfe, 0xff, 0x03]); // ausiliario 32767 del tag
        exp.push(2); // quantità 1
        exp.extend([0, 0xfe, 0xff, 0x03, 0]); // casella vuota: variante 0, ausiliario 32767, quantità 0
        exp.push(1); // un risultato
        exp.extend([2, 1, 0, 0, 0]);
        exp.push(10);
        exp.extend([0; 10]);
        exp.extend([0; 16]);
        exp.push(14);
        exp.extend(b"crafting_table");
        exp.extend([100, 1, 0, 0xac, 0x02]); // priorità 50 zigzag, simmetrica, nessun requisito, ID 300 varuint
        assert_eq!(r.to_bytes(), exp);
    }

    #[test]
    fn packet_puts_shaped_before_shapeless() {
        let s = ShapelessRecipe { recipe_id: "a".into(), input: vec![], output: vec![], block: "b".into(), priority: 0, network_id: 1 };
        let b = CraftingData { shaped: vec![], shapeless: vec![s.clone()], clear_recipes: false }.to_bytes();
        assert_eq!(&b[..2], [0, 1]); // 0 shaped, 1 shapeless
        assert_eq!(b.len(), 2 + s.to_bytes().len() + 9 + 1);
    }
}
