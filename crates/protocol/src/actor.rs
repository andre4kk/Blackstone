//! Dati sincronizzati di un'entità (`SetActorData`, pacchetto 39), scritti a mano: il generatore non gestisce i `oneOf`.
//!
//! Dal protocollo 2225 ogni voce è `{ ID, Payload }` e il payload è una variante con etichetta (developer note us1418626):
//! indice di variante varuint32, poi il tipo (u8, ridondante) e il valore. Stesso schema delle azioni di
//! `ItemStackRequest`, verificato sulle catture del client.
use crate::wire::Writer;
use crate::Packet;

pub const SET_ACTOR_DATA: u32 = 39;

/// Chiavi dei dati (stabili da anni: PocketMine `EntityMetadataProperties`, gophertunnel `EntityDataKey*`).
pub mod key {
    pub const FLAGS: u32 = 0;
    pub const BOUNDING_BOX_WIDTH: u32 = 53;
    pub const BOUNDING_BOX_HEIGHT: u32 = 54;
}

/// Bit della chiave `FLAGS` (`ActorFlags`). Dal protocollo 560 CAN_DASH(46) ha spostato di uno tutti i successivi
/// (changelog_560 di bedrock-protocol-docs): gravità 49, collisioni 48. Con 48 il giocatore restava senza gravità.
pub mod flag {
    pub const SNEAKING: u32 = 1;
    pub const SPRINTING: u32 = 3;
    pub const CAN_CLIMB: u32 = 19;
    pub const GLIDING: u32 = 32;
    pub const BREATHING: u32 = 35;
    pub const HAS_COLLISION: u32 = 48;
    pub const HAS_GRAVITY: u32 = 49;
    pub const SWIMMING: u32 = 56;
}

/// Un valore sincronizzato; il numero è sia l'indice di variante sia il `DataItemType`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DataValue {
    Float(f32),
    Int64(i64),
}

impl DataValue {
    fn write(self, w: &mut Writer) {
        let ty: u8 = match self {
            DataValue::Float(_) => 3,
            DataValue::Int64(_) => 7,
        };
        w.var_u32(ty as u32); // indice di variante
        w.u8(ty); // tipo
        match self {
            DataValue::Float(v) => w.f32(v),
            DataValue::Int64(v) => w.var_i64(v),
        }
    }
}

/// Lista di dati sincronizzati (`SynchedActorDataList`): contatore varuint32 e voci `{ ID, Payload }`.
pub(crate) fn write_data(w: &mut Writer, data: &[(u32, DataValue)]) {
    w.var_u32(data.len() as u32);
    for (id, value) in data {
        w.var_u32(*id);
        value.write(w);
    }
}

/// `SetActorData` senza proprietà sincronizzate, al tick 0.
pub fn set_actor_data(runtime_id: u64, data: &[(u32, DataValue)]) -> Packet {
    let mut w = Writer::default();
    w.var_u64(runtime_id);
    write_data(&mut w, data);
    w.var_u32(0); // proprietà intere
    w.var_u32(0); // proprietà float
    w.var_u64(0); // tick
    Packet::new(SET_ACTOR_DATA, w.0)
}

/// Dati di base di un giocatore: gravità, collisioni, arrampicata, respiro e riquadro 0.6 x 1.8 (come Dragonfly).
pub fn player_data(runtime_id: u64) -> Packet {
    let flags = [flag::HAS_GRAVITY, flag::HAS_COLLISION, flag::CAN_CLIMB, flag::BREATHING].iter().fold(0i64, |f, b| f | 1 << b);
    set_actor_data(
        runtime_id,
        &[(key::FLAGS, DataValue::Int64(flags)), (key::BOUNDING_BOX_WIDTH, DataValue::Float(0.6)), (key::BOUNDING_BOX_HEIGHT, DataValue::Float(1.8))],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_entry_is_tagged_variant_then_zigzag_value() {
        let p = set_actor_data(1, &[(key::FLAGS, DataValue::Int64(1 << 49))]);
        assert_eq!(p.id, 39);
        let mut expected = vec![1, 1, 0, 7, 7];
        let mut v = Writer::default();
        v.var_i64(1 << 49);
        expected.extend(v.0);
        expected.extend([0, 0, 0]);
        assert_eq!(p.body, expected);
    }

    #[test]
    fn player_data_has_gravity_and_collision() {
        let body = player_data(5).body;
        let mut flags = Writer::default();
        flags.var_i64((1 << 49) | (1 << 48) | (1 << 19) | (1 << 35)); // gravità 49 e collisioni 48 (changelog_560)
        assert_eq!(body[..5], [5, 3, 0, 7, 7]);
        assert_eq!(body[5..5 + flags.0.len()], flags.0[..]);
    }
}
