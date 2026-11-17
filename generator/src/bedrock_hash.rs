//! Hash degli stati di blocco Bedrock (copiato da `crates/server/src/world.rs`, MIT).
//!
//! Dalla 1.26 l'ID di rete di un blocco (`block_network_ids_are_hashes`) è l'hash FNV-1a a 32 bit dell'NBT
//! little-endian `{name, states}`: lo stesso NBT è quello della voce di palette inviata al server.

/// Valore di uno stato di blocco Bedrock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State<'a> {
    Byte(u8),
    Int(i32),
    Str(&'a str),
}

fn nbt_name(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(&(s.len() as u16).to_le_bytes());
    b.extend_from_slice(s.as_bytes());
}

/// NBT su disco little-endian `{name, states}` di uno stato di blocco; gli stati vanno in ordine alfabetico.
pub fn block_nbt(name: &str, states: &[(&str, State)]) -> Vec<u8> {
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
    nbt
}

/// FNV-1a a 32 bit di un NBT prodotto da [`block_nbt`].
pub fn hash_nbt(nbt: &[u8]) -> i32 {
    nbt.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193)) as i32
}

/// Hash dello stato di un blocco, usato come ID di rete. Gli stati vanno in ordine alfabetico.
pub fn block_hash(name: &str, states: &[(&str, State)]) -> i32 {
    hash_nbt(&block_nbt(name, states))
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
}
