//! Inventario di un giocatore e risoluzione delle richieste di pila (`ItemStackRequest`).
//!
//! Il comportamento segue Dragonfly (MIT, `server/session/handler_item_stack_request.go`): ogni pila ha un ID di rete scelto dal
//! server, il client lo ripete in ogni richiesta e una richiesta sbagliata annulla tutte le modifiche che aveva già fatto.
//! Gli ID negativi nelle richieste sono previsioni del client: puntano a una richiesta precedente e si risolvono con
//! gli ID che il server le ha assegnato (finché il client non conferma la casella con un ID non negativo).
use crate::registry::{self, CreativeItem};
use bedrock_protocol::{
    generated::{
        enums::container_enum_name::{
            ARMOR_CONTAINER, COMBINED_HOTBAR_AND_INVENTORY_CONTAINER, CREATED_OUTPUT_CONTAINER, CURSOR_CONTAINER, HOTBAR_CONTAINER, INVENTORY_CONTAINER,
            OFFHAND_CONTAINER,
        },
        FullContainerName, InventorySlot, ItemStackResponse, ItemStackResponseContainerInfo, ItemStackResponseInfo, ItemStackResponseSlotInfo,
    },
    input::{SlotRef, StackAction, StackRequest},
    item::ItemStack,
    wire::ServerPacket,
    Packet,
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Caselle dell'inventario principale: 0-8 sono la barra.
pub(crate) const MAIN_SLOTS: usize = 36;
/// Caselle della barra rapida.
pub(crate) const HOTBAR_SLOTS: u8 = 9;
/// Caselle di armatura (elmo, corpetto, gambali, stivali).
pub(crate) const ARMOR_SLOTS: usize = 4;
/// Casella di `CreatedOutput` dove sta l'oggetto appena preso dal menu creativo (P§5.3).
const CREATED_SLOT: u8 = 50;
/// Massimo di una pila. Il server non conosce ancora il massimo di ogni oggetto: 64 vale come tetto per tutti.
pub(crate) const MAX_STACK: u16 = 64;
/// Richieste con modifiche non ancora confermate dal client (D§8.2): oltre, le richieste si rifiutano.
const MAX_PENDING: usize = 256;
/// Dopo questo tempo una modifica non confermata si dimentica.
const PENDING_TTL: Duration = Duration::from_secs(5);

/// Risultati di `ItemStackResponse`.
const RESULT_OK: u8 = 0;
const RESULT_ERROR: u8 = 1;

/// Finestre "vecchie" di `InventoryContent`/`InventorySlot` (P§2.1).
pub(crate) mod window {
    pub const INVENTORY: u32 = 0;
    pub const OFFHAND: u32 = 119;
    pub const ARMOR: u32 = 120;
    pub const UI: u32 = 124;
}

/// Un oggetto con quantità. Vuoto quando l'ID o la quantità sono zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Item {
    pub id: i16,
    pub count: u16,
    pub aux: u32,
    /// Hash dello stato del blocco che l'oggetto piazza.
    pub block: Option<i32>,
    /// NBT su disco dell'oggetto (incantesimi...).
    pub nbt: Option<Vec<u8>>,
}

impl Item {
    pub fn is_empty(&self) -> bool {
        self.id == 0 || self.count == 0
    }

    /// Stesso genere di oggetto (ID, valore ausiliario e stato del blocco), quantità a parte.
    pub fn same_kind(&self, other: &Item) -> bool {
        self.id == other.id && self.aux == other.aux && self.block == other.block
    }

    /// Due pile si fondono solo se, oltre al genere, hanno lo stesso NBT (due libri incantati diversi non si impilano).
    fn stacks_with(&self, other: &Item) -> bool {
        self.same_kind(other) && self.nbt == other.nbt
    }

    /// Il genere di oggetto che il client dichiara di avere in mano (la quantità e i dati utente non contano per `same_kind`).
    pub fn kind_of(stack: &ItemStack) -> Item {
        Item { id: stack.id, count: stack.count, aux: stack.aux, block: (stack.block_runtime_id != 0).then_some(stack.block_runtime_id as i32), nbt: None }
    }

    /// Voce del menu creativo come oggetto (una pila piena); `None` se l'oggetto non ha un ID.
    fn from_creative(c: &CreativeItem) -> Option<Item> {
        Some(Item { id: registry::items().id(&c.name)?, count: MAX_STACK, aux: c.meta, block: c.block, nbt: c.nbt.clone() })
    }

    fn with_count(&self, count: u16) -> Item {
        Item { count, ..self.clone() }
    }

    /// L'oggetto come lo vede il client, con l'ID di pila dato. I dati utente non sono mai vuoti per un oggetto non-aria
    /// (`ItemStack::default()` li lascia a zero byte, che il client vero potrebbe rifiutare).
    pub fn stack(&self, net_id: i32) -> ItemStack {
        if self.is_empty() {
            return ItemStack::air();
        }
        ItemStack {
            id: self.id,
            count: self.count,
            aux: self.aux,
            net_id: Some(net_id),
            block_runtime_id: self.block.unwrap_or(0) as u32,
            user_data: match &self.nbt {
                Some(nbt) => ItemStack::user_data_with_nbt(nbt),
                None => ItemStack::empty_user_data(),
            },
        }
    }
}

