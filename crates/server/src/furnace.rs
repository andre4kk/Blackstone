//! Fornace: block entity `Furnace` (NBT come vanilla e Dragonfly `block/furnace.go`: `Items`, `BurnTime`, `CookTime`, `BurnDuration`,
//! `StoredXPInt`), apertura con `ContainerOpen` (tipo fornace), cottura nel ciclo a 20 Hz (`smelter.go`, `tickSmelting`, qui in tick invece
//! che in millisecondi) e blocco acceso/spento (`lit_furnace`).
//!
//! Come per i forzieri il contenuto vero sta nel block entity del chunk; chi la apre ne tiene una copia nelle caselle della finestra
//! (`Inventory::open_furnace`, `Loc::Chest(0..3)`) e `sync_furnace` riscrive il block entity dopo le sue richieste di pila. La cottura vive
//! solo nei chunk in memoria (`furnaces_tick` li percorre): un chunk scaricato è fermo, come in Dragonfly.
use crate::{
    breaking::in_reach,
    events::BlockChange,
    inventory::{Item, CONTAINER_TYPE_FURNACE},
    nbt::{write_root, Enc, Reader, Tag},
    playerdata::{int, stack_from_tag, stack_to_tag},
    registry::{blocks, StateValue},
    smelting::{tables, COOK_TICKS},
    GameMode, Server,
};
use bedrock_net::log;
use bedrock_protocol::{
    container_set_data::{ContainerSetData, FURNACE_LIT_DURATION, FURNACE_LIT_TIME, FURNACE_TICK_COUNT},
    generated::{BlockPos, ContainerOpen},
    wire::ServerPacket,
    Packet,
};
use vgen::{BlockEntity, Dim};

/// Caselle della fornace: ingresso, combustibile, risultato.
pub(crate) const FURNACE_SLOTS: usize = 3;
/// Spinta degli oggetti che cadono da una fornace rotta (come quelli di un blocco rotto).
const DROP_SPREAD: f32 = 0.1;
const DROP_LIFT: f32 = 0.2;
const DROP_PICKUP_DELAY: u64 = 10;
/// Quanto scende la cottura a fornace spenta ogni tick (Dragonfly: 100 ms).
const COOLDOWN_TICKS: i32 = 2;

fn name_of(hash: i32) -> Option<&'static str> {
    blocks().state(hash).map(|s| s.name.as_str())
}

/// Vero se lo stato di blocco è una fornace (accesa o spenta).
pub(crate) fn is_furnace(hash: i32) -> bool {
    matches!(name_of(hash), Some("minecraft:furnace" | "minecraft:lit_furnace"))
}

/// Vero se `old` e `new` sono la stessa fornace che si accende o si spegne: il block entity resta.
pub(crate) fn is_lit_swap(old: i32, new: i32) -> bool {
    is_furnace(old) && is_furnace(new)
}

/// Lo stato della fornace `hash` acceso o spento (stessa direzione).
fn with_lit(hash: i32, lit: bool) -> Option<i32> {
    let state = blocks().state(hash)?;
    let facing = state.get("minecraft:cardinal_direction").cloned().unwrap_or(StateValue::Str("south".into()));
    let target = blocks().default_hash(if lit { "minecraft:lit_furnace" } else { "minecraft:furnace" })?;
    Some(blocks().state(target)?.with("minecraft:cardinal_direction", facing).hash())
}

/// Il contenuto e i tempi di un block entity di fornace. I tempi sono in tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FurnaceData {
    /// Sempre `FURNACE_SLOTS` oggetti: ingresso, combustibile, risultato.
    pub items: Vec<Item>,
    /// Tick di combustibile rimasti (`BurnTime`).
    pub burn_time: i32,
    /// Durata del combustibile in uso (`BurnDuration`).
    pub burn_duration: i32,
    /// Tick di cottura dell'oggetto in ingresso (`CookTime`).
    pub cook_time: i32,
    /// Esperienza raccolta dalle cotture (`StoredXPInt`).
    pub xp: i32,
}

impl FurnaceData {
    pub fn empty() -> FurnaceData {
        FurnaceData { items: vec![Item::default(); FURNACE_SLOTS], ..FurnaceData::default() }
    }

