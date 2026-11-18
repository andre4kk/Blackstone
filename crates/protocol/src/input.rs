//! Decoder dei pacchetti client -> server di input, transazioni e richieste di inventario (layout in
//! `docs/superpowers/notes/2026-11-18-protocollo-tappa1.md`, sezioni 3-5 e 7). Un corpo malformato dà `None`, mai un panico.
use crate::generated::MobEquipment;
use crate::item::ItemStack;
use crate::reader::Reader;

/// Limite di elementi per lista (azioni, richieste, azioni di blocco): lo stesso dello schema Mojang.
const MAX_LIST: u32 = 100;
/// I flag di input sono id 0..=127: stanno in un `u128`.
const MAX_FLAG_ID: i32 = 127;

/// Indici dei flag di `PlayerAuthInput` (`PlayerAuthInputData`).
pub mod flag {
    pub const JUMPING: u32 = 6;
    pub const SNEAKING: u32 = 8;
    pub const START_SNEAKING: u32 = 27;
    pub const STOP_SNEAKING: u32 = 28;
    pub const PERFORM_ITEM_INTERACTION: u32 = 34;
    pub const PERFORM_BLOCK_ACTIONS: u32 = 35;
    pub const PERFORM_ITEM_STACK_REQUEST: u32 = 36;
}

/// `ItemUseActionType`.
pub mod use_item {
    pub const PLACE: i32 = 0;
    pub const USE: i32 = 1;
    pub const DESTROY: i32 = 2;
}

/// `PlayerActionType` (solo quelli che il server usa).
pub mod player_action {
    pub const START_BREAK: i32 = 0;
    pub const ABORT_BREAK: i32 = 1;
    pub const STOP_BREAK: i32 = 2;
    pub const CREATIVE_DESTROY: i32 = 13;
    pub const CRACK: i32 = 18;
    pub const PREDICT_DESTROY: i32 = 26;
    pub const CONTINUE_DESTROY: i32 = 27;
}

/// `ActionType` delle azioni di una richiesta di pila (u8 sul filo).
mod stack_type {
    pub const TAKE: u8 = 0;
    pub const PLACE: u8 = 1;
    pub const SWAP: u8 = 2;
    pub const DROP: u8 = 3;
    pub const DESTROY: u8 = 4;
    pub const CONSUME: u8 = 5;
    pub const CREATE: u8 = 6;
    pub const CRAFT_CREATIVE: u8 = 14;
}

fn vec2(r: &mut Reader) -> Option<[f32; 2]> {
    Some([r.f32()?, r.f32()?])
}

/// Lista con contatore VarUInt32 limitato a `MAX_LIST`. Non si prealloca: un contatore falso non costa memoria.
fn list<T>(r: &mut Reader, mut item: impl FnMut(&mut Reader) -> Option<T>) -> Option<Vec<T>> {
    let n = r.var_u32()?;
    if n > MAX_LIST {
        return None;
    }
    (0..n).map(|_| item(r)).collect()
}

/// Uso di un oggetto (tipo 2 di `InventoryTransaction`, o transazione impacchettata in `PlayerAuthInput`).
#[derive(Clone, Debug, PartialEq)]
pub struct UseItem {
    pub action: i32,
    pub trigger: u8,
    pub pos: [i32; 3],
    pub face: u8,
    pub hotbar_slot: i32,
    pub hand: u8,
    /// Oggetto in mano secondo il client.
    pub held: ItemStack,
    /// Posizione degli occhi.
    pub player_pos: [f32; 3],
    /// Punto cliccato, relativo al blocco.
    pub click_pos: [f32; 3],
    /// Hash del blocco colpito secondo il client.
    pub block_runtime_id: u32,
    pub prediction: u8,
    pub cooldown: u8,
}

