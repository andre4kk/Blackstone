//! Decoder dei pacchetti client -> server di input, transazioni e richieste di inventario (layout in
//! `docs/superpowers/notes/2026-11-18-protocollo-tappa1.md`, sezioni 3-5 e 7). Un corpo malformato dà `None`, mai un panico.
use crate::generated::{ContainerClose, MobEquipment};
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
    pub const START_SPRINTING: u32 = 25;
    pub const STOP_SPRINTING: u32 = 26;
    pub const START_SNEAKING: u32 = 27;
    pub const STOP_SNEAKING: u32 = 28;
    pub const START_SWIMMING: u32 = 29;
    pub const STOP_SWIMMING: u32 = 30;
    /// `InputFlagStartJumping` (gophertunnel `player_auth_input.go`): il client ha iniziato un salto.
    pub const START_JUMPING: u32 = 31;
    pub const START_GLIDING: u32 = 32;
    pub const STOP_GLIDING: u32 = 33;
    /// Il client vuole iniziare/smettere di volare (doppio salto): il server risponde con `UpdateAbilities`.
    pub const START_FLYING: u32 = 42;
    pub const STOP_FLYING: u32 = 43;
    pub const PERFORM_ITEM_INTERACTION: u32 = 34;
    pub const PERFORM_BLOCK_ACTIONS: u32 = 35;
    pub const PERFORM_ITEM_STACK_REQUEST: u32 = 36;
    /// Il client ha urtato qualcosa in verticale (a terra o contro un soffitto).
    pub const VERTICAL_COLLISION: u32 = 50;
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
    /// Il client morto chiede di rinascere (`PlayerActionRespawn` di gophertunnel).
    pub const RESPAWN: i32 = 7;
    pub const CREATIVE_DESTROY: i32 = 13;
    pub const CRACK: i32 = 18;
    pub const PREDICT_DESTROY: i32 = 26;
    pub const CONTINUE_DESTROY: i32 = 27;
}

