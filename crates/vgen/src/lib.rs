//! Protocollo locale (TCP) tra il server Bedrock e il processo generatore del mondo.
//! Solo std, nessuna dipendenza. Formato: interi little-endian, `Vec` preceduti da `u32` di lunghezza,
//! stringhe UTF-8 con lunghezza, un byte di tag per variante. Un frame è `u32` LE di lunghezza + corpo.

use std::io::{self, Read, Write};

/// Versione del protocollo scambiata nell'Hello.
pub const VERSION: u16 = 1;
/// Dimensione massima di un frame accettata da `read_frame`.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

const BLOCKS_PER_SECTION: usize = 4096;
const BIOMES_PER_SECTION: usize = 64;
const WATER_WORDS: usize = 4096 / 64;

/// Dimensione del mondo; il valore numerico è quello sul filo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Dim {
    Overworld = 0,
    Nether = 1,
    End = 2,
}

impl Dim {
    /// Intervallo verticale `(min_y, max_y)` della dimensione.
    pub fn height_range(self) -> (i32, i32) {
        match self {
            Dim::Overworld => (-64, 320),
            Dim::Nether => (0, 128),
            Dim::End => (0, 256),
        }
    }
}

impl TryFrom<u8> for Dim {
    type Error = ();
    fn try_from(v: u8) -> Result<Self, ()> {
        match v {
            0 => Ok(Dim::Overworld),
            1 => Ok(Dim::Nether),
            2 => Ok(Dim::End),
            _ => Err(()),
        }
    }
}

/// Sezione 16x16x16. `blocks`: indici nella palette, ordine `(x*16 + z)*16 + y`; `biomes`: ordine `(x*4 + z)*4 + y`.
/// `water`: maschera di 4096 bit dei blocchi con acqua nel secondo strato (stesso ordine dei blocchi; bit `i` = parola `i/64`,
/// bit `i%64`), `None` se nessun blocco della sezione ne ha.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub index: i8,
    pub palette: Vec<i32>,
    pub blocks: Box<[u16; 4096]>,
    pub biomes: Box<[u32; 64]>,
    pub water: Option<Box<[u64; 64]>>,
}

/// Block entity con NBT di rete Bedrock (little-endian con varint).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEntity {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub nbt: Vec<u8>,
}

/// Chunk generato; le sezioni di sola aria sono omesse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkData {
    pub dim: Dim,
    pub x: i32,
    pub z: i32,
    pub sections: Vec<Section>,
    pub block_entities: Vec<BlockEntity>,
}

/// Messaggi server -> generatore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToGen {
    Hello { version: u16, seed: u64 },
    Generate { req: u32, dim: Dim, x: i32, z: i32 },
    Spawn { req: u32, dim: Dim },
}

/// Messaggi generatore -> server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromGen {
    Hello {
        version: u16,
    },
    Chunk {
        req: u32,
        data: ChunkData,
    },
    Spawn {
        req: u32,
        x: i32,
        y: i32,
        z: i32,
    },
    Error {
        req: u32,
        message: String,
    },
    /// Inviata una volta dopo l'Hello: (hash dello stato Bedrock, NBT su disco little-endian di `{name, states}`).
    Palette {
        entries: Vec<(i32, Vec<u8>)>,
    },
}

// ---------- scrittura ----------

fn put_len(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_len(out, b.len());
    out.extend_from_slice(b);
}

fn put_chunk(out: &mut Vec<u8>, c: &ChunkData) {
    out.push(c.dim as u8);
    out.extend_from_slice(&c.x.to_le_bytes());
    out.extend_from_slice(&c.z.to_le_bytes());
    put_len(out, c.sections.len());
    for s in &c.sections {
        out.push(s.index as u8);
        put_len(out, s.palette.len());
        for p in &s.palette {
            out.extend_from_slice(&p.to_le_bytes());
        }
        for b in s.blocks.iter() {
            out.extend_from_slice(&b.to_le_bytes());
        }
        for b in s.biomes.iter() {
            out.extend_from_slice(&b.to_le_bytes());
        }
        match &s.water {
            Some(mask) => {
                out.push(1);
                for w in mask.iter() {
                    out.extend_from_slice(&w.to_le_bytes());
                }
            }
            None => out.push(0),
        }
    }
    put_len(out, c.block_entities.len());
    for e in &c.block_entities {
        out.extend_from_slice(&e.x.to_le_bytes());
        out.extend_from_slice(&e.y.to_le_bytes());
        out.extend_from_slice(&e.z.to_le_bytes());
        put_bytes(out, &e.nbt);
    }
}

