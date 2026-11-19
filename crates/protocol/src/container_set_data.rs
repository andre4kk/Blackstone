//! Pacchetto `ContainerSetData` (ID 51): un dato di una finestra aperta (le barre della fornace). Byte da gophertunnel v1.63.0
//! (`packet/container_set_data.go`): ID della finestra (byte), chiave e valore (varint con segno).
use crate::wire::{Encode, ServerPacket, Writer};

/// Chiavi per la fornace (`ContainerDataFurnace*`).
pub const FURNACE_TICK_COUNT: i32 = 0;
pub const FURNACE_LIT_TIME: i32 = 1;
pub const FURNACE_LIT_DURATION: i32 = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContainerSetData {
    pub window_id: u8,
    pub key: i32,
    pub value: i32,
}

impl Encode for ContainerSetData {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.window_id);
        w.var_i32(self.key);
        w.var_i32(self.value);
    }
}

impl ServerPacket for ContainerSetData {
    const ID: u32 = 51;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn furnace_progress_bytes() {
        let p = ContainerSetData { window_id: 7, key: FURNACE_LIT_TIME, value: 1600 }.packet();
        assert_eq!(p.id, 51);
        // zigzag: 1 -> 2; 1600 -> 3200 = 0x80 0x19
        assert_eq!(p.body, [7, 2, 0x80, 0x19]);
    }
}