/// Una casella: l'oggetto e il suo ID di pila (0 se vuota).
#[derive(Clone, Debug, Default, PartialEq)]
struct Slot {
    item: Item,
    stack_id: i32,
}

/// Dove sta una casella, indipendentemente dal nome del contenitore usato dal client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Loc {
    Main(u8),
    Offhand,
    Armor(u8),
    Cursor,
    Created,
}

impl Loc {
    /// Contenitore e casella dati dal client. Barra e inventario sono due viste dello stesso inventario.
    fn resolve(container: u8, slot: u8) -> Option<Loc> {
        match container {
            HOTBAR_CONTAINER if slot < HOTBAR_SLOTS => Some(Loc::Main(slot)),
            INVENTORY_CONTAINER | COMBINED_HOTBAR_AND_INVENTORY_CONTAINER if (slot as usize) < MAIN_SLOTS => Some(Loc::Main(slot)),
            OFFHAND_CONTAINER => Some(Loc::Offhand), // il client numera la mano secondaria a modo suo: c'è una sola casella
            ARMOR_CONTAINER if (slot as usize) < ARMOR_SLOTS => Some(Loc::Armor(slot)),
            CURSOR_CONTAINER => Some(Loc::Cursor),
            CREATED_OUTPUT_CONTAINER if slot == CREATED_SLOT => Some(Loc::Created),
            _ => None,
        }
    }

    /// Finestra, casella e nome del contenitore per `InventorySlot`.
    fn window(self) -> (u32, u32, u8) {
        match self {
            Loc::Main(i) => (window::INVENTORY, i as u32, INVENTORY_CONTAINER),
            Loc::Offhand => (window::OFFHAND, 0, OFFHAND_CONTAINER),
            Loc::Armor(i) => (window::ARMOR, i as u32, ARMOR_CONTAINER),
            Loc::Cursor => (window::UI, 0, CURSOR_CONTAINER),
            Loc::Created => (window::UI, CREATED_SLOT as u32, CREATED_OUTPUT_CONTAINER),
        }
    }
}

/// Tutte le caselle: è ciò che si copia prima di una richiesta per poterla annullare.
#[derive(Clone, Debug)]
struct Contents {
    main: [Slot; MAIN_SLOTS],
    offhand: Slot,
    armor: [Slot; ARMOR_SLOTS],
    cursor: Slot,
    created: Slot,
}

impl Contents {
    fn get(&self, loc: Loc) -> &Slot {
        match loc {
            Loc::Main(i) => &self.main[i as usize],
            Loc::Offhand => &self.offhand,
            Loc::Armor(i) => &self.armor[i as usize],
            Loc::Cursor => &self.cursor,
            Loc::Created => &self.created,
        }
    }

    fn get_mut(&mut self, loc: Loc) -> &mut Slot {
        match loc {
            Loc::Main(i) => &mut self.main[i as usize],
            Loc::Offhand => &mut self.offhand,
            Loc::Armor(i) => &mut self.armor[i as usize],
            Loc::Cursor => &mut self.cursor,
            Loc::Created => &mut self.created,
        }
    }
}

/// ID che il server ha dato alle caselle toccate da una richiesta, in attesa che il client li confermi.
struct Pending {
    at: Instant,
    ids: HashMap<Loc, i32>,
}

/// Esito di `Inventory::handle`.
#[derive(Debug, PartialEq)]
pub(crate) struct StackResult {
    pub ok: bool,
    /// (contenitore, casella) come li ha scritti il client. Se `ok` sono le caselle modificate; altrimenti tutte quelle
    /// che la richiesta nominava (lo stato vero è quello di prima e va rimandato al client).
    pub changed: Vec<(u8, u8)>,
    /// Perché la richiesta è stata rifiutata (per il log).
    pub error: Option<String>,
}

/// L'inventario di un giocatore: 36 caselle (0-8 barra), mano secondaria, armatura, cursore e casella creata.
pub(crate) struct Inventory {
    contents: Contents,
    /// Casella della barra in mano (0-8).
    pub selected: u8,
    next_id: i32,
    pending: HashMap<i32, Pending>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            contents: Contents {
                main: std::array::from_fn(|_| Slot::default()),
                offhand: Slot::default(),
                armor: std::array::from_fn(|_| Slot::default()),
                cursor: Slot::default(),
                created: Slot::default(),
            },
            selected: 0,
            next_id: 1,
            pending: HashMap::new(),
        }
    }
}

/// Raccolta delle modifiche di una richiesta.
struct Run {
    request_id: i32,
    changed: Vec<(u8, u8)>,
    now: Instant,
}

impl Inventory {
    /// L'oggetto in mano (casella `selected` della barra).
    pub fn held(&self) -> &Item {
        &self.contents.main[self.selected as usize].item
    }