// ---------- lettura ----------

/// Lettore con controllo dei limiti: ogni lunghezza è validata contro i byte rimasti prima di allocare.
struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.b.len() {
            return None;
        }
        let (h, t) = self.b.split_at(n);
        self.b = t;
        Some(h)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
    }
    fn dim(&mut self) -> Option<Dim> {
        Dim::try_from(self.u8()?).ok()
    }
    /// Legge un conteggio di elementi da almeno `min_size` byte ciascuno e verifica che ci stiano ancora.
    fn count(&mut self, min_size: usize) -> Option<usize> {
        let n = self.u32()? as usize;
        if n.checked_mul(min_size)? > self.b.len() {
            return None;
        }
        Some(n)
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.count(1)?;
        Some(self.take(n)?.to_vec())
    }
    fn finish<T>(self, v: T) -> Option<T> {
        self.b.is_empty().then_some(v)
    }
}

fn read_chunk(r: &mut Reader) -> Option<ChunkData> {
    let dim = r.dim()?;
    let x = r.i32()?;
    let z = r.i32()?;
    let n = r.count(1 + 4 + BLOCKS_PER_SECTION * 2 + BIOMES_PER_SECTION * 4 + 1)?;
    let mut sections = Vec::with_capacity(n);
    for _ in 0..n {
        let index = r.u8()? as i8;
        let np = r.count(4)?;
        let mut palette = Vec::with_capacity(np);
        for _ in 0..np {
            palette.push(r.i32()?);
        }
        let mut blocks = Box::new([0u16; BLOCKS_PER_SECTION]);
        for b in blocks.iter_mut() {
            *b = r.u16()?;
        }
        let mut biomes = Box::new([0u32; BIOMES_PER_SECTION]);
        for b in biomes.iter_mut() {
            *b = r.u32()?;
        }
        let water = match r.u8()? {
            0 => None,
            1 => {
                let mut mask = Box::new([0u64; WATER_WORDS]);
                for w in mask.iter_mut() {
                    *w = r.u64()?;
                }
                Some(mask)
            }
            _ => return None,
        };
        sections.push(Section { index, palette, blocks, biomes, water });
    }
    let ne = r.count(16)?;
    let mut block_entities = Vec::with_capacity(ne);
    for _ in 0..ne {
        let (x, y, z) = (r.i32()?, r.i32()?, r.i32()?);
        block_entities.push(BlockEntity { x, y, z, nbt: r.bytes()? });
    }
    Some(ChunkData { dim, x, z, sections, block_entities })
}

impl ToGen {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        match self {
            ToGen::Hello { version, seed } => {
                o.push(0);
                o.extend_from_slice(&version.to_le_bytes());
                o.extend_from_slice(&seed.to_le_bytes());
            }
            ToGen::Generate { req, dim, x, z } => {
                o.push(1);
                o.extend_from_slice(&req.to_le_bytes());
                o.push(*dim as u8);
                o.extend_from_slice(&x.to_le_bytes());
                o.extend_from_slice(&z.to_le_bytes());
            }
            ToGen::Spawn { req, dim } => {
                o.push(2);
                o.extend_from_slice(&req.to_le_bytes());
                o.push(*dim as u8);
            }
        }
        o
    }

    pub fn decode(b: &[u8]) -> Option<ToGen> {
        let mut r = Reader { b };
        let m = match r.u8()? {
            0 => ToGen::Hello { version: r.u16()?, seed: r.u64()? },
            1 => ToGen::Generate { req: r.u32()?, dim: r.dim()?, x: r.i32()?, z: r.i32()? },
            2 => ToGen::Spawn { req: r.u32()?, dim: r.dim()? },
            _ => return None,
        };
        r.finish(m)
    }
}

