//! Skin del giocatore, letta dai dati del client del login e scritta come `SerializedSkin` (PlayerList / PlayerSkin).
//! Layout dei byte: docs/superpowers/notes/2026-11-19-protocollo-tappa2.md (sezione SerializedSkin).
use crate::{
    entity::{read_uuid, write_uuid},
    id,
    reader::Reader,
    wire::Writer,
    Packet,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use std::fmt;

/// Lato massimo accettato per le immagini (limite dello schema Mojang).
const MAX_SIDE: u32 = 4096;

/// Nomi dei tipi di pezzo persona (senza il prefisso `persona_`), indicizzati dal valore binario di `persona::PieceType`.
const PIECE_TYPES: [&str; 30] = [
    "unknown",
    "skeleton",
    "body",
    "skin",
    "bottom",
    "feet",
    "dress",
    "top",
    "high_pants",
    "hands",
    "outerwear",
    "facial_hair",
    "mouth",
    "eyes",
    "hair",
    "hood",
    "back",
    "face_accessory",
    "head",
    "legs",
    "left_leg",
    "right_leg",
    "arms",
    "left_arm",
    "right_arm",
    "capes",
    "classic_skin",
    "emote",
    "coco",
    "unsupported",
];

/// Geometria standard dei giocatori (`geometry.humanoid.custom`), usata dalla skin di ripiego.
const STEVE_GEOMETRY: &str = r#"{"format_version":"1.12.0","minecraft:geometry":[{"description":{"identifier":"geometry.humanoid.custom","texture_width":64,"texture_height":64,"visible_bounds_width":2,"visible_bounds_height":1,"visible_bounds_offset":[0,0.5,0]},"bones":[{"name":"root","pivot":[0,0,0]},{"name":"waist","parent":"root","pivot":[0,12,0]},{"name":"body","parent":"waist","pivot":[0,24,0],"cubes":[{"origin":[-4,12,-2],"size":[8,12,4],"uv":[16,16]}]},{"name":"head","parent":"body","pivot":[0,24,0],"cubes":[{"origin":[-4,24,-4],"size":[8,8,8],"uv":[0,0]}]},{"name":"rightArm","parent":"body","pivot":[-5,22,0],"cubes":[{"origin":[-8,12,-2],"size":[4,12,4],"uv":[40,16]}]},{"name":"leftArm","parent":"body","pivot":[5,22,0],"cubes":[{"origin":[4,12,-2],"size":[4,12,4],"uv":[32,48]}]},{"name":"rightLeg","parent":"root","pivot":[-1.9,12,0],"cubes":[{"origin":[-3.9,0,-2],"size":[4,12,4],"uv":[0,16]}]},{"name":"leftLeg","parent":"root","pivot":[1.9,12,0],"cubes":[{"origin":[-0.1,0,-2],"size":[4,12,4],"uv":[16,48]}]}]}]}"#;
const STEVE_RESOURCE_PATCH: &str = r#"{"geometry":{"default":"geometry.humanoid.custom"}}"#;

#[derive(Clone, PartialEq, Default)]
pub struct SkinImage {
    pub width: u32,
    pub height: u32,
    /// RGBA, 4 byte per pixel.
    pub data: Vec<u8>,
}

impl fmt::Debug for SkinImage {
    // Senza stampare i pixel: sono decine di KB.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SkinImage({}x{}, {} byte)", self.width, self.height, self.data.len())
    }
}

impl SkinImage {
    fn encode(&self, w: &mut Writer) {
        w.u32(self.width);
        w.u32(self.height);
        w.bytes(&self.data);
    }