    /// Toglie un oggetto da quello in mano (piazzamento in sopravvivenza) e dà alla casella un nuovo ID di pila.
    /// Restituisce l'`InventorySlot` che dice al client com'è ora la casella; `None` se la mano è vuota.
    pub fn consume_held(&mut self) -> Option<Packet> {
        let loc = Loc::Main(self.selected);
        if self.contents.get(loc).item.is_empty() {
            return None;
        }
        let slot = self.contents.get_mut(loc);
        slot.item.count -= 1;
        if slot.item.is_empty() {
            *slot = Slot::default();
        } else {
            let id = self.new_id();
            self.contents.get_mut(loc).stack_id = id;
        }
        let (window, index, name) = loc.window();
        Some(
            InventorySlot {
                container_id: window as u8,
                slot: index,
                full_container_name: Some(FullContainerName { container_name: name, dynamic_id: None }),
                storage_item: None,
                item: self.stack_of(loc),
            }
            .packet(),
        )
    }

    /// Oggetto della casella `i` dell'inventario principale (0-8 barra).
    pub fn main(&self, i: usize) -> &Item {
        &self.contents.main[i].item
    }

    pub fn offhand(&self) -> &Item {
        &self.contents.offhand.item
    }

    /// Oggetto dell'armatura: 0 elmo, 1 corpetto, 2 gambali, 3 stivali.
    pub fn armor(&self, i: usize) -> &Item {
        &self.contents.armor[i].item
    }

    /// Mette un oggetto in una casella (dati caricati dal disco): una casella piena prende un ID di pila nuovo.
    fn fill(&mut self, loc: Loc, item: Item) {
        let stack_id = if item.is_empty() { 0 } else { self.new_id() };
        *self.contents.get_mut(loc) = Slot { item: if stack_id == 0 { Item::default() } else { item }, stack_id };
    }

    pub fn set_main(&mut self, i: usize, item: Item) {
        if i < MAIN_SLOTS {
            self.fill(Loc::Main(i as u8), item);
        }
    }

    pub fn set_offhand(&mut self, item: Item) {
        self.fill(Loc::Offhand, item);
    }

    pub fn set_armor(&mut self, i: usize, item: Item) {
        if i < ARMOR_SLOTS {
            self.fill(Loc::Armor(i as u8), item);
        }
    }

    fn stack_of(&self, loc: Loc) -> ItemStack {
        let slot = self.contents.get(loc);
        slot.item.stack(slot.stack_id)
    }

    /// La casella com'è per il client; `None` se contenitore e casella non esistono.
    pub fn stack_at(&self, container: u8, slot: u8) -> Option<ItemStack> {
        Loc::resolve(container, slot).map(|loc| self.stack_of(loc))
    }

    /// Le 36 caselle principali per `InventoryContent`.
    pub fn main_stacks(&self) -> Vec<ItemStack> {
        (0..MAIN_SLOTS as u8).map(|i| self.stack_of(Loc::Main(i))).collect()
    }

    pub fn offhand_stacks(&self) -> Vec<ItemStack> {
        vec![self.stack_of(Loc::Offhand)]
    }

    pub fn armor_stacks(&self) -> Vec<ItemStack> {
        (0..ARMOR_SLOTS as u8).map(|i| self.stack_of(Loc::Armor(i))).collect()
    }

    fn new_id(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id = if id == i32::MAX { 1 } else { id + 1 };
        id
    }

    /// Esegue una richiesta del client. Se una azione non è valida, tutte le modifiche della richiesta si annullano.
    pub fn handle(&mut self, req: &StackRequest, creative: bool) -> StackResult {
        let backup = self.contents.clone();
        let mut run = Run { request_id: req.request_id, changed: Vec::new(), now: Instant::now() };
        match self.run(req, creative, &mut run) {
            Ok(()) => {
                // La casella creata è di passaggio: quello che il client non ha preso (il server non sa quanto sta in una pila) si butta.
                self.contents.created = Slot::default();
                StackResult { ok: true, changed: run.changed, error: None }
            }
            Err(error) => {
                self.contents = backup;
                self.pending.remove(&req.request_id);
                let mut changed = Vec::new();
                for (container, slot) in referenced(req) {
                    if Loc::resolve(container, slot).is_some() && !changed.contains(&(container, slot)) {
                        changed.push((container, slot));
                    }
                }
                StackResult { ok: false, changed, error: Some(error) }
            }
        }
    }