impl FromGen {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        match self {
            FromGen::Hello { version } => {
                o.push(0);
                o.extend_from_slice(&version.to_le_bytes());
            }
            FromGen::Chunk { req, data } => {
                o.push(1);
                o.extend_from_slice(&req.to_le_bytes());
                put_chunk(&mut o, data);
            }
            FromGen::Spawn { req, x, y, z } => {
                o.push(2);
                o.extend_from_slice(&req.to_le_bytes());
                for v in [x, y, z] {
                    o.extend_from_slice(&v.to_le_bytes());
                }
            }
            FromGen::Error { req, message } => {
                o.push(3);
                o.extend_from_slice(&req.to_le_bytes());
                put_bytes(&mut o, message.as_bytes());
            }
            FromGen::Palette { entries } => {
                o.push(4);
                put_len(&mut o, entries.len());
                for (h, nbt) in entries {
                    o.extend_from_slice(&h.to_le_bytes());
                    put_bytes(&mut o, nbt);
                }
            }
        }
        o
    }

    pub fn decode(b: &[u8]) -> Option<FromGen> {
        let mut r = Reader { b };
        let m = match r.u8()? {
            0 => FromGen::Hello { version: r.u16()? },
            1 => {
                let req = r.u32()?;
                FromGen::Chunk { req, data: read_chunk(&mut r)? }
            }
            2 => FromGen::Spawn { req: r.u32()?, x: r.i32()?, y: r.i32()?, z: r.i32()? },
            3 => {
                let req = r.u32()?;
                FromGen::Error { req, message: String::from_utf8(r.bytes()?).ok()? }
            }
            4 => {
                let n = r.count(8)?;
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let h = r.i32()?;
                    entries.push((h, r.bytes()?));
                }
                FromGen::Palette { entries }
            }
            _ => return None,
        };
        r.finish(m)
    }
}

