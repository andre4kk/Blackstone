//! Pacchetti che descrivono le entità (giocatori, oggetti a terra) e il loro movimento, server -> client.
//! Layout e confronti in `docs/superpowers/notes/2026-11-19-protocollo-tappa2.md`. I pacchetti semplici sono generati
//! (`generated.rs`), qui restano quelli con dati sincronizzati (`oneOf`, non gestiti dal generatore) e le funzioni di comodo.
use crate::actor::{write_data, DataValue};
use crate::generated::{
    MobArmorEquipment, MoveActorAbsolute, MoveActorAbsoluteData, RemoveActor, SerializedAbilitiesDataSerializedLayer, SetActorMotion, TakeItemActor, Vec3,
};
use crate::item::ItemStack;
use crate::skin::Skin;
use crate::wire::{Encode, ServerPacket, Writer};
use crate::{id, Packet};

/// `PlayerPermissionLevel::Member`, come nei pacchetti di abilità di `join.rs`.
const PERMISSION_MEMBER: i8 = 1;

/// Un giocatore da mostrare agli altri. Niente skin (viaggia nella `PlayerList`), niente legami, niente blocco passeggero.
/// Le abilità sono passate già pronte in `layers` (il crate del protocollo non conosce le modalità di gioco): si usa lo stesso
/// elenco di livelli che il server manda con `UpdateAbilities`; i permessi sono quelli di un membro e il bersaglio è `runtime_id`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AddPlayerData {
    /// UUID nell'ordine testuale (primi 8 byte = bit più significativi).
    pub uuid: [u8; 16],
    pub name: String,
    pub runtime_id: u64,
    pub platform_chat_id: String,
    /// Posizione dei piedi più l'altezza degli occhi, già sommata dal chiamante (lo schema dice "actor height offset applied").
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub head_yaw: f32,
    pub held: ItemStack,
    pub game_type: i32,
    pub data: Vec<(u32, DataValue)>,
    pub device_id: String,
    pub build_platform: i32,
    pub layers: Vec<SerializedAbilitiesDataSerializedLayer>,
}

fn vec3(v: [f32; 3]) -> Vec3 {
    Vec3 { x: v[0], y: v[1], z: v[2] }
}

fn write_vec3(w: &mut Writer, v: [f32; 3]) {
    w.f32(v[0]);
    w.f32(v[1]);
    w.f32(v[2]);
}

/// Un angolo in gradi in un byte (256 passi per giro), come gophertunnel.
fn angle_byte(deg: f32) -> u8 {
    (deg.rem_euclid(360.0) / 360.0 * 256.0) as u8
}

/// `AddPlayer` (12). Ordine dei campi dallo schema (indice 0..16).
pub fn add_player(p: &AddPlayerData) -> Packet {
    let mut w = Writer::default();
    // mce::UUID = due u64 little-endian: prima i bit alti, poi quelli bassi.
    w.u64(u64::from_be_bytes(p.uuid[..8].try_into().unwrap()));
    w.u64(u64::from_be_bytes(p.uuid[8..].try_into().unwrap()));
    w.string(&p.name);
    w.var_u64(p.runtime_id);
    w.string(&p.platform_chat_id);
    write_vec3(&mut w, p.position);
    write_vec3(&mut w, p.velocity);
    w.f32(p.pitch);
    w.f32(p.yaw);
    w.f32(p.head_yaw);
    p.held.encode(&mut w);
    w.var_i32(p.game_type);
    write_data(&mut w, &p.data);
    w.var_u32(0); // proprietà sincronizzate intere
    w.var_u32(0); // proprietà sincronizzate float
                  // SerializedAbilitiesData
    w.i64(p.runtime_id as i64);
    w.i8(PERMISSION_MEMBER);
    w.u8(0); // permessi dei comandi: qualsiasi
    w.var_u32(p.layers.len() as u32);
    for l in &p.layers {
        l.encode(&mut w);
    }
    w.var_u32(0); // legami
    w.u8(0); // blocco passeggero assente
    w.string(&p.device_id);
    w.i32(p.build_platform);
    Packet::new(id::ADD_PLAYER, w.0)
}

/// Una voce "aggiungi" della `PlayerList` (63): un giocatore con la sua skin.
pub struct PlayerListEntry<'a> {
    /// UUID nell'ordine testuale (primi 8 byte = bit più significativi).
    pub uuid: [u8; 16],
    pub unique_id: i64,
    pub name: String,
    pub xuid: String,
    pub platform_chat_id: String,
    pub build_platform: i32,
    pub skin: &'a Skin,
    pub teacher: bool,
    pub host: bool,
    pub sub_client: bool,
}