    fn run(&mut self, req: &StackRequest, creative: bool, run: &mut Run) -> Result<(), String> {
        self.pending.retain(|_, p| run.now.duration_since(p.at) <= PENDING_TTL);
        for action in &req.actions {
            match action {
                StackAction::Take { count, src, dst } | StackAction::Place { count, src, dst } => self.transfer(run, src, dst, *count)?,
                StackAction::Swap { src, dst } => self.swap(run, src, dst)?,
                StackAction::Drop { count, src, .. } => self.remove(run, src, *count)?,
                StackAction::Destroy { count, src } => {
                    if !creative {
                        return Err("Destroy è permessa solo con l'inventario creativo".into());
                    }
                    self.remove(run, src, *count)?
                }
                StackAction::CraftCreative { net_id, .. } => {
                    if !creative {
                        return Err("CraftCreative è permessa solo con l'inventario creativo".into());
                    }
                    let item = (*net_id as usize)
                        .checked_sub(1)
                        .and_then(|i| registry::creative().1.get(i))
                        .and_then(Item::from_creative)
                        .ok_or_else(|| format!("voce creativa {net_id} inesistente"))?;
                    self.put(run, CREATED_OUTPUT_CONTAINER, CREATED_SLOT, Loc::Created, item);
                }
                StackAction::Other(kind) => return Err(format!("azione {kind} non supportata")),
            }
        }
        Ok(())
    }

    /// Dà la casella indicata dal client e controlla che l'ID di pila che il client ci vede sia quello del server.
    fn verified(&mut self, r: &SlotRef) -> Result<Loc, String> {
        let loc = Loc::resolve(r.container, r.slot).ok_or_else(|| format!("casella {}/{} inesistente", r.container, r.slot))?;
        if loc == Loc::Created {
            return Ok(loc); // il contenuto lo ha deciso il server in questa stessa richiesta: niente da confrontare
        }
        if r.stack_id >= 0 {
            // Il client ha visto la casella con un ID vero: le previsioni su di essa non servono più.
            self.pending.retain(|_, p| {
                p.ids.remove(&loc);
                !p.ids.is_empty()
            });
        }
        if self.pending.len() > MAX_PENDING {
            return Err("troppe modifiche non confermate".into());
        }
        let expected = if r.stack_id >= 0 {
            r.stack_id
        } else {
            let p = self.pending.get(&r.stack_id).ok_or_else(|| format!("ID di pila {} punta a una richiesta sconosciuta", r.stack_id))?;
            *p.ids.get(&loc).ok_or_else(|| format!("la richiesta {} non ha toccato la casella {}/{}", r.stack_id, r.container, r.slot))?
        };
        let actual = self.contents.get(loc).stack_id;
        if actual != expected {
            return Err(format!("ID di pila diverso nella casella {}/{}: il client ha {expected}, il server {actual}", r.container, r.slot));
        }
        Ok(loc)
    }

    /// Mette `item` nella casella, le dà un ID nuovo e ricorda la modifica.
    fn put(&mut self, run: &mut Run, container: u8, slot: u8, loc: Loc, item: Item) {
        let stack_id = if item.is_empty() { 0 } else { self.new_id() };
        let item = if item.is_empty() { Item::default() } else { item };
        *self.contents.get_mut(loc) = Slot { item, stack_id };
        self.record(run, container, slot, loc);
    }

    fn record(&mut self, run: &mut Run, container: u8, slot: u8, loc: Loc) {
        if !run.changed.contains(&(container, slot)) {
            run.changed.push((container, slot));
        }
        if loc == Loc::Created {
            return; // il client non la nomina mai con un ID di previsione: niente da ricordare
        }
        let id = self.contents.get(loc).stack_id;
        let pending = self.pending.entry(run.request_id).or_insert_with(|| Pending { at: run.now, ids: HashMap::new() });
        pending.ids.insert(loc, id);
    }

    /// Take e Place: spostano `count` oggetti da una casella all'altra.
    fn transfer(&mut self, run: &mut Run, src: &SlotRef, dst: &SlotRef, count: u8) -> Result<(), String> {
        let count = count as u16;
        if count == 0 || count > MAX_STACK {
            return Err(format!("quantità {count} non valida"));
        }
        let (from, to) = (self.verified(src)?, self.verified(dst)?);
        if from == to {
            return Err("origine e destinazione sono la stessa casella".into());
        }
        if to == Loc::Created {
            return Err("nella casella creata non si può mettere nulla".into());
        }
        let (a, b) = (self.contents.get(from).item.clone(), self.contents.get(to).item.clone());
        if a.is_empty() || a.count < count {
            return Err(format!("si chiedono {count} oggetti ma la casella ne ha {}", if a.is_empty() { 0 } else { a.count }));
        }
        if !b.is_empty() && (!a.stacks_with(&b) || b.count + count > MAX_STACK) {
            return Err("la destinazione ha un oggetto diverso o non c'è posto".into());
        }
        let moved = if b.is_empty() { a.with_count(count) } else { b.with_count(b.count + count) };
        self.put(run, src.container, src.slot, from, a.with_count(a.count - count));
        self.put(run, dst.container, dst.slot, to, moved);
        Ok(())
    }

    fn swap(&mut self, run: &mut Run, src: &SlotRef, dst: &SlotRef) -> Result<(), String> {
        let (from, to) = (self.verified(src)?, self.verified(dst)?);
        if from == to || from == Loc::Created || to == Loc::Created {
            return Err("scambio non valido".into());
        }
        let (a, b) = (self.contents.get(from).clone(), self.contents.get(to).clone());
        // Le pile cambiano posto con il loro ID.
        *self.contents.get_mut(from) = b;
        *self.contents.get_mut(to) = a;
        self.record(run, src.container, src.slot, from);
        self.record(run, dst.container, dst.slot, to);
        Ok(())
    }