impl UseItem {
    /// Dal tipo di azione in poi (P§4.2 a-l).
    fn read(r: &mut Reader) -> Option<Self> {
        Some(Self {
            action: r.var_i32()?,
            trigger: r.u8()?,
            pos: r.block_pos()?,
            face: r.u8()?,
            hotbar_slot: r.var_i32()?,
            hand: r.u8()?,
            held: ItemStack::decode(r)?,
            player_pos: r.vec3()?,
            click_pos: r.vec3()?,
            block_runtime_id: r.var_u32()?,
            prediction: r.u8()?,
            cooldown: r.u8()?,
        })
    }
}

/// Salta gli slot legacy opzionali: presenza + [contenitore u8, lista di slot u8].
fn skip_legacy_slots(r: &mut Reader) -> Option<()> {
    r.optional(|r| {
        for _ in 0..r.var_u32()? {
            r.u8()?;
            for _ in 0..r.var_u32()? {
                r.u8()?;
            }
        }
        Some(())
    })?;
    Some(())
}

/// Salta le `InventoryAction`: il server non le usa, l'inventario passa dalle richieste di pila.
fn skip_inventory_actions(r: &mut Reader) -> Option<()> {
    list(r, |r| {
        r.var_u32()?; // tipo di sorgente
        r.optional(|r| r.i8())?; // id contenitore
        r.optional(|r| r.var_u32())?; // flag
        r.var_u32()?; // slot
        ItemStack::decode(r)?; // da
        ItemStack::decode(r)?; // a
        Some(())
    })?;
    Some(())
}

/// Pacchetto 30. Solo il tipo `ItemUse` (2) porta dati che ci servono.
#[derive(Clone, Debug, PartialEq)]
pub struct InventoryTransaction {
    pub legacy_request_id: i32,
    pub kind: u32,
    pub use_item: Option<UseItem>,
}

impl InventoryTransaction {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        let legacy_request_id = r.var_i32()?;
        skip_legacy_slots(r)?;
        let kind = r.var_u32()?;
        skip_inventory_actions(r)?;
        let use_item = if kind == 2 { Some(UseItem::read(r)?) } else { None };
        Some(Self { legacy_request_id, kind, use_item })
    }
}

/// Azione sui blocchi dentro `PlayerAuthInput`.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockAction {
    pub action: i32,
    pub pos: [i32; 3],
    pub face: i32,
}

impl BlockAction {
    fn read(r: &mut Reader) -> Option<Self> {
        Some(Self { action: r.var_i32()?, pos: r.block_pos()?, face: r.var_i32()? })
    }
}

/// Pacchetto 36.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerAction {
    pub runtime_id: u64,
    pub action: i32,
    pub pos: [i32; 3],
    pub result_pos: [i32; 3],
    pub face: i32,
}

impl PlayerAction {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        Some(Self { runtime_id: r.var_u64()?, action: r.var_i32()?, pos: r.block_pos()?, result_pos: r.block_pos()?, face: r.var_i32()? })
    }
}

/// Uno slot di un contenitore nelle richieste. L'ID di pila è un i32 fisso (come gophertunnel).
#[derive(Clone, Debug, PartialEq)]
pub struct SlotRef {
    pub container: u8,
    pub dynamic_id: Option<u32>,
    pub slot: u8,
    pub stack_id: i32,
}

