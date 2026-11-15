//! Pacchetti Bedrock implementati finora. Ognuno converte struct <-> corpo del pacchetto (senza header).
use crate::{varint, COMPRESSION_THRESHOLD};

/// ID dei pacchetti usati finora.
pub mod id {
    pub const LOGIN: u32 = 1;
    pub const PLAY_STATUS: u32 = 2;
    pub const DISCONNECT: u32 = 5;
    pub const CLIENT_CACHE_STATUS: u32 = 129;
    pub const PLAYER_AUTH_INPUT: u32 = 144;
    pub const REQUEST_CHUNK_RADIUS: u32 = 69;
    pub const SET_LOCAL_PLAYER_AS_INITIALIZED: u32 = 113;
    pub const PACKET_VIOLATION_WARNING: u32 = 156;
    pub const RESOURCE_PACKS_INFO: u32 = 6;
    pub const RESOURCE_PACK_STACK: u32 = 7;
    pub const RESOURCE_PACK_CLIENT_RESPONSE: u32 = 8;
    pub const NETWORK_SETTINGS: u32 = 143;
    pub const REQUEST_NETWORK_SETTINGS: u32 = 193;
}

/// Client -> server: primo pacchetto, dichiara il protocollo del client.
pub struct RequestNetworkSettings {
    pub protocol: i32,
}

impl RequestNetworkSettings {
    pub fn decode(body: &[u8]) -> Option<Self> {
        Some(Self { protocol: i32::from_be_bytes(body.get(..4)?.try_into().ok()?) })
    }

    pub fn encode(&self) -> Vec<u8> {
        self.protocol.to_be_bytes().to_vec()
    }
}

/// Server -> client: parametri di compressione e throttling (10 byte per il protocollo 2225).
pub struct NetworkSettings {
    pub compression_threshold: u16,
    /// 0 = deflate, 1 = snappy.
    pub compression_algorithm: u16,
    pub client_throttle: bool,
    pub client_throttle_threshold: u8,
    pub client_throttle_scalar: f32,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            compression_threshold: COMPRESSION_THRESHOLD,
            compression_algorithm: 0,
            client_throttle: false,
            client_throttle_threshold: 0,
            client_throttle_scalar: 0.0,
        }
    }
}

impl NetworkSettings {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = self.compression_threshold.to_le_bytes().to_vec();
        b.extend_from_slice(&self.compression_algorithm.to_le_bytes());
        b.push(self.client_throttle as u8);
        b.push(self.client_throttle_threshold);
        b.extend_from_slice(&self.client_throttle_scalar.to_le_bytes());
        b
    }
}

/// Client -> server: protocollo + richiesta di connessione, che contiene due stringhe:
/// la catena di identità (JSON) e i dati del client (JWT con skin, dispositivo, lingua...).
/// Formato: protocollo i32 BE, lunghezza varint, poi due stringhe con lunghezza u32 LE.
pub struct Login<'a> {
    pub protocol: i32,
    pub identity: &'a str,
    pub client_data: &'a str,
}

impl<'a> Login<'a> {
    pub fn decode(body: &'a [u8]) -> Option<Self> {
        let protocol = i32::from_be_bytes(body.get(..4)?.try_into().ok()?);
        let mut rest = &body[4..];
        let len = varint::read(&mut rest)? as usize;
        let mut request = rest.get(..len)?;
        Some(Self { protocol, identity: take_string(&mut request)?, client_data: take_string(&mut request)? })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut request = Vec::new();
        for s in [self.identity, self.client_data] {
            request.extend_from_slice(&(s.len() as u32).to_le_bytes());
            request.extend_from_slice(s.as_bytes());
        }
        let mut b = self.protocol.to_be_bytes().to_vec();
        varint::write(&mut b, request.len() as u32);
        b.extend(request);
        b
    }
}

/// Stringa con lunghezza u32 little-endian.
fn take_string<'a>(b: &mut &'a [u8]) -> Option<&'a str> {
    let len = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
    let s = b.get(4..)?.get(..len)?;
    *b = &b[4 + len..];
    std::str::from_utf8(s).ok()
}

