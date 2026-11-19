//! NBT minimale di Bedrock, in due codifiche: su disco (little-endian, lunghezze fisse) e di rete (little-endian con varint).
//! Serve a salvare `level.dat`, le palette dei sotto-chunk e le block entity, e a ricavare l'hash di uno stato di blocco.
use blackstone_protocol::wire::Writer;

/// Profondità massima di annidamento accettata in lettura (protegge lo stack da dati malevoli o corrotti).
const MAX_DEPTH: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enc {
    /// File e database: interi fissi, stringhe con lunghezza `u16`.
    Disk,
    /// Pacchetti: interi in ZigZag varint, stringhe con lunghezza varint.
    Network,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    /// Tipo degli elementi e valori (il tipo serve anche a liste vuote).
    List(u8, Vec<Tag>),
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(..) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    /// Valore di un campo di un compound.
    pub fn get(&self, key: &str) -> Option<&Tag> {
        match self {
            Tag::Compound(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Imposta (o aggiunge) un campo di un compound, mantenendo l'ordine di quelli già presenti.
    pub fn set(&mut self, key: &str, value: Tag) {
        if let Tag::Compound(items) = self {
            match items.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = value,
                None => items.push((key.to_string(), value)),
            }
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Tag::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Tag::Long(v) => Some(*v),
            _ => None,
        }
    }
}

// ---------- scrittura ----------

fn put_str(w: &mut Writer, s: &str, enc: Enc) {
    match enc {
        Enc::Disk => w.u16(s.len() as u16),
        Enc::Network => w.var_u32(s.len() as u32),
    }
    w.raw(s.as_bytes());
}

fn put_int(w: &mut Writer, v: i32, enc: Enc) {
    match enc {
        Enc::Disk => w.i32(v),
        Enc::Network => w.var_i32(v),
    }
}

fn put_payload(w: &mut Writer, tag: &Tag, enc: Enc) {
    match tag {
        Tag::Byte(v) => w.i8(*v),
        Tag::Short(v) => w.i16(*v),
        Tag::Int(v) => put_int(w, *v, enc),
        Tag::Long(v) => match enc {
            Enc::Disk => w.i64(*v),
            Enc::Network => w.var_i64(*v),
        },
        Tag::Float(v) => w.f32(*v),
        Tag::Double(v) => w.f64(*v),
        Tag::ByteArray(v) => {
            put_int(w, v.len() as i32, enc);
            w.raw(v);
        }
        Tag::String(s) => put_str(w, s, enc),
        Tag::List(ty, items) => {
            w.u8(*ty);
            put_int(w, items.len() as i32, enc);
            items.iter().for_each(|t| put_payload(w, t, enc));
        }
        Tag::Compound(items) => {
            for (key, value) in items {
                w.u8(value.id());
                put_str(w, key, enc);
                put_payload(w, value, enc);
            }
            w.u8(0);
        }
        Tag::IntArray(v) => {
            put_int(w, v.len() as i32, enc);
            v.iter().for_each(|&i| put_int(w, i, enc));
        }
        Tag::LongArray(v) => {
            put_int(w, v.len() as i32, enc);
            v.iter().for_each(|&i| match enc {
                Enc::Disk => w.i64(i),
                Enc::Network => w.var_i64(i),
            });
        }
    }
}

/// Scrive una radice con nome (di solito vuoto).
pub fn write_root(name: &str, tag: &Tag, enc: Enc) -> Vec<u8> {
    let mut w = Writer::default();
    w.u8(tag.id());
    put_str(&mut w, name, enc);
    put_payload(&mut w, tag, enc);
    w.0
}

// ---------- lettura ----------

/// Cursore sui byte: ogni lunghezza viene controllata contro i byte rimasti prima di allocare.
pub struct Reader<'a> {
    b: &'a [u8],
    enc: Enc,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8], enc: Enc) -> Self {
        Reader { b, enc }
    }

    /// Byte non ancora letti.
    pub fn rest(&self) -> &'a [u8] {
        self.b
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.b.len() {
            return None;
        }
        let (head, tail) = self.b.split_at(n);
        self.b = tail;
        Some(head)
    }

    fn var_u64(&mut self, max_bytes: usize) -> Option<u64> {
        let mut v = 0u64;
        for i in 0..max_bytes {
            let byte = self.take(1)?[0];
            v |= ((byte & 0x7f) as u64) << (7 * i);
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    fn int(&mut self) -> Option<i32> {
        match self.enc {
            Enc::Disk => Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?)),
            Enc::Network => {
                let v = self.var_u64(5)? as u32;
                Some(((v >> 1) as i32) ^ -((v & 1) as i32))
            }
        }
    }

    fn long(&mut self) -> Option<i64> {
        match self.enc {
            Enc::Disk => Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            Enc::Network => {
                let v = self.var_u64(10)?;
                Some(((v >> 1) as i64) ^ -((v & 1) as i64))
            }
        }
    }

    /// Lunghezza di un array o di una lista: non negativa e non più grande dei byte rimasti (ogni elemento pesa almeno un byte
    /// salvo le liste di tag vuoti, che qui non servono).
    fn len(&mut self) -> Option<usize> {
        let n = self.int()?;
        (n >= 0 && n as usize <= self.b.len()).then_some(n as usize)
    }

    fn string(&mut self) -> Option<String> {
        let n = match self.enc {
            Enc::Disk => u16::from_le_bytes(self.take(2)?.try_into().ok()?) as usize,
            Enc::Network => self.var_u64(5)? as usize,
        };
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }

    fn payload(&mut self, ty: u8, depth: usize) -> Option<Tag> {
        if depth > MAX_DEPTH {
            return None;
        }
        Some(match ty {
            1 => Tag::Byte(self.take(1)?[0] as i8),
            2 => Tag::Short(i16::from_le_bytes(self.take(2)?.try_into().ok()?)),
            3 => Tag::Int(self.int()?),
            4 => Tag::Long(self.long()?),
            5 => Tag::Float(f32::from_le_bytes(self.take(4)?.try_into().ok()?)),
            6 => Tag::Double(f64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            7 => {
                let n = self.len()?;
                Tag::ByteArray(self.take(n)?.to_vec())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let elem = self.take(1)?[0];
                let n = self.len()?;
                if elem == 0 && n > 0 {
                    return None;
                }
                let items = (0..n).map(|_| self.payload(elem, depth + 1)).collect::<Option<Vec<_>>>()?;
                Tag::List(elem, items)
            }
            10 => {
                let mut items = Vec::new();
                loop {
                    let ty = self.take(1)?[0];
                    if ty == 0 {
                        break Tag::Compound(items);
                    }
                    let key = self.string()?;
                    items.push((key, self.payload(ty, depth + 1)?));
                }
            }
            11 => {
                let n = self.len()?;
                Tag::IntArray((0..n).map(|_| self.int()).collect::<Option<Vec<_>>>()?)
            }
            12 => {
                let n = self.len()?;
                Tag::LongArray((0..n).map(|_| self.long()).collect::<Option<Vec<_>>>()?)
            }
            _ => return None,
        })
    }

    /// Legge una radice (tipo, nome, valore) e avanza oltre di essa. `None` se i dati sono troncati o non validi.
    pub fn root(&mut self) -> Option<(String, Tag)> {
        let ty = self.take(1)?[0];
        if ty == 0 {
            return None;
        }
        let name = self.string()?;
        Some((name, self.payload(ty, 0)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tag {
        Tag::Compound(vec![
            ("id".into(), Tag::String("Sign".into())),
            ("x".into(), Tag::Int(-5)),
            ("y".into(), Tag::Int(70)),
            ("big".into(), Tag::Long(-(1 << 40))),
            ("flag".into(), Tag::Byte(1)),
            ("f".into(), Tag::Float(1.5)),
            ("d".into(), Tag::Double(-2.25)),
            ("s".into(), Tag::Short(-300)),
            ("bytes".into(), Tag::ByteArray(vec![1, 2, 3])),
            ("ints".into(), Tag::IntArray(vec![-1, 2, 300_000])),
            ("longs".into(), Tag::LongArray(vec![i64::MIN, 7])),
            ("empty".into(), Tag::List(0, vec![])),
            ("items".into(), Tag::List(10, vec![Tag::Compound(vec![("Count".into(), Tag::Byte(3))]), Tag::Compound(vec![])])),
        ])
    }

    #[test]
    fn roundtrips_in_both_encodings() {
        for enc in [Enc::Disk, Enc::Network] {
            let bytes = write_root("", &sample(), enc);
            let mut r = Reader::new(&bytes, enc);
            assert_eq!(r.root(), Some((String::new(), sample())), "{enc:?}");
            assert!(r.rest().is_empty());
        }
    }

    #[test]
    fn network_to_disk_conversion_changes_only_the_encoding() {
        let net = write_root("", &sample(), Enc::Network);
        let (_, tag) = Reader::new(&net, Enc::Network).root().unwrap();
        let disk = write_root("", &tag, Enc::Disk);
        assert_ne!(net, disk);
        let (_, back) = Reader::new(&disk, Enc::Disk).root().unwrap();
        assert_eq!(write_root("", &back, Enc::Network), net);
    }

    #[test]
    fn truncated_or_hostile_input_is_rejected() {
        let bytes = write_root("", &sample(), Enc::Disk);
        for cut in 0..bytes.len() {
            assert!(Reader::new(&bytes[..cut], Enc::Disk).root().is_none(), "troncato a {cut}");
        }
        // array che dichiara 2 miliardi di elementi
        let hostile = [10, 0, 0, 11, 1, 0, b'a', 0xff, 0xff, 0xff, 0x7f, 0];
        assert!(Reader::new(&hostile, Enc::Disk).root().is_none());
        // annidamento oltre il limite
        let mut deep = vec![10u8, 0, 0];
        for _ in 0..100 {
            deep.extend_from_slice(&[10, 1, 0, b'a']);
        }
        assert!(Reader::new(&deep, Enc::Disk).root().is_none());
    }
}
