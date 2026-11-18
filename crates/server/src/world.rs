//! Mondo piatto: ID di rete dei blocchi e serializzazione dei chunk.
//!
//! Dalla 1.26 l'ID di rete di un blocco (`block_network_ids_are_hashes`) è l'hash FNV-1a a 32 bit dell'NBT
//! little-endian `{name, states}`: si calcola, senza bisogno di una tabella dei blocchi.
use bedrock_protocol::wire::Writer;
use vgen::{BlockEntity, ChunkData, Dim, Section};

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

/// NBT su disco little-endian `{name, states}` di uno stato di blocco. Gli stati vanno in ordine alfabetico.
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

/// Hash dello stato di un blocco, usato come ID di rete (FNV-1a di [`block_nbt`]).
pub fn block_hash(name: &str, states: &[(&str, State)]) -> i32 {
    block_nbt(name, states).iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193)) as i32
}

/// ID numerico del bioma "pianura" (dai dati ufficiali di Mojang).
pub const PLAINS: u16 = 1;

/// Livello del suolo del mondo piatto: bedrock a y=-64, terra a -63 e -62, erba a -61.
pub const GROUND_Y: i32 = -60; // y del primo blocco d'aria sopra l'erba

/// Spawn del mondo piatto (piedi): il primo blocco d'aria sopra l'erba al centro del chunk (0, 0).
pub const FLAT_SPAWN: (i32, i32, i32) = (0, GROUND_Y, 0);

/// Larghezze in bit per blocco ammesse da Bedrock.
pub(crate) const BIT_WIDTHS: [u8; 8] = [1, 2, 3, 4, 5, 6, 8, 16];

/// Bit per blocco per una palette di `len` voci: 0 se ce n'è una sola, altrimenti la più piccola larghezza ammessa che basta.
pub(crate) fn bits_for_palette(len: usize) -> u8 {
    if len <= 1 {
        return 0;
    }
    BIT_WIDTHS.iter().copied().find(|&b| len <= 1usize << b).unwrap_or(16)
}

/// Parole impacchettate di uno storage a `bits` bit per voce (`32 / bits` voci per parola, senza spezzarle); nessuna se `bits` è 0.
pub(crate) fn pack_words(bits: u8, index_at: impl Fn(usize) -> usize) -> Vec<u32> {
    if bits == 0 {
        return Vec::new();
    }
    let per_word = 32 / bits as usize;
    (0..4096usize.div_ceil(per_word))
        .map(|word| (0..per_word.min(4096 - word * per_word)).fold(0u32, |v, k| v | (index_at(word * per_word + k) as u32) << (k * bits as usize)))
        .collect()
}

/// Scrive uno storage con palette: intestazione, parole impacchettate, numero di voci della palette e voci.
/// Con una sola voce (0 bit) non si scrivono né parole né numero di voci: segue la voce unica
/// (come nel vecchio mondo piatto, verificato col client).
fn write_storage(w: &mut Writer, palette: &[i32], index_at: impl Fn(usize) -> usize) {
    // Diagnostica (VERSMC_DEBUG=nozero): niente storage a 0 bit, la voce unica si duplica e si usa 1 bit.
    let doubled: Vec<i32>;
    let palette = if palette.len() == 1 && debug_flag("nozero") {
        doubled = vec![palette[0], palette[0]];
        &doubled[..]
    } else {
        palette
    };
    let bits = bits_for_palette(palette.len());
    w.u8((bits << 1) | 1); // bit basso = ID di rete (non NBT)
    pack_words(bits, index_at).into_iter().for_each(|word| w.u32(word));
    if bits > 0 {
        w.var_i32(palette.len() as i32);
    }
    palette.iter().for_each(|&id| w.var_i32(id));
}

/// Interruttori di diagnostica per capire cosa rifiuta il client: variabile `VERSMC_DEBUG` con nomi separati da virgola
/// (`nozero`, `nowater`, `plains`, `nobe`). Senza la variabile non cambia nulla.
pub(crate) fn debug_flag(name: &str) -> bool {
    static FLAGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    FLAGS
        .get_or_init(|| std::env::var("VERSMC_DEBUG").map(|v| v.split(',').map(|s| s.trim().to_string()).collect()).unwrap_or_default())
        .iter()
        .any(|f| f == name)
}

