//! Scrittura dei tipi primitivi del protocollo Bedrock. Usato dal codice generato (`generated.rs`).
use crate::{varint, Packet};

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

macro_rules! fixed {
    ($($name:ident: $ty:ty),*) => {$(
        pub fn $name(&mut self, v: $ty) { self.0.extend_from_slice(&v.to_le_bytes()) }
    )*};
}
macro_rules! fixed_be {
    ($($name:ident: $ty:ty),*) => {$(
        pub fn $name(&mut self, v: $ty) { self.0.extend_from_slice(&v.to_be_bytes()) }
    )*};
}

impl Writer {
    fixed!(u8: u8, i8: i8, u16: u16, i16: i16, u32: u32, i32: i32, u64: u64, i64: i64, f32: f32, f64: f64);
    fixed_be!(u16_be: u16, i16_be: i16, u32_be: u32, i32_be: i32, u64_be: u64, i64_be: i64);

    pub fn bool(&mut self, v: bool) {
        self.0.push(v as u8);
    }

    /// VarUInt32 / VarInt32 (ZigZag) / VarUInt64 / VarInt64 (ZigZag).
    pub fn var_u32(&mut self, v: u32) {
        varint::write(&mut self.0, v);
    }
    pub fn var_i32(&mut self, v: i32) {
        varint::write_signed(&mut self.0, v);
    }
    pub fn var_u64(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.0.push(v as u8 | 0x80);
            v >>= 7;
        }
        self.0.push(v as u8);
    }
    pub fn var_i64(&mut self, v: i64) {
        self.var_u64(((v << 1) ^ (v >> 63)) as u64);
    }

    /// Stringa con lunghezza VarUInt32.
    pub fn string(&mut self, s: &str) {
        self.var_u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }

    /// Dati binari con lunghezza VarUInt32.
    pub fn bytes(&mut self, b: &[u8]) {
        self.var_u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }

    /// Byte già codificati (NBT, ecc.).
    pub fn raw(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// Ogni struct generata sa scriversi.
pub trait Encode {
    fn encode(&self, w: &mut Writer);

    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::default();
        self.encode(&mut w);
        w.0
    }
}

/// Un pacchetto server -> client: ha un ID e diventa un `Packet` pronto per il batch.
pub trait ServerPacket: Encode {
    const ID: u32;

    fn packet(&self) -> Packet {
        Packet::new(Self::ID, self.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives() {
        let mut w = Writer::default();
        w.u16(0x0102);
        w.i32_be(-2);
        w.var_i64(-1);
        w.var_u64(300);
        w.string("ab");
        assert_eq!(w.0, [2, 1, 0xff, 0xff, 0xff, 0xfe, 1, 0xac, 2, 2, b'a', b'b']);
    }
}
