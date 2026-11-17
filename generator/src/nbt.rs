//! NBT Bedrock little-endian, nelle due varianti:
//! - su disco: lunghezze delle stringhe `u16` LE, interi a larghezza fissa;
//! - di rete (come in LevelChunk): lunghezze delle stringhe varint senza segno,
//!   `Int` e lunghezze delle liste varint zigzag.

/// Un valore NBT (solo i tipi che servono ai block entity Bedrock).
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Int(i32),
    Float(f32),
    Str(String),
    #[cfg_attr(not(test), allow(dead_code))]
    List(Vec<Tag>),
    Compound(Vec<(String, Tag)>),
}

impl Tag {
    /// Valore di una chiave, se `self` è un compound.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get(&self, key: &str) -> Option<&Tag> {
        match self {
            Tag::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Int(_) => 3,
            Tag::Float(_) => 5,
            Tag::Str(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
        }
    }

    /// Scrive `self` come radice senza nome in NBT di rete.
    pub fn to_network(&self) -> Vec<u8> {
        self.to_bytes(true)
    }

    /// Scrive `self` come radice senza nome in NBT su disco.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn to_disk(&self) -> Vec<u8> {
        self.to_bytes(false)
    }

    fn to_bytes(&self, network: bool) -> Vec<u8> {
        let mut w = Writer { out: Vec::new(), network };
        w.out.push(self.id());
        w.string("");
        w.payload(self);
        w.out
    }
}

struct Writer {
    out: Vec<u8>,
    network: bool,
}

impl Writer {
    fn varuint(&mut self, mut v: u32) {
        while v >= 0x80 {
            self.out.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.out.push(v as u8);
    }

    fn int(&mut self, v: i32) {
        if self.network {
            self.varuint(((v << 1) ^ (v >> 31)) as u32);
        } else {
            self.out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn string(&mut self, s: &str) {
        if self.network {
            self.varuint(s.len() as u32);
        } else {
            self.out.extend_from_slice(&(s.len() as u16).to_le_bytes());
        }
        self.out.extend_from_slice(s.as_bytes());
    }

    fn payload(&mut self, tag: &Tag) {
        match tag {
            Tag::Byte(v) => self.out.push(*v as u8),
            Tag::Int(v) => self.int(*v),
            Tag::Float(v) => self.out.extend_from_slice(&v.to_le_bytes()),
            Tag::Str(s) => self.string(s),
            Tag::List(items) => {
                self.out.push(items.first().map_or(0, Tag::id));
                self.int(items.len() as i32);
                for item in items {
                    self.payload(item);
                }
            }
            Tag::Compound(entries) => {
                for (key, value) in entries {
                    self.out.push(value.id());
                    self.string(key);
                    self.payload(value);
                }
                self.out.push(0);
            }
        }
    }
}

/// Legge una radice NBT di rete (solo per i test e la diagnostica).
#[cfg(test)]
pub fn read_network(data: &[u8]) -> Option<Tag> {
    Reader { data, pos: 0, network: true }.root()
}

/// Legge una radice NBT su disco little-endian (solo per i test e la diagnostica).
#[cfg(test)]
pub fn read_disk(data: &[u8]) -> Option<Tag> {
    Reader { data, pos: 0, network: false }.root()
}

#[cfg(test)]
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    network: bool,
}

#[cfg(test)]
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.data.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn varuint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..70).step_by(7) {
            let b = self.u8()?;
            v |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Some(v);
            }
        }
        None
    }

    fn int(&mut self) -> Option<i32> {
        if self.network {
            let z = self.varuint()? as u32;
            Some(((z >> 1) as i32) ^ -((z & 1) as i32))
        } else {
            Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
    }

    fn string(&mut self) -> Option<String> {
        let len = if self.network { self.varuint()? as usize } else { u16::from_le_bytes(self.take(2)?.try_into().ok()?) as usize };
        String::from_utf8(self.take(len)?.to_vec()).ok()
    }

    fn root(mut self) -> Option<Tag> {
        let id = self.u8()?;
        self.string()?;
        let tag = self.payload(id)?;
        (self.pos == self.data.len()).then_some(tag)
    }

    fn payload(&mut self, id: u8) -> Option<Tag> {
        Some(match id {
            1 => Tag::Byte(self.u8()? as i8),
            3 => Tag::Int(self.int()?),
            5 => Tag::Float(f32::from_le_bytes(self.take(4)?.try_into().ok()?)),
            8 => Tag::Str(self.string()?),
            9 => {
                let item = self.u8()?;
                let len = self.int()?;
                let mut items = Vec::new();
                for _ in 0..len {
                    items.push(self.payload(item)?);
                }
                Tag::List(items)
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let id = self.u8()?;
                    if id == 0 {
                        break;
                    }
                    let key = self.string()?;
                    entries.push((key, self.payload(id)?));
                }
                Tag::Compound(entries)
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_round_trip_uses_varints() {
        let tag = Tag::Compound(vec![("id".into(), Tag::Str("Chest".into())), ("x".into(), Tag::Int(-1)), ("l".into(), Tag::List(vec![Tag::Int(300)]))]);
        let bytes = tag.to_network();
        // radice compound, nome vuoto (varint 0), stringa "id" con lunghezza varint 2
        assert_eq!(&bytes[..5], &[10, 0, 8, 2, b'i']);
        assert_eq!(read_network(&bytes), Some(tag.clone()));
        assert_eq!(read_disk(&tag.to_disk()), Some(tag));
    }
}
