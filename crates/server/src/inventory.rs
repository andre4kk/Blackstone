//! Inventario di un giocatore e risoluzione delle richieste di pila (`ItemStackRequest`).
//!
//! Il comportamento segue Dragonfly (MIT, `server/session/handler_item_stack_request.go`): ogni pila ha un ID di rete scelto dal
//! server, il client lo ripete in ogni richiesta e una richiesta sbagliata annulla tutte le modifiche che aveva già fatto.
//! Gli ID negativi nelle richieste sono previsioni del client: puntano a una richiesta precedente e si risolvono con
//! gli ID che il server le ha assegnato (finché il client non conferma la casella con un ID non negativo).
use crate::{
    recipes::{recipes, Ingredient, Recipe},
    registry::{self, CreativeItem},
};
use blackstone_protocol::{
    generated::{
        enums::container_enum_name::{
            ARMOR_CONTAINER, COMBINED_HOTBAR_AND_INVENTORY_CONTAINER, CRAFTING_INPUT_CONTAINER, CREATED_OUTPUT_CONTAINER, CURSOR_CONTAINER,
            FURNACE_FUEL_CONTAINER, FURNACE_INGREDIENT_CONTAINER, FURNACE_RESULT_CONTAINER, HOTBAR_CONTAINER, INVENTORY_CONTAINER, LEVEL_ENTITY_CONTAINER,
            OFFHAND_CONTAINER,
        },
        FullContainerName, InventoryContent, InventorySlot, ItemStackResponse, ItemStackResponseContainerInfo, ItemStackResponseInfo,
        ItemStackResponseSlotInfo, PlayerHotbar,
    },
    input::{stack_type, SlotRef, StackAction, StackRequest},
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
/// Caselle della griglia di crafting: 4 (2x2 dell'inventario) o 9 (3x3 del banco da lavoro).
const GRID_SMALL: u8 = 4;
const GRID_LARGE: u8 = 9;
/// Numero della prima casella della griglia come lo scrive il client nel contenitore `CraftingInput`: 28 per la 2x2, 32 per la 3x3
/// (Dragonfly `craftingGridSmallOffset`/`craftingGridLargeOffset`).
const GRID_SMALL_OFFSET: u8 = 28;
const GRID_LARGE_OFFSET: u8 = 32;
/// Tipo di contenitore del banco da lavoro in `ContainerOpen` (`ContainerTypeWorkbench` di gophertunnel).
pub(crate) const CONTAINER_TYPE_WORKBENCH: u8 = 1;
/// Massimo di una pila di un oggetto qualunque (il tetto): il massimo vero dipende dall'oggetto, vedi `max_stack_of`.
pub(crate) const MAX_STACK: u16 = 64;

/// Massimo di una pila dato il nome dell'oggetto (`MaxCount` di Dragonfly, `server/item`): 1 per spade, attrezzi, armature, pozioni, stufati,
/// dischi, archi, secchi pieni e simili; 16 per uova, palle di neve, perle, secchi vuoti, miele, libri scritti; 64 per il resto.
/// `items_meta.json` non basta: i suoi componenti non portano `max_stack_size` (quasi tutti gli oggetti non sono data-driven), quindi
/// la tabella è questa. Oltre a Dragonfly ci sono gli oggetti vanilla che Dragonfly non ha (barche, carrelli, letti, cartelli, stendardi,
/// scatole di Shulker, armature per cavalli), con il massimo del gioco originale.
pub(crate) fn max_stack_of(name: &str) -> u16 {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    const ONE_EXACT: &[&str] = &[
        "bow",
        "crossbow",
        "elytra",
        "shears",
        "flint_and_steel",
        "fishing_rod",
        "carrot_on_a_stick",
        "warped_fungus_on_a_stick",
        "totem_of_undying",
        "shield",
        "trident",
        "mace",
        "spyglass",
        "brush",
        "enchanted_book",
        "potion",
        "splash_potion",
        "lingering_potion",
        "writable_book",
        "mushroom_stew",
        "rabbit_stew",
        "beetroot_soup",
        "suspicious_stew",
        "goat_horn",
        "saddle",
        "bundle",
        "cake",
        "minecart",
        "wolf_armor",
        "turtle_helmet",
    ];
    const ONE_SUFFIX: &[&str] = &[
        "_sword",
        "_axe",
        "_pickaxe",
        "_shovel",
        "_hoe",
        "_helmet",
        "_chestplate",
        "_leggings",
        "_boots",
        "_boat",
        "_raft",
        "_minecart",
        "_horse_armor",
        "_banner_pattern",
        "_bed",
        "_shulker_box",
        "_bucket",
    ];
    const SIXTEEN_EXACT: &[&str] = &["egg", "snowball", "ender_pearl", "bucket", "honey_bottle", "written_book", "armor_stand"];
    const SIXTEEN_SUFFIX: &[&str] = &["sign", "_banner"];
    if name == "bucket" || SIXTEEN_EXACT.contains(&name) || SIXTEEN_SUFFIX.iter().any(|s| name.ends_with(s)) {
        16
    } else if ONE_EXACT.contains(&name) || name.starts_with("music_disc_") || ONE_SUFFIX.iter().any(|s| name.ends_with(s)) {
        1
    } else {
        MAX_STACK
    }
}
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// Oggetto con questo nome (`minecraft:cobblestone`) e quantità; porta lo stato del blocco che piazza, se è un blocco.
    /// `None` se il nome non è nel registro degli oggetti.
    pub fn from_name(name: &str, count: u16) -> Option<Item> {
        let items = registry::items();
        Some(Item { id: items.id(name)?, count, aux: 0, block: items.block_of(name), nbt: None })
    }

    pub fn is_empty(&self) -> bool {
        self.id == 0 || self.count == 0
    }

    /// Stesso genere di oggetto (ID, valore ausiliario e stato del blocco), quantità a parte.
    pub fn same_kind(&self, other: &Item) -> bool {
        self.id == other.id && self.aux == other.aux && self.block == other.block
    }

    /// Due pile si fondono solo se, oltre al genere, hanno lo stesso NBT (due libri incantati diversi non si impilano).
    pub(crate) fn stacks_with(&self, other: &Item) -> bool {
        self.same_kind(other) && self.nbt == other.nbt
    }

    /// Il genere di oggetto che il client dichiara di avere in mano (la quantità e i dati utente non contano per `same_kind`).
    pub fn kind_of(stack: &ItemStack) -> Item {
        Item { id: stack.id, count: stack.count, aux: stack.aux, block: (stack.block_runtime_id != 0).then_some(stack.block_runtime_id as i32), nbt: None }
    }

    /// Voce del menu creativo come oggetto (una pila piena del suo massimo); `None` se l'oggetto non ha un ID.
    fn from_creative(c: &CreativeItem) -> Option<Item> {
        let item = Item { id: registry::items().id(&c.name)?, count: 1, aux: c.meta, block: c.block, nbt: c.nbt.clone() };
        Some(item.with_count(item.max_stack()))
    }

    /// Massimo della pila di questo oggetto.
    pub fn max_stack(&self) -> u16 {
        registry::items().name(self.id).map_or(MAX_STACK, max_stack_of)
    }

    pub(crate) fn with_count(&self, count: u16) -> Item {
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
    /// Casella della griglia di crafting (0..4 senza banco, 0..9 col banco).
    Grid(u8),
    /// Casella del contenitore a blocco aperto: forziere (0..27, 0..54 se doppio) o fornace (0 ingresso, 1 combustibile, 2 risultato).
    Chest(u8),
}

/// Caselle della fornace nella finestra (ordine di Dragonfly: ingresso, combustibile, risultato).
pub(crate) const FURNACE_INGREDIENT: u8 = 0;
pub(crate) const FURNACE_FUEL: u8 = 1;
pub(crate) const FURNACE_RESULT: u8 = 2;
/// Tipo di contenitore della fornace in `ContainerOpen` (`ContainerTypeFurnace` di gophertunnel).
pub(crate) const CONTAINER_TYPE_FURNACE: u8 = 2;

/// Quali finestre ha aperte il giocatore: serve a dare un senso ai numeri di casella e di finestra che scrive il client.
#[derive(Clone, Copy, Debug, Default)]
struct Ctx {
    /// Banco da lavoro aperto (griglia 3x3).
    table: bool,
    /// Caselle del forziere aperto (0 se nessuno) e ID della sua finestra.
    chest: usize,
    chest_window: u8,
    /// Il contenitore aperto è una fornace: le sue tre caselle hanno tre contenitori diversi.
    furnace: bool,
}

impl Loc {
    /// Contenitore e casella dati dal client. Barra e inventario sono due viste dello stesso inventario.
    fn resolve(container: u8, slot: u8, ctx: Ctx) -> Option<Loc> {
        let table = ctx.table;
        match container {
            LEVEL_ENTITY_CONTAINER if !ctx.furnace && (slot as usize) < ctx.chest => Some(Loc::Chest(slot)),
            // Dragonfly usa il numero di casella come indice nella finestra; qui conta il contenitore (l'indice può essere 0 o quello vero).
            FURNACE_INGREDIENT_CONTAINER if ctx.furnace => Some(Loc::Chest(FURNACE_INGREDIENT)),
            FURNACE_FUEL_CONTAINER if ctx.furnace => Some(Loc::Chest(FURNACE_FUEL)),
            FURNACE_RESULT_CONTAINER if ctx.furnace => Some(Loc::Chest(FURNACE_RESULT)),
            CRAFTING_INPUT_CONTAINER => {
                let (offset, size) = if table { (GRID_LARGE_OFFSET, GRID_LARGE) } else { (GRID_SMALL_OFFSET, GRID_SMALL) };
                slot.checked_sub(offset).filter(|i| *i < size).map(Loc::Grid)
            }
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
    fn window(self, ctx: Ctx) -> (u32, u32, u8) {
        let table = ctx.table;
        match self {
            Loc::Chest(i) if ctx.furnace => (
                ctx.chest_window as u32,
                i as u32,
                match i {
                    FURNACE_INGREDIENT => FURNACE_INGREDIENT_CONTAINER,
                    FURNACE_FUEL => FURNACE_FUEL_CONTAINER,
                    _ => FURNACE_RESULT_CONTAINER,
                },
            ),
            Loc::Chest(i) => (ctx.chest_window as u32, i as u32, LEVEL_ENTITY_CONTAINER),
            Loc::Grid(i) => (window::UI, i as u32 + if table { GRID_LARGE_OFFSET } else { GRID_SMALL_OFFSET } as u32, CRAFTING_INPUT_CONTAINER),
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
    grid: [Slot; GRID_LARGE as usize],
    /// Caselle del forziere aperto (vuoto se non ce n'è uno).
    chest: Vec<Slot>,
}

impl Contents {
    fn get(&self, loc: Loc) -> &Slot {
        match loc {
            Loc::Main(i) => &self.main[i as usize],
            Loc::Offhand => &self.offhand,
            Loc::Armor(i) => &self.armor[i as usize],
            Loc::Cursor => &self.cursor,
            Loc::Created => &self.created,
            Loc::Grid(i) => &self.grid[i as usize],
            Loc::Chest(i) => &self.chest[i as usize],
        }
    }

    fn get_mut(&mut self, loc: Loc) -> &mut Slot {
        match loc {
            Loc::Main(i) => &mut self.main[i as usize],
            Loc::Offhand => &mut self.offhand,
            Loc::Armor(i) => &mut self.armor[i as usize],
            Loc::Cursor => &mut self.cursor,
            Loc::Created => &mut self.created,
            Loc::Grid(i) => &mut self.grid[i as usize],
            Loc::Chest(i) => &mut self.chest[i as usize],
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
    /// Gli oggetti buttati dalle azioni `Drop` (già tolti dalle caselle) e, in parallelo, la casella da cui vengono: li spawna il server.
    pub dropped: Vec<Item>,
    pub dropped_from: Vec<(u8, u8)>,
}

/// L'inventario di un giocatore: 36 caselle (0-8 barra), mano secondaria, armatura, cursore e casella creata.
pub(crate) struct Inventory {
    contents: Contents,
    /// Casella della barra in mano (0-8).
    pub selected: u8,
    next_id: i32,
    pending: HashMap<i32, Pending>,
    /// Banco da lavoro aperto (posizione del blocco): la griglia è 3x3 e i suoi numeri di casella partono da 32.
    table: Option<[i32; 3]>,
    /// ID della finestra aperta del banco (1..=99, come Dragonfly `nextWindowID`) e ultimo assegnato.
    table_window: u8,
    last_window: u8,
    /// Contenitore a blocco aperto (forziere o fornace): finestra e posizioni dei blocchi che ne contengono le caselle (una, o due se è un
    /// forziere doppio: prima metà della finestra nel primo blocco, seconda metà nel secondo). Le caselle stanno in `contents.chest`.
    chest: Option<ChestWindow>,
}

/// Che genere di contenitore a blocco è aperto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowKind {
    Chest,
    Furnace,
}

/// La finestra di un contenitore a blocco aperto (forziere o fornace).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChestWindow {
    pub kind: WindowKind,
    pub window: u8,
    /// Blocchi che la compongono, nell'ordine delle caselle (27 ciascuno).
    pub parts: Vec<[i32; 3]>,
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
                grid: std::array::from_fn(|_| Slot::default()),
                chest: Vec::new(),
            },
            selected: 0,
            next_id: 1,
            pending: HashMap::new(),
            table: None,
            table_window: 0,
            last_window: 0,
            chest: None,
        }
    }
}

/// Raccolta delle modifiche di una richiesta.
struct Run {
    request_id: i32,
    changed: Vec<(u8, u8)>,
    dropped: Vec<(Item, (u8, u8))>,
    now: Instant,
    /// Risultati di una ricetta in attesa di `Create` (più di uno quando il totale supera una pila).
    results: Vec<Item>,
}

impl Inventory {
    /// L'oggetto in mano (casella `selected` della barra).
    pub fn held(&self) -> &Item {
        &self.contents.main[self.selected as usize].item
    }

    /// Il pacchetto `InventorySlot` con la casella in mano com'è per il server (anche vuota): serve a riallineare il client
    /// quando ha previsto qualcosa di diverso (un oggetto consumato, un'altra casella in mano).
    pub fn held_slot_packet(&self) -> Packet {
        self.slot_packet(Loc::Main(self.selected))
    }

    /// Il pacchetto `InventorySlot` della casella `i` dell'inventario principale.
    pub fn main_slot_packet(&self, i: u8) -> Packet {
        self.slot_packet(Loc::Main(i))
    }

    /// Il pacchetto `InventorySlot` per una casella nominata dal client (`None` se non esiste).
    pub fn slot_packet_at(&self, container: u8, slot: u8) -> Option<Packet> {
        Loc::resolve(container, slot, self.ctx()).map(|loc| self.slot_packet(loc))
    }

    fn slot_packet(&self, loc: Loc) -> Packet {
        let (window, index, name) = loc.window(self.ctx());
        InventorySlot {
            container_id: window as u8,
            slot: index,
            full_container_name: Some(FullContainerName { container_name: name, dynamic_id: None }),
            storage_item: None,
            item: self.stack_of(loc),
        }
        .packet()
    }

    /// Il pacchetto `PlayerHotbar` che dice al client quale casella della barra è in mano e gliela fa selezionare.
    pub fn hotbar_packet(&self) -> Packet {
        PlayerHotbar { selected_slot: self.selected as u32, container_id: window::INVENTORY as u8, should_select_slot: true }.packet()
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

    /// Svuota inventario, mano secondaria, armatura e oggetto sul cursore (non quello creato dal menu creativo) e restituisce ciò che c'era.
    pub fn take_all(&mut self) -> Vec<Item> {
        let locs = (0..MAIN_SLOTS as u8).map(Loc::Main).chain([Loc::Offhand]).chain((0..ARMOR_SLOTS as u8).map(Loc::Armor));
        let items: Vec<Item> = locs.map(|loc| std::mem::take(self.contents.get_mut(loc))).map(|slot| slot.item).filter(|item| !item.is_empty()).collect();
        self.contents.created = Slot::default();
        items.into_iter().chain(self.release_ui()).collect()
    }

    /// Mette un oggetto in una casella della griglia di crafting (solo per i test).
    #[cfg(test)]
    pub fn set_grid(&mut self, i: usize, item: Item) {
        if i < GRID_LARGE as usize {
            self.fill(Loc::Grid(i as u8), item);
        }
    }

    /// Il banco da lavoro aperto, se c'è.
    pub fn table(&self) -> Option<[i32; 3]> {
        self.table
    }

    /// L'ID della finestra del banco aperto.
    pub fn table_window(&self) -> Option<u8> {
        self.table.map(|_| self.table_window)
    }

    /// Apre il banco in `pos` e dà l'ID della finestra (1..=99). Gli oggetti della griglia di prima vanno restituiti prima con `release_ui`.
    pub fn open_table(&mut self, pos: [i32; 3]) -> u8 {
        self.table = Some(pos);
        self.table_window = self.next_window();
        self.table_window
    }

    /// Il prossimo ID di finestra (1..=99, come Dragonfly `nextWindowID`): uno solo per banco, forzieri e il resto.
    fn next_window(&mut self) -> u8 {
        self.last_window = if self.last_window >= 99 { 1 } else { self.last_window + 1 };
        self.last_window
    }

    fn ctx(&self) -> Ctx {
        Ctx {
            table: self.table.is_some(),
            chest: self.contents.chest.len(),
            chest_window: self.chest.as_ref().map_or(0, |c| c.window),
            furnace: self.chest.as_ref().is_some_and(|c| c.kind == WindowKind::Furnace),
        }
    }

    /// Il forziere aperto, se c'è.
    pub fn chest(&self) -> Option<&ChestWindow> {
        self.chest.as_ref()
    }

    /// Apre un forziere composto dai blocchi `parts` con le caselle `items` (27 per blocco) e dà l'ID della finestra. Ogni casella piena
    /// prende un ID di pila nuovo. Ciò che era aperto va chiuso prima.
    pub fn open_chest(&mut self, parts: Vec<[i32; 3]>, items: Vec<Item>) -> u8 {
        self.open_container(WindowKind::Chest, parts, items)
    }

    /// Apre la fornace in `pos` con le sue tre caselle (ingresso, combustibile, risultato).
    pub fn open_furnace(&mut self, pos: [i32; 3], items: Vec<Item>) -> u8 {
        self.open_container(WindowKind::Furnace, vec![pos], items)
    }

    fn open_container(&mut self, kind: WindowKind, parts: Vec<[i32; 3]>, items: Vec<Item>) -> u8 {
        let window = self.next_window();
        self.contents.chest = vec![Slot::default(); items.len()];
        for (i, item) in items.into_iter().enumerate() {
            self.fill(Loc::Chest(i as u8), item);
        }
        self.chest = Some(ChestWindow { kind, window, parts });
        window
    }

    /// Chiude il forziere e restituisce la sua finestra (`None` se non ce n'era uno).
    pub fn close_chest(&mut self) -> Option<ChestWindow> {
        self.contents.chest.clear();
        self.chest.take()
    }

    /// Gli oggetti delle caselle del forziere aperto.
    pub fn chest_items(&self) -> Vec<Item> {
        self.contents.chest.iter().map(|s| s.item.clone()).collect()
    }

    /// Allinea la casella `i` del forziere a `item` (cambiata da un altro giocatore): se è diversa prende un ID nuovo e restituisce il
    /// pacchetto `InventorySlot` da mandare; `None` se era già uguale.
    pub fn refresh_chest_slot(&mut self, i: usize, item: Item) -> Option<Packet> {
        if i >= self.contents.chest.len() || self.contents.chest[i].item == item {
            return None;
        }
        self.fill(Loc::Chest(i as u8), item);
        Some(self.slot_packet(Loc::Chest(i as u8)))
    }

    /// `InventoryContent` del contenitore aperto (tutte le caselle). Per la fornace il nome è quello dell'ingresso (Dragonfly non ne mette).
    pub fn chest_content_packet(&self) -> Option<Packet> {
        let window = self.chest.as_ref()?.window;
        let name = if self.ctx().furnace { FURNACE_INGREDIENT_CONTAINER } else { LEVEL_ENTITY_CONTAINER };
        let slots = (0..self.contents.chest.len() as u8).map(|i| self.stack_of(Loc::Chest(i))).collect();
        Some(
            InventoryContent {
                container_id: window as u32,
                slots,
                full_container_name: FullContainerName { container_name: name, dynamic_id: None },
                storage_item: ItemStack::air(),
            }
            .packet(),
        )
    }

    /// Vero se la richiesta tocca caselle del contenitore a blocco aperto.
    pub fn touches_chest(&self, req: &StackRequest) -> bool {
        self.chest.is_some() && referenced(req).iter().any(|&(container, slot)| matches!(Loc::resolve(container, slot, self.ctx()), Some(Loc::Chest(_))))
    }

    /// Vero se `loc` è la casella del risultato della fornace aperta: ci si può solo prendere.
    fn is_furnace_result(&self, loc: Loc) -> bool {
        loc == Loc::Chest(FURNACE_RESULT) && self.ctx().furnace
    }

    /// Chiude il banco e svuota la griglia di crafting e il cursore (Dragonfly `MoveItemsToInventory`: `ui.Clear()`): restituisce
    /// gli oggetti, che chi chiama rimette nell'inventario o butta a terra.
    pub fn release_ui(&mut self) -> Vec<Item> {
        self.table = None;
        let locs = (0..GRID_LARGE).map(Loc::Grid).chain([Loc::Cursor]);
        locs.map(|loc| std::mem::take(self.contents.get_mut(loc))).map(|slot| slot.item).filter(|item| !item.is_empty()).collect()
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
        Loc::resolve(container, slot, self.ctx()).map(|loc| self.stack_of(loc))
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
        let mut run = Run { request_id: req.request_id, changed: Vec::new(), dropped: Vec::new(), now: Instant::now(), results: Vec::new() };
        match self.run(req, creative, &mut run) {
            Ok(()) => {
                // La casella creata è di passaggio: quello che il client non ha preso (il server non sa quanto sta in una pila) si butta.
                self.contents.created = Slot::default();
                let (dropped, dropped_from) = run.dropped.into_iter().unzip();
                StackResult { ok: true, changed: run.changed, error: None, dropped, dropped_from }
            }
            Err(error) => {
                self.contents = backup;
                self.pending.remove(&req.request_id);
                let mut changed = Vec::new();
                for (container, slot) in referenced(req) {
                    if Loc::resolve(container, slot, self.ctx()).is_some() && !changed.contains(&(container, slot)) {
                        changed.push((container, slot));
                    }
                }
                StackResult { ok: false, changed, error: Some(error), dropped: Vec::new(), dropped_from: Vec::new() }
            }
        }
    }

    fn run(&mut self, req: &StackRequest, creative: bool, run: &mut Run) -> Result<(), String> {
        self.pending.retain(|_, p| run.now.duration_since(p.at) <= PENDING_TTL);
        for action in &req.actions {
            match action {
                StackAction::Take { count, src, dst } | StackAction::Place { count, src, dst } => self.transfer(run, src, dst, *count)?,
                StackAction::Swap { src, dst } => self.swap(run, src, dst)?,
                StackAction::Drop { count, src, .. } => {
                    let item = self.remove(run, src, *count)?;
                    run.dropped.push((item, (src.container, src.slot)));
                }
                StackAction::Destroy { count, src } => {
                    if !creative {
                        return Err("Destroy è permessa solo con l'inventario creativo".into());
                    }
                    self.remove(run, src, *count)?;
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
                StackAction::CraftRecipe { recipe_id, crafts } => self.craft(run, *recipe_id, *crafts, false)?,
                StackAction::CraftRecipeAuto { recipe_id, crafts } => self.craft(run, *recipe_id, *crafts, true)?,
                StackAction::Create { results_slot } => self.create(run, *results_slot)?,
                // Il consumo lo ha già fatto la ricetta (Dragonfly ignora `Consume`).
                StackAction::Other(stack_type::CONSUME) => {}
                // I risultati del crafting sono solo informativi: li calcola il server (come Dragonfly, che li ignora).
                StackAction::Other(stack_type::CRAFT_RESULTS) => {}
                StackAction::Other(kind) => return Err(format!("azione {kind} non supportata")),
            }
        }
        Ok(())
    }

    /// Dà la casella indicata dal client e controlla che l'ID di pila che il client ci vede sia quello del server.
    fn verified(&mut self, r: &SlotRef) -> Result<Loc, String> {
        let loc = Loc::resolve(r.container, r.slot, self.ctx()).ok_or_else(|| format!("casella {}/{} inesistente", r.container, r.slot))?;
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

    /// Contenitore e numero di casella della griglia come li nomina il client.
    fn grid_name(&self, i: u8) -> (u8, u8) {
        (CRAFTING_INPUT_CONTAINER, i + if self.table.is_some() { GRID_LARGE_OFFSET } else { GRID_SMALL_OFFSET })
    }

    /// `CraftRecipe` e `CraftRecipeAuto` (Dragonfly `handleCraft`/`handleAutoCraft`): controlla che gli ingredienti ci siano, li consuma
    /// (`crafts` volte) e prepara il risultato. Con `auto` si prende anche dall'inventario (il libro delle ricette con Maiusc); altrimenti
    /// ogni ingrediente deve stare in una casella diversa della griglia. Le caselle vuote della ricetta (forma) si ignorano: Dragonfly
    /// non controlla la posizione, solo che ci siano gli ingredienti.
    fn craft(&mut self, run: &mut Run, recipe_id: u32, crafts: u8, auto: bool) -> Result<(), String> {
        let recipe = recipes().by_network_id(recipe_id).ok_or_else(|| format!("ricetta {recipe_id} sconosciuta"))?;
        if recipe.block != "crafting_table" {
            return Err(format!("la ricetta {recipe_id} non è di banco da lavoro"));
        }
        if crafts < 1 {
            return Err("si fa almeno una volta".into());
        }
        if self.table.is_none() && recipe.needs_table() {
            return Err(format!("la ricetta {recipe_id} non entra nella griglia 2x2"));
        }
        let times = crafts as u32;
        let size = if self.table.is_some() { GRID_LARGE } else { GRID_SMALL };
        if auto {
            // Ingredienti uguali si sommano; si consuma prima dalla griglia, poi dall'inventario.
            let mut flat: Vec<(&Ingredient, u32)> = Vec::new();
            for ing in recipe.input.iter().filter(|i| **i != Ingredient::Empty) {
                match flat.iter_mut().find(|(f, _)| same_ingredient(f, ing)) {
                    Some((_, n)) => *n += ing.count(),
                    None => flat.push((ing, ing.count())),
                }
            }
            for (ing, count) in flat {
                let mut remaining = count * times;
                let locs = (0..size).map(Loc::Grid).chain((0..MAIN_SLOTS as u8).map(Loc::Main));
                for loc in locs {
                    if remaining == 0 {
                        break;
                    }
                    let item = self.contents.get(loc).item.clone();
                    if item.is_empty() || !item_matches(ing, &item) {
                        continue;
                    }
                    let take = remaining.min(item.count as u32);
                    remaining -= take;
                    let (container, slot) = match loc {
                        Loc::Grid(i) => self.grid_name(i),
                        Loc::Main(i) => (COMBINED_HOTBAR_AND_INVENTORY_CONTAINER, i),
                        _ => unreachable!(),
                    };
                    self.put(run, container, slot, loc, item.with_count(item.count - take as u16));
                }
                if remaining != 0 {
                    return Err(format!("ricetta {recipe_id}: mancano {remaining} di {ing:?}"));
                }
            }
        } else {
            let mut used = [false; GRID_LARGE as usize];
            for ing in recipe.input.iter().filter(|i| **i != Ingredient::Empty) {
                let need = ing.count() * times;
                let found = (0..size).find(|&i| {
                    let item = &self.contents.grid[i as usize].item;
                    !used[i as usize] && !item.is_empty() && item.count as u32 >= need && item_matches(ing, item)
                });
                let Some(i) = found else { return Err(format!("ricetta {recipe_id}: ingrediente {ing:?} assente nella griglia")) };
                used[i as usize] = true;
                let item = self.contents.grid[i as usize].item.clone();
                let (container, slot) = self.grid_name(i);
                self.put(run, container, slot, Loc::Grid(i), item.with_count(item.count - need as u16));
            }
        }
        let results = recipe_results(recipe, times);
        let single = results.len() <= 1;
        run.results.extend(results);
        if single {
            self.create(run, 0)?; // un solo risultato: si crea subito, come `defaultCreation` di Dragonfly
        }
        Ok(())
    }

    /// `Create`: il risultato `slot` tra quelli preparati passa nella casella creata (una volta sola).
    fn create(&mut self, run: &mut Run, slot: u8) -> Result<(), String> {
        let item = run.results.get_mut(slot as usize).ok_or_else(|| format!("risultato {slot} inesistente"))?;
        let item = std::mem::take(item);
        if item.is_empty() {
            return Err(format!("risultato {slot} già creato"));
        }
        self.put(run, CREATED_OUTPUT_CONTAINER, CREATED_SLOT, Loc::Created, item);
        Ok(())
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
        if self.is_furnace_result(to) {
            return Err("nel risultato della fornace non si può mettere nulla".into());
        }
        let (a, b) = (self.contents.get(from).item.clone(), self.contents.get(to).item.clone());
        if a.is_empty() || a.count < count {
            return Err(format!("si chiedono {count} oggetti ma la casella ne ha {}", if a.is_empty() { 0 } else { a.count }));
        }
        if !b.is_empty() && (!a.stacks_with(&b) || b.count + count > a.max_stack()) {
            return Err("la destinazione ha un oggetto diverso o non c'è posto".into());
        }
        let moved = if b.is_empty() { a.with_count(count) } else { b.with_count(b.count + count) };
        self.put(run, src.container, src.slot, from, a.with_count(a.count - count));
        self.put(run, dst.container, dst.slot, to, moved);
        Ok(())
    }

    fn swap(&mut self, run: &mut Run, src: &SlotRef, dst: &SlotRef) -> Result<(), String> {
        let (from, to) = (self.verified(src)?, self.verified(dst)?);
        if from == to || from == Loc::Created || to == Loc::Created || self.is_furnace_result(from) || self.is_furnace_result(to) {
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

    /// Toglie `count` oggetti da una casella (Drop e Destroy) e li restituisce: chi li butta li fa diventare un'entità nel mondo.
    fn remove(&mut self, run: &mut Run, src: &SlotRef, count: u8) -> Result<Item, String> {
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
        Ok(a.with_count(count))
    }

    /// Il giocatore butta `count` oggetti dalla casella `slot` (0-35) con Q (`handleNormalTransaction` di Dragonfly): la casella deve avere un
    /// oggetto uguale a `expected` (quello che il client dice di buttare) e almeno `count` pezzi. Si toglie e si restituisce ciò che è uscito;
    /// la casella resta com'è se qualcosa non torna. Dell'oggetto del client si usa solo il genere (e l'NBT, se l'oggetto vero ne ha): mai la quantità.
    pub fn take_dropped(&mut self, slot: u32, expected: &ItemStack, count: u32) -> Result<Item, String> {
        if slot as usize >= MAIN_SLOTS {
            return Err(format!("casella {slot} fuori dall'inventario"));
        }
        let loc = Loc::Main(slot as u8);
        let actual = self.contents.get(loc).item.clone();
        if count < 1 {
            return Err(format!("si butta almeno un oggetto, non {count}"));
        }
        if actual.is_empty() || count > actual.count as u32 {
            return Err(format!("si buttano {count} oggetti ma la casella {slot} ne ha {}", if actual.is_empty() { 0 } else { actual.count }));
        }
        let declared = Item::kind_of(expected);
        let nbt_differs = actual.nbt.is_some() && actual.stack(0).user_data != expected.user_data;
        if expected.id == 0 || !actual.same_kind(&declared) || nbt_differs {
            return Err(format!("si butta un oggetto diverso da quello nella casella {slot}"));
        }
        let count = count as u16;
        let id = self.new_id();
        let left = actual.with_count(actual.count - count);
        *self.contents.get_mut(loc) = if left.is_empty() { Slot::default() } else { Slot { item: left, stack_id: id } };
        Ok(actual.with_count(count))
    }

    /// Rimette nella casella `slot` dell'inventario ciò che un gancio ha impedito di buttare (vedi `restore`).
    pub fn restore_main(&mut self, slot: u32, item: Item) {
        self.restore(INVENTORY_CONTAINER, slot as u8, item);
    }

    /// Vero se `add` metterebbe almeno un oggetto di `item` nell'inventario (una pila uguale non piena o una casella vuota).
    pub fn accepts(&self, item: &Item) -> bool {
        !item.is_empty() && self.contents.main.iter().any(|s| s.item.is_empty() || (s.item.stacks_with(item) && s.item.count < item.max_stack()))
    }

    /// Raccoglie `item` nell'inventario: prima riempie le pile uguali, poi le caselle vuote, sempre dalla barra verso l'alto.
    /// Restituisce il resto che non ci sta (vuoto se è entrato tutto) e le caselle cambiate, come (contenitore, casella) per `slot_packet_at`.
    /// Ogni casella cambiata prende un ID di pila nuovo.
    pub fn add(&mut self, item: Item) -> (Item, Vec<(u8, u8)>) {
        let mut left = item.count;
        let max = item.max_stack();
        let mut changed = Vec::new();
        if item.is_empty() {
            return (item, changed);
        }
        for pass_empty in [false, true] {
            for i in 0..MAIN_SLOTS {
                if left == 0 {
                    break;
                }
                let slot = &self.contents.main[i].item;
                let room = if pass_empty && slot.is_empty() {
                    max
                } else if !pass_empty && !slot.is_empty() && slot.stacks_with(&item) {
                    max.saturating_sub(slot.count)
                } else {
                    0
                };
                let moved = room.min(left);
                if moved == 0 {
                    continue;
                }
                left -= moved;
                let total = self.contents.main[i].item.count * u16::from(!pass_empty) + moved;
                let stack_id = self.new_id();
                self.contents.main[i] = Slot { item: item.with_count(total), stack_id };
                changed.push((INVENTORY_CONTAINER, i as u8));
            }
        }
        (if left == 0 { Item::default() } else { item.with_count(left) }, changed)
    }

    /// Scelta del blocco col tasto centrale (Dragonfly `PickBlock`): se l'inventario ha già l'oggetto, una casella della barra si
    /// seleziona e una dell'inventario si scambia con quella in mano; altrimenti solo in creativo l'oggetto (quantità 1) va nella prima
    /// casella libera (barra: si seleziona) o, a inventario pieno, al posto di quello in mano. Restituisce le caselle cambiate
    /// (indici dell'inventario principale); `None` se non cambia niente (il cambio di casella selezionata si legge da `selected`).
    pub fn pick(&mut self, item: &Item, creative: bool) -> Option<Vec<u8>> {
        let found = self.contents.main.iter().position(|s| !s.item.is_empty() && s.item.same_kind(item));
        let held = self.selected as usize;
        if let Some(slot) = found {
            if slot < HOTBAR_SLOTS as usize {
                self.selected = slot as u8;
                return Some(Vec::new());
            }
            self.contents.main.swap(slot, held);
            return Some(vec![slot as u8, held as u8]);
        }
        if !creative {
            return None;
        }
        let picked = item.with_count(1);
        match self.contents.main.iter().position(|s| s.item.is_empty()) {
            Some(empty) if empty < HOTBAR_SLOTS as usize => {
                self.selected = empty as u8;
                self.fill(Loc::Main(empty as u8), picked);
                Some(vec![empty as u8])
            }
            Some(empty) => {
                self.contents.main.swap(empty, held);
                self.fill(Loc::Main(held as u8), picked);
                Some(vec![empty as u8, held as u8])
            }
            None => {
                self.fill(Loc::Main(held as u8), picked);
                Some(vec![held as u8])
            }
        }
    }

    /// Rimette in una casella un oggetto buttato che un gancio ha annullato. Se nel frattempo la casella contiene altro, l'oggetto va
    /// nella prima casella libera dell'inventario; se è pieno, si perde (non può accadere: l'oggetto era appena uscito da lì).
    pub fn restore(&mut self, container: u8, slot: u8, item: Item) {
        let Some(loc) = Loc::resolve(container, slot, self.ctx()) else { return };
        let current = self.contents.get(loc).item.clone();
        if current.is_empty() {
            return self.fill(loc, item);
        }
        if current.with_count(item.count) == item {
            let merged = current.with_count(current.count + item.count);
            return self.fill(loc, merged);
        }
        if let Some(free) = (0..MAIN_SLOTS as u8).map(Loc::Main).find(|&l| self.contents.get(l).item.is_empty()) {
            self.fill(free, item);
        }
    }

    /// I pacchetti di risposta a una richiesta: `ItemStackResponse` e, se è stata rifiutata, `InventorySlot` per ogni casella
    /// nominata, così il client torna allo stato vero del server.
    pub fn response(&self, request_id: i32, result: &StackResult) -> Vec<Packet> {
        if !result.ok {
            let mut packets =
                vec![ItemStackResponse { responses: vec![ItemStackResponseInfo { result: RESULT_ERROR, client_request_id: request_id, containers: None }] }
                    .packet()];
            for &(container, slot) in &result.changed {
                if let Some(loc) = Loc::resolve(container, slot, self.ctx()) {
                    let (window, index, name) = loc.window(self.ctx());
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

/// Vero se l'oggetto soddisfa l'ingrediente (nome e variante, o tag): la quantità non si guarda.
fn item_matches(ingredient: &Ingredient, item: &Item) -> bool {
    registry::items().name(item.id).is_some_and(|name| recipes().matches(ingredient, name, item.aux as i32))
}

/// Due ingredienti dello stesso genere (stesso oggetto e variante, o stesso tag), quantità a parte.
fn same_ingredient(a: &Ingredient, b: &Ingredient) -> bool {
    match (a, b) {
        (Ingredient::Item { name: n1, meta: m1, .. }, Ingredient::Item { name: n2, meta: m2, .. }) => n1 == n2 && m1 == m2,
        (Ingredient::Tag { tag: t1, .. }, Ingredient::Tag { tag: t2, .. }) => t1 == t2,
        _ => false,
    }
}

/// I risultati di `times` esecuzioni della ricetta, divisi in pile del massimo dell'oggetto (Dragonfly `repeatStacks`).
fn recipe_results(recipe: &Recipe, times: u32) -> Vec<Item> {
    let mut out = Vec::new();
    for o in &recipe.output {
        let item = Item { id: o.item_id, count: 1, aux: o.meta, block: o.block.or_else(|| registry::items().block_of(&o.name)), nbt: None };
        let (max, mut total) = (item.max_stack() as u32, o.count as u32 * times);
        while total > 0 {
            let n = total.min(max);
            total -= n;
            out.push(item.with_count(n as u16));
        }
    }
    out
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
            StackAction::CraftCreative { .. } | StackAction::CraftRecipe { .. } | StackAction::CraftRecipeAuto { .. } | StackAction::Create { .. } => {
                out.push((CREATED_OUTPUT_CONTAINER, CREATED_SLOT))
            }
            StackAction::Other(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use blackstone_protocol::id;

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
        assert!(!inv.handle(&request(-9, vec![StackAction::Other(7)]), false).ok);
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
        blackstone_protocol::varint::write_signed(&mut zigzag, net_id);
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

    #[test]
    fn add_fills_equal_stacks_then_empty_slots_hotbar_first() {
        let mut inv = Inventory::default();
        give(&mut inv, 3, "minecraft:stone", 60);
        give(&mut inv, 20, "minecraft:stone", 10);
        let (rest, changed) = inv.add(Item { id: stone(), count: 80, ..Default::default() });
        // 4 nella casella 3, 54 nella 20, poi 22 nella prima vuota (la 0)
        assert!(rest.is_empty());
        assert_eq!(changed, vec![(INVENTORY_CONTAINER, 3), (INVENTORY_CONTAINER, 20), (INVENTORY_CONTAINER, 0)]);
        assert_eq!((inv.main(3).count, inv.main(20).count, inv.main(0).count), (64, 64, 22));
        assert!(changed.iter().all(|&(c, s)| inv.stack_at(c, s).unwrap().net_id.is_some_and(|id| id > 0)));
    }

    #[test]
    fn add_returns_the_rest_when_full_and_skips_other_kinds() {
        let mut inv = Inventory::default();
        let dirt = registry::items().id("minecraft:dirt").unwrap();
        for i in 0..MAIN_SLOTS {
            inv.set_main(i, Item { id: dirt, count: 64, ..Default::default() });
        }
        inv.set_main(7, Item { id: stone(), count: 63, ..Default::default() });
        let stones = Item { id: stone(), count: 5, ..Default::default() };
        assert!(inv.accepts(&stones));
        let (rest, changed) = inv.add(stones.clone());
        assert_eq!((rest.count, changed), (4, vec![(INVENTORY_CONTAINER, 7)]));
        assert!(!inv.accepts(&stones));
        assert_eq!(inv.add(stones.clone()), (stones, vec![]));
    }

    fn named(name: &str, count: u16) -> Item {
        Item { id: registry::items().id(name).unwrap(), count, ..Default::default() }
    }

    #[test]
    fn max_stack_depends_on_the_item() {
        for (name, max) in [
            ("minecraft:diamond_sword", 1),
            ("minecraft:netherite_pickaxe", 1),
            ("minecraft:iron_chestplate", 1),
            ("minecraft:bow", 1),
            ("minecraft:oak_boat", 1),
            ("minecraft:music_disc_13", 1),
            ("minecraft:egg", 16),
            ("minecraft:snowball", 16),
            ("minecraft:ender_pearl", 16),
            ("minecraft:oak_sign", 16),
            ("minecraft:bucket", 16),
            ("minecraft:water_bucket", 1),
            ("minecraft:stone", 64),
            ("minecraft:apple", 64),
        ] {
            assert_eq!(named(name, 1).max_stack(), max, "{name}");
        }
    }

    #[test]
    fn add_never_stacks_beyond_the_real_maximum() {
        let mut inv = Inventory::default();
        let (rest, changed) = inv.add(named("minecraft:diamond_sword", 3));
        assert!(rest.is_empty());
        assert_eq!(changed.len(), 3, "una spada per casella");
        assert!((0..3).all(|i| inv.main(i).count == 1));
        let (rest, _) = inv.add(named("minecraft:egg", 20));
        assert!(rest.is_empty());
        assert_eq!((inv.main(3).count, inv.main(4).count), (16, 4));
        // una pila di spade non "accetta" altre spade se non c'è una casella vuota
        let mut full = Inventory::default();
        for i in 0..MAIN_SLOTS {
            full.set_main(i, named("minecraft:diamond_sword", 1));
        }
        assert!(!full.accepts(&named("minecraft:diamond_sword", 1)));
    }

    // ---------- crafting ----------

    /// ID di rete della ricetta che dà `output` e il cui primo ingrediente è `first` (nome di oggetto o tag).
    fn recipe_id(output: &str, first: &str) -> u32 {
        recipes()
            .all()
            .iter()
            .find(|r| {
                r.output[0].name == output
                    && matches!(r.input.iter().find(|i| **i != Ingredient::Empty), Some(Ingredient::Item { name, .. } | Ingredient::Tag { tag: name, .. }) if name == first)
            })
            .unwrap_or_else(|| panic!("ricetta {output} da {first}"))
            .id
    }

    /// Mette `count` oggetti `name` nella casella `i` della griglia.
    fn grid(inv: &mut Inventory, i: u8, name: &str, count: u16) {
        let stack_id = inv.new_id();
        inv.contents.grid[i as usize] = Slot { item: named(name, count), stack_id };
    }

    fn craft_recipe(recipe_id: u32, crafts: u8) -> StackAction {
        StackAction::CraftRecipe { recipe_id, crafts }
    }

    #[test]
    fn two_by_two_grid_slots_are_numbered_from_28_and_three_by_three_from_32() {
        let mut inv = Inventory::default();
        assert_eq!(Loc::resolve(CRAFTING_INPUT_CONTAINER, 28, Ctx::default()), Some(Loc::Grid(0)));
        assert_eq!(Loc::resolve(CRAFTING_INPUT_CONTAINER, 31, Ctx::default()), Some(Loc::Grid(3)));
        assert_eq!(Loc::resolve(CRAFTING_INPUT_CONTAINER, 32, Ctx::default()), None);
        assert_eq!(Loc::resolve(CRAFTING_INPUT_CONTAINER, 40, Ctx { table: true, ..Ctx::default() }), Some(Loc::Grid(8)));
        assert_eq!(Loc::resolve(CRAFTING_INPUT_CONTAINER, 28, Ctx { table: true, ..Ctx::default() }), None);
        inv.open_table([1, 2, 3]);
        assert_eq!(inv.slot_packet_at(CRAFTING_INPUT_CONTAINER, 40).map(|p| p.id), Some(id::INVENTORY_SLOT));
    }

    /// Le due richieste vere del client (log del gioco): prende 64 pezzi dalla casella 1 della barra e li mette nella casella 28
    /// del contenitore `CraftingInput` (griglia 2x2).
    #[test]
    fn real_client_grid_requests_are_accepted() {
        use blackstone_protocol::input::ItemStackRequestPacket;
        let take = [
            0x01, 0x3d, 0x01, 0x00, 0x00, 0x40, 0x1c, 0x00, 0x01, 0x03, 0x00, 0x00, 0x00, 0x3b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff,
            0xff,
        ];
        let place = [
            0x01, 0x41, 0x01, 0x01, 0x01, 0x40, 0x3b, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff,
            0xff,
        ];
        let mut inv = Inventory::default();
        inv.contents.main[1] = Slot { item: named("minecraft:oak_log", 64), stack_id: 3 };
        inv.next_id = 4;
        for body in [take, place] {
            let packet = ItemStackRequestPacket::decode(&body).expect("leggibile");
            let result = inv.handle(&packet.requests[0], false);
            assert!(result.ok, "{:?}", result.error);
        }
        assert_eq!(inv.contents.grid[0].item.count, 64);
        assert!(inv.contents.main[1].item.is_empty() && inv.contents.cursor.item.is_empty());
    }

    #[test]
    fn log_becomes_planks_in_the_inventory_grid() {
        let mut inv = Inventory::default();
        grid(&mut inv, 0, "minecraft:oak_log", 3);
        let planks = recipe_id("minecraft:oak_planks", "minecraft:oak_log");
        let req = request(-1, vec![craft_recipe(planks, 1), StackAction::Take { count: 4, src: created(-1), dst: cursor(0) }]);
        let result = inv.handle(&req, false);
        assert!(result.ok, "{:?}", result.error);
        assert_eq!((inv.contents.cursor.item.id, inv.contents.cursor.item.count), (named("minecraft:oak_planks", 1).id, 4));
        assert_eq!(inv.contents.grid[0].item.count, 2, "un tronco consumato");
        assert!(inv.contents.created.item.is_empty());
        assert!(result.changed.contains(&(CRAFTING_INPUT_CONTAINER, 28)), "{:?}", result.changed);
        // la risposta nomina la casella della griglia con la quantità che resta
        let body = &inv.response(-1, &result)[0].body;
        assert!(body.windows(2).any(|w| w == [CRAFTING_INPUT_CONTAINER, 0]), "{body:02x?}");
    }

    #[test]
    fn table_recipes_need_the_table_and_crafting_table_fits_in_two_by_two() {
        let mut inv = Inventory::default();
        for i in 0..4 {
            grid(&mut inv, i, "minecraft:oak_planks", 2);
        }
        let table = recipe_id("minecraft:crafting_table", "minecraft:oak_planks");
        let req = request(-1, vec![craft_recipe(table, 1), StackAction::Take { count: 1, src: created(-1), dst: cursor(0) }]);
        assert!(inv.handle(&req, false).ok);
        assert_eq!(inv.contents.cursor.item.id, named("minecraft:crafting_table", 1).id);
        assert!(inv.contents.grid[..4].iter().all(|s| s.item.count == 1));
        // il piccone di legno è 3x3: senza banco si rifiuta, con il banco riesce
        let mut inv = Inventory::default();
        let pickaxe = recipe_id("minecraft:wooden_pickaxe", "minecraft:planks");
        for i in 0..3 {
            grid(&mut inv, i, "minecraft:oak_planks", 1);
        }
        grid(&mut inv, 4, "minecraft:stick", 1);
        grid(&mut inv, 7, "minecraft:stick", 1);
        let take = StackAction::Take { count: 1, src: created(-1), dst: cursor(0) };
        let result = inv.handle(&request(-1, vec![craft_recipe(pickaxe, 1), take.clone()]), false);
        assert!(!result.ok && result.error.unwrap().contains("2x2"));
        inv.open_table([0, 64, 0]);
        let result = inv.handle(&request(-3, vec![craft_recipe(pickaxe, 1), take]), false);
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(inv.contents.cursor.item.id, named("minecraft:wooden_pickaxe", 1).id);
        assert!(inv.contents.grid.iter().all(|s| s.item.is_empty()), "ingredienti consumati con esattezza");
    }

    #[test]
    fn recipe_with_missing_ingredients_is_refused_and_changes_nothing() {
        let mut inv = Inventory::default();
        grid(&mut inv, 0, "minecraft:oak_planks", 1);
        let before = inv.contents.clone();
        let sticks = recipe_id("minecraft:stick", "minecraft:planks");
        let req = request(-1, vec![craft_recipe(sticks, 1), StackAction::Take { count: 4, src: created(-1), dst: cursor(0) }]);
        let result = inv.handle(&req, false);
        assert!(!result.ok && result.error.unwrap().contains("assente"));
        assert_eq!(inv.contents.grid, before.grid);
        assert!(inv.contents.cursor.item.is_empty());
        // una ricetta sconosciuta e crafts = 0 si rifiutano
        assert!(!inv.handle(&request(-3, vec![craft_recipe(999_999, 1)]), false).ok);
        grid(&mut inv, 1, "minecraft:oak_planks", 1);
        assert!(!inv.handle(&request(-5, vec![craft_recipe(sticks, 0)]), false).ok);
        // due assi in una sola casella non bastano (servono due caselle)
        let mut inv = Inventory::default();
        grid(&mut inv, 0, "minecraft:oak_planks", 2);
        assert!(!inv.handle(&request(-7, vec![craft_recipe(sticks, 1)]), false).ok);
    }

    #[test]
    fn crafting_n_times_consumes_n_times_the_ingredients() {
        let mut inv = Inventory::default();
        grid(&mut inv, 0, "minecraft:oak_planks", 5);
        grid(&mut inv, 1, "minecraft:oak_planks", 5);
        let sticks = recipe_id("minecraft:stick", "minecraft:planks");
        let req = request(-1, vec![craft_recipe(sticks, 3), StackAction::Take { count: 12, src: created(-1), dst: cursor(0) }]);
        assert!(inv.handle(&req, false).ok);
        assert_eq!(inv.contents.cursor.item.count, 12, "4 bastoncini x 3");
        assert_eq!((inv.contents.grid[0].item.count, inv.contents.grid[1].item.count), (2, 2));
        // più ripetizioni di quante gli ingredienti permettano si rifiuta
        assert!(!inv.handle(&request(-3, vec![craft_recipe(sticks, 3)]), false).ok);
        assert_eq!(inv.contents.grid[0].item.count, 2);
    }

    #[test]
    fn auto_craft_takes_from_the_grid_then_the_inventory() {
        let mut inv = Inventory::default();
        give(&mut inv, 0, "minecraft:oak_planks", 5);
        give(&mut inv, 9, "minecraft:oak_planks", 5);
        grid(&mut inv, 0, "minecraft:oak_planks", 1);
        let sticks = recipe_id("minecraft:stick", "minecraft:planks");
        let auto = StackAction::CraftRecipeAuto { recipe_id: sticks, crafts: 5 };
        let result = inv.handle(&request(-1, vec![auto, StackAction::Take { count: 20, src: created(-1), dst: cursor(0) }]), false);
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(inv.contents.cursor.item.count, 20, "5 volte 4 bastoncini");
        // servono 10 assi: una dalla griglia, poi 5 dalla casella 0 e 4 dalla 9
        assert!(inv.contents.grid[0].item.is_empty() && inv.contents.main[0].item.is_empty());
        assert_eq!(inv.contents.main[9].item.count, 1);
        assert!(result.changed.contains(&(COMBINED_HOTBAR_AND_INVENTORY_CONTAINER, 9)));
        // non bastano più assi: rifiutata, e l'inventario resta com'era
        let auto = StackAction::CraftRecipeAuto { recipe_id: sticks, crafts: 2 };
        assert!(!inv.handle(&request(-3, vec![auto]), false).ok);
        assert_eq!(inv.contents.main[9].item.count, 1);
    }

    #[test]
    fn a_big_batch_is_split_into_stacks_created_one_by_one() {
        let mut inv = Inventory::default();
        give(&mut inv, 0, "minecraft:oak_planks", 64);
        give(&mut inv, 1, "minecraft:oak_planks", 64);
        let sticks = recipe_id("minecraft:stick", "minecraft:planks");
        // 17 volte 4 = 68 bastoncini: una pila da 64 e una da 4; la seconda si crea solo con `Create`
        let auto = |crafts| StackAction::CraftRecipeAuto { recipe_id: sticks, crafts };
        let first = request(-1, vec![auto(17), StackAction::Create { results_slot: 0 }, StackAction::Take { count: 64, src: created(-1), dst: cursor(0) }]);
        let result = inv.handle(&first, false);
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(inv.contents.cursor.item.count, 64);
        // lo stesso risultato non si crea due volte
        let twice = request(-3, vec![auto(17), StackAction::Create { results_slot: 0 }, StackAction::Create { results_slot: 0 }]);
        assert!(!inv.handle(&twice, false).ok);
        let second = request(-5, vec![auto(17), StackAction::Create { results_slot: 1 }, StackAction::Place { count: 4, src: created(-1), dst: hotbar(8, 0) }]);
        assert!(inv.handle(&second, false).ok);
        assert_eq!(inv.contents.main[8].item.count, 4);
    }

    #[test]
    fn closing_the_table_gives_back_grid_and_cursor() {
        let mut inv = Inventory::default();
        inv.open_table([0, 64, 0]);
        grid(&mut inv, 5, "minecraft:stone", 7);
        inv.contents.cursor = Slot { item: named("minecraft:dirt", 2), stack_id: 5 };
        let items = inv.release_ui();
        assert_eq!(items.iter().map(|i| i.count).collect::<Vec<_>>(), [7, 2]);
        assert!(inv.table().is_none() && inv.contents.grid.iter().all(|s| s.item.is_empty()) && inv.contents.cursor.item.is_empty());
    }

    #[test]
    fn grid_items_go_through_place_with_stack_ids() {
        let mut inv = Inventory::default();
        let window = inv.open_table([0, 64, 0]);
        assert_eq!(inv.table_window(), Some(window));
        let id = give(&mut inv, 0, "minecraft:oak_planks", 8);
        let req = request(-1, vec![StackAction::Place { count: 3, src: hotbar(0, id), dst: at(CRAFTING_INPUT_CONTAINER, 34, 0) }]);
        assert!(inv.handle(&req, false).ok);
        assert_eq!(inv.contents.grid[2].item.count, 3);
        // la casella 2x2 non esiste col banco aperto
        let bad = request(-3, vec![StackAction::Place { count: 1, src: hotbar(0, -1), dst: at(CRAFTING_INPUT_CONTAINER, 28, 0) }]);
        assert!(!inv.handle(&bad, false).ok);
    }

    #[test]
    fn place_cannot_stack_unstackable_items() {
        let mut inv = Inventory::default();
        let id0 = give(&mut inv, 0, "minecraft:diamond_sword", 1);
        let id1 = give(&mut inv, 1, "minecraft:diamond_sword", 1);
        let before = inv.contents.main.clone();
        let req = request(-1, vec![StackAction::Place { count: 1, src: hotbar(0, id0), dst: hotbar(1, id1) }]);
        assert!(!inv.handle(&req, true).ok);
        assert_eq!(inv.contents.main, before);
        // le uova arrivano a 16 e non oltre
        let a = give(&mut inv, 2, "minecraft:egg", 10);
        let b = give(&mut inv, 3, "minecraft:egg", 10);
        let req = request(-2, vec![StackAction::Place { count: 10, src: hotbar(2, a), dst: hotbar(3, b) }]);
        assert!(!inv.handle(&req, true).ok);
    }
}
