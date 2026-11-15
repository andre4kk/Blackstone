//! Mondo piatto: ID di rete dei blocchi e serializzazione dei chunk.
//!
//! Dalla 1.26 l'ID di rete di un blocco (`block_network_ids_are_hashes`) è l'hash FNV-1a a 32 bit dell'NBT
//! little-endian `{name, states}`: si calcola, senza bisogno di una tabella dei blocchi.
use bedrock_protocol::wire::Writer;
use std::sync::OnceLock;

/// Valore di uno stato di blocco. `Int` e `Str` serviranno ai blocchi con più stati.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum State<'a> {
    Byte(u8),
    Int(i32),
    Str(&'a str),
}

fn nbt_name(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(&(s.len() as u16).to_le_bytes());
    b.extend_from_slice(s.as_bytes());
}

/// Hash dello stato di un blocco, usato come ID di rete. Gli stati vanno in ordine alfabetico.
pub fn block_hash(name: &str, states: &[(&str, State)]) -> i32 {
    let mut states = states.to_vec();
    states.sort_by_key(|s| s.0);
    let mut nbt = vec![0x0a, 0, 0, 0x08]; // compound radice senza nome, stringa "name"
    nbt_name(&mut nbt, "name");
    nbt_name(&mut nbt, name);
    nbt.push(0x0a); // compound "states"
    nbt_name(&mut nbt, "states");
    for (key, value) in states {
        match value {
            State::Byte(v) => (nbt.push(0x01), nbt_name(&mut nbt, key), nbt.push(v)),
            State::Int(v) => (nbt.push(0x03), nbt_name(&mut nbt, key), nbt.extend_from_slice(&v.to_le_bytes())),
            State::Str(v) => (nbt.push(0x08), nbt_name(&mut nbt, key), nbt_name(&mut nbt, v)),
        };
    }
    nbt.extend_from_slice(&[0, 0]); // fine di "states" e della radice
    nbt.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193)) as i32
}

/// ID numerico del bioma "pianura" (dai dati ufficiali di Mojang).
pub const PLAINS: u16 = 1;

/// Altezza minima dell'overworld e numero di sezioni (sotto-chunk) da -64 a 320.
const SECTIONS: usize = 24;
/// Livello del suolo del mondo piatto: bedrock a y=-64, terra a -63 e -62, erba a -61.
pub const GROUND_Y: i32 = -60; // y del primo blocco d'aria sopra l'erba

/// Chunk del mondo piatto, già serializzato (è lo stesso per tutti): sotto-chunk, biomi, bordi, blocchi con dati.
/// `v9` scrive il sotto-chunk in versione 9, con l'indice della sezione, invece della 8 (due varianti accettate dal client).
pub fn flat_chunk(v9: bool) -> &'static [u8] {
    static V8: OnceLock<Vec<u8>> = OnceLock::new();
    static V9: OnceLock<Vec<u8>> = OnceLock::new();
    (if v9 { &V9 } else { &V8 }).get_or_init(|| build_flat_chunk(v9))
}

fn build_flat_chunk(v9: bool) -> Vec<u8> {
    let palette = [
        block_hash("minecraft:air", &[]),
        block_hash("minecraft:bedrock", &[("infiniburn_bit", State::Byte(0))]),
        block_hash("minecraft:dirt", &[]),
        block_hash("minecraft:grass_block", &[]),
    ];
    // 2 bit per blocco, 16 blocchi per parola da 32 bit; indice del blocco = x*256 + z*16 + y.
    let mut words = [0u32; 4096 / 16];
    for column in 0..256 {
        for (y, id) in [(0, 1), (1, 2), (2, 2), (3, 3)] {
            let i = column * 16 + y;
            words[i / 16] |= id << ((i % 16) * 2);
        }
    }
    let mut w = Writer::default();
    w.u8(if v9 { 9 } else { 8 }); // versione del sotto-chunk
    w.u8(1); // un solo livello di blocchi
    if v9 {
        w.i8(-4); // la v9 indica la sezione: la prima dell'overworld è a y=-64
    }
    w.u8((2 << 1) | 1); // bit per blocco, bit basso = ID di rete (non NBT)
    words.iter().for_each(|&word| w.u32(word));
    w.var_i32(palette.len() as i32);
    palette.iter().for_each(|&id| w.var_i32(id));
    for _ in 0..SECTIONS {
        w.u8(1); // biomi: 0 bit per blocco, una sola voce
        w.var_i32(PLAINS as i32);
    }
    w.u8(0); // blocchi di bordo
    w.0 // nessun blocco con dati (casse, cartelli...)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_hash_matches_known_value() {
        assert_eq!(block_hash("minecraft:air", &[]), -604_749_536);
    }

    #[test]
    fn state_order_does_not_matter() {
        let a = block_hash("x", &[("b", State::Byte(1)), ("a", State::Int(2))]);
        assert_eq!(a, block_hash("x", &[("a", State::Int(2)), ("b", State::Byte(1))]));
        assert_ne!(a, block_hash("x", &[("a", State::Int(3)), ("b", State::Byte(1))]));
    }

    #[test]
    fn flat_chunk_layout() {
        let c = flat_chunk(false);
        assert_eq!(&c[..3], &[8, 1, 5], "versione 8, un livello, 2 bit per blocco");
        // 3 intestazione + 1024 parole + 1 (numero di voci) + palette + 24 biomi da 2 byte + 1 byte di bordi
        assert_eq!(c.len(), 3 + 1024 + 1 + palette_bytes() + 24 * 2 + 1);
        assert_eq!(&c[c.len() - 3..], &[1, 2, 0], "ultimo bioma (pianura) e bordi");
    }

    #[test]
    fn flat_chunk_v9_adds_the_section_index() {
        let (v8, v9) = (flat_chunk(false), flat_chunk(true));
        assert_eq!(&v9[..4], &[9, 1, 0xfc, 5]);
        assert_eq!(&v9[3..], &v8[2..], "a parte l'intestazione i due formati coincidono");
    }

    #[test]
    fn flat_chunk_columns_are_bedrock_dirt_dirt_grass_air() {
        let c = flat_chunk(false);
        let words: Vec<u32> = c[3..3 + 1024].chunks(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        let at = |x: usize, z: usize, y: usize| {
            let i = x * 256 + z * 16 + y;
            (words[i / 16] >> ((i % 16) * 2)) & 3
        };
        for (x, z) in [(0, 0), (7, 3), (15, 15)] {
            assert_eq!((0..6).map(|y| at(x, z, y)).collect::<Vec<_>>(), [1, 2, 2, 3, 0, 0]);
        }
    }

    /// Byte occupati dalla palette (gli ID sono hash a 32 bit: in ZigZag quasi sempre 5 byte).
    fn palette_bytes() -> usize {
        [("minecraft:air", vec![]), ("minecraft:bedrock", vec![("infiniburn_bit", State::Byte(0))]), ("minecraft:dirt", vec![]), ("minecraft:grass_block", vec![])]
            .iter()
            .map(|(name, states)| {
                let mut w = Writer::default();
                w.var_i32(block_hash(name, states));
                w.0.len()
            })
            .sum()
    }
}