/// Chunk in memoria: sezioni indicate dalla più bassa della dimensione (`None` = sezione di sola aria).
#[derive(Clone, Debug)]
pub(crate) struct Chunk {
    pub dim: Dim,
    pub x: i32,
    pub z: i32,
    pub(crate) sections: Vec<Option<Section>>,
    pub(crate) block_entities: Vec<BlockEntity>,
    pub dirty: bool,
    /// Vero per il mondo piatto di ripiego (`Chunk::flat`): le sue modifiche non si salvano mai.
    pub(crate) fallback: bool,
}

impl Chunk {
    /// Indice della sezione più bassa della dimensione e numero di sezioni.
    pub(crate) fn section_range(dim: Dim) -> (i32, usize) {
        let (min, max) = dim.height_range();
        (min.div_euclid(16), ((max - min) / 16) as usize)
    }

    pub fn from_gen(data: ChunkData) -> Chunk {
        let (lowest, count) = Self::section_range(data.dim);
        let mut sections: Vec<Option<Section>> = vec![None; count];
        for s in data.sections {
            let slot = s.index as i32 - lowest;
            if (0..count as i32).contains(&slot) {
                sections[slot as usize] = Some(s);
            }
        }
        Chunk { dim: data.dim, x: data.x, z: data.z, sections, block_entities: data.block_entities, dirty: false, fallback: false }
    }

    /// Mondo piatto di ripiego: solo la sezione più bassa, con bedrock, due strati di terra e erba; biomi pianura.
    pub fn flat(dim: Dim, x: i32, z: i32) -> Chunk {
        let (lowest, count) = Self::section_range(dim);
        let palette = vec![
            block_hash("minecraft:air", &[]),
            block_hash("minecraft:bedrock", &[("infiniburn_bit", State::Byte(0))]),
            block_hash("minecraft:dirt", &[]),
            block_hash("minecraft:grass_block", &[]),
        ];
        let mut blocks = Box::new([0u16; 4096]);
        for (i, b) in blocks.iter_mut().enumerate() {
            *b = match i % 16 {
                0 => 1,
                1 | 2 => 2,
                3 => 3,
                _ => 0,
            };
        }
        let section = Section { index: lowest as i8, palette, blocks, biomes: Box::new([PLAINS as u32; 64]), water: None };
        let mut sections = vec![None; count];
        sections[0] = Some(section);
        Chunk { dim, x, z, sections, block_entities: Vec::new(), dirty: false, fallback: true }
    }

    /// Posizione di (y) nel chunk: indice nel vettore delle sezioni e y locale; `None` fuori dall'altezza della dimensione.
    fn locate(&self, y: i32) -> Option<(usize, usize)> {
        let (lowest, count) = Self::section_range(self.dim);
        let slot = y.div_euclid(16) - lowest;
        (0..count as i32).contains(&slot).then_some((slot as usize, y.rem_euclid(16) as usize))
    }

    /// Hash del blocco alle coordinate locali `lx`, `lz` (0..16) e `y` assoluta; aria se la sezione manca o `y` è fuori dal mondo.
    pub fn block(&self, lx: usize, y: i32, lz: usize) -> i32 {
        match self.locate(y).and_then(|(slot, ly)| self.sections[slot].as_ref().map(|s| (s, ly))) {
            Some((s, ly)) => s.palette[s.blocks[(lx * 16 + lz) * 16 + ly] as usize],
            None => block_hash("minecraft:air", &[]),
        }
    }

    /// Vero se nel secondo strato (liquido) di quella cella c'è acqua.
    pub fn water(&self, lx: usize, y: i32, lz: usize) -> bool {
        match self.locate(y).and_then(|(slot, ly)| self.sections[slot].as_ref().map(|s| (s, ly))) {
            Some((s, ly)) => s.water.as_ref().is_some_and(|m| {
                let n = (lx * 16 + lz) * 16 + ly;
                m[n / 64] >> (n % 64) & 1 == 1
            }),
            None => false,
        }
    }

