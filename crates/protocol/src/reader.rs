//! Lettura dei tipi primitivi del protocollo Bedrock (speculare di `wire::Writer`).
//! Ogni metodo avanza il cursore e restituisce `None` quando i byte finiscono (o il valore è malformato).
use crate::varint;

/// Cursore su un corpo di pacchetto.
pub struct Reader<'a>(pub &'a [u8]);

macro_rules! fixed {
    ($($name:ident: $ty:ty),*) => {$(
        pub fn $name(&mut self) -> Option<$ty> {
            const N: usize = std::mem::size_of::<$ty>();
            let (head, rest) = self.0.split_first_chunk::<N>()?;
            self.0 = rest;
            Some(<$ty>::from_le_bytes(*head))
        }
    )*};
}

impl<'a> Reader<'a> {
    fixed!(u8: u8, i8: i8, u16: u16, i16: i16, u32: u32, i32: i32, u64: u64, i64: i64, f32: f32);

    pub fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }

    /// Byte non ancora letti.
    pub fn remaining(&self) -> usize {
        self.0.len()
    }

    pub fn var_u32(&mut self) -> Option<u32> {
        varint::read(&mut self.0)
    }

    pub fn var_i32(&mut self) -> Option<i32> {
        varint::read_signed(&mut self.0)
    }

    pub fn var_u64(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for i in 0..10 {
            let x = self.u8()?;
            v |= ((x & 0x7f) as u64) << (7 * i);
            if x & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    pub fn var_i64(&mut self) -> Option<i64> {
        let v = self.var_u64()?;
        Some(((v >> 1) as i64) ^ -((v & 1) as i64))
    }

    /// Blocco di byte con lunghezza VarUInt32. La lunghezza è controllata contro i byte rimasti.
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.var_u32()? as usize;
        let (head, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(head)
    }

    /// Stringa UTF-8 con lunghezza VarUInt32.
    pub fn string(&mut self) -> Option<&'a str> {
        std::str::from_utf8(self.bytes()?).ok()
    }

    /// Posizione di blocco: tre VarInt32 (anche Y).
    pub fn block_pos(&mut self) -> Option<[i32; 3]> {
        Some([self.var_i32()?, self.var_i32()?, self.var_i32()?])
    }

    pub fn vec3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }

    /// Valore opzionale: byte di presenza, poi il valore se presente. `None` esterno = dati finiti,
    /// `Some(None)` = assente.
    pub fn optional<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<Option<T>> {
        if self.bool()? {
            Some(Some(f(self)?))
        } else {
            Some(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Writer;

    #[test]
    fn primitives_round_trip() {
        let mut w = Writer::default();
        w.u8(7);
        w.i8(-3);
        w.bool(true);
        w.i16(-2);
        w.u16(513);
        w.i32(-100_000);
        w.u32(4_000_000_000);
        w.u64(u64::MAX - 1);
        w.i64(-5);
        w.f32(1.5);
        w.var_u32(300);
        w.var_i32(-300);
        w.var_u64(u64::MAX);
        w.var_i64(i64::MIN);
        w.string("ciao");
        w.bytes(&[1, 2, 3]);
        let mut r = Reader(&w.0);
        assert_eq!(r.u8(), Some(7));
        assert_eq!(r.i8(), Some(-3));
        assert_eq!(r.bool(), Some(true));
        assert_eq!(r.i16(), Some(-2));
        assert_eq!(r.u16(), Some(513));
        assert_eq!(r.i32(), Some(-100_000));
        assert_eq!(r.u32(), Some(4_000_000_000));
        assert_eq!(r.u64(), Some(u64::MAX - 1));
        assert_eq!(r.i64(), Some(-5));
        assert_eq!(r.f32(), Some(1.5));
        assert_eq!(r.var_u32(), Some(300));
        assert_eq!(r.var_i32(), Some(-300));
        assert_eq!(r.var_u64(), Some(u64::MAX));
        assert_eq!(r.var_i64(), Some(i64::MIN));
        assert_eq!(r.string(), Some("ciao"));
        assert_eq!(r.bytes(), Some(&[1u8, 2, 3][..]));
        assert_eq!(r.remaining(), 0);
        assert_eq!(r.u8(), None);
    }

    #[test]
    fn composite_types() {
        let mut w = Writer::default();
        w.var_i32(-1);
        w.var_i32(64);
        w.var_i32(i32::MAX);
        w.f32(1.0);
        w.f32(2.0);
        w.f32(3.0);
        w.u8(1);
        w.u16(9);
        w.u8(0);
        let mut r = Reader(&w.0);
        assert_eq!(r.block_pos(), Some([-1, 64, i32::MAX]));
        assert_eq!(r.vec3(), Some([1.0, 2.0, 3.0]));
        assert_eq!(r.optional(|r| r.u16()), Some(Some(9)));
        assert_eq!(r.optional(|r| r.u16()), Some(None));
        assert_eq!(r.optional(|r| r.u16()), None);
    }

    #[test]
    fn truncated_inputs_give_none() {
        assert_eq!(Reader(&[1]).u16(), None);
        assert_eq!(Reader(&[0x80]).var_u32(), None);
        assert_eq!(Reader(&[0x80; 10]).var_u64(), None);
        assert_eq!(Reader(&[5, 1, 2]).bytes(), None); // lunghezza oltre i dati
        assert_eq!(Reader(&[2, 0xff, 0xfe]).string(), None); // UTF-8 non valido
        assert_eq!(Reader(&[1]).optional(|r| r.u8()), None); // presente ma senza valore
    }
}