/// `mce::UUID` = due u64 little-endian: prima i bit alti, poi quelli bassi.
pub(crate) fn write_uuid(w: &mut Writer, uuid: &[u8; 16]) {
    w.u64(u64::from_be_bytes(uuid[..8].try_into().unwrap()));
    w.u64(u64::from_be_bytes(uuid[8..].try_into().unwrap()));
}

/// Inverso di `write_uuid`.
pub(crate) fn read_uuid(r: &mut crate::reader::Reader) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&r.u64()?.to_be_bytes());
    out[8..].copy_from_slice(&r.u64()?.to_be_bytes());
    Some(out)
}

/// `PlayerList` (63) con solo voci "aggiungi". Ogni voce è una variante con etichetta: indice di variante (posizione nel `oneOf`:
/// Remove 0, Add 1) e poi i campi della voce, che iniziano con il suo byte `Action` (Add 0).
pub fn player_list_add(entries: &[PlayerListEntry]) -> Packet {
    let mut w = Writer::default();
    w.var_u32(entries.len() as u32);
    for e in entries {
        w.var_u32(1);
        w.u8(0);
        write_uuid(&mut w, &e.uuid);
        w.var_i64(e.unique_id);
        w.string(&e.name);
        w.string(&e.xuid);
        w.string(&e.skin.play_fab_id);
        w.string(&e.platform_chat_id);
        w.i32(e.build_platform);
        e.skin.encode(&mut w);
        w.bool(e.teacher);
        w.bool(e.host);
        w.bool(e.sub_client);
        w.i32(-1); // colore del giocatore: bianco opaco (ARGB)
    }
    Packet::new(id::PLAYER_LIST, w.0)
}

/// `PlayerList` (63) con solo voci "rimuovi" (variante 0, `Action` 1, UUID).
pub fn player_list_remove(uuids: &[[u8; 16]]) -> Packet {
    let mut w = Writer::default();
    w.var_u32(uuids.len() as u32);
    for u in uuids {
        w.var_u32(0);
        w.u8(1);
        write_uuid(&mut w, u);
    }
    Packet::new(id::PLAYER_LIST, w.0)
}

/// `AddItemActor` (15): un oggetto a terra. Mai da pesca.
pub fn add_item_actor(unique_id: i64, runtime_id: u64, item: &ItemStack, position: [f32; 3], velocity: [f32; 3], data: &[(u32, DataValue)]) -> Packet {
    let mut w = Writer::default();
    w.var_i64(unique_id);
    w.var_u64(runtime_id);
    item.encode(&mut w);
    write_vec3(&mut w, position);
    write_vec3(&mut w, velocity);
    write_data(&mut w, data);
    w.bool(false); // da pesca
    Packet::new(id::ADD_ITEM_ACTOR, w.0)
}

/// `MoveActorAbsolute` (18). Intestazione: bit 0 a terra, bit 1 spostamento forzato (teletrasporto); angoli in un byte.
pub fn move_actor_absolute(runtime_id: u64, position: [f32; 3], pitch: f32, yaw: f32, head_yaw: f32, on_ground: bool, teleport: bool) -> Packet {
    MoveActorAbsolute {
        move_data: MoveActorAbsoluteData {
            actor_runtime_id: runtime_id,
            header: on_ground as u8 | (teleport as u8) << 1,
            position: vec3(position),
            rotation_x: angle_byte(pitch),
            rotation_y: angle_byte(yaw),
            rotation_y_head: angle_byte(head_yaw),
        },
    }
    .packet()
}

/// `RemoveActor` (14): l'ID unico, non quello di runtime.
pub fn remove_actor(unique_id: i64) -> Packet {
    RemoveActor { target_actor_id: unique_id }.packet()
}

/// `TakeItemActor` (17): l'animazione dell'oggetto che vola verso chi lo raccoglie.
pub fn take_item_actor(item_runtime_id: u64, taker_runtime_id: u64) -> Packet {
    TakeItemActor { item_runtime_id, actor_runtime_id: taker_runtime_id }.packet()
}

/// `Animate` (44) server -> client: oscillazione del braccio (azione 1) senza origine né mano.
pub fn animate_swing(runtime_id: u64) -> Packet {
    crate::generated::Animate { action: 1, target_actor_runtime_id: runtime_id, data: 0.0, swing_source: None, hand: None }.packet()
}

/// `SetActorMotion` (40), al tick 0 (non riguarda il giocatore che controlla).
pub fn set_actor_motion(runtime_id: u64, velocity: [f32; 3]) -> Packet {
    SetActorMotion { target_runtime_id: runtime_id, motion: vec3(velocity), tick: 0 }.packet()
}

