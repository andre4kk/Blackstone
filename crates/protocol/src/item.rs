//! Pila di oggetti nella forma `NetworkItemStackDescriptor` (inventario, equipaggiamento, richieste).
use crate::reader::Reader;
use crate::wire::{Encode, Writer};

/// Un oggetto con quantità. L'aria è tutto a zero (8 byte sul filo).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemStack {
    /// ID numerico dell'oggetto (i16 fisso); 0 = aria.
    pub id: i16,
    pub count: u16,
    /// Valore ausiliario (metadata).
    pub aux: u32,
    /// ID di rete della pila assegnato dal server (> 0); `None` = assente.
    pub net_id: Option<i32>,
    /// Hash del blocco se l'oggetto è un blocco, altrimenti 0.
    pub block_runtime_id: u32,
    /// Dati utente già codificati (vedi `empty_user_data` e `user_data_with_nbt`).
    pub user_data: Vec<u8>,
}

impl ItemStack {
    pub fn air() -> Self {
        Self::default()
    }

    /// Dati utente senza NBT: `i16 0` + `u32 0` (può posare su) + `u32 0` (può rompere), 10 byte.
    pub fn empty_user_data() -> Vec<u8> {
        vec![0; 10]
    }

    /// Dati utente con un NBT little-endian: marcatore `i16 -1`, versione `u8 1`, NBT, poi le due liste vuote.
    pub fn user_data_with_nbt(nbt_le: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(nbt_le.len() + 11);
        v.extend_from_slice(&(-1i16).to_le_bytes());
        v.push(1);
        v.extend_from_slice(nbt_le);
        v.extend_from_slice(&[0; 8]);
        v
    }

    pub fn encode(&self, w: &mut Writer) {
        w.i16(self.id);
        w.u16(self.count);
        w.var_u32(self.aux);
        match self.net_id {
            Some(n) => {
                w.u8(1);
                w.var_i32(n);
            }
            None => w.u8(0),
        }
        w.var_u32(self.block_runtime_id);
        w.bytes(&self.user_data);
    }

    pub fn decode(r: &mut Reader) -> Option<Self> {
        Some(Self {
            id: r.i16()?,
            count: r.u16()?,
            aux: r.var_u32()?,
            net_id: r.optional(|r| r.var_i32())?,
            block_runtime_id: r.var_u32()?,
            user_data: r.bytes()?.to_vec(),
        })
    }
}

/// Serve al codice generato (`generated.rs`), che scrive i campi `NetworkItemStackDescriptor` con `Encode`.
impl Encode for ItemStack {
    fn encode(&self, w: &mut Writer) {
        ItemStack::encode(self, w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE_HASH: u32 = (-2_144_268_767i32) as u32;

    fn stone() -> ItemStack {
        ItemStack { id: 1, count: 64, aux: 0, net_id: Some(5), block_runtime_id: STONE_HASH, user_data: ItemStack::empty_user_data() }
    }

    fn bytes_of(i: &ItemStack) -> Vec<u8> {
        let mut w = Writer::default();
        i.encode(&mut w);
        w.0
    }

    #[test]
    fn air_is_eight_zero_bytes() {
        assert_eq!(bytes_of(&ItemStack::air()), [0u8; 8]);
        assert_eq!(ItemStack::decode(&mut Reader(&[0; 8])), Some(ItemStack::air()));
    }

    #[test]
    fn stone_round_trip() {
        let bytes = bytes_of(&stone());
        // id i16 LE, count u16 LE, aux, presenza + net id zigzag (5 -> 10), hash varuint32 (5 byte), len 10 + 10 zeri
        assert_eq!(&bytes[..7], [1, 0, 64, 0, 0, 1, 10]);
        let mut r = Reader(&bytes);
        assert_eq!(ItemStack::decode(&mut r), Some(stone()));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn decode_rejects_truncated() {
        let bytes = bytes_of(&stone());
        for n in 0..bytes.len() {
            assert_eq!(ItemStack::decode(&mut Reader(&bytes[..n])), None, "tagliato a {n} byte");
        }
    }

    #[test]
    fn user_data_layouts() {
        assert_eq!(ItemStack::empty_user_data(), [0; 10]);
        assert_eq!(ItemStack::user_data_with_nbt(&[9, 9]), [0xff, 0xff, 1, 9, 9, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
}