    /// Cambia il blocco (e l'acqua del secondo strato) di una cella. Crea la sezione se manca, aggiunge `hash` alla palette
    /// se serve e segna il chunk come modificato. Fuori dall'altezza della dimensione non fa nulla.
    pub fn set_block(&mut self, lx: usize, y: i32, lz: usize, hash: i32, water: bool) {
        let Some((slot, ly)) = self.locate(y) else { return };
        let air = block_hash("minecraft:air", &[]);
        if self.sections[slot].is_none() {
            if hash == air && !water {
                return; // aria in una sezione di sola aria: non cambia nulla
            }
            let (lowest, _) = Self::section_range(self.dim);
            self.sections[slot] = Some(Section {
                index: (lowest + slot as i32) as i8,
                palette: vec![air],
                blocks: Box::new([0u16; 4096]),
                biomes: Box::new([PLAINS as u32; 64]),
                water: None,
            });
        }
        let s = self.sections[slot].as_mut().expect("sezione appena creata");
        let n = (lx * 16 + lz) * 16 + ly;
        let idx = s.palette.iter().position(|&p| p == hash).unwrap_or_else(|| (s.palette.push(hash), s.palette.len() - 1).1);
        s.blocks[n] = idx as u16;
        let has_water = s.water.as_ref().is_some_and(|m| m[n / 64] >> (n % 64) & 1 == 1);
        if water != has_water {
            if water {
                s.water.get_or_insert_with(|| Box::new([0u64; 64]))[n / 64] |= 1 << (n % 64);
            } else if let Some(m) = s.water.as_mut() {
                m[n / 64] &= !(1 << (n % 64));
                if m.iter().all(|&w| w == 0) {
                    s.water = None;
                }
            }
        }
        self.dirty = true;
    }

    /// Mette il block entity di una cella (sostituisce quello che c'era) e segna il chunk come modificato.
    pub fn put_block_entity(&mut self, entity: BlockEntity) {
        self.block_entities.retain(|b| (b.x, b.y, b.z) != (entity.x, entity.y, entity.z));
        self.block_entities.push(entity);
        self.dirty = true;
    }

    /// Toglie il block entity di una cella, se c'è.
    pub fn remove_block_entity(&mut self, pos: [i32; 3]) {
        let before = self.block_entities.len();
        self.block_entities.retain(|b| [b.x, b.y, b.z] != pos);
        self.dirty |= self.block_entities.len() != before;
    }

    /// Numero di sotto-chunk da annunciare: dalla sezione più bassa fino all'ultima non vuota (0 se il chunk è vuoto).
    pub fn sub_chunk_count(&self) -> u32 {
        self.sections.iter().rposition(Option::is_some).map_or(0, |i| i as u32 + 1)
    }