/// Client -> server: il client avverte che un nostro pacchetto è malformato. È lo strumento principale per
/// capire quale dei pacchetti che inviamo non è come se lo aspetta.
pub struct PacketViolationWarning {
    /// -1 = sconosciuto, 0 = pacchetto malformato.
    pub violation_type: i32,
    /// -1 = sconosciuta, 0 = avviso, 1 = ultimo avviso, 2 = connessione terminata.
    pub severity: i32,
    /// ID del pacchetto incriminato.
    pub packet_id: i32,
    pub context: String,
}

impl PacketViolationWarning {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let mut b = body;
        let (violation_type, severity, packet_id) = (varint::read_signed(&mut b)?, varint::read_signed(&mut b)?, varint::read_signed(&mut b)?);
        let len = varint::read(&mut b)? as usize;
        let context = String::from_utf8_lossy(b.get(..len)?).into_owned();
        Some(Self { violation_type, severity, packet_id, context })
    }
}

/// Client -> server: raggio di chunk che il client vorrebbe vedere (in chunk, non in blocchi).
pub struct RequestChunkRadius {
    pub chunk_radius: i32,
    pub max_radius: u8,
}

impl RequestChunkRadius {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let mut b = body;
        Some(Self { chunk_radius: varint::read_signed(&mut b)?, max_radius: *b.first()? })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        varint::write_signed(&mut b, self.chunk_radius);
        b.push(self.max_radius);
        b
    }
}

/// Valori di `PlayStatus`.
pub mod play_status {
    pub const LOGIN_SUCCESS: i32 = 0;
    pub const PLAYER_SPAWN: i32 = 3;
}

/// Server -> client: esito del login (LoginSuccess) e, alla fine, PlayerSpawn.
pub struct PlayStatus {
    pub status: i32,
}

impl PlayStatus {
    pub fn encode(&self) -> Vec<u8> {
        self.status.to_be_bytes().to_vec() // l'unico intero big-endian del protocollo di gioco
    }
}

/// Server -> client: elenco dei resource pack. Per ora nessun pack: il client risponde "ho tutto".
#[derive(Default)]
pub struct ResourcePacksInfo {
    pub must_accept: bool,
}

impl ResourcePacksInfo {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![self.must_accept as u8, 0, 0, 0]; // obbligatori, addon, script, niente "vibrant visuals" forzati
        b.extend_from_slice(&[0; 16]); // UUID del template del mondo (2 x u64 LE): nessuno
        varint::write(&mut b, 0); // versione del template: stringa vuota
        varint::write(&mut b, 0); // numero di pack (varint dalla 1.26.60; prima era un u16): nessuno
        b
    }
}

/// Server -> client: pila dei resource pack da applicare, versione di gioco e esperimenti attivi.
pub struct ResourcePackStack<'a> {
    pub must_accept: bool,
    pub base_game_version: &'a str,
}

impl ResourcePackStack<'_> {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![self.must_accept as u8];
        varint::write(&mut b, 0); // pack nella pila (varint): nessuno
        put_string(&mut b, self.base_game_version);
        b.extend_from_slice(&0u32.to_le_bytes()); // esperimenti (u32 LE, "No size compression"): nessuno
        b.push(0); // "esperimenti mai attivati"
        b.push(0); // niente pack dell'editor
        b
    }
}

/// Tag di `ResourcePackClientResponse` (dalla 1.26.60 partono da 0: la numerazione di PocketMine è quella vecchia).
pub mod pack_response {
    pub const CANCEL: u32 = 0;
    pub const DOWNLOADING: u32 = 1;
    /// "Ho finito di scaricare": è la risposta quando non ci sono pack da scaricare.
    pub const DOWNLOADING_FINISHED: u32 = 2;
    /// Il client ha applicato la pila: il server può mandare StartGame.
    pub const STACK_FINISHED: u32 = 3;

