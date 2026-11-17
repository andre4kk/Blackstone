//! Protocollo di gioco Bedrock, senza I/O: byte dentro, pacchetti fuori (e viceversa).
//! Per aggiornare il server a una nuova versione del gioco si parte dalle costanti qui sotto.
use miniz_oxide::{deflate::compress_to_vec, inflate::decompress_to_vec_with_limit};
use std::borrow::Cow;

pub mod generated;
pub mod login;
pub mod packets;
pub mod varint;
pub mod wire;
pub use packets::id;

/// Versione di gioco e numero di protocollo supportati (github.com/Mojang/bedrock-protocol-docs).
pub const PROTOCOL: u32 = 2225;
pub const VERSION: &str = "1.26.63";
/// Sotto questa dimensione i batch non vengono compressi.
pub const COMPRESSION_THRESHOLD: u16 = 256;

const ALGO_DEFLATE: u8 = 0x00;
const ALGO_NONE: u8 = 0xFF;
const MAX_BATCH: usize = 16 << 20; // limite alla decompressione, contro i "zip bomb"

/// Un pacchetto: ID + corpo (senza header né lunghezza).
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub id: u32,
    pub body: Vec<u8>,
}

impl Packet {
    pub fn new(id: u32, body: Vec<u8>) -> Self {
        Self { id, body }
    }
}

/// Stato di codifica di una connessione. Dopo `NetworkSettings` i batch hanno un byte di compressione davanti.
#[derive(Default)]
pub struct Session {
    compression: bool,
}

impl Session {
    pub fn enable_compression(&mut self) {
        self.compression = true;
    }

    /// Un messaggio di trasporto = un batch: [lunghezza varint][id varint + corpo]...
    /// Su NetherNet non c'è il byte 0xFE di RakNet. `None` se il batch è malformato.
    pub fn decode(&self, msg: &[u8]) -> Option<Vec<Packet>> {
        let data: Cow<[u8]> = if !self.compression {
            Cow::Borrowed(msg)
        } else {
            match msg.split_first()? {
                (&ALGO_NONE, rest) => Cow::Borrowed(rest),
                (&ALGO_DEFLATE, rest) => Cow::Owned(decompress_to_vec_with_limit(rest, MAX_BATCH).ok()?),
                _ => return None,
            }
        };
        let (mut rest, mut packets) = (&data[..], Vec::new());
        while !rest.is_empty() {
            let len = varint::read(&mut rest)? as usize;
            if len > rest.len() {
                return None;
            }
            let (mut p, tail) = rest.split_at(len);
            rest = tail;
            let header = varint::read(&mut p)?;
            packets.push(Packet::new(header & 0x3ff, p.to_vec())); // gli altri bit sono i sub-client (split screen)
        }
        Some(packets)
    }

    /// Raggruppa i pacchetti in un solo batch e lo comprime se supera la soglia.
    pub fn encode(&self, packets: &[Packet]) -> Vec<u8> {
        // Si scrive direttamente nel batch: lunghezza (id + corpo), id, corpo. Niente vettori intermedi.
        let size: usize = packets.iter().map(|p| p.body.len() + 10).sum();
        let mut batch = Vec::with_capacity(size + 1);
        if self.compression {
            batch.push(ALGO_NONE); // segnaposto: diventa ALGO_DEFLATE se si comprime
        }
        for p in packets {
            varint::write(&mut batch, (varint::len(p.id) + p.body.len()) as u32);
            varint::write(&mut batch, p.id);
            batch.extend_from_slice(&p.body);
        }
        if self.compression && batch.len() > COMPRESSION_THRESHOLD as usize { // il primo byte è il segnaposto
            let mut out = Vec::with_capacity(batch.len() / 2 + 1);
            out.push(ALGO_DEFLATE);
            out.extend_from_slice(&compress_to_vec(&batch[1..], 6));
            return out;
        }
        batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Packet> {
        vec![Packet::new(1, vec![1, 2, 3]), Packet::new(193, vec![]), Packet::new(300, vec![7; 1000])]
    }

    #[test]
    fn roundtrip_without_compression() {
        let s = Session::default();
        assert_eq!(s.decode(&s.encode(&sample())), Some(sample()));
    }

    #[test]
    fn roundtrip_with_compression_small_and_large() {
        let mut s = Session::default();
        s.enable_compression();
        let small = vec![Packet::new(1, vec![1])];
        let (m_small, m_large) = (s.encode(&small), s.encode(&sample()));
        assert_eq!(m_small[0], ALGO_NONE);
        assert_eq!(m_large[0], ALGO_DEFLATE);
        assert!(m_large.len() < 1000, "il batch grande deve risultare compresso");
        assert_eq!(s.decode(&m_small), Some(small));
        assert_eq!(s.decode(&m_large), Some(sample()));
    }

    #[test]
    fn rejects_garbage() {
        let mut s = Session::default();
        assert_eq!(s.decode(&[5, 1]), None); // lunghezza oltre i dati
        s.enable_compression();
        assert_eq!(s.decode(&[]), None);
        assert_eq!(s.decode(&[0x42, 1, 2]), None); // algoritmo sconosciuto
        assert_eq!(s.decode(&[ALGO_DEFLATE, 0xff, 0xff]), None); // deflate non valido
    }
}