/// `ActionType` delle azioni di una richiesta di pila (u8 sul filo).
pub mod stack_type {
    pub const TAKE: u8 = 0;
    pub const PLACE: u8 = 1;
    pub const SWAP: u8 = 2;
    pub const DROP: u8 = 3;
    pub const DESTROY: u8 = 4;
    pub const CONSUME: u8 = 5;
    pub const CREATE: u8 = 6;
    /// Ricetta scelta (gophertunnel `StackRequestActionCraftRecipe`): il server consuma gli ingredienti e crea il risultato.
    pub const CRAFT_RECIPE: u8 = 12;
    /// Ricetta fatta dal libro delle ricette con Maiusc (`StackRequestActionCraftRecipeAuto`): gli ingredienti si prendono anche dall'inventario.
    pub const CRAFT_RECIPE_AUTO: u8 = 13;
    pub const CRAFT_CREATIVE: u8 = 14;
    /// Risultati del crafting (deprecato ma il client lo manda ancora dopo `CraftCreative`): solo informativo.
    pub const CRAFT_RESULTS: u8 = 20;
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

/// Caselle che il client chiede di rimandargli (`LegacySetItemSlots`): presenza + [contenitore u8, lista di caselle u8].
/// Il client le nomina poi con l'ID della sua richiesta legacy finché il server non le rimanda con l'ID vero.
pub type LegacySlots = Vec<(u8, Vec<u8>)>;

fn read_legacy_slots(r: &mut Reader) -> Option<LegacySlots> {
    let slots = r.optional(|r| {
        let mut out = Vec::new();
        for _ in 0..r.var_u32()? {
            let container = r.u8()?;
            let mut slots = Vec::new();
            for _ in 0..r.var_u32()? {
                slots.push(r.u8()?);
            }
            out.push((container, slots));
        }
        Some(out)
    })?;
    Some(slots.unwrap_or_default())
}

/// Tipi di sorgente di una `InventoryAction` (`InventoryActionSource*` di gophertunnel).
pub mod action_source {
    pub const CONTAINER: u32 = 0;
    pub const WORLD: u32 = 2;
    pub const CREATIVE: u32 = 3;
}

/// Un'azione di una transazione (`InventoryAction.Marshal` di gophertunnel): per buttare un oggetto con Q sono due, una sul mondo e una sull'inventario.
#[derive(Clone, Debug, PartialEq)]
pub struct InventoryAction {
    /// Uno di [`action_source`].
    pub source_type: u32,
    pub window_id: Option<i8>,
    pub source_flags: Option<u32>,
    pub slot: u32,
    pub old_item: ItemStack,
    pub new_item: ItemStack,
}

fn read_inventory_actions(r: &mut Reader) -> Option<Vec<InventoryAction>> {
    list(r, |r| {
        Some(InventoryAction {
            source_type: r.var_u32()?,
            window_id: r.optional(|r| r.i8())?,
            source_flags: r.optional(|r| r.var_u32())?,
            slot: r.var_u32()?,
            old_item: ItemStack::decode(r)?,
            new_item: ItemStack::decode(r)?,
        })
    })
}

/// Salta le `InventoryAction` (dentro `PlayerAuthInput` non servono: l'inventario passa dalle richieste di pila).
fn skip_inventory_actions(r: &mut Reader) -> Option<()> {
    read_inventory_actions(r).map(|_| ())
}

/// Rilascio dell'oggetto usato a lungo (tipo 4 di `InventoryTransaction`, `ReleaseItemTransactionData` di gophertunnel): il client smette di
/// tenere premuto mentre mangia, tende l'arco...
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseItem {
    /// Sempre 0 dal 1.13 (gophertunnel): rilasciare un cibo e un arco è la stessa cosa per il client.
    pub action: i32,
    pub hotbar_slot: i32,
    /// Oggetto in mano secondo il client.
    pub held: ItemStack,
    /// Posizione della testa.
    pub head_pos: [f32; 3],
    /// 0 = mano principale, 1 = secondaria.
    pub hand: u8,
}

impl ReleaseItem {
    fn read(r: &mut Reader) -> Option<Self> {
        Some(Self { action: r.var_i32()?, hotbar_slot: r.var_i32()?, held: ItemStack::decode(r)?, head_pos: r.vec3()?, hand: r.u8()? })
    }
}

/// `UseItemOnEntityActionType` di gophertunnel: interagire o attaccare.
pub mod use_entity {
    pub const INTERACT: i32 = 0;
    pub const ATTACK: i32 = 1;
}

/// Uso di un oggetto su un'entità (tipo 3 di `InventoryTransaction`, `UseItemOnEntityTransactionData` di gophertunnel v1.63.0): il colpo a un
/// giocatore, un bot o un mob.
#[derive(Clone, Debug, PartialEq)]
pub struct UseItemOnEntity {
    /// ID di runtime dell'entità colpita.
    pub target: u64,
    /// Uno di [`use_entity`].
    pub action: i32,
    pub hotbar_slot: i32,
    /// 0 = mano principale, 1 = secondaria.
    pub hand: u8,
    /// Oggetto in mano secondo il client.
    pub held: ItemStack,
    /// Posizione del giocatore (occhi) al momento del clic.
    pub player_pos: [f32; 3],
    /// Punto cliccato, relativo all'origine dell'entità.
    pub click_pos: [f32; 3],
}

impl UseItemOnEntity {
    fn read(r: &mut Reader) -> Option<Self> {
        Some(Self {
            target: r.var_u64()?,
            action: r.var_i32()?,
            hotbar_slot: r.var_i32()?,
            hand: r.u8()?,
            held: ItemStack::decode(r)?,
            player_pos: r.vec3()?,
            click_pos: r.vec3()?,
        })
    }
}

/// Pacchetto 30. Solo i tipi `ItemUse` (2), `UseItemOnEntity` (3) e `ReleaseItem` (4) portano dati che ci servono.
#[derive(Clone, Debug, PartialEq)]
pub struct InventoryTransaction {
    pub legacy_request_id: i32,
    pub legacy_slots: LegacySlots,
    pub kind: u32,
    /// Le azioni (per il tipo Normal: buttare un oggetto con Q).
    pub actions: Vec<InventoryAction>,
    pub use_item: Option<UseItem>,
    pub use_entity: Option<UseItemOnEntity>,
    pub release: Option<ReleaseItem>,
}

impl InventoryTransaction {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        let legacy_request_id = r.var_i32()?;
        let legacy_slots = read_legacy_slots(r)?;
        let kind = r.var_u32()?;
        let actions = read_inventory_actions(r)?;
        let use_item = if kind == 2 { Some(UseItem::read(r)?) } else { None };
        let use_entity = if kind == 3 { Some(UseItemOnEntity::read(r)?) } else { None };
        let release = if kind == 4 { Some(ReleaseItem::read(r)?) } else { None };
        Some(Self { legacy_request_id, legacy_slots, kind, actions, use_item, use_entity, release })
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

/// Stati di `Respawn` (`packet/respawn.go` di gophertunnel v1.63.0, `iota` da 0).
pub mod respawn_state {
    pub const SEARCHING_FOR_SPAWN: u8 = 0;
    /// Server -> client: la rinascita è pronta.
    pub const READY_TO_SPAWN: u8 = 1;
    /// Client -> server: il client ha finito e vuole rinascere.
    pub const CLIENT_READY_TO_SPAWN: u8 = 2;
}

/// `Respawn` (45) dal client: posizione (vec3 f32), stato (u8) e ID di runtime (varint u64), come `Respawn.Marshal` di gophertunnel.
#[derive(Clone, Debug, PartialEq)]
pub struct Respawn {
    pub position: [f32; 3],
    pub state: u8,
    pub runtime_id: u64,
}

impl Respawn {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        Some(Self { position: r.vec3()?, state: r.u8()?, runtime_id: r.var_u64()? })
    }
}

/// `Animate` (44) dal client: il braccio che oscilla. Sul filo l'azione è un u8 (schema 2225), non un varint come nei protocolli vecchi;
/// seguono dati f32 e due opzionali (origine dell'oscillazione, mano) che il server non usa.
#[derive(Clone, Debug, PartialEq)]
pub struct Animate {
    pub action: i32,
    pub runtime_id: u64,
}

impl Animate {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        Some(Self { action: r.u8()? as i32, runtime_id: r.var_u64()? })
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
    /// `CraftRecipe`: ID di rete della ricetta (da `CraftingData`) e quante volte.
    CraftRecipe {
        recipe_id: u32,
        crafts: u8,
    },
    /// `CraftRecipeAuto`: come `CraftRecipe`; gli ingredienti dichiarati dal client si leggono e si scartano (il server li ricava dalla ricetta).
    CraftRecipeAuto {
        recipe_id: u32,
        crafts: u8,
    },
    /// `Create`: prende il risultato `results_slot` tra quelli preparati da una ricetta con più risultati.
    Create {
        results_slot: u8,
    },
    /// Azione letta ma che il server non gestisce (`Consume`): porta il suo `ActionType`.
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
            stack_type::CREATE => Self::Create { results_slot: r.u8()? },
            stack_type::CRAFT_RECIPE => Self::CraftRecipe { recipe_id: r.var_u32()?, crafts: r.u8()? },
            stack_type::CRAFT_RECIPE_AUTO => {
                let (recipe_id, crafts) = (r.var_u32()?, r.u8()?);
                list(r, skip_descriptor_count)?;
                Self::CraftRecipeAuto { recipe_id, crafts }
            }
            stack_type::CRAFT_RESULTS => {
                list(r, skip_request_item)?;
                r.u8()?; // numero di crafting
                Self::Other(ty)
            }
            _ => return Some(None),
        }))
    }
}