impl SlotRef {
    fn read(r: &mut Reader) -> Option<Self> {
        Some(Self { container: r.u8()?, dynamic_id: r.optional(|r| r.u32())?, slot: r.u8()?, stack_id: r.i32()? })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StackAction {
    Take {
        count: u8,
        src: SlotRef,
        dst: SlotRef,
    },
    Place {
        count: u8,
        src: SlotRef,
        dst: SlotRef,
    },
    Swap {
        src: SlotRef,
        dst: SlotRef,
    },
    Drop {
        count: u8,
        src: SlotRef,
        randomly: bool,
    },
    Destroy {
        count: u8,
        src: SlotRef,
    },
    CraftCreative {
        net_id: u32,
        crafts: u8,
    },
    /// Azione letta ma che il server non gestisce (`Consume`, `Create`): porta il suo `ActionType`.
    Other(u8),
}

impl StackAction {
    /// `Some(None)`: tipo di azione di cui non si conosce il payload, il resto della richiesta non è più leggibile.
    fn read(r: &mut Reader) -> Option<Option<Self>> {
        r.var_u32()?; // indice di variante: ridondante con il tipo
        let ty = r.u8()?;
        Some(Some(match ty {
            stack_type::TAKE => Self::Take { count: r.u8()?, src: SlotRef::read(r)?, dst: SlotRef::read(r)? },
            stack_type::PLACE => Self::Place { count: r.u8()?, src: SlotRef::read(r)?, dst: SlotRef::read(r)? },
            stack_type::SWAP => Self::Swap { src: SlotRef::read(r)?, dst: SlotRef::read(r)? },
            stack_type::DROP => Self::Drop { count: r.u8()?, src: SlotRef::read(r)?, randomly: r.bool()? },
            stack_type::DESTROY => Self::Destroy { count: r.u8()?, src: SlotRef::read(r)? },
            stack_type::CRAFT_CREATIVE => Self::CraftCreative { net_id: r.var_u32()?, crafts: r.u8()? },
            stack_type::CONSUME => {
                r.u8()?;
                SlotRef::read(r)?;
                Self::Other(ty)
            }
            stack_type::CREATE => {
                r.u8()?;
                Self::Other(ty)
            }
            _ => return Some(None),
        }))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StackRequest {
    pub request_id: i32,
    pub actions: Vec<StackAction>,
}

impl StackRequest {
    /// `None` = corpo malformato; `Some(None)` = richiesta con un'azione sconosciuta, scartata intera (e il flusso non è più leggibile).
    fn read(r: &mut Reader) -> Option<Option<Self>> {
        let request_id = r.var_i32()?;
        let count = r.var_u32()?;
        if count > MAX_LIST {
            return None;
        }
        let mut actions = Vec::new();
        for _ in 0..count {
            match StackAction::read(r)? {
                Some(a) => actions.push(a),
                None => return Some(None),
            }
        }
        list(r, |r| r.string().map(|_| ()))?; // stringhe da filtrare
        r.i32()?; // origine del testo
        Some(Some(Self { request_id, actions }))
    }
}

/// Pacchetto 147.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemStackRequestPacket {
    pub requests: Vec<StackRequest>,
}

impl ItemStackRequestPacket {
    /// Una richiesta con un'azione sconosciuta e tutte le seguenti si scartano; quelle già lette restano.
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        let count = r.var_u32()?;
        if count > MAX_LIST {
            return None;
        }
        let mut requests = Vec::new();
        for _ in 0..count {
            match StackRequest::read(r)? {
                Some(q) => requests.push(q),
                None => break,
            }
        }
        Some(Self { requests })
    }
}

/// Pacchetto 144: input del giocatore a ogni tick.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerAuthInput {
    pub pitch: f32,
    pub yaw: f32,
    /// Posizione degli occhi.
    pub position: [f32; 3],
    pub move_vector: [f32; 2],
    pub head_yaw: f32,
    /// Un bit per ogni flag attivo (il filo porta la lista degli id).
    pub flags: u128,
    pub input_mode: u32,
    pub play_mode: u32,
    pub interaction_model: i32,
    pub interact_rotation: [f32; 2],
    pub tick: u64,
    pub delta: [f32; 3],
    pub item_use: Option<UseItem>,
    pub stack_request: Option<StackRequest>,
    pub block_actions: Vec<BlockAction>,
}

impl PlayerAuthInput {
    pub fn has(&self, flag: u32) -> bool {
        flag <= MAX_FLAG_ID as u32 && self.flags >> flag & 1 == 1
    }