    /// Toglie `count` oggetti da una casella (Drop e Destroy). L'oggetto buttato non diventa ancora un'entità nel mondo.
    fn remove(&mut self, run: &mut Run, src: &SlotRef, count: u8) -> Result<(), String> {
        let count = count as u16;
        if count == 0 || count > MAX_STACK {
            return Err(format!("quantità {count} non valida"));
        }
        let loc = self.verified(src)?;
        let a = self.contents.get(loc).item.clone();
        if a.is_empty() || a.count < count {
            return Err(format!("si chiedono {count} oggetti ma la casella ne ha {}", if a.is_empty() { 0 } else { a.count }));
        }
        self.put(run, src.container, src.slot, loc, a.with_count(a.count - count));
        Ok(())
    }

    /// I pacchetti di risposta a una richiesta: `ItemStackResponse` e, se è stata rifiutata, `InventorySlot` per ogni casella
    /// nominata, così il client torna allo stato vero del server.
    pub fn response(&self, request_id: i32, result: &StackResult) -> Vec<Packet> {
        if !result.ok {
            let mut packets =
                vec![ItemStackResponse { responses: vec![ItemStackResponseInfo { result: RESULT_ERROR, client_request_id: request_id, containers: None }] }
                    .packet()];
            for &(container, slot) in &result.changed {
                if let Some(loc) = Loc::resolve(container, slot) {
                    let (window, index, name) = loc.window();
                    packets.push(
                        InventorySlot {
                            container_id: window as u8,
                            slot: index,
                            full_container_name: Some(FullContainerName { container_name: name, dynamic_id: None }),
                            storage_item: None,
                            item: self.stack_of(loc),
                        }
                        .packet(),
                    );
                }
            }
            return packets;
        }
        let mut containers: Vec<ItemStackResponseContainerInfo> = Vec::new();
        for &(container, slot) in &result.changed {
            let Some(stack) = self.stack_at(container, slot) else { continue };
            let info = ItemStackResponseSlotInfo {
                requested_slot: slot,
                slot,
                amount: stack.count.min(u8::MAX as u16) as u8,
                item_stack_net_id: stack.net_id.filter(|_| stack.count > 0),
                ..Default::default()
            };
            match containers.iter_mut().find(|c| c.full_container_name.container_name == container) {
                Some(c) => c.slots.push(info),
                None => containers.push(ItemStackResponseContainerInfo {
                    full_container_name: FullContainerName { container_name: container, dynamic_id: None },
                    slots: vec![info],
                }),
            }
        }
        vec![ItemStackResponse { responses: vec![ItemStackResponseInfo { result: RESULT_OK, client_request_id: request_id, containers: Some(containers) }] }
            .packet()]
    }
}