/// Salta un `ItemDescriptorCount` (`StackRequestItemDescriptorCount` di gophertunnel): indice di variante, tipo, campi del descrittore e quantità u16.
fn skip_descriptor_count(r: &mut Reader) -> Option<()> {
    let variant = r.var_u32()?;
    r.u8()?;
    match variant {
        0 => {}
        1 => {
            r.string()?;
            r.var_i32()?; // aux
        }
        2 => {
            r.string()?;
            r.i16()?;
        }
        3 => {
            r.string()?;
        }
        _ => return None,
    }
    r.u16()?;
    Some(())
}

/// Salta un `ItemStackRequestNetworkItemInstanceDescriptor`: descrittore (indice di variante + tipo + campi), quantità,
/// runtime id del blocco, dati utente.
fn skip_request_item(r: &mut Reader) -> Option<()> {
    let variant = r.var_u32()?;
    r.u8()?; // tipo del descrittore, ridondante con la variante
    match variant {
        0 => {}
        1 => {
            r.string()?;
            r.var_i32()?; // aux
        }
        2 => {
            r.string()?;
            r.i16()?; // versione molang
        }
        3 => {
            r.string()?;
        }
        _ => return None,
    }
    r.u16()?;
    r.var_u32()?;
    r.bytes()?;
    Some(())
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
    /// Le caselle da rimandare chieste insieme all'uso dell'oggetto.
    pub legacy_slots: LegacySlots,
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
            legacy_slots: Vec::new(),
            stack_request: None,
            block_actions: Vec::new(),
        };
        if let Some((slots, u)) = r.optional(|r| {
            r.var_i32()?; // id legacy della richiesta
            let slots = read_legacy_slots(r)?;
            skip_inventory_actions(r)?;
            Some((slots, UseItem::read(r)?))
        })? {
            p.legacy_slots = slots;
            p.item_use = Some(u);
        }
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