    /// Legge il block entity; un NBT illeggibile dà una fornace vuota.
    pub fn parse(nbt: &[u8]) -> FurnaceData {
        let mut data = FurnaceData::empty();
        let Some((_, tag)) = Reader::new(nbt, Enc::Network).root() else { return data };
        if let Some(Tag::List(_, stacks)) = tag.get("Items") {
            for stack in stacks {
                let slot = stack.get("Slot").and_then(int).unwrap_or(-1);
                if let (Some(cell), Some(item)) = (usize::try_from(slot).ok().and_then(|s| data.items.get_mut(s)), stack_from_tag(stack)) {
                    *cell = item;
                }
            }
        }
        let read = |key: &str| tag.get(key).and_then(int).unwrap_or(0).clamp(0, i16::MAX as i64) as i32;
        data.burn_time = read("BurnTime");
        data.burn_duration = read("BurnDuration");
        data.cook_time = read("CookTime");
        data.xp = read("StoredXPInt");
        data
    }

    /// Il block entity per la cella `pos` (formato di Dragonfly `Furnace.EncodeNBT`: i tempi sono `short`).
    pub fn entity(&self, pos: [i32; 3]) -> BlockEntity {
        let stacks = self.items.iter().enumerate().filter_map(|(i, item)| stack_to_tag(item, Some(i as u8))).collect();
        let short = |v: i32| Tag::Short(v.clamp(0, i16::MAX as i32) as i16);
        let fields = vec![
            ("id".to_string(), Tag::String("Furnace".into())),
            ("BurnTime".into(), short(self.burn_time)),
            ("CookTime".into(), short(self.cook_time)),
            ("BurnDuration".into(), short(self.burn_duration)),
            ("StoredXPInt".into(), short(self.xp)),
            ("Items".into(), Tag::List(10, stacks)),
            ("x".into(), Tag::Int(pos[0])),
            ("y".into(), Tag::Int(pos[1])),
            ("z".into(), Tag::Int(pos[2])),
        ];
        BlockEntity { x: pos[0], y: pos[1], z: pos[2], nbt: write_root("", &Tag::Compound(fields), Enc::Network) }
    }

    /// Un tick di cottura (Dragonfly `smelter.tickSmelting`, in tick invece che in millisecondi). `lit` è se il blocco è acceso;
    /// restituisce se deve esserlo adesso. `roll` dà un numero in [0, 1) per l'esperienza frazionaria.
    pub fn tick(&mut self, mut lit: bool, mut roll: impl FnMut() -> f32) -> bool {
        let (input, fuel, product) = (self.items[0].clone(), self.items[1].clone(), self.items[2].clone());
        let smelt = if input.is_empty() { None } else { tables().smelt(&input) };
        let fuel_info = if fuel.is_empty() { None } else { tables().fuel(&fuel) };
        // si può cuocere se c'è un ingresso cuocibile e il risultato ha posto (vuoto, o lo stesso oggetto non pieno)
        let can_smelt =
            smelt.is_some_and(|s| product.is_empty() || (product.same_kind(&s.output) && product.nbt.is_none() && product.count < product.max_stack()));
        if self.burn_time <= 0 && can_smelt {
            if let Some(f) = fuel_info {
                self.burn_time = f.ticks;
                self.burn_duration = f.ticks;
                lit = true;
                self.items[1] = match &f.residue {
                    Some(residue) => residue.clone(),
                    None => decreased(&fuel),
                };
            }
        }
        if self.burn_time > 0 {
            self.burn_time -= 1;
            if let (true, Some(s)) = (can_smelt, smelt) {
                self.cook_time += 1;
                if self.cook_time >= COOK_TICKS {
                    self.items[0] = decreased(&input);
                    self.items[2] = if product.is_empty() { s.output.clone() } else { product.with_count(product.count + s.output.count) };
                    // esperienza: la parte intera sempre, la frazione come probabilità (Dragonfly)
                    let xp = s.xp * f64::from(s.output.count);
                    let earned = s.xp.floor();
                    let chance = xp - earned;
                    self.xp += earned as i32 + i32::from(chance > 0.0 && f64::from(roll()) < chance);
                    self.cook_time -= COOK_TICKS;
                }
            } else if self.burn_time == 0 {
                self.burn_duration = 0;
            } else {
                self.cook_time = 0;
            }
            if self.burn_time > 0 {
                lit = true; // anche una fornace caricata da disco con combustibile residuo si accende
            }
        } else {
            self.burn_duration = 0;
            lit = false;
        }
        if self.cook_time > 0 && !lit {
            self.cook_time = (self.cook_time - COOLDOWN_TICKS).max(0);
        }
        lit
    }
}