/// Tutte le caselle nominate dalle azioni di una richiesta (più la casella creata se si crea qualcosa).
fn referenced(req: &StackRequest) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    for action in &req.actions {
        match action {
            StackAction::Take { src, dst, .. } | StackAction::Place { src, dst, .. } | StackAction::Swap { src, dst } => {
                out.extend([(src.container, src.slot), (dst.container, dst.slot)])
            }
            StackAction::Drop { src, .. } | StackAction::Destroy { src, .. } => out.push((src.container, src.slot)),
            StackAction::CraftCreative { .. } => out.push((CREATED_OUTPUT_CONTAINER, CREATED_SLOT)),
            StackAction::Other(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_protocol::id;

    /// ID di rete (1-based) di una voce del menu creativo.
    fn creative_id(name: &str) -> u32 {
        registry::creative().1.iter().position(|c| c.name == name && c.meta == 0).expect("voce creativa") as u32 + 1
    }

    fn at(container: u8, slot: u8, stack_id: i32) -> SlotRef {
        SlotRef { container, dynamic_id: None, slot, stack_id }
    }

    fn hotbar(slot: u8, stack_id: i32) -> SlotRef {
        at(HOTBAR_CONTAINER, slot, stack_id)
    }

    fn created(stack_id: i32) -> SlotRef {
        at(CREATED_OUTPUT_CONTAINER, CREATED_SLOT, stack_id)
    }

    fn cursor(stack_id: i32) -> SlotRef {
        at(CURSOR_CONTAINER, 0, stack_id)
    }

    fn request(request_id: i32, actions: Vec<StackAction>) -> StackRequest {
        StackRequest { request_id, actions }
    }

    fn craft(name: &str) -> StackAction {
        StackAction::CraftCreative { net_id: creative_id(name), crafts: 1 }
    }

    /// Mette direttamente `count` oggetti `name` nella casella `slot` e dà l'ID di pila.
    fn give(inv: &mut Inventory, slot: u8, name: &str, count: u16) -> i32 {
        let id = inv.new_id();
        let item = Item { id: registry::items().id(name).unwrap(), count, ..Default::default() };
        inv.contents.main[slot as usize] = Slot { item, stack_id: id };
        id
    }

    fn stone() -> i16 {
        registry::items().id("minecraft:stone").unwrap()
    }

    #[test]
    fn craft_creative_then_place_into_hotbar() {
        let mut inv = Inventory::default();
        let req = request(-1, vec![craft("minecraft:stone"), StackAction::Place { count: 64, src: created(-1), dst: hotbar(0, 0) }]);
        let result = inv.handle(&req, true);
        assert!(result.ok, "{:?}", result.error);
        let held = &inv.contents.main[0];
        assert_eq!((held.item.id, held.item.count), (stone(), 64));
        assert!(held.stack_id > 0, "una pila piena ha un ID positivo");
        assert_eq!(held.item.block, registry::items().block_of("minecraft:stone"));
        assert!(result.changed.contains(&(HOTBAR_CONTAINER, 0)) && result.changed.contains(&(CREATED_OUTPUT_CONTAINER, CREATED_SLOT)));
        assert!(inv.contents.created.item.is_empty(), "la casella creata si svuota a fine richiesta");
        // quello che vede il client: pietra con ID di rete e dati utente non vuoti
        let stack = inv.stack_at(HOTBAR_CONTAINER, 0).unwrap();
        assert_eq!((stack.id, stack.count, stack.net_id), (stone(), 64, Some(held.stack_id)));
        assert_eq!(stack.user_data, ItemStack::empty_user_data());
        assert_eq!(inv.stack_at(CREATED_OUTPUT_CONTAINER, CREATED_SLOT), Some(ItemStack::air()));
    }

    #[test]
    fn creative_flow_through_the_cursor_uses_ids_of_the_previous_request() {
        let mut inv = Inventory::default();
        let first = request(-1, vec![craft("minecraft:stone"), StackAction::Take { count: 64, src: created(-1), dst: cursor(0) }]);
        assert!(inv.handle(&first, true).ok);
        let cursor_id = inv.contents.cursor.stack_id;
        assert!(cursor_id > 0);
        // il client non ha ancora l'ID vero: lo chiama con il numero della richiesta di prima
        let second = request(-3, vec![StackAction::Place { count: 64, src: cursor(-1), dst: hotbar(4, 0) }]);
        let result = inv.handle(&second, true);
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(inv.contents.main[4].item.count, 64);
        assert!(inv.contents.cursor.item.is_empty());
        // un ID negativo che non punta a nulla si rifiuta
        let third = request(-5, vec![StackAction::Take { count: 1, src: hotbar(4, -9), dst: cursor(0) }]);
        assert!(!inv.handle(&third, true).ok);
    }

    #[test]
    fn confirming_a_slot_forgets_the_prediction() {
        let mut inv = Inventory::default();
        assert!(inv.handle(&request(-1, vec![craft("minecraft:stone"), StackAction::Place { count: 64, src: created(-1), dst: hotbar(0, 0) }]), true).ok);
        let id = inv.contents.main[0].stack_id;
        assert!(inv.pending.contains_key(&-1));
        // il client conferma la casella con l'ID vero
        assert!(inv.handle(&request(-3, vec![StackAction::Take { count: 1, src: hotbar(0, id), dst: hotbar(1, 0) }]), true).ok);
        assert!(!inv.pending.contains_key(&-1), "la previsione della prima richiesta non serve più");
        // adesso -1 non vale più per la casella 0
        assert!(!inv.handle(&request(-5, vec![StackAction::Take { count: 1, src: hotbar(0, -1), dst: hotbar(2, 0) }]), true).ok);
    }

    #[test]
    fn swap_and_destroy() {
        let mut inv = Inventory::default();
        let a = give(&mut inv, 0, "minecraft:stone", 10);
        let b = give(&mut inv, 1, "minecraft:dirt", 5);
        let swap = request(-1, vec![StackAction::Swap { src: hotbar(0, a), dst: hotbar(1, b) }]);
        let result = inv.handle(&swap, true);
        assert!(result.ok, "{:?}", result.error);
        let dirt = registry::items().id("minecraft:dirt").unwrap();
        assert_eq!((inv.contents.main[0].item.id, inv.contents.main[0].stack_id), (dirt, b), "la pila porta il suo ID");
        assert_eq!((inv.contents.main[1].item.id, inv.contents.main[1].item.count, inv.contents.main[1].stack_id), (stone(), 10, a));
        // cestino: toglie una parte della pila, poi il resto
        let id = inv.contents.main[1].stack_id;
        assert!(inv.handle(&request(-3, vec![StackAction::Destroy { count: 4, src: hotbar(1, id) }]), true).ok);
        assert_eq!(inv.contents.main[1].item.count, 6);
        let id = inv.contents.main[1].stack_id;
        assert_ne!(id, a, "una pila cambiata ha un ID nuovo");
        assert!(inv.handle(&request(-5, vec![StackAction::Destroy { count: 6, src: hotbar(1, id) }]), true).ok);
        assert_eq!((inv.contents.main[1].item.is_empty(), inv.contents.main[1].stack_id), (true, 0));
        // in sopravvivenza il cestino non esiste
        let id = inv.contents.main[0].stack_id;
        let result = inv.handle(&request(-7, vec![StackAction::Destroy { count: 1, src: hotbar(0, id) }]), false);
        assert!(!result.ok);
        assert_eq!(inv.contents.main[0].item.count, 5);
    }

    #[test]
    fn take_more_than_present_fails_and_rolls_back() {
        let mut inv = Inventory::default();
        let id = give(&mut inv, 0, "minecraft:stone", 3);
        let before = inv.contents.clone();
        // la prima azione è valida, la seconda no: nulla deve restare
        let req = request(
            -1,
            vec![StackAction::Take { count: 1, src: hotbar(0, id), dst: cursor(0) }, StackAction::Take { count: 5, src: hotbar(0, -1), dst: hotbar(1, 0) }],
        );
        let result = inv.handle(&req, true);
        assert!(!result.ok && result.error.is_some());
        assert_eq!(inv.contents.main, before.main);
        assert_eq!((&inv.contents.cursor, &inv.contents.created), (&before.cursor, &before.created));
        assert!(!inv.pending.contains_key(&-1), "una richiesta annullata non lascia previsioni");
        // si rimandano le caselle nominate
        assert_eq!(result.changed, [(HOTBAR_CONTAINER, 0), (CURSOR_CONTAINER, 0), (HOTBAR_CONTAINER, 1)]);
    }

    #[test]
    fn craft_creative_refused_in_survival() {
        let mut inv = Inventory::default();
        let req = request(-1, vec![craft("minecraft:stone"), StackAction::Place { count: 64, src: created(-1), dst: hotbar(0, 0) }]);
        let result = inv.handle(&req, false);
        assert!(!result.ok);
        assert!(inv.contents.main[0].item.is_empty() && inv.contents.created.item.is_empty());
        // una voce creativa inesistente si rifiuta anche in creativo
        for net_id in [0, 1_000_000] {
            assert!(!inv.handle(&request(-3, vec![StackAction::CraftCreative { net_id, crafts: 1 }]), true).ok, "{net_id}");
        }
    }

    #[test]
    fn stack_id_mismatch_is_rejected() {
        let mut inv = Inventory::default();
        let id = give(&mut inv, 0, "minecraft:stone", 8);
        let before = inv.contents.main.clone();
        for wrong in [id + 1, 0, id - 1] {
            let req = request(-1, vec![StackAction::Take { count: 2, src: hotbar(0, wrong), dst: hotbar(1, 0) }]);
            let result = inv.handle(&req, true);
            assert!(!result.ok, "ID {wrong}");
            assert!(result.error.unwrap().contains("ID di pila"));
            assert_eq!(inv.contents.main, before);
        }
        // una destinazione che il client crede vuota ma non lo è
        let other = give(&mut inv, 1, "minecraft:dirt", 1);
        let req = request(-3, vec![StackAction::Take { count: 2, src: hotbar(0, id), dst: hotbar(1, 0) }]);
        assert!(!inv.handle(&req, true).ok);
        assert_eq!(inv.contents.main[1].stack_id, other);
    }

    #[test]
    fn transfers_respect_item_kind_and_limits() {
        let mut inv = Inventory::default();
        let a = give(&mut inv, 0, "minecraft:stone", 40);
        let b = give(&mut inv, 1, "minecraft:stone", 30);
        let c = give(&mut inv, 2, "minecraft:dirt", 1);
        let take =
            |inv: &mut Inventory, n, from: SlotRef, to: SlotRef| inv.handle(&request(-1, vec![StackAction::Take { count: n, src: from, dst: to }]), true);
        assert!(!take(&mut inv, 40, hotbar(0, a), hotbar(1, b)).ok, "30 + 40 supera 64");
        assert!(!take(&mut inv, 1, hotbar(0, a), hotbar(2, c)).ok, "oggetti diversi");
        assert!(!take(&mut inv, 1, hotbar(0, a), hotbar(0, a)).ok, "stessa casella: raddoppierebbe la pila");
        assert!(!take(&mut inv, 0, hotbar(0, a), hotbar(3, 0)).ok, "quantità zero");
        assert!(!take(&mut inv, 65, hotbar(0, a), hotbar(3, 0)).ok, "oltre una pila");
        assert!(!take(&mut inv, 1, hotbar(0, a), created(0)).ok, "la casella creata non riceve");
        assert!(!take(&mut inv, 1, hotbar(0, a), at(HOTBAR_CONTAINER, 9, 0)).ok, "la barra ha 9 caselle");
        assert!(!take(&mut inv, 1, hotbar(0, a), at(99, 0, 0)).ok, "contenitore sconosciuto");
        assert!(take(&mut inv, 4, hotbar(0, a), hotbar(1, b)).ok);
        assert_eq!((inv.contents.main[0].item.count, inv.contents.main[1].item.count), (36, 34));
        // l'inventario (29) e la barra (28) sono lo stesso inventario
        let id = inv.contents.main[1].stack_id;
        assert!(take(&mut inv, 4, hotbar(1, id), at(INVENTORY_CONTAINER, 20, 0)).ok);
        assert_eq!(inv.contents.main[20].item.count, 4);
    }

    #[test]
    fn stacks_with_different_nbt_do_not_merge() {
        let mut inv = Inventory::default();
        let a = give(&mut inv, 0, "minecraft:stone", 1);
        let b = give(&mut inv, 1, "minecraft:stone", 1);
        inv.contents.main[1].item.nbt = Some(vec![0x0a, 0, 0, 0]);
        let req = request(-1, vec![StackAction::Take { count: 1, src: hotbar(0, a), dst: hotbar(1, b) }]);
        assert!(!inv.handle(&req, true).ok);
    }

    #[test]
    fn drop_removes_items_and_offhand_armor_work() {
        let mut inv = Inventory::default();
        let a = give(&mut inv, 0, "minecraft:stone", 5);
        assert!(inv.handle(&request(-1, vec![StackAction::Drop { count: 2, src: hotbar(0, a), randomly: false }]), false).ok);
        assert_eq!(inv.contents.main[0].item.count, 3);
        let a = inv.contents.main[0].stack_id;
        let to_offhand = StackAction::Take { count: 3, src: hotbar(0, a), dst: at(OFFHAND_CONTAINER, 1, 0) };
        assert!(inv.handle(&request(-3, vec![to_offhand]), false).ok);
        assert_eq!(inv.contents.offhand.item.count, 3);
        let off = inv.contents.offhand.stack_id;
        let to_armor = StackAction::Take { count: 3, src: at(OFFHAND_CONTAINER, 1, off), dst: at(ARMOR_CONTAINER, 3, 0) };
        assert!(inv.handle(&request(-5, vec![to_armor]), false).ok);
        assert_eq!(inv.contents.armor[3].item.count, 3);
        let bad = StackAction::Take { count: 1, src: at(ARMOR_CONTAINER, 3, inv.contents.armor[3].stack_id), dst: at(ARMOR_CONTAINER, 4, 0) };
        assert!(!inv.handle(&request(-7, vec![bad]), false).ok, "l'armatura ha 4 caselle");
        assert!(!inv.handle(&request(-9, vec![StackAction::Other(5)]), false).ok);
    }

    #[test]
    fn response_packets_describe_the_changes() {
        let mut inv = Inventory::default();
        let req = request(-1, vec![craft("minecraft:stone"), StackAction::Place { count: 64, src: created(-1), dst: hotbar(2, 0) }]);
        let result = inv.handle(&req, true);
        let packets = inv.response(-1, &result);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].id, id::ITEM_STACK_RESPONSE);
        let net_id = inv.contents.main[2].stack_id;
        let mut zigzag = Vec::new();
        bedrock_protocol::varint::write_signed(&mut zigzag, net_id);
        let body = &packets[0].body;
        // 1 risposta, esito 0, ID richiesta (-1 -> 1), contenitori presenti, 2 contenitori (creato per primo: è la prima azione)
        assert_eq!(&body[..5], [1, 0, 1, 1, 2]);
        // contenitore 28 con la casella 2: richiesta 2, casella 2, quantità 64, ID di rete presente
        let hotbar_info = [HOTBAR_CONTAINER, 0, 1, 2, 2, 64, 1];
        assert!(body.windows(hotbar_info.len()).any(|w| w == hotbar_info), "{body:02x?}");
        assert!(body.windows(zigzag.len()).any(|w| w == zigzag.as_slice()));
        // la casella creata torna vuota: quantità 0 e nessun ID
        let created_info = [CREATED_OUTPUT_CONTAINER, 0, 1, CREATED_SLOT, CREATED_SLOT, 0, 0];
        assert!(body.windows(created_info.len()).any(|w| w == created_info), "{body:02x?}");
    }

    #[test]
    fn rejected_request_resends_the_real_slots() {
        let mut inv = Inventory::default();
        let id = give(&mut inv, 0, "minecraft:stone", 3);
        let req = request(-1, vec![StackAction::Take { count: 9, src: hotbar(0, id), dst: cursor(0) }]);
        let result = inv.handle(&req, true);
        let packets = inv.response(-1, &result);
        assert_eq!(packets.iter().map(|p| p.id).collect::<Vec<_>>(), [id::ITEM_STACK_RESPONSE, id::INVENTORY_SLOT, id::INVENTORY_SLOT]);
        assert_eq!(packets[0].body, [1, 1, 1, 0], "errore, ID richiesta, nessun contenitore");
        // prima la casella 0 della finestra inventario (pietra), poi il cursore (finestra UI, vuoto)
        let stone_slot = &packets[1].body;
        assert_eq!(&stone_slot[..3], [0, 0, 1], "finestra 0, casella 0, nome del contenitore presente");
        assert_eq!(&packets[2].body[..2], [window::UI as u8, 0]);
    }
}