    /// Payload di `LevelChunk`: sotto-chunk v9, biomi di tutte le sezioni della dimensione, bordi (0), NBT dei blocchi con dati.
    ///
    /// Le sezioni mancanti sotto l'ultima non vuota diventano sotto-chunk d'aria (1 strato, 0 bit, una voce). I biomi delle
    /// sezioni mancanti (anche sopra l'ultima non vuota, fino alla cima della dimensione) sono una palette a voce unica con
    /// il bioma dell'ultima cella (in alto, angolo x=0 z=0) della sezione presente più vicina sotto, oppure pianura se non ce n'è.
    pub fn network_bytes(&self) -> Vec<u8> {
        let (lowest, _) = Self::section_range(self.dim);
        let air = block_hash("minecraft:air", &[]);
        let water = block_hash("minecraft:water", &[("liquid_depth", State::Int(0))]);
        let mut w = Writer::default();
        for (i, slot) in self.sections.iter().take(self.sub_chunk_count() as usize).enumerate() {
            w.u8(9); // versione del sotto-chunk
            match slot {
                Some(s) => {
                    let water_mask = s.water.as_ref().filter(|_| !debug_flag("nowater"));
                    w.u8(if water_mask.is_some() { 2 } else { 1 }); // strati: blocchi ed eventualmente acqua
                    w.i8(s.index);
                    write_storage(&mut w, &s.palette, |n| s.blocks[n] as usize);
                    if let Some(mask) = water_mask {
                        write_storage(&mut w, &[air, water], |n| (mask[n / 64] >> (n % 64) & 1) as usize);
                    }
                }
                None => {
                    w.u8(1);
                    w.i8((lowest + i as i32) as i8);
                    write_storage(&mut w, &[air], |_| 0);
                }
            }
        }
        let mut below = PLAINS as u32;
        for slot in &self.sections {
            match slot {
                Some(_) if debug_flag("plains") => write_storage(&mut w, &[PLAINS as i32], |_| 0),
                Some(s) => {
                    // 4096 celle: ogni cella di bioma 4x4x4 vale per i suoi 64 blocchi, ordine (x*16 + z)*16 + y
                    let mut palette: Vec<u32> = Vec::new();
                    let idx: Vec<usize> =
                        s.biomes.iter().map(|&b| palette.iter().position(|&p| p == b).unwrap_or_else(|| (palette.push(b), palette.len() - 1).1)).collect();
                    let ids: Vec<i32> = palette.iter().map(|&b| b as i32).collect();
                    write_storage(&mut w, &ids, |n| {
                        let (x, z, y) = (n / 256, n / 16 % 16, n % 16);
                        idx[(x / 4 * 4 + z / 4) * 4 + y / 4]
                    });
                    below = s.biomes[3];
                }
                None => write_storage(&mut w, &[below as i32], |_| 0),
            }
        }
        w.u8(0); // blocchi di bordo
        if !debug_flag("nobe") {
            self.block_entities.iter().for_each(|be| w.bytes(&be.nbt));
        }
        w.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Riferimento: il vecchio mondo piatto scritto a mano (prima sezione, biomi, bordi), solo v9.
    fn reference_flat_chunk() -> Vec<u8> {
        let palette = [
            block_hash("minecraft:air", &[]),
            block_hash("minecraft:bedrock", &[("infiniburn_bit", State::Byte(0))]),
            block_hash("minecraft:dirt", &[]),
            block_hash("minecraft:grass_block", &[]),
        ];
        let mut words = [0u32; 4096 / 16];
        for column in 0..256 {
            for (y, id) in [(0, 1), (1, 2), (2, 2), (3, 3)] {
                let i = column * 16 + y;
                words[i / 16] |= id << ((i % 16) * 2);
            }
        }
        let mut w = Writer::default();
        w.u8(9);
        w.u8(1);
        w.i8(-4);
        w.u8((2 << 1) | 1);
        words.iter().for_each(|&word| w.u32(word));
        w.var_i32(palette.len() as i32);
        palette.iter().for_each(|&id| w.var_i32(id));
        for _ in 0..24 {
            w.u8(1);
            w.var_i32(PLAINS as i32);
        }
        w.u8(0);
        w.0
    }

    fn section(index: i8, palette: Vec<i32>, blocks: Box<[u16; 4096]>) -> Section {
        Section { index, palette, blocks, biomes: Box::new([PLAINS as u32; 64]), water: None }
    }

    fn chunk_with(sections: Vec<Section>) -> Chunk {
        Chunk::from_gen(ChunkData { dim: Dim::Overworld, x: 0, z: 0, sections, block_entities: vec![] })
    }

    /// Intestazione dello strato blocchi di una sezione presente: byte dei bit per blocco (dopo versione, strati, indice).
    fn bits_byte(c: &Chunk) -> u8 {
        c.network_bytes()[3] >> 1
    }

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
    fn flat_chunk_bytes_are_unchanged() {
        let c = Chunk::flat(Dim::Overworld, 3, -2);
        assert_eq!(c.sub_chunk_count(), 1);
        assert_eq!(c.network_bytes(), reference_flat_chunk());
    }

    #[test]
    fn bits_per_block_follow_palette_size() {
        for (len, bits) in [(1, 0), (2, 1), (3, 2), (4, 2), (5, 3), (9, 4), (17, 5), (33, 6), (65, 8), (256, 8), (257, 16), (300, 16)] {
            assert_eq!(bits_for_palette(len), bits, "palette di {len}");
        }
        for (len, bits) in [(2usize, 1u8), (17, 5), (300, 16)] {
            let mut blocks = Box::new([0u16; 4096]);
            blocks.iter_mut().enumerate().for_each(|(i, b)| *b = (i % len) as u16);
            let c = chunk_with(vec![section(-4, (0..len as i32).collect(), blocks)]);
            assert_eq!(bits_byte(&c), bits);
        }
    }

    #[test]
    fn packing_does_not_straddle_words() {
        // 5 bit: 6 voci per parola, 683 parole
        let mut blocks = Box::new([0u16; 4096]);
        blocks.iter_mut().enumerate().for_each(|(i, b)| *b = (i % 17) as u16);
        let c = chunk_with(vec![section(-4, (0..17).collect(), blocks)]);
        let b = c.network_bytes();
        let words = |n: usize| u32::from_le_bytes(b[4 + n * 4..8 + n * 4].try_into().unwrap());
        assert_eq!(words(0), (0..6).fold(0u32, |a, k| a | (k << (k * 5))));
        assert_eq!(words(0) >> 30, 0, "i 2 bit alti restano vuoti");
        let palette_at = 4 + 683 * 4;
        assert_eq!(b[palette_at], 34, "17 voci in ZigZag");
    }

    #[test]
    fn uniform_section_uses_zero_bits() {
        let c = chunk_with(vec![section(-4, vec![42], Box::new([0u16; 4096]))]);
        let b = c.network_bytes();
        assert_eq!(&b[..4], &[9, 1, 0xfc, 1], "0 bit, bit basso = ID di rete");
        assert_eq!(b[4], 84, "subito la voce unica (42 in ZigZag), nessuna parola né conteggio");
    }

    #[test]
    fn biomes_are_written_per_section() {
        let air = block_hash("minecraft:air", &[]);
        let mut s0 = section(-4, vec![air], Box::new([0u16; 4096]));
        s0.biomes[3] = 7; // cella in alto di (0,0)
        s0.biomes[0] = 5;
        let s2 = section(-2, vec![air], Box::new([0u16; 4096])); // pianura
        let c = chunk_with(vec![s0, s2]);
        assert_eq!(c.sub_chunk_count(), 3, "fino alla sezione -2 inclusa");
        let b = c.network_bytes();
        // tre sotto-chunk: 4 byte di intestazione + la voce d'aria (hash in ZigZag)
        let hash = {
            let mut w = Writer::default();
            w.var_i32(air);
            w.0.len()
        };
        let sub = 4 + hash;
        let bio = &b[3 * sub..];
        assert_eq!(bio[0], (2 << 1) | 1, "sezione -4: tre biomi distinti, 2 bit");
        let after0 = 1 + 4096usize.div_ceil(16) * 4 + 1 + 3; // intestazione + 256 parole + n voci + 3 voci da 1 byte
        assert_eq!(&bio[after0 - 3..after0], &[10, 2, 14], "palette 5, 1, 7");
        assert_eq!(&bio[after0..after0 + 4], &[1, 14, 1, 2], "-3 mancante: ripete l'ultima cella della -4 (7); -2: pianura");
    }

    #[test]
    fn water_mask_adds_second_layer() {
        let stone = block_hash("minecraft:stone", &[]);
        let mut s = section(-4, vec![stone], Box::new([0u16; 4096]));
        let mut mask = Box::new([0u64; 64]);
        mask[0] = 1 << 5; // blocco 5
        s.water = Some(mask);
        let b = chunk_with(vec![s]).network_bytes();
        assert_eq!(&b[..3], &[9, 2, 0xfc], "due strati");
        // strato 0: intestazione a 0 bit + voce unica; strato 1 parte dopo
        let l1 = 3 + 1 + {
            let mut w = Writer::default();
            w.var_i32(stone);
            w.0.len()
        };
        assert_eq!(b[l1], (1 << 1) | 1, "1 bit");
        let word0 = u32::from_le_bytes(b[l1 + 1..l1 + 5].try_into().unwrap());
        assert_eq!(word0, 1 << 5, "bit impostato -> indice 1");
        let p = l1 + 1 + 128 * 4;
        assert_eq!(b[p], 4, "due voci in ZigZag");
        let mut w = Writer::default();
        w.var_i32(block_hash("minecraft:air", &[]));
        w.var_i32(block_hash("minecraft:water", &[("liquid_depth", State::Int(0))]));
        assert_eq!(&b[p + 1..p + 1 + w.0.len()], &w.0[..]);
    }

    /// Decodifica i blocchi (strato 0) della prima sezione presente di `network_bytes`: 4096 hash, ordine (x*16 + z)*16 + y.
    fn decode_layer0(bytes: &[u8]) -> Vec<i32> {
        let mut r = bedrock_protocol::reader::Reader(bytes);
        assert_eq!(r.u8(), Some(9), "sotto-chunk v9");
        r.u8().unwrap(); // numero di strati
        r.i8().unwrap(); // indice della sezione
        decode_storage(&mut r)
    }

    fn decode_storage(r: &mut bedrock_protocol::reader::Reader) -> Vec<i32> {
        let head = r.u8().unwrap();
        let bits = (head >> 1) as usize;
        if bits == 0 {
            return vec![r.var_i32().unwrap(); 4096];
        }
        let per_word = 32 / bits;
        let words: Vec<u32> = (0..4096usize.div_ceil(per_word)).map(|_| r.u32().unwrap()).collect();
        let n = r.var_i32().unwrap() as usize;
        let palette: Vec<i32> = (0..n).map(|_| r.var_i32().unwrap()).collect();
        (0..4096).map(|i| palette[(words[i / per_word] >> ((i % per_word) * bits) & ((1 << bits) - 1)) as usize]).collect()
    }

    #[test]
    fn set_block_in_missing_section_creates_it() {
        let mut c = Chunk::flat(Dim::Overworld, 0, 0);
        let stone = block_hash("minecraft:stone", &[]);
        assert!(c.sections[5].is_none());
        assert_eq!(c.block(3, 20, 4), block_hash("minecraft:air", &[]), "sezione assente = aria");
        c.set_block(3, 20, 4, stone, false); // y=20 sta nella sezione 1, slot 5
        let s = c.sections[5].as_ref().expect("sezione creata");
        assert_eq!(s.index, 1);
        assert_eq!(s.palette[0], block_hash("minecraft:air", &[]));
        assert_eq!(c.block(3, 20, 4), stone);
        assert_eq!(c.block(3, 21, 4), block_hash("minecraft:air", &[]));
        assert!(c.dirty);
        assert_eq!(s.blocks[(3 * 16 + 4) * 16 + 4], 1, "indice (x*16 + z)*16 + y con y locale 4");
    }

    #[test]
    fn set_block_grows_palette_and_bits() {
        let mut c = Chunk::flat(Dim::Overworld, 0, 0);
        assert_eq!(bits_byte(&c), 2, "4 voci: 2 bit");
        for k in 0..13 {
            c.set_block(k, -50, 0, 1000 + k as i32, false); // 4 + 13 = 17 voci
        }
        assert_eq!(c.sections[0].as_ref().unwrap().palette.len(), 17);
        assert_eq!(bits_byte(&c), 5, "17 voci: 5 bit");
        let decoded = decode_layer0(&c.network_bytes());
        for k in 0..13usize {
            assert_eq!(decoded[(k * 16) * 16 + 14], 1000 + k as i32);
        }
        assert_eq!(decoded[3], block_hash("minecraft:grass_block", &[]), "il resto è intatto");
        c.set_block(0, -50, 0, 1000, false); // un valore già in palette non la allunga
        assert_eq!(c.sections[0].as_ref().unwrap().palette.len(), 17);
    }

    #[test]
    fn waterlogged_block_keeps_water() {
        let mut c = Chunk::flat(Dim::Overworld, 0, 0);
        let slab = block_hash("minecraft:stone", &[]);
        c.set_block(1, -62, 3, slab, true);
        assert_eq!((c.block(1, -62, 3), c.water(1, -62, 3)), (slab, true));
        assert!(!c.water(1, -61, 3));
        c.set_block(1, -62, 3, block_hash("minecraft:dirt", &[]), true); // cambia il blocco: l'acqua resta
        assert!(c.water(1, -62, 3));
        let b = c.network_bytes();
        assert_eq!(b[1], 2, "due strati");
        c.set_block(1, -62, 3, slab, false);
        assert!(!c.water(1, -62, 3));
        assert!(c.sections[0].as_ref().unwrap().water.is_none(), "senza più acqua la maschera sparisce");
        assert_eq!(c.network_bytes()[1], 1);
    }
}