    /// Nome del tipo di risposta, che il client scrive come stringa dopo il tag.
    pub fn type_name(tag: u32) -> &'static str {
        match tag {
            CANCEL => "cancel",
            DOWNLOADING => "downloading",
            DOWNLOADING_FINISHED => "downloadingfinished",
            STACK_FINISHED => "resourcepackstackfinished",
            _ => "",
        }
    }
}

/// Client -> server: stato del download/applicazione dei resource pack.
/// Formato (verificato con il vero client): tag varint, tipo di risposta (stringa), e solo per `DOWNLOADING`
/// l'elenco degli id dei pack in scaricamento.
pub struct ResourcePackClientResponse {
    pub tag: u32,
    pub response_type: String,
    pub pack_ids: Vec<String>,
}

impl ResourcePackClientResponse {
    pub fn new(tag: u32) -> Self {
        Self { tag, response_type: pack_response::type_name(tag).into(), pack_ids: Vec::new() }
    }

    pub fn decode(body: &[u8]) -> Option<Self> {
        let mut b = body;
        let tag = varint::read(&mut b)?;
        let response_type = take_varint_string(&mut b)?;
        let mut pack_ids = Vec::new();
        if tag == pack_response::DOWNLOADING {
            // Elenco id: il formato esatto del contatore non è verificato, quindi si legge in modo permissivo.
            for _ in 0..varint::read(&mut b).unwrap_or(0) {
                let Some(id) = take_varint_string(&mut b) else { break };
                pack_ids.push(id);
            }
        }
        Some(Self { tag, response_type, pack_ids })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        varint::write(&mut b, self.tag);
        put_string(&mut b, &self.response_type);
        if self.tag == pack_response::DOWNLOADING {
            varint::write(&mut b, self.pack_ids.len() as u32);
            self.pack_ids.iter().for_each(|id| put_string(&mut b, id));
        }
        b
    }
}

/// Stringa con lunghezza varint.
fn take_varint_string(b: &mut &[u8]) -> Option<String> {
    let len = varint::read(b)? as usize;
    let s = std::str::from_utf8(b.get(..len)?).ok()?.to_owned();
    *b = &b[len..];
    Some(s)
}

/// Motivi di disconnessione (DisconnectFailReason). Ne usiamo uno generico: il testo lo scegliamo noi.
pub mod disconnect_reason {
    pub const UNKNOWN: i32 = 0;
}

/// Server -> client: chiude la connessione. Con `message` il client mostra quel testo, senza lo nasconde.
pub struct Disconnect<'a> {
    pub reason: i32,
    pub message: Option<&'a str>,
}

impl Disconnect<'_> {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        varint::write_signed(&mut b, self.reason);
        match self.message {
            Some(m) => {
                b.push(0); // "nascondi schermata" = falso: seguono messaggio e messaggio filtrato
                put_string(&mut b, m);
                put_string(&mut b, m);
            }
            None => b.push(1),
        }
        b
    }
}