    /// Solo rotazione e posizione (i primi 5 f32): la parte del layout verificata sul client reale. Serve da ripiego
    /// quando `decode` rifiuta il resto del corpo.
    pub fn decode_rotation_position(body: &[u8]) -> Option<(f32, f32, [f32; 3])> {
        let r = &mut Reader(body);
        Some((r.f32()?, r.f32()?, r.vec3()?))
    }

    /// Dopo `block_actions` si leggono veicolo, vettore analogico, camera e vettore grezzo, ma non si conservano.
    /// Una richiesta di pila con un'azione sconosciuta si scarta e il resto del corpo si abbandona: restano i campi già letti.
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        let (pitch, yaw, position, move_vector, head_yaw) = (r.f32()?, r.f32()?, r.vec3()?, vec2(r)?, r.f32()?);
        let mut flags = 0u128;
        let n = r.var_u32()?;
        if n > MAX_FLAG_ID as u32 + 1 {
            return None;
        }
        for _ in 0..n {
            let id = r.var_i32()?;
            if !(0..=MAX_FLAG_ID).contains(&id) {
                return None;
            }
            flags |= 1 << id;
        }
        let mut p = Self {
            pitch,
            yaw,
            position,
            move_vector,
            head_yaw,
            flags,
            input_mode: r.var_u32()?,
            play_mode: r.var_u32()?,
            interaction_model: r.var_i32()?,
            interact_rotation: vec2(r)?,
            tick: r.var_u64()?,
            delta: r.vec3()?,
            item_use: None,
            stack_request: None,
            block_actions: Vec::new(),
        };
        p.item_use = r.optional(|r| {
            r.var_i32()?; // id legacy della richiesta
            skip_legacy_slots(r)?;
            skip_inventory_actions(r)?;
            UseItem::read(r)
        })?;
        if r.bool()? {
            match StackRequest::read(r)? {
                Some(q) => p.stack_request = Some(q),
                None => return Some(p),
            }
        }
        p.block_actions = r.optional(|r| list(r, BlockAction::read))?.unwrap_or_default();
        r.optional(vec2)?; // rotazione del veicolo
        r.optional(|r| r.var_i64())?; // veicolo predetto
        vec2(r)?; // vettore analogico
        r.vec3()?; // orientamento della camera
        vec2(r)?; // vettore grezzo
        Some(p)
    }
}