/// `MobArmorEquipment` (32): elmo, corazza, gambali, stivali; il quinto posto (corpo) è vuoto come in Dragonfly.
pub fn mob_armor_equipment(runtime_id: u64, armor: &[ItemStack; 4]) -> Packet {
    let [head, torso, legs, feet] = armor.clone();
    MobArmorEquipment { target_runtime_id: runtime_id, head, torso, legs, feet, body: ItemStack::air() }.packet()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{flag, key};

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    fn sample_player() -> AddPlayerData {
        AddPlayerData {
            uuid: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            name: "Ab".into(),
            runtime_id: 3,
            platform_chat_id: String::new(),
            position: [1.0, 2.0, 3.0],
            velocity: [0.0; 3],
            pitch: 10.0,
            yaw: 20.0,
            head_yaw: 30.0,
            held: ItemStack::air(),
            game_type: 1,
            data: vec![(key::FLAGS, DataValue::Int64(1 << flag::SNEAKING))],
            device_id: "dev".into(),
            build_platform: 7,
            layers: vec![SerializedAbilitiesDataSerializedLayer {
                serialized_layer: 1,
                abilities_set: 0xFFFFF,
                ability_values: 0x3F,
                fly_speed: 0.05,
                vertical_fly_speed: 1.0,
                walk_speed: 0.1,
            }],
        }
    }

    #[test]
    fn add_player_layout() {
        let p = add_player(&sample_player());
        assert_eq!(p.id, id::ADD_PLAYER);
        let b = &p.body;
        // UUID: due u64 little-endian, bit alti poi bassi
        assert_eq!(b[..16], [8, 7, 6, 5, 4, 3, 2, 1, 16, 15, 14, 13, 12, 11, 10, 9]);
        // nome, runtime id, id chat piattaforma vuoto
        assert_eq!(b[16..21], [2, b'A', b'b', 3, 0]);
        let mut at = 21;
        let floats = f32s(&[1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 10.0, 20.0, 30.0]);
        assert_eq!(b[at..at + 36], floats[..]);
        at += 36;
        // oggetto in mano: aria = 8 byte a zero; modalità creativa zigzag(1) = 2
        assert_eq!(b[at..at + 9], [0, 0, 0, 0, 0, 0, 0, 0, 2]);
        at += 9;
        // un dato (chiave 0, variante 7, tipo 7, flag sneaking zigzag(2) = 4), poi 0 proprietà intere e 0 float
        assert_eq!(b[at..at + 7], [1, 0, 7, 7, 4, 0, 0]);
        at += 7;
        // abilità: bersaglio i64 = 3, membro, comandi 0, un livello
        assert_eq!(b[at..at + 11], [3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1]);
        at += 11;
        // livello: u16 1, u32 0xFFFFF, u32 0x3F, tre f32
        assert_eq!(b[at..at + 10], [1, 0, 0xFF, 0xFF, 0x0F, 0, 0x3F, 0, 0, 0]);
        at += 10 + 12;
        // zero legami, blocco passeggero assente, id dispositivo "dev", piattaforma i32 = 7
        assert_eq!(b[at..], [0, 0, 3, b'd', b'e', b'v', 7, 0, 0, 0]);
    }

    #[test]
    fn add_player_never_carries_skin_bytes() {
        let mut d = sample_player();
        d.name = "x".repeat(16);
        d.data = (0..40).map(|i| (i, DataValue::Float(1.0))).collect();
        assert!(add_player(&d).body.len() < 1024);
    }

    #[test]
    fn add_item_actor_layout() {
        let item = ItemStack { id: 5, count: 2, ..ItemStack::air() };
        let p = add_item_actor(-1, 9, &item, [1.0, 2.0, 3.0], [0.0, 0.5, 0.0], &[]);
        assert_eq!(p.id, id::ADD_ITEM_ACTOR);
        let mut expected = vec![1, 9]; // id unico zigzag(-1), runtime id
        let mut w = Writer::default();
        item.encode(&mut w);
        expected.extend(w.0);
        expected.extend(f32s(&[1.0, 2.0, 3.0, 0.0, 0.5, 0.0]));
        expected.extend([0, 0]); // nessun dato, non da pesca
        assert_eq!(p.body, expected);
    }

    #[test]
    fn move_actor_absolute_layout() {
        let p = move_actor_absolute(7, [1.0, 2.0, 3.0], 10.0, 90.0, 180.0, true, false);
        assert_eq!(p.id, id::MOVE_ACTOR_ABSOLUTE);
        let mut expected = vec![7, 1];
        expected.extend(f32s(&[1.0, 2.0, 3.0]));
        expected.extend([7, 64, 128]); // 10/360*256 = 7,1; 90 -> 64; 180 -> 128
        assert_eq!(p.body, expected);
        let t = move_actor_absolute(7, [0.0; 3], 0.0, -90.0, 360.0, false, true);
        assert_eq!(t.body[1], 2);
        assert_eq!(t.body[14..], [0, 192, 0]);
    }

    #[test]
    fn remove_actor_is_zigzag_unique_id() {
        assert_eq!(remove_actor(-1).body, vec![1]);
        assert_eq!(remove_actor(5).id, id::REMOVE_ACTOR);
    }

    #[test]
    fn take_item_actor_puts_item_first() {
        let p = take_item_actor(10, 3);
        assert_eq!((p.id, p.body), (id::TAKE_ITEM_ACTOR, vec![10, 3]));
    }

    #[test]
    fn animate_swing_has_no_optionals() {
        let p = animate_swing(4);
        // azione u8 = 1, runtime id, dati f32 = 0, origine e mano assenti
        assert_eq!((p.id, p.body), (id::ANIMATE, vec![1, 4, 0, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn set_actor_motion_layout() {
        let p = set_actor_motion(2, [0.0, 0.25, 0.0]);
        let mut expected = vec![2];
        expected.extend(f32s(&[0.0, 0.25, 0.0]));
        expected.push(0); // tick
        assert_eq!((p.id, p.body), (id::SET_ACTOR_MOTION, expected));
    }

    #[test]
    fn mob_armor_equipment_has_empty_body_slot() {
        let helmet = ItemStack { id: 3, count: 1, ..ItemStack::air() };
        let p = mob_armor_equipment(6, &[helmet.clone(), ItemStack::air(), ItemStack::air(), ItemStack::air()]);
        assert_eq!(p.id, id::MOB_ARMOR_EQUIPMENT);
        let mut w = Writer::default();
        helmet.encode(&mut w);
        assert_eq!(p.body[0], 6);
        assert_eq!(p.body[1..1 + w.0.len()], w.0[..]);
        assert_eq!(p.body.len(), 1 + w.0.len() + 4 * 8);
    }

    #[test]
    fn player_list_add_layout() {
        let skin = Skin::fallback();
        let uuid: [u8; 16] = core::array::from_fn(|i| i as u8 + 1);
        let e = PlayerListEntry {
            uuid,
            unique_id: 5,
            name: "Al".into(),
            xuid: "99".into(),
            platform_chat_id: "".into(),
            build_platform: 7,
            skin: &skin,
            teacher: false,
            host: true,
            sub_client: false,
        };
        let p = player_list_add(&[e]);
        assert_eq!(p.id, id::PLAYER_LIST);
        let mut expected = vec![1, 1, 0]; // una voce, variante Add (seconda del oneOf), Action Add = 0
        expected.extend(0x0102030405060708u64.to_le_bytes());
        expected.extend(0x090a0b0c0d0e0f10u64.to_le_bytes());
        expected.push(10); // id unico 5 (zigzag)
        expected.extend([2, b'A', b'l', 2, b'9', b'9']); // nome, XUID
        expected.extend([0, 0]); // PlayFab ID (skin di ripiego: vuoto), id chat piattaforma
        expected.extend(7i32.to_le_bytes());
        let mut w = Writer::default();
        skin.encode(&mut w);
        expected.extend(w.0);
        expected.extend([0, 1, 0]); // insegnante, host, sub-client
        expected.extend((-1i32).to_le_bytes()); // colore
        assert_eq!(p.body, expected);
    }

    #[test]
    fn player_list_remove_layout() {
        let a: [u8; 16] = core::array::from_fn(|i| i as u8 + 1);
        let b = [0xffu8; 16];
        let p = player_list_remove(&[a, b]);
        assert_eq!(p.id, id::PLAYER_LIST);
        let mut expected = vec![2, 0, 1]; // due voci; variante Remove (prima del oneOf), Action Remove = 1
        expected.extend(0x0102030405060708u64.to_le_bytes());
        expected.extend(0x090a0b0c0d0e0f10u64.to_le_bytes());
        expected.extend([0, 1]);
        expected.extend([0xff; 16]);
        assert_eq!(p.body, expected);
    }

    #[test]
    fn ids_match_schema() {
        assert_eq!((id::PLAYER_LIST, id::PLAYER_SKIN), (63, 93));
    }
}