/// La pila con un oggetto in meno (vuota se era l'ultimo).
fn decreased(item: &Item) -> Item {
    if item.count <= 1 {
        Item::default()
    } else {
        item.with_count(item.count - 1)
    }
}

impl Server {
    /// Il contenuto salvato nel block entity della cella (vuoto se non c'è).
    pub(crate) fn furnace_data(&self, pos: [i32; 3]) -> FurnaceData {
        let chunk = self.chunks.get(&(Dim::Overworld as u8, pos[0].div_euclid(16), pos[2].div_euclid(16)));
        chunk.and_then(|c| c.block_entities.iter().find(|b| [b.x, b.y, b.z] == pos)).map_or_else(FurnaceData::empty, |b| FurnaceData::parse(&b.nbt))
    }

    pub(crate) fn save_furnace(&mut self, pos: [i32; 3], data: &FurnaceData, notify: bool) {
        self.store_block_entity(Dim::Overworld, data.entity(pos), notify);
    }

    /// Una fornace appena piazzata in `pos`: crea il suo block entity vuoto.
    pub(crate) fn furnace_placed(&mut self, pos: [i32; 3]) {
        self.save_furnace(pos, &FurnaceData::empty(), true);
    }

    /// Chi ha aperto la fornace in `pos`.
    fn furnace_viewers(&self, pos: [i32; 3]) -> Vec<u64> {
        self.players
            .iter()
            .filter(|(_, p)| p.inventory.chest().is_some_and(|c| c.kind == crate::inventory::WindowKind::Furnace && c.parts.contains(&pos)))
            .map(|(&pid, _)| pid)
            .collect()
    }

    /// `ContainerSetData` per i tre valori delle barre (cottura, fiamma, durata) della finestra `window`.
    fn furnace_progress(window: u8, data: &FurnaceData) -> [Packet; 3] {
        let set = |key, value| ContainerSetData { window_id: window, key, value }.packet();
        [set(FURNACE_TICK_COUNT, data.cook_time), set(FURNACE_LIT_TIME, data.burn_time), set(FURNACE_LIT_DURATION, data.burn_duration)]
    }

    /// Apre la fornace in `pos` a `pid`: chiude ciò che aveva aperto, manda `ContainerOpen` (tipo fornace), le tre caselle e le barre.
    pub(crate) fn open_furnace(&mut self, pid: u64, pos: [i32; 3]) {
        let Some(p) = self.players.get(&pid) else { return };
        if p.inventory.chest().is_some_and(|c| c.parts.contains(&pos)) {
            return;
        }
        self.close_window(pid);
        let data = self.furnace_data(pos);
        let Some(p) = self.players.get_mut(&pid) else { return };
        let window = p.inventory.open_furnace(pos, data.items.clone());
        log!("[#{pid}] apre la fornace {pos:?} (finestra {window})");
        let position = BlockPos { x: pos[0], y: pos[1], z: pos[2] };
        let mut packets = vec![ContainerOpen { container_id: window, container_type: CONTAINER_TYPE_FURNACE, position, target_actor_id: -1 }.packet()];
        packets.extend(p.inventory.chest_content_packet());
        // Dragonfly manda le barre solo quando cambiano: chi apre una fornace già accesa non vedrebbe la fiamma, quindi si mandano subito
        packets.extend(Self::furnace_progress(window, &data));
        self.send(pid, &packets);
    }

    /// Il giocatore preme sulla fornace in `pos`: la apre se può (modalità, portata); altrimenti il clic è comunque consumato.
    pub(crate) fn use_furnace(&mut self, pid: u64, pos: [i32; 3], mode: GameMode, eyes: [f32; 3]) {
        if mode == GameMode::Spectator || !in_reach(eyes, pos, mode == GameMode::Creative) {
            return log!("[#{pid}] apertura della fornace {pos:?} rifiutata: modalità o portata");
        }
        self.open_furnace(pid, pos);
    }