/// Stringa con lunghezza varint.
fn put_string(out: &mut Vec<u8>, s: &str) {
    varint::write(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_settings_is_10_bytes() {
        let b = NetworkSettings::default().encode();
        assert_eq!(b.len(), 10);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), COMPRESSION_THRESHOLD);
    }

    #[test]
    fn request_network_settings_roundtrip() {
        let body = RequestNetworkSettings { protocol: 2225 }.encode();
        assert_eq!(RequestNetworkSettings::decode(&body).unwrap().protocol, 2225);
        assert!(RequestNetworkSettings::decode(&[1, 2]).is_none());
    }

    #[test]
    fn login_roundtrip() {
        let body = Login { protocol: 2225, identity: "{\"chain\":[]}", client_data: "a.b.c" }.encode();
        let l = Login::decode(&body).unwrap();
        assert_eq!((l.protocol, l.identity, l.client_data), (2225, "{\"chain\":[]}", "a.b.c"));
    }

    #[test]
    fn login_rejects_truncated_and_lying_lengths() {
        let body = Login { protocol: 2225, identity: "abc", client_data: "def" }.encode();
        for n in 0..body.len() {
            assert!(Login::decode(&body[..n]).is_none(), "troncato a {n} byte");
        }
        let mut lying = body.clone();
        lying[5] = 0xff; // la prima stringa dichiara più byte di quelli presenti
        assert!(Login::decode(&lying).is_none());
    }

    #[test]
    fn packet_violation_warning_decode() {
        let mut body = Vec::new();
        for v in [0, 1, 6] {
            varint::write_signed(&mut body, v);
        }
        put_string(&mut body, "Unread bytes");
        let w = PacketViolationWarning::decode(&body).unwrap();
        assert_eq!((w.violation_type, w.severity, w.packet_id, w.context.as_str()), (0, 1, 6, "Unread bytes"));
        assert!(PacketViolationWarning::decode(&body[..body.len() - 1]).is_none());
    }

    #[test]
    fn request_chunk_radius_roundtrip() {
        let r = RequestChunkRadius::decode(&RequestChunkRadius { chunk_radius: 12, max_radius: 16 }.encode()).unwrap();
        assert_eq!((r.chunk_radius, r.max_radius), (12, 16));
        assert!(RequestChunkRadius::decode(&[24]).is_none());
    }

    #[test]
    fn play_status_is_big_endian() {
        assert_eq!(PlayStatus { status: play_status::PLAYER_SPAWN }.encode(), [0, 0, 0, 3]);
    }

    #[test]
    fn resource_pack_packets_layout() {
        // 4 flag, UUID del template (16 byte), versione (stringa vuota), numero di pack (varint)
        assert_eq!(ResourcePacksInfo::default().encode(), [&[0u8; 4][..], &[0; 16], &[0], &[0]].concat());
        let stack = ResourcePackStack { must_accept: false, base_game_version: "1.26" }.encode();
        // obbligatorio, 0 pack, stringa, 0 esperimenti (u32), "mai attivati", niente pack dell'editor
        assert_eq!(stack, [&[0u8, 0, 4][..], b"1.26", &[0; 4], &[0, 0]].concat());
    }

    #[test]
    fn pack_response_matches_what_the_real_client_sent() {
        // Byte reali ricevuti da Minecraft 26.63: tag 2 + stringa "downloadingfinished".
        let real = [&[0x02u8, 0x13][..], b"downloadingfinished"].concat();
        let r = ResourcePackClientResponse::decode(&real).unwrap();
        assert_eq!((r.tag, r.response_type.as_str()), (pack_response::DOWNLOADING_FINISHED, "downloadingfinished"));
        assert_eq!(ResourcePackClientResponse::new(pack_response::DOWNLOADING_FINISHED).encode(), real);
    }

    #[test]
    fn pack_response_roundtrip() {
        let mut r = ResourcePackClientResponse::new(pack_response::DOWNLOADING);
        r.pack_ids = vec!["a-b".into(), "c".into()];
        let d = ResourcePackClientResponse::decode(&r.encode()).unwrap();
        assert_eq!((d.tag, d.response_type.as_str(), d.pack_ids), (1, "downloading", vec!["a-b".to_string(), "c".to_string()]));
        for tag in [pack_response::CANCEL, pack_response::STACK_FINISHED] {
            assert_eq!(ResourcePackClientResponse::decode(&ResourcePackClientResponse::new(tag).encode()).unwrap().tag, tag);
        }
        assert!(ResourcePackClientResponse::decode(&[]).is_none());
        assert!(ResourcePackClientResponse::decode(&[2, 5, b'a']).is_none(), "stringa troncata");
    }

    #[test]
    fn disconnect_layout() {
        let b = Disconnect { reason: 0, message: Some("ciao") }.encode();
        assert_eq!(b, [&[0u8, 0, 4][..], b"ciao", &[4], b"ciao"].concat());
        assert_eq!(Disconnect { reason: 0, message: None }.encode(), [0, 1]);
    }
}