impl MobEquipment {
    /// Il pacchetto 31 viaggia nei due versi: la codifica è nel codice generato, qui c'è solo la lettura.
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        Some(Self { target_runtime_id: r.var_u64()?, item: ItemStack::decode(r)?, slot: r.u8()?, selected_slot: r.u8()?, container_id: r.u8()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Writer;

    /// I 53 byte reali di P§4.4 (un Place su un blocco, con l'aria in mano).
    const REAL_CAPTURE: [u8; 53] = [
        0x00, 0x00, 0x02, 0x00, 0x00, 0x01, 0xb5, 0x02, 0x90, 0x01, 0x03, 0x02, 0x00, 0x00, // fino alla mano
        0, 0, 0, 0, 0, 0, 0, 0, // aria
        0xa4, 0x64, 0x19, 0xc3, 0x58, 0x8d, 0x93, 0x42, 0xf7, 0xa0, 0x71, 0xc0, // posizione giocatore
        0x00, 0x82, 0x5b, 0x3f, 0x80, 0x43, 0x0f, 0x3f, 0xc0, 0xcc, 0xcc, 0x3c, // posizione del click
        0xdd, 0xa9, 0xc0, 0xa9, 0x05, // blocco bersaglio
        0x01, 0x00, // predizione, cooldown
    ];

    /// Prefisso di PlayerAuthInput fino al delta: pitch 0, yaw 90, posizione (1, 2, 3), movimento (0, 0), head yaw 90.
    fn pai_head(w: &mut Writer, flags: &[i32]) {
        for v in [0.0, 90.0, 1.0, 2.0, 3.0, 0.0, 0.0, 90.0] {
            w.f32(v);
        }
        w.var_u32(flags.len() as u32);
        flags.iter().for_each(|&f| w.var_i32(f));
        w.var_u32(1); // modalità di input
        w.var_u32(0); // modalità di gioco
        w.var_i32(2); // modello di interazione
        w.f32(0.0);
        w.f32(0.0);
        w.var_u64(300); // tick
        for v in [0.0, -0.1, 0.0] {
            w.f32(v);
        }
    }

    /// Coda: veicolo assente (due opzionali), poi vettore analogico, orientamento camera, vettore grezzo.
    fn pai_tail(w: &mut Writer) {
        w.u8(0);
        w.u8(0);
        for v in [0.0; 7] {
            w.f32(v);
        }
    }

    fn slot(w: &mut Writer, container: u8, slot: u8, stack_id: i32) {
        w.u8(container);
        w.u8(0); // niente dynamic id
        w.u8(slot);
        w.i32(stack_id);
    }

    fn pai_minimal() -> Vec<u8> {
        let mut w = Writer::default();
        pai_head(&mut w, &[]);
        w.u8(0); // niente transazione
        w.u8(0); // niente richiesta
        w.u8(0); // niente azioni di blocco
        pai_tail(&mut w);
        w.0
    }

    fn pai_with_block_actions() -> Vec<u8> {
        let mut w = Writer::default();
        pai_head(&mut w, &[6, 35]);
        w.u8(0);
        w.u8(0);
        w.u8(1);
        w.var_u32(2);
        for (action, pos, face) in [(0, [-5, 64, 7], 1), (2, [10, -60, 300], 4)] {
            w.var_i32(action);
            pos.iter().for_each(|&c| w.var_i32(c));
            w.var_i32(face);
        }
        pai_tail(&mut w);
        w.0
    }

    fn pai_with_request() -> Vec<u8> {
        let mut w = Writer::default();
        pai_head(&mut w, &[36]);
        w.u8(0);
        w.u8(1);
        w.var_i32(-1); // id richiesta
        w.var_u32(2);
        w.var_u32(12); // variante CraftCreative
        w.u8(14);
        w.var_u32(7);
        w.u8(1);
        w.var_u32(1); // variante Place
        w.u8(1);
        w.u8(64);
        slot(&mut w, 60, 50, 99);
        slot(&mut w, 28, 0, 0);
        w.var_u32(0); // stringhe da filtrare
        w.i32(-1); // origine
        w.u8(0);
        pai_tail(&mut w);
        w.0
    }

    #[test]
    fn inventory_transaction_real_capture() {
        let t = InventoryTransaction::decode(&REAL_CAPTURE).expect("leggibile");
        assert_eq!((t.legacy_request_id, t.kind), (0, 2));
        let u = t.use_item.expect("transazione di uso");
        assert_eq!((u.action, u.trigger, u.pos, u.face), (use_item::PLACE, 1, [-155, 72, -2], 2));
        assert_eq!((u.hotbar_slot, u.hand, u.held), (0, 0, ItemStack::air()));
        assert_eq!(u.block_runtime_id, 0x553014dd);
        assert_eq!((u.prediction, u.cooldown), (1, 0));
        assert!((u.player_pos[0] + 153.393).abs() < 1e-3 && (u.click_pos[1] - 0.560).abs() < 1e-3);
        for n in 0..REAL_CAPTURE.len() {
            assert!(InventoryTransaction::decode(&REAL_CAPTURE[..n]).is_none(), "tagliato a {n}");
        }
    }

    #[test]
    fn inventory_transaction_of_other_kind_has_no_use_item() {
        let t = InventoryTransaction::decode(&[0, 0, 1, 0]).expect("Mismatch senza azioni");
        assert_eq!((t.kind, t.use_item), (1, None));
    }

    #[test]
    fn inventory_transaction_skips_legacy_slots_and_actions() {
        let mut w = Writer::default();
        w.var_i32(-2);
        w.u8(1); // slot legacy presenti
        w.var_u32(1);
        w.u8(12);
        w.var_u32(2);
        w.u8(3);
        w.u8(4);
        w.var_u32(2); // tipo ItemUse
        w.var_u32(1); // una azione
        w.var_u32(0); // sorgente: contenitore
        w.u8(1);
        w.i8(0);
        w.u8(1);
        w.var_u32(1); // flag
        w.var_u32(5); // slot
        ItemStack::air().encode(&mut w);
        ItemStack::air().encode(&mut w);
        w.raw(&REAL_CAPTURE[4..]); // dal tipo di azione in poi
        let t = InventoryTransaction::decode(&w.0).expect("leggibile");
        assert_eq!(t.legacy_request_id, -2);
        assert_eq!(t.use_item.expect("uso").pos, [-155, 72, -2]);
    }

    #[test]
    fn player_auth_input_minimal() {
        let p = PlayerAuthInput::decode(&pai_minimal()).expect("leggibile");
        assert_eq!((p.pitch, p.yaw, p.position, p.head_yaw), (0.0, 90.0, [1.0, 2.0, 3.0], 90.0));
        assert_eq!((p.flags, p.input_mode, p.play_mode, p.interaction_model, p.tick), (0, 1, 0, 2, 300));
        assert_eq!(p.delta, [0.0, -0.1, 0.0]);
        assert!(p.item_use.is_none() && p.stack_request.is_none() && p.block_actions.is_empty());
        assert!(!p.has(flag::JUMPING));
    }

    #[test]
    fn player_auth_input_with_block_actions() {
        let p = PlayerAuthInput::decode(&pai_with_block_actions()).expect("leggibile");
        assert!(p.has(flag::JUMPING) && p.has(flag::PERFORM_BLOCK_ACTIONS) && !p.has(flag::SNEAKING));
        assert_eq!(p.block_actions, [BlockAction { action: 0, pos: [-5, 64, 7], face: 1 }, BlockAction { action: 2, pos: [10, -60, 300], face: 4 }]);
    }

    #[test]
    fn player_auth_input_with_creative_request() {
        let p = PlayerAuthInput::decode(&pai_with_request()).expect("leggibile");
        assert!(p.has(flag::PERFORM_ITEM_STACK_REQUEST));
        let req = p.stack_request.expect("richiesta");
        assert_eq!(req.request_id, -1);
        assert_eq!(
            req.actions,
            [
                StackAction::CraftCreative { net_id: 7, crafts: 1 },
                StackAction::Place {
                    count: 64,
                    src: SlotRef { container: 60, dynamic_id: None, slot: 50, stack_id: 99 },
                    dst: SlotRef { container: 28, dynamic_id: None, slot: 0, stack_id: 0 },
                },
            ]
        );
    }

    #[test]
    fn player_auth_input_with_item_use() {
        let mut w = Writer::default();
        pai_head(&mut w, &[34]);
        w.u8(1);
        w.var_i32(0); // id legacy
        w.u8(0); // niente slot legacy
        w.var_u32(0); // niente azioni
        w.raw(&REAL_CAPTURE[4..]);
        w.u8(0);
        w.u8(0);
        pai_tail(&mut w);
        let p = PlayerAuthInput::decode(&w.0).expect("leggibile");
        let u = p.item_use.expect("uso");
        assert_eq!((u.action, u.pos, u.face, u.block_runtime_id), (use_item::PLACE, [-155, 72, -2], 2, 0x553014dd));
    }

    #[test]
    fn player_auth_input_flag_ids_out_of_range_are_invalid() {
        for bad in [128, -1, 1000] {
            let mut w = Writer::default();
            pai_head(&mut w, &[bad]);
            w.raw(&[0, 0, 0]);
            pai_tail(&mut w);
            assert!(PlayerAuthInput::decode(&w.0).is_none(), "flag {bad}");
        }
        let mut w = Writer::default();
        pai_head(&mut w, &[127]);
        w.raw(&[0, 0, 0]);
        pai_tail(&mut w);
        assert!(PlayerAuthInput::decode(&w.0).expect("127 è valido").has(127));
    }

    #[test]
    fn decode_rotation_position_reads_only_the_first_five_floats() {
        let body = pai_minimal();
        assert_eq!(PlayerAuthInput::decode_rotation_position(&body), Some((0.0, 90.0, [1.0, 2.0, 3.0])));
        // il corpo intero viene rifiutato, il ripiego no
        assert!(PlayerAuthInput::decode(&body[..24]).is_none());
        assert_eq!(PlayerAuthInput::decode_rotation_position(&body[..20]), Some((0.0, 90.0, [1.0, 2.0, 3.0])));
        assert_eq!(PlayerAuthInput::decode_rotation_position(&body[..19]), None);
        assert_eq!(PlayerAuthInput::decode_rotation_position(&[]), None);
    }

    #[test]
    fn truncated_bodies_return_none() {
        for body in [pai_minimal(), pai_with_block_actions(), pai_with_request()] {
            assert!(PlayerAuthInput::decode(&body).is_some());
            for n in 0..body.len() {
                assert!(PlayerAuthInput::decode(&body[..n]).is_none(), "tagliato a {n} di {}", body.len());
            }
        }
    }

    #[test]
    fn item_stack_request_with_zero_actions_is_empty() {
        let mut w = Writer::default();
        w.var_u32(1);
        w.var_i32(-3);
        w.var_u32(0);
        w.var_u32(0);
        w.i32(-1);
        let p = ItemStackRequestPacket::decode(&w.0).expect("leggibile");
        assert_eq!(p.requests.len(), 1);
        assert_eq!((p.requests[0].request_id, p.requests[0].actions.len()), (-3, 0));
        assert_eq!(ItemStackRequestPacket::decode(&[0]).expect("nessuna richiesta").requests.len(), 0);

        let mut w = Writer::default();
        pai_head(&mut w, &[36]);
        w.u8(0);
        w.u8(1);
        w.var_i32(-5);
        w.var_u32(0);
        w.var_u32(0);
        w.i32(-1);
        w.u8(0);
        pai_tail(&mut w);
        let p = PlayerAuthInput::decode(&w.0).expect("PAI con richiesta vuota");
        assert!(p.stack_request.expect("richiesta").actions.is_empty());
    }

    #[test]
    fn item_stack_request_all_known_actions() {
        let mut w = Writer::default();
        w.var_u32(1);
        w.var_i32(-7);
        w.var_u32(6);
        // Take, con dynamic id sulla destinazione
        w.var_u32(0);
        w.u8(0);
        w.u8(3);
        slot(&mut w, 29, 1, 4);
        w.u8(59);
        w.u8(1);
        w.u32(77);
        w.u8(0);
        w.i32(0);
        // Swap
        w.var_u32(2);
        w.u8(2);
        slot(&mut w, 28, 0, 1);
        slot(&mut w, 28, 1, 2);
        // Drop
        w.var_u32(3);
        w.u8(3);
        w.u8(5);
        slot(&mut w, 28, 0, 1);
        w.bool(true);
        // Destroy
        w.var_u32(4);
        w.u8(4);
        w.u8(64);
        slot(&mut w, 59, 0, 8);
        // Consume (payload noto, saltato) e Create
        w.var_u32(5);
        w.u8(5);
        w.u8(1);
        slot(&mut w, 28, 0, 1);
        w.var_u32(6);
        w.u8(6);
        w.u8(0);
        w.var_u32(0);
        w.i32(-1);
        let p = ItemStackRequestPacket::decode(&w.0).expect("leggibile");
        let a = &p.requests[0].actions;
        assert_eq!(a.len(), 6);
        let StackAction::Take { count: 3, src, dst } = &a[0] else { panic!("{:?}", a[0]) };
        assert_eq!((src.stack_id, dst.dynamic_id, dst.container), (4, Some(77), 59));
        assert!(matches!(a[1], StackAction::Swap { .. }));
        assert!(matches!(&a[2], StackAction::Drop { count: 5, randomly: true, .. }));
        assert!(matches!(&a[3], StackAction::Destroy { count: 64, .. }));
        assert_eq!((&a[4], &a[5]), (&StackAction::Other(5), &StackAction::Other(6)));
    }

    #[test]
    fn unknown_action_discards_the_request() {
        let mut w = Writer::default();
        w.var_u32(2);
        // prima richiesta valida
        w.var_i32(-1);
        w.var_u32(1);
        w.var_u32(12);
        w.u8(14);
        w.var_u32(3);
        w.u8(1);
        w.var_u32(0);
        w.i32(-1);
        // seconda con un'azione sconosciuta: il resto non è leggibile
        w.var_i32(-3);
        w.var_u32(1);
        w.var_u32(9);
        w.u8(11);
        w.raw(&[1, 2, 3]);
        let p = ItemStackRequestPacket::decode(&w.0).expect("la prima richiesta si salva");
        assert_eq!(p.requests.len(), 1);
        assert_eq!(p.requests[0].request_id, -1);

        // dentro PAI la richiesta sconosciuta si scarta e il resto del corpo si abbandona, ma la posizione resta
        let mut w = Writer::default();
        pai_head(&mut w, &[36]);
        w.u8(0);
        w.u8(1);
        w.var_i32(-3);
        w.var_u32(1);
        w.var_u32(9);
        w.u8(11);
        w.raw(&[1, 2, 3]);
        let p = PlayerAuthInput::decode(&w.0).expect("posizione leggibile");
        assert!(p.stack_request.is_none() && p.block_actions.is_empty());
        assert_eq!(p.position, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn item_stack_request_packet_truncation_and_limits() {
        let mut w = Writer::default();
        w.var_u32(1);
        w.var_i32(-1);
        w.var_u32(1);
        w.var_u32(12);
        w.u8(14);
        w.var_u32(3);
        w.u8(1);
        w.var_u32(1);
        w.string("ab");
        w.i32(-1);
        assert!(ItemStackRequestPacket::decode(&w.0).is_some());
        for n in 0..w.0.len() {
            assert!(ItemStackRequestPacket::decode(&w.0[..n]).is_none(), "tagliato a {n}");
        }
        assert!(ItemStackRequestPacket::decode(&[101]).is_none(), "troppe richieste");
        assert!(ItemStackRequestPacket::decode(&[1, 1, 101]).is_none(), "troppe azioni");
    }

    #[test]
    fn player_action_decodes_and_rejects_truncation() {
        let mut w = Writer::default();
        w.var_u64(4);
        w.var_i32(player_action::CREATIVE_DESTROY);
        for c in [-155, 72, -2, 0, 0, 0] {
            w.var_i32(c);
        }
        w.var_i32(2);
        let a = PlayerAction::decode(&w.0).expect("leggibile");
        assert_eq!(a, PlayerAction { runtime_id: 4, action: 13, pos: [-155, 72, -2], result_pos: [0, 0, 0], face: 2 });
        for n in 0..w.0.len() {
            assert!(PlayerAction::decode(&w.0[..n]).is_none(), "tagliato a {n}");
        }
    }

    #[test]
    fn mob_equipment_decodes_what_the_generated_encoder_writes() {
        use crate::wire::Encode;
        let m = MobEquipment {
            target_runtime_id: 9,
            item: ItemStack { id: 1, count: 3, aux: 0, net_id: Some(4), block_runtime_id: 5, user_data: ItemStack::empty_user_data() },
            slot: 2,
            selected_slot: 2,
            container_id: 0,
        };
        let bytes = m.to_bytes();
        assert_eq!(MobEquipment::decode(&bytes), Some(m));
        for n in 0..bytes.len() {
            assert!(MobEquipment::decode(&bytes[..n]).is_none(), "tagliato a {n}");
        }
    }
}
