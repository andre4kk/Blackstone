//! Pacchetto `BlockEvent` (ID 26): un evento di blocco, per esempio l'apertura di un forziere. Byte da gophertunnel v1.63.0
//! (`packet/block_event.go`): posizione (`BlockPos`), tipo e dati, ciascuno varint con segno.
use crate::generated::BlockPos;
use crate::wire::{Encode, ServerPacket, Writer};

/// Tipo di evento "cambia lo stato del forziere" (`BlockEventChangeChestState`): dati 1 = aperto, 0 = chiuso.
pub const CHANGE_CHEST_STATE: i32 = 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockEvent {
    pub position: BlockPos,
    pub event_type: i32,
    pub event_data: i32,
}

impl Encode for BlockEvent {
    fn encode(&self, w: &mut Writer) {
        self.position.encode(w);
        w.var_i32(self.event_type);
        w.var_i32(self.event_data);
    }
}

impl ServerPacket for BlockEvent {
    const ID: u32 = 26;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_chest_event_bytes() {
        let p = BlockEvent { position: BlockPos { x: 3, y: 11, z: -4 }, event_type: CHANGE_CHEST_STATE, event_data: 1 }.packet();
        assert_eq!(p.id, 26);
        // zigzag: 3 -> 6, 11 -> 22, -4 -> 7; tipo 1 -> 2; dati 1 -> 2
        assert_eq!(p.body, [6, 22, 7, 2, 2]);
        let closed = BlockEvent { position: BlockPos::default(), event_type: CHANGE_CHEST_STATE, event_data: 0 }.packet();
        assert_eq!(closed.body, [0, 0, 0, 2, 0]);
    }
}
