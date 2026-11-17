//! VarUInt32: 7 bit per byte, il bit alto indica "continua".

/// Legge un valore e avanza lo slice. `None` se i dati finiscono o il valore è troppo lungo.
pub fn read(b: &mut &[u8]) -> Option<u32> {
    let mut v = 0;
    for i in 0..5 {
        let (&x, rest) = b.split_first()?;
        *b = rest;
        v |= ((x & 0x7f) as u32) << (7 * i);
        if x & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

pub fn write(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Byte occupati da `v` scritto come VarUInt32 (da 1 a 5).
pub fn len(v: u32) -> usize {
    (32 - (v | 1).leading_zeros() as usize).div_ceil(7)
}

/// Legge un VarInt32 con segno (ZigZag).
pub fn read_signed(b: &mut &[u8]) -> Option<i32> {
    let v = read(b)?;
    Some(((v >> 1) as i32) ^ -((v & 1) as i32))
}

/// VarInt32 con segno: codifica ZigZag (0, -1, 1, -2... -> 0, 1, 2, 3...).
pub fn write_signed(out: &mut Vec<u8>, v: i32) {
    write(out, ((v << 1) ^ (v >> 31)) as u32);
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        for v in [0, 1, 127, 128, 193, 300, 16_383, 16_384, u32::MAX] {
            let mut out = Vec::new();
            super::write(&mut out, v);
            let mut s = &out[..];
            assert_eq!(super::read(&mut s), Some(v));
            assert!(s.is_empty());
        }
    }

    #[test]
    fn len_matches_written_bytes() {
        for v in [0, 1, 127, 128, 16_383, 16_384, 2_097_151, 2_097_152, u32::MAX] {
            let mut out = Vec::new();
            super::write(&mut out, v);
            assert_eq!(super::len(v), out.len(), "valore {v}");
        }
    }

    #[test]
    fn zigzag() {
        for (v, bytes) in [(0, &[0][..]), (-1, &[1]), (1, &[2]), (35, &[70]), (149, &[0xaa, 0x02])] {
            let mut out = Vec::new();
            super::write_signed(&mut out, v);
            assert_eq!(out, bytes, "valore {v}");
            assert_eq!(super::read_signed(&mut &out[..]), Some(v));
        }
    }

    #[test]
    fn rejects_truncated_and_overlong() {
        assert_eq!(super::read(&mut &[0x80][..]), None);
        assert_eq!(super::read(&mut &[0xff; 6][..]), None);
    }
}