/// Legge un frame (`u32` LE di lunghezza + corpo). Rifiuta oltre `MAX_FRAME` senza allocare.
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame troppo grande"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// Scrive un frame (`u32` LE di lunghezza + corpo).
pub fn write_frame<W: Write>(w: &mut W, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame troppo grande"));
    }
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind};

    fn section(index: i8) -> Section {
        let mut blocks = Box::new([0u16; 4096]);
        blocks[1] = 1;
        blocks[4095] = 2;
        let mut biomes = Box::new([0u32; 64]);
        biomes[63] = 7;
        Section { index, palette: vec![10, -20, 30], blocks, biomes, water: None }
    }

    /// Sezione con acqua nel secondo strato in alcuni blocchi.
    fn watery(index: i8) -> Section {
        let mut water = Box::new([0u64; 64]);
        water[0] = 1 << 5;
        water[63] = 1 << 63;
        Section { water: Some(water), ..section(index) }
    }

    fn chunk() -> ChunkData {
        ChunkData {
            dim: Dim::Nether,
            x: -3,
            z: 9,
            sections: vec![section(-4), watery(5)],
            block_entities: vec![BlockEntity { x: 1, y: 2, z: 3, nbt: vec![1, 2, 3] }],
        }
    }

    #[test]
    fn roundtrip_every_message() {
        let to = [
            ToGen::Hello { version: VERSION, seed: u64::MAX },
            ToGen::Generate { req: 5, dim: Dim::End, x: -1, z: 2 },
            ToGen::Spawn { req: 6, dim: Dim::Overworld },
        ];
        for m in to {
            assert_eq!(ToGen::decode(&m.encode()), Some(m));
        }
        let from = [
            FromGen::Hello { version: 1 },
            FromGen::Chunk { req: 1, data: chunk() },
            FromGen::Spawn { req: 2, x: -5, y: 70, z: 8 },
            FromGen::Error { req: 3, message: "errore àè".into() },
            FromGen::Palette { entries: vec![(i32::MIN, vec![1, 2]), (7, vec![])] },
        ];
        for m in from {
            assert_eq!(FromGen::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn chunk_with_two_sections_roundtrips() {
        let m = FromGen::Chunk { req: 9, data: chunk() };
        let FromGen::Chunk { data, .. } = FromGen::decode(&m.encode()).unwrap() else { panic!() };
        assert_eq!(data.sections.len(), 2);
        assert_eq!(data.sections[1].blocks[4095], 2);
        assert_eq!(data.sections[0].biomes[63], 7);
    }

    #[test]
    fn decode_rejects_truncated_and_trailing_bytes() {
        let bytes = FromGen::Chunk { req: 1, data: chunk() }.encode();
        for n in 0..bytes.len() {
            assert!(FromGen::decode(&bytes[..n]).is_none(), "len {n}");
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(FromGen::decode(&extra).is_none());
        let mut extra = ToGen::Spawn { req: 1, dim: Dim::End }.encode();
        extra.push(0);
        assert!(ToGen::decode(&extra).is_none());
        assert!(ToGen::decode(&[]).is_none());
        assert!(ToGen::decode(&[99]).is_none());
    }

    #[test]
    fn decode_rejects_hostile_lengths_and_bad_sizes() {
        // Palette con un miliardo di voci ma 10 byte rimasti
        let mut b = vec![4u8];
        b.extend_from_slice(&1_000_000_000u32.to_le_bytes());
        b.extend_from_slice(&[0; 10]);
        assert!(FromGen::decode(&b).is_none());
        // Error con stringa di lunghezza enorme
        let mut b = vec![3u8, 0, 0, 0, 0];
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(FromGen::decode(&b).is_none());
        // Sezione con blocks diversi da 4096 non e' codificabile: la decodifica rifiuta lunghezze errate
        let mut c = chunk();
        c.sections.truncate(1);
        let mut b = FromGen::Chunk { req: 1, data: c }.encode();
        // dim(1) x(4) z(4) count(4) index(1) = offset del palette len: 1+4+1+4+4+4+1
        let off = 1 + 4 + 1 + 4 + 4 + 4 + 1;
        b.drain(off + 4 + 12..off + 4 + 12 + 2); // rimuove un u16 di blocks
        assert!(FromGen::decode(&b).is_none());
    }

    #[test]
    fn water_mask_roundtrips_with_and_without_water() {
        let FromGen::Chunk { data, .. } = FromGen::decode(&FromGen::Chunk { req: 1, data: chunk() }.encode()).unwrap() else { panic!() };
        assert_eq!(data.sections[0].water, None);
        let water = data.sections[1].water.as_ref().expect("maschera dell'acqua");
        assert_eq!((water[0], water[63]), (1 << 5, 1 << 63));
    }

    #[test]
    fn decode_rejects_truncated_water_mask_and_bad_presence_byte() {
        let mut c = chunk();
        c.sections.remove(0);
        c.block_entities.clear();
        let bytes = FromGen::Chunk { req: 1, data: c }.encode();
        // In fondo: maschera (512 byte) + numero di block entity (4 byte).
        let mask_end = bytes.len() - 4;
        let mut cut = bytes[..mask_end - 8].to_vec();
        cut.extend_from_slice(&0u32.to_le_bytes());
        assert!(FromGen::decode(&cut).is_none());
        let mut bad = bytes.clone();
        bad[mask_end - 64 * 8 - 1] = 2; // byte di presenza non valido
        assert!(FromGen::decode(&bad).is_none());
        assert!(FromGen::decode(&bytes).is_some());
    }

    #[test]
    fn dim_height_ranges() {
        assert_eq!(Dim::Overworld.height_range(), (-64, 320));
        assert_eq!(Dim::Nether.height_range(), (0, 128));
        assert_eq!(Dim::End.height_range(), (0, 256));
    }

    #[test]
    fn dim_wire_byte_is_stable() {
        for (d, v) in [(Dim::Overworld, 0u8), (Dim::Nether, 1), (Dim::End, 2)] {
            assert_eq!(d as u8, v);
            assert_eq!(Dim::try_from(v), Ok(d));
        }
        assert!(Dim::try_from(3u8).is_err());
        let mut b = ToGen::Spawn { req: 1, dim: Dim::End }.encode();
        *b.last_mut().unwrap() = 3;
        assert!(ToGen::decode(&b).is_none());
    }

    #[test]
    fn read_frame_rejects_oversized_length() {
        let mut r = Cursor::new(u32::MAX.to_le_bytes().to_vec());
        assert_eq!(read_frame(&mut r).unwrap_err().kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn frame_roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &[1, 2, 3]).unwrap();
        assert_eq!(buf, [3, 0, 0, 0, 1, 2, 3]);
        assert_eq!(read_frame(&mut Cursor::new(buf)).unwrap(), vec![1, 2, 3]);
    }
}