    /// Larghezza, altezza e byte RGBA: i byte devono coincidere con le dimensioni.
    fn decode(r: &mut Reader) -> Option<SkinImage> {
        let (width, height) = (r.u32()?, r.u32()?);
        if width > MAX_SIDE || height > MAX_SIDE {
            return None;
        }
        image(width, height, r.bytes()?.to_vec())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkinAnimation {
    pub image: SkinImage,
    /// `persona::AnimatedTextureType`: 0 nessuno, 1 volto, 2 corpo 32x32, 3 corpo 128x128.
    pub animation_type: u32,
    pub frames: f32,
    /// `persona::AnimationExpression`: 0 lineare, 1 sbattere le palpebre.
    pub expression: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersonaPiece {
    pub piece_id: String,
    /// `persona::PieceType` (valore binario).
    pub piece_type: u32,
    /// UUID del pacchetto, nell'ordine testuale (vedi `uuid_from_str`).
    pub pack_id: [u8; 16],
    pub is_default: bool,
    pub product_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PieceTint {
    pub piece_type: u32,
    /// Colori ARGB (`Color` Mojang = int32).
    pub colors: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skin {
    pub id: String,
    pub play_fab_id: String,
    /// JSON (testo UTF-8) con la geometria di default.
    pub resource_patch: Vec<u8>,
    pub image: SkinImage,
    pub animations: Vec<SkinAnimation>,
    pub cape: SkinImage,
    /// JSON (testo UTF-8) delle geometrie.
    pub geometry: Vec<u8>,
    pub geometry_engine_version: Vec<u8>,
    pub animation_data: Vec<u8>,
    pub cape_id: String,
    pub full_id: String,
    /// "slim" o "wide".
    pub arm_size: String,
    /// "#AARRGGBB".
    pub color: String,
    pub persona_pieces: Vec<PersonaPiece>,
    pub piece_tints: Vec<PieceTint>,
    pub premium: bool,
    pub persona: bool,
    pub cape_on_classic: bool,
    pub primary_user: bool,
    pub overrides_appearance: bool,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Base64 standard, con o senza padding.
fn b64(v: &Value, key: &str) -> Option<Vec<u8>> {
    STANDARD.decode(v.get(key)?.as_str()?.trim_end_matches('=')).ok().or_else(|| STANDARD.decode(v.get(key)?.as_str()?).ok())
}

fn dim(v: &Value, key: &str) -> Option<u32> {
    v.get(key)?.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|&n| n <= MAX_SIDE)
}

/// Immagine RGBA: i byte devono coincidere esattamente con larghezza x altezza x 4.
fn image(width: u32, height: u32, data: Vec<u8>) -> Option<SkinImage> {
    (width as usize * height as usize * 4 == data.len()).then_some(SkinImage { width, height, data })
}

/// Colore "#AARRGGBB" (o "#RRGGBB", opaco) come int32 ARGB.
fn color_argb(s: &str) -> Option<i32> {
    let hex = s.strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(if hex.len() == 6 { v | 0xff00_0000 } else { v } as i32)
}

fn piece_type(name: &str) -> u32 {
    let name = name.strip_prefix("persona_").unwrap_or(name).to_ascii_lowercase();
    PIECE_TYPES.iter().position(|p| *p == name).unwrap_or(0) as u32
}

/// UUID testuale ("aabbccdd-eeff-...") in 16 byte nell'ordine del testo. `None` se non e' un UUID.
pub fn uuid_from_str(s: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = s.bytes().filter(|b| *b != b'-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in hex.chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

impl Skin {
    /// Legge la skin dai claim del JWT dei dati del client. `None` se manca, e' incoerente o malformata.
    pub fn from_client_data(c: &Value) -> Option<Skin> {
        let id = text(c, "SkinId");
        if id.is_empty() {
            return None;
        }
        let img = image(dim(c, "SkinImageWidth")?, dim(c, "SkinImageHeight")?, b64(c, "SkinData")?)?;
        if img.data.is_empty() {
            return None;
        }
        // La cape e' facoltativa: assente o 0x0 = nessuna cape; presente ma incoerente = skin malformata.
        let cape = match (dim(c, "CapeImageWidth"), dim(c, "CapeImageHeight")) {
            (Some(w), Some(h)) if w > 0 && h > 0 => image(w, h, b64(c, "CapeData")?)?,
            _ => SkinImage::default(),
        };
        let geometry = b64(c, "SkinGeometryData").unwrap_or_default();
        let resource_patch = b64(c, "SkinResourcePatch").unwrap_or_default();
        // Le geometrie e la patch viaggiano come JSON: se non lo sono la skin viene scartata.
        if (!geometry.is_empty() && serde_json::from_slice::<Value>(&geometry).is_err())
            || (!resource_patch.is_empty() && serde_json::from_slice::<Value>(&resource_patch).is_err())
        {
            return None;
        }
        let animations = match c.get("AnimatedImageData").and_then(Value::as_array) {
            None => Vec::new(),
            Some(list) => list
                .iter()
                .map(|an| {
                    Some(SkinAnimation {
                        image: image(dim(an, "ImageWidth")?, dim(an, "ImageHeight")?, b64(an, "Image")?)?,
                        animation_type: an.get("Type").and_then(Value::as_u64)? as u32,
                        frames: an.get("Frames").and_then(Value::as_f64)? as f32,
                        expression: an.get("AnimationExpression").and_then(Value::as_u64).unwrap_or(0) as u32,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        };
        let persona_pieces = c
            .get("PersonaPieces")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|p| PersonaPiece {
                        piece_id: text(p, "PieceId"),
                        piece_type: piece_type(&text(p, "PieceType")),
                        pack_id: uuid_from_str(&text(p, "PackId")).unwrap_or_default(),
                        is_default: flag(p, "IsDefault"),
                        product_id: text(p, "ProductId"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let piece_tints = c
            .get("PieceTintColors")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|t| PieceTint {
                        piece_type: piece_type(&text(t, "PieceType")),
                        colors: t
                            .get("Colors")
                            .and_then(Value::as_array)
                            .map(|cs| cs.iter().filter_map(|c| color_argb(c.as_str()?)).collect())
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cape_id = text(c, "CapeId");
        Some(Skin {
            full_id: format!("{id}{cape_id}"),
            id,
            play_fab_id: text(c, "PlayFabId"),
            resource_patch,
            image: img,
            animations,
            cape,
            geometry,
            geometry_engine_version: b64(c, "SkinGeometryDataEngineVersion").unwrap_or_default(),
            animation_data: b64(c, "SkinAnimationData").unwrap_or_default(),
            cape_id,
            arm_size: text(c, "ArmSize"),
            color: text(c, "SkinColor"),
            persona_pieces,
            piece_tints,
            premium: flag(c, "PremiumSkin"),
            persona: flag(c, "PersonaSkin"),
            cape_on_classic: flag(c, "CapeOnClassicSkin"),
            primary_user: true,
            overrides_appearance: flag(c, "OverrideSkin"),
        })
    }

    /// Steve 64x64 di un solo colore con la geometria standard: per chi non manda una skin valida.
    pub fn fallback() -> Skin {
        Skin {
            id: "versmcbe:steve".into(),
            play_fab_id: String::new(),
            resource_patch: STEVE_RESOURCE_PATCH.into(),
            image: SkinImage { width: 64, height: 64, data: [0xb3, 0x7b, 0x62, 0xff].repeat(64 * 64) },
            animations: Vec::new(),
            cape: SkinImage::default(),
            geometry: STEVE_GEOMETRY.into(),
            geometry_engine_version: b"1.14.0".to_vec(),
            animation_data: Vec::new(),
            cape_id: String::new(),
            full_id: "versmcbe:steve".into(),
            arm_size: "wide".into(),
            color: "#00000000".into(),
            persona_pieces: Vec::new(),
            piece_tints: Vec::new(),
            premium: false,
            persona: false,
            cape_on_classic: false,
            primary_user: true,
            overrides_appearance: false,
        }
    }

    /// Scrive la `SerializedSkin` nell'ordine di `SerializedSkin.json` (indici 0..=20).
    pub fn encode(&self, w: &mut Writer) {
        w.string(&self.id);
        w.bytes(&self.resource_patch);
        self.image.encode(w);
        w.var_u32(self.animations.len() as u32);
        for a in &self.animations {
            a.image.encode(w);
            w.var_u32(a.animation_type);
            w.f32(a.frames);
            w.var_u32(a.expression);
        }
        self.cape.encode(w);
        w.bytes(&self.geometry);
        w.bytes(&self.geometry_engine_version);
        w.bytes(&self.animation_data);
        w.string(&self.cape_id);
        w.string(&self.full_id);
        w.u8(u8::from(!self.arm_size.eq_ignore_ascii_case("slim")));
        w.i32(color_argb(&self.color).unwrap_or(0));
        w.var_u32(self.persona_pieces.len() as u32);
        for p in &self.persona_pieces {
            w.string(&p.piece_id);
            w.u32(p.piece_type);
            w.u64(u64::from_be_bytes(p.pack_id[..8].try_into().unwrap()));
            w.u64(u64::from_be_bytes(p.pack_id[8..].try_into().unwrap()));
            w.bool(p.is_default);
            w.string(&p.product_id);
        }
        w.var_u32(self.piece_tints.len() as u32);
        for t in &self.piece_tints {
            w.u32(t.piece_type);
            w.var_u32(t.colors.len() as u32);
            for c in &t.colors {
                w.i32(*c);
            }
        }
        w.bool(self.premium);
        w.bool(self.persona);
        w.bool(self.cape_on_classic);
        w.bool(self.primary_user);
        w.bool(self.overrides_appearance);
        w.u8(2); // TrustedSkinFlag::True: il server ha gia' validato la skin
        w.string(""); // ProfileHash
    }
}

impl Skin {
    /// Legge una `SerializedSkin` (inverso di `encode`). `None` se i byte finiscono, le immagini sono incoerenti o il JSON e' malformato.
    /// Il flag "fidato" e l'hash del profilo si leggono e si scartano: il server decide da solo cosa e' fidato.
    pub fn decode(r: &mut Reader) -> Option<Skin> {
        let id = r.string()?.to_owned();
        let resource_patch = r.bytes()?.to_vec();
        let img = SkinImage::decode(r)?;
        let mut animations = Vec::new();
        for _ in 0..r.var_u32()? {
            animations.push(SkinAnimation { image: SkinImage::decode(r)?, animation_type: r.var_u32()?, frames: r.f32()?, expression: r.var_u32()? });
        }
        let cape = SkinImage::decode(r)?;
        let geometry = r.bytes()?.to_vec();
        let geometry_engine_version = r.bytes()?.to_vec();
        let animation_data = r.bytes()?.to_vec();
        let cape_id = r.string()?.to_owned();
        let full_id = r.string()?.to_owned();
        let arm_size = if r.u8()? == 0 { "slim" } else { "wide" }.to_owned();
        let color = format!("#{:08x}", r.i32()? as u32);
        let mut persona_pieces = Vec::new();
        for _ in 0..r.var_u32()? {
            persona_pieces.push(PersonaPiece {
                piece_id: r.string()?.to_owned(),
                piece_type: r.u32()?,
                pack_id: read_uuid(r)?,
                is_default: r.bool()?,
                product_id: r.string()?.to_owned(),
            });
        }
        let mut piece_tints = Vec::new();
        for _ in 0..r.var_u32()? {
            let piece_type = r.u32()?;
            let mut colors = Vec::new();
            for _ in 0..r.var_u32()? {
                colors.push(r.i32()?);
            }
            piece_tints.push(PieceTint { piece_type, colors });
        }
        let (premium, persona, cape_on_classic, primary_user, overrides_appearance) = (r.bool()?, r.bool()?, r.bool()?, r.bool()?, r.bool()?);
        r.u8()?; // TrustedSkinFlag
        r.string()?; // ProfileHash
                     // Come per i dati del login: geometrie e patch viaggiano come JSON, altrimenti la skin e' scartata.
        if (!geometry.is_empty() && serde_json::from_slice::<Value>(&geometry).is_err())
            || (!resource_patch.is_empty() && serde_json::from_slice::<Value>(&resource_patch).is_err())
        {
            return None;
        }
        Some(Skin {
            id,
            play_fab_id: String::new(),
            resource_patch,
            image: img,
            animations,
            cape,
            geometry,
            geometry_engine_version,
            animation_data,
            cape_id,
            full_id,
            arm_size,
            color,
            persona_pieces,
            piece_tints,
            premium,
            persona,
            cape_on_classic,
            primary_user,
            overrides_appearance,
        })
    }
}

/// `PlayerSkin` (93): un giocatore cambia skin. Layout di `PlayerSkinPacketPayload.json`: UUID, skin, nome nuovo e nome vecchio.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSkinChange {
    pub uuid: [u8; 16],
    pub skin: Skin,
    pub new_name: String,
    pub old_name: String,
}

impl PlayerSkinChange {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let mut r = Reader(body);
        Some(Self { uuid: read_uuid(&mut r)?, skin: Skin::decode(&mut r)?, new_name: r.string()?.to_owned(), old_name: r.string()?.to_owned() })
    }
}

pub fn player_skin_packet(change: &PlayerSkinChange) -> Packet {
    let mut w = Writer::default();
    write_uuid(&mut w, &change.uuid);
    change.skin.encode(&mut w);
    w.string(&change.new_name);
    w.string(&change.old_name);
    Packet::new(id::PLAYER_SKIN, w.0)
}

/// UUID del giocatore: quello del login se valido, altrimenti uno derivato dall'identita' (stabile, versione 3, variante RFC 4122).
pub fn uuid_bytes(info: &crate::login::PlayerInfo, identity: &str) -> [u8; 16] {
    if let Some(u) = info.uuid.as_deref().and_then(uuid_from_str) {
        return u;
    }
    let fnv = |seed: u64| identity.bytes().fold(seed, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&fnv(0xcbf2_9ce4_8422_2325).to_be_bytes());
    out[8..].copy_from_slice(&fnv(0x9e37_79b9_7f4a_7c15).to_be_bytes());
    out[6] = out[6] & 0x0f | 0x30;
    out[8] = out[8] & 0x3f | 0x80;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login::PlayerInfo;
    use serde_json::json;

    fn client_data(w: u32, h: u32) -> Value {
        json!({
            "SkinId": "abc", "SkinData": STANDARD.encode(vec![7u8; (w * h * 4) as usize]), "SkinImageWidth": w, "SkinImageHeight": h,
            "SkinGeometryData": STANDARD.encode(r#"{"geometry":{}}"#), "ArmSize": "wide", "CapeId": "", "SkinColor": "#ffb37b62",
        })
    }

    fn info(uuid: Option<&str>) -> PlayerInfo {
        PlayerInfo {
            name: "A".into(),
            xuid: None,
            uuid: uuid.map(String::from),
            device_os: None,
            game_version: None,
            authentication_type: None,
            token_claim_keys: Vec::new(),
            skin: Skin::fallback(),
        }
    }

    #[test]
    fn skin_from_client_data_reads_image_and_geometry() {
        let s = Skin::from_client_data(&client_data(64, 64)).unwrap();
        assert_eq!((s.image.data.len(), s.arm_size.as_str(), s.id.as_str()), (16384, "wide", "abc"));
        assert_eq!(s.geometry, br#"{"geometry":{}}"#);
    }

    #[test]
    fn malformed_skin_falls_back() {
        let mut c = client_data(64, 64);
        c["SkinData"] = json!("@@non base64@@");
        assert!(Skin::from_client_data(&c).is_none());
        let mut c = client_data(64, 64);
        c["SkinImageHeight"] = json!(32); // i byte non tornano con le dimensioni
        assert!(Skin::from_client_data(&c).is_none());
        let f = Skin::fallback();
        assert_eq!((f.image.width, f.image.height, f.image.data.len()), (64, 64, 16384));
        assert!(serde_json::from_slice::<Value>(&f.geometry).is_ok());
    }

    #[test]
    fn missing_uuid_gives_stable_uuid_from_identity() {
        let i = info(None);
        let (a, b, c) = (uuid_bytes(&i, "xuid-1"), uuid_bytes(&i, "xuid-1"), uuid_bytes(&i, "xuid-2"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!((a[6] >> 4, a[8] >> 6), (3, 2));
        let given = info(Some("00112233-4455-6677-8899-aabbccddeeff"));
        assert_eq!(uuid_bytes(&given, "x")[..4], [0x00, 0x11, 0x22, 0x33]);
    }

    #[test]
    fn encode_round_trip_length() {
        let len = |h| {
            let mut w = Writer::default();
            Skin::from_client_data(&client_data(64, h)).unwrap().encode(&mut w);
            w.0.len()
        };
        assert_eq!(len(128) - len(64), 64 * 64 * 4);
    }

    #[test]
    fn skin_change_round_trips() {
        let mut c = client_data(64, 64);
        c["AnimatedImageData"] = json!([{"Image": STANDARD.encode(vec![1u8; 32 * 32 * 4]), "ImageWidth": 32, "ImageHeight": 32, "Type": 1, "Frames": 2.0, "AnimationExpression": 1}]);
        c["PersonaPieces"] =
            json!([{"IsDefault": true, "PackId": "00112233-4455-6677-8899-aabbccddeeff", "PieceId": "p", "PieceType": "persona_hair", "ProductId": "x"}]);
        c["PieceTintColors"] = json!([{"PieceType": "persona_hair", "Colors": ["#ff112233"]}]);
        c["ArmSize"] = json!("slim");
        c["CapeId"] = json!("cape1");
        c["CapeImageWidth"] = json!(64);
        c["CapeImageHeight"] = json!(32);
        c["CapeData"] = json!(STANDARD.encode(vec![9u8; 64 * 32 * 4]));
        let change = PlayerSkinChange {
            uuid: uuid_from_str("00112233-4455-6677-8899-aabbccddeeff").unwrap(),
            skin: Skin::from_client_data(&c).unwrap(),
            new_name: "nuova".into(),
            old_name: "vecchia".into(),
        };
        let packet = player_skin_packet(&change);
        assert_eq!(packet.id, crate::id::PLAYER_SKIN);
        assert_eq!(PlayerSkinChange::decode(&packet.body), Some(change));
    }

    #[test]
    fn skin_change_truncated_or_incoherent_is_none() {
        let change = PlayerSkinChange { uuid: [1; 16], skin: Skin::fallback(), new_name: String::new(), old_name: String::new() };
        let body = player_skin_packet(&change).body;
        assert_eq!(PlayerSkinChange::decode(&body).as_ref(), Some(&change));
        for cut in [0, 10, 40, body.len() - 1] {
            assert!(PlayerSkinChange::decode(&body[..cut]).is_none(), "tagliato a {cut}");
        }
        // Immagine con dimensioni che non tornano con i byte.
        let mut skin = Skin::fallback();
        skin.image.height = 32;
        assert!(PlayerSkinChange::decode(&player_skin_packet(&PlayerSkinChange { skin, ..change }).body).is_none());
    }

    #[test]
    fn persona_pieces_and_tints_are_read() {
        let mut c = client_data(64, 64);
        c["PersonaPieces"] =
            json!([{"IsDefault": true, "PackId": "00112233-4455-6677-8899-aabbccddeeff", "PieceId": "p", "PieceType": "persona_hair", "ProductId": ""}]);
        c["PieceTintColors"] = json!([{"PieceType": "persona_hair", "Colors": ["#ff112233", "#ff000000", "#ff000000", "#ff000000"]}]);
        let s = Skin::from_client_data(&c).unwrap();
        assert_eq!((s.persona_pieces[0].piece_type, s.persona_pieces[0].pack_id[15]), (14, 0xff));
        assert_eq!((s.piece_tints[0].piece_type, s.piece_tints[0].colors[0]), (14, 0xff11_2233_u32 as i32));
    }
}