    /// Dopo una richiesta di pila di `pid` con una fornace aperta: riscrive il block entity (tenendo i tempi) e allinea le caselle degli
    /// altri che la hanno aperta.
    pub(crate) fn sync_furnace(&mut self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let Some(window) = p.inventory.chest().cloned() else { return };
        let items = p.inventory.chest_items();
        let pos = window.parts[0];
        let mut data = self.furnace_data(pos);
        if data.items != items {
            data.items = items;
            self.save_furnace(pos, &data, false);
        }
        self.refresh_furnace_viewers(pos, &data, Some(pid));
    }

    /// Manda le caselle cambiate a chi guarda la fornace (tranne `skip`).
    fn refresh_furnace_viewers(&mut self, pos: [i32; 3], data: &FurnaceData, skip: Option<u64>) {
        for viewer in self.furnace_viewers(pos).into_iter().filter(|&v| Some(v) != skip) {
            let Some(o) = self.players.get_mut(&viewer) else { continue };
            let packets: Vec<Packet> = data.items.iter().enumerate().filter_map(|(i, item)| o.inventory.refresh_chest_slot(i, item.clone())).collect();
            if !packets.is_empty() {
                self.send(viewer, &packets);
            }
        }
    }

    /// Una fornace in `pos` non c'è più (il suo block entity è già tolto): il contenuto cade a terra.
    pub(crate) fn furnace_removed(&mut self, pos: [i32; 3], nbt: &[u8]) {
        let data = FurnaceData::parse(nbt);
        let center = [pos[0] as f32 + 0.5, pos[1] as f32 + 0.5, pos[2] as f32 + 0.5];
        for item in data.items.into_iter().filter(|i| !i.is_empty()) {
            let velocity = [(self.roll() * 2.0 - 1.0) * DROP_SPREAD, DROP_LIFT, (self.roll() * 2.0 - 1.0) * DROP_SPREAD];
            self.spawn_item(item, center, velocity, DROP_PICKUP_DELAY);
        }
    }

    /// Un tick per ogni fornace nei chunk in memoria: cottura, barre per chi l'ha aperta, blocco acceso o spento.
    pub(crate) fn furnaces_tick(&mut self) {
        let cells: Vec<([i32; 3], i32, bool)> = self
            .chunks
            .iter()
            .filter(|(key, _)| key.0 == Dim::Overworld as u8)
            .flat_map(|(_, c)| c.block_entities.iter().map(|b| [b.x, b.y, b.z]))
            .filter_map(|pos| self.block_at(Dim::Overworld, pos).filter(|&(hash, _)| is_furnace(hash)).map(|(hash, water)| (pos, hash, water)))
            .collect();
        for (pos, hash, water) in cells {
            let before = self.furnace_data(pos);
            let mut data = before.clone();
            let lit_now = name_of(hash) == Some("minecraft:lit_furnace");
            let lit = data.tick(lit_now, || self.roll());
            if data != before {
                self.save_furnace(pos, &data, false);
                self.refresh_furnace_viewers(pos, &data, None);
                for viewer in self.furnace_viewers(pos) {
                    let Some(window) = self.players.get(&viewer).and_then(|p| p.inventory.chest()).map(|c| c.window) else { continue };
                    let [cook, burn, duration] = Self::furnace_progress(window, &data);
                    let changed: Vec<Packet> = [
                        (before.cook_time != data.cook_time, cook),
                        (before.burn_time != data.burn_time, burn),
                        (before.burn_duration != data.burn_duration, duration),
                    ]
                    .into_iter()
                    .filter_map(|(differs, packet)| differs.then_some(packet))
                    .collect();
                    if !changed.is_empty() {
                        self.send(viewer, &changed);
                    }
                }
            }
            if lit != lit_now {
                if let Some(new_hash) = with_lit(hash, lit) {
                    self.set_blocks(Dim::Overworld, &[BlockChange { pos, hash: new_hash, water }]);
                }
            }
        }
    }
}