/// Azioni di `Interact` (pacchetto 33) che il server usa.
pub mod interact_action {
    pub const OPEN_INVENTORY: u8 = 6;
}

/// `Interact` dal client: interessa solo l'azione e il bersaglio (la posizione opzionale che segue si ignora).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interact {
    pub action: u8,
    pub target_runtime_id: u64,
}

impl Interact {
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        Some(Self { action: r.u8()?, target_runtime_id: r.var_u64()? })
    }
}

impl ContainerClose {
    /// Il pacchetto 47 viaggia nei due versi: la codifica è nel codice generato, qui c'è solo la lettura.
    pub fn decode(body: &[u8]) -> Option<Self> {
        let r = &mut Reader(body);
        Some(Self { container_id: r.u8()?, container_type: r.u8()?, server_initiated_close: r.bool()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_values_match_gophertunnel() {
        assert_eq!(flag::START_JUMPING, 31);
        assert_eq!(player_action::RESPAWN, 7);
        assert_eq!((respawn_state::READY_TO_SPAWN, respawn_state::CLIENT_READY_TO_SPAWN), (1, 2));
    }
    use crate::wire::Writer;

    #[test]
    fn respawn_decodes_like_gophertunnel_marshal() {
        let mut w = Writer::default();
        [1.0f32, 2.0, 3.0].iter().for_each(|&v| w.f32(v));
        w.u8(respawn_state::CLIENT_READY_TO_SPAWN);
        w.var_u64(300);
        assert_eq!(Respawn::decode(&w.0), Some(Respawn { position: [1.0, 2.0, 3.0], state: 2, runtime_id: 300 }));
        assert_eq!(Respawn::decode(&w.0[..13]), None);
    }

    /// Presa dal menu creativo catturata nel gioco (1.26.6x): CraftCreative, CraftResults, Take.
    #[test]
    fn creative_take_with_craft_results_decodes() {
        let body: [u8; 78] = [
            0x01, 0x05, 0x03, 0x0c, 0x0e, 0x04, 0x01, 0x12, 0x14, 0x01, 0x01, 0x01, 0x17, 0x6d, 0x69, 0x6e, 0x65, 0x63, 0x72, 0x61, 0x66, 0x74, 0x3a, 0x6a,
            0x75, 0x6e, 0x67, 0x6c, 0x65, 0x5f, 0x70, 0x6c, 0x61, 0x6e, 0x6b, 0x73, 0x00, 0x01, 0x00, 0x97, 0xa5, 0x81, 0x93, 0x04, 0x0a, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x40, 0x3c, 0x00, 0x32, 0xfd, 0xff, 0xff, 0xff, 0x3b, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
        ];
        let p = ItemStackRequestPacket::decode(&body).unwrap();
        assert_eq!(p.requests.len(), 1, "la richiesta non va scartata");
        let q = &p.requests[0];
        assert_eq!(q.request_id, -3);
        assert_eq!(q.actions[0], StackAction::CraftCreative { net_id: 4, crafts: 1 });
        assert_eq!(q.actions[1], StackAction::Other(stack_type::CRAFT_RESULTS));
        assert!(matches!(
            q.actions[2],
            StackAction::Take { count: 64, src: SlotRef { container: 0x3c, slot: 0x32, stack_id: -3, .. }, dst: SlotRef { container: 0x3b, slot: 0, .. } }
        ));
    }

    /// `CraftRecipe`, `CraftRecipeAuto` (con ingredienti), `Consume` e `Create` come li scrive gophertunnel (l'indice di variante si ignora: conta il tipo).
    #[test]
    fn crafting_actions_decode_like_gophertunnel() {
        let mut w = Writer::default();
        w.var_i32(-5);
        w.var_u32(5);
        w.var_u32(10);
        w.u8(stack_type::CRAFT_RECIPE);
        w.var_u32(1530);
        w.u8(2);
        w.var_u32(11);
        w.u8(stack_type::CRAFT_RECIPE_AUTO);
        w.var_u32(2397);
        w.u8(3);
        w.var_u32(2); // due ingredienti: uno per nome, uno per tag
        w.var_u32(1);
        w.u8(1);
        w.string("minecraft:oak_planks");
        w.var_i32(0);
        w.u16(2);
        w.var_u32(3);
        w.u8(3);
        w.string("minecraft:planks");
        w.u16(1);
        w.var_u32(5);
        w.u8(stack_type::CONSUME);
        w.u8(1);
        slot(&mut w, 13, 28, 7);
        w.var_u32(6);
        w.u8(stack_type::CREATE);
        w.u8(1);
        w.var_u32(1);
        w.u8(stack_type::PLACE);
        w.u8(4);
        slot(&mut w, 60, 50, -5);
        slot(&mut w, 28, 0, 0);
        w.var_u32(0);
        w.i32(-1);
        let mut body = Writer::default();
        body.var_u32(1);
        body.0.extend_from_slice(&w.0);
        let p = ItemStackRequestPacket::decode(&body.0).unwrap();
        let q = &p.requests[0];
        assert_eq!(q.request_id, -5);
        assert_eq!(q.actions[0], StackAction::CraftRecipe { recipe_id: 1530, crafts: 2 });
        assert_eq!(q.actions[1], StackAction::CraftRecipeAuto { recipe_id: 2397, crafts: 3 });
        assert_eq!(q.actions[2], StackAction::Other(stack_type::CONSUME));
        assert_eq!(q.actions[3], StackAction::Create { results_slot: 1 });
        assert!(matches!(q.actions[4], StackAction::Place { count: 4, .. }));
    }

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
    fn inventory_transaction_release_item() {
        // layout di `ReleaseItemTransactionData.Marshal` (gophertunnel v1.63.0)
        let mut w = Writer::default();
        w.var_i32(0); // id legacy
        w.u8(0); // niente slot legacy
        w.var_u32(4); // tipo ReleaseItem
        w.var_u32(0); // niente azioni
        w.var_i32(0); // azione
        w.var_i32(3); // casella
        ItemStack::air().encode(&mut w);
        [1.5f32, 70.0, -2.5].iter().for_each(|&v| w.f32(v));
        w.u8(0); // mano
        let t = InventoryTransaction::decode(&w.0).expect("leggibile");
        assert_eq!((t.kind, t.use_item), (4, None));
        let r = t.release.expect("rilascio");
        assert_eq!((r.action, r.hotbar_slot, r.held, r.head_pos, r.hand), (0, 3, ItemStack::air(), [1.5, 70.0, -2.5], 0));
        for n in 0..w.0.len() {
            assert!(InventoryTransaction::decode(&w.0[..n]).is_none(), "tagliato a {n}");
        }
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
        assert_eq!(t.legacy_slots, vec![(12, vec![3, 4])]);
        assert_eq!(t.use_item.expect("uso").pos, [-155, 72, -2]);
    }

    #[test]
    fn use_item_on_entity_follows_gophertunnel_layout() {
        // layout di `UseItemOnEntityTransactionData.Marshal` (gophertunnel v1.63.0)
        let mut w = Writer::default();
        w.var_i32(0); // richiesta legacy
        w.u8(0); // niente slot legacy
        w.var_u32(3); // tipo UseItemOnEntity
        w.var_u32(0); // nessuna azione
        w.var_u64(7); // entità
        w.var_i32(1); // attacco
        w.var_i32(4); // casella
        w.u8(0); // mano
        ItemStack::air().encode(&mut w);
        for v in [1.0f32, 2.0, 3.0, 0.5, 0.5, 0.5] {
            w.f32(v);
        }
        let t = InventoryTransaction::decode(&w.0).expect("leggibile");
        let u = t.use_entity.expect("uso sull'entità");
        assert_eq!((u.target, u.action, u.hotbar_slot, u.hand), (7, use_entity::ATTACK, 4, 0));
        assert_eq!((u.player_pos, u.click_pos), ([1.0, 2.0, 3.0], [0.5, 0.5, 0.5]));
        assert!(t.use_item.is_none() && t.release.is_none());
        for n in 0..w.0.len() {
            assert!(InventoryTransaction::decode(&w.0[..n]).is_none(), "tagliato a {n}");
        }
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
        assert_eq!((&a[4], &a[5]), (&StackAction::Other(5), &StackAction::Create { results_slot: 0 }));
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
    fn animate_decodes_action_byte_and_runtime_id() {
        let mut w = Writer::default();
        w.u8(1); // oscillazione
        w.var_u64(300);
        w.f32(0.0);
        w.u8(0); // origine assente
        w.u8(0); // mano assente
        assert_eq!(Animate::decode(&w.0), Some(Animate { action: 1, runtime_id: 300 }));
        assert!(Animate::decode(&w.0[..2]).is_none());
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
