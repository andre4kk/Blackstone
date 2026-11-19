//! Forzieri: block entity `Chest` (NBT come vanilla e Dragonfly `block/chest.go`: `Items`, `CustomName`, `pairx`/`pairz`), apertura con
//! `ContainerOpen`, animazione con `BlockEvent`, forziere doppio (due forzieri vicini con lo stesso orientamento), più giocatori
//! sullo stesso forziere, e la chiusura delle finestre quando il blocco sparisce.
//!
//! Il contenuto vero sta nel block entity del chunk. Chi apre un forziere ne tiene una copia nelle caselle della sua finestra
//! (`Inventory::open_chest`): le richieste di pila lavorano su quella copia, poi `sync_chest` riscrive i block entity e allinea
//! le copie degli altri giocatori che lo hanno aperto. Le caselle si nominano con il contenitore `LevelEntity`, come Dragonfly.
use crate::{
    breaking::in_reach,
    inventory::Item,
    nbt::{write_root, Enc, Reader, Tag},
    placement::{neighbors, rules::Dir},
    playerdata::{int, stack_from_tag, stack_to_tag},
    registry::{blocks, StateValue},
    GameMode, Server,
};
use bedrock_net::log;
use bedrock_protocol::{
    block_event::{BlockEvent, CHANGE_CHEST_STATE},
    generated::{BlockPos, ContainerClose, ContainerOpen},
    wire::ServerPacket,
    Packet,
};
use vgen::{BlockEntity, Dim};

/// Caselle di un forziere (due blocchi in un forziere doppio).
pub(crate) const CHEST_SLOTS: usize = 27;
/// Tipo di contenitore del forziere in `ContainerOpen` (`ContainerTypeContainer` di gophertunnel).
pub(crate) const CONTAINER_TYPE_CONTAINER: u8 = 0;
/// Spinta degli oggetti che cadono da un forziere rotto (come quelli di un blocco rotto).
const DROP_SPREAD: f32 = 0.1;
const DROP_LIFT: f32 = 0.2;
const DROP_PICKUP_DELAY: u64 = 10;
/// Faccia inferiore, per chiedere se un blocco sopra il forziere lo copre.
const DOWN: u8 = 0;

/// Vero se lo stato di blocco è un forziere (non quello intrappolato né quello di Ender).
pub(crate) fn is_chest(hash: i32) -> bool {
    blocks().state(hash).is_some_and(|s| s.name == "minecraft:chest")
}

/// L'orientamento di un forziere.
fn facing_of(hash: i32) -> Option<Dir> {
    match blocks().state(hash)?.get("minecraft:cardinal_direction") {
        Some(StateValue::Str(name)) => Dir::from_name(name),
        _ => None,
    }
}

/// Il contenuto di un block entity di forziere.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ChestData {
    /// Sempre `CHEST_SLOTS` oggetti (caselle vuote comprese).
    pub items: Vec<Item>,
    pub name: Option<String>,
    /// X e Z del forziere con cui è abbinato (`pairx`/`pairz`).
    pub pair: Option<[i32; 2]>,
}

impl ChestData {
    pub fn empty() -> ChestData {
        ChestData { items: vec![Item::default(); CHEST_SLOTS], name: None, pair: None }
    }

    /// Legge il block entity; un NBT illeggibile dà un forziere vuoto.
    pub fn parse(nbt: &[u8]) -> ChestData {
        let mut data = ChestData::empty();
        let Some((_, tag)) = Reader::new(nbt, Enc::Network).root() else { return data };
        if let Some(Tag::List(_, stacks)) = tag.get("Items") {
            for stack in stacks {
                let slot = stack.get("Slot").and_then(int).unwrap_or(-1);
                if let (Some(cell), Some(item)) = (usize::try_from(slot).ok().and_then(|s| data.items.get_mut(s)), stack_from_tag(stack)) {
                    *cell = item;
                }
            }
        }
        data.name = tag.get("CustomName").and_then(Tag::as_str).filter(|n| !n.is_empty()).map(str::to_string);
        if let (Some(x), Some(z)) = (tag.get("pairx").and_then(int), tag.get("pairz").and_then(int)) {
            data.pair = Some([x as i32, z as i32]);
        }
        data
    }

    /// Il block entity per la cella `pos` (formato di Dragonfly `Chest.EncodeNBT`).
    pub fn entity(&self, pos: [i32; 3]) -> BlockEntity {
        let stacks = self.items.iter().enumerate().filter_map(|(i, item)| stack_to_tag(item, Some(i as u8))).collect();
        let mut fields = vec![
            ("id".to_string(), Tag::String("Chest".into())),
            ("Items".into(), Tag::List(10, stacks)),
            ("x".into(), Tag::Int(pos[0])),
            ("y".into(), Tag::Int(pos[1])),
            ("z".into(), Tag::Int(pos[2])),
        ];
        if let Some(name) = &self.name {
            fields.push(("CustomName".into(), Tag::String(name.clone())));
        }
        if let Some([x, z]) = self.pair {
            fields.push(("pairx".into(), Tag::Int(x)));
            fields.push(("pairz".into(), Tag::Int(z)));
        }
        BlockEntity { x: pos[0], y: pos[1], z: pos[2], nbt: write_root("", &Tag::Compound(fields), Enc::Network) }
    }
}

impl Server {
    /// Lo stato di blocco della cella se è un forziere.
    fn chest_hash_at(&self, pos: [i32; 3]) -> Option<i32> {
        self.block_at(Dim::Overworld, pos).map(|(hash, _)| hash).filter(|&h| is_chest(h))
    }

    /// Il contenuto salvato nel block entity della cella (vuoto se non c'è).
    pub(crate) fn chest_data(&self, pos: [i32; 3]) -> ChestData {
        let chunk = self.chunks.get(&(Dim::Overworld as u8, pos[0].div_euclid(16), pos[2].div_euclid(16)));
        chunk.and_then(|c| c.block_entities.iter().find(|b| [b.x, b.y, b.z] == pos)).map_or_else(ChestData::empty, |b| ChestData::parse(&b.nbt))
    }

    #[cfg(test)]
    pub(crate) fn save_chest_for_test(&mut self, pos: [i32; 3], data: &ChestData) {
        self.save_chest(pos, data, false);
    }

    fn save_chest(&mut self, pos: [i32; 3], data: &ChestData, notify: bool) {
        self.store_block_entity(Dim::Overworld, data.entity(pos), notify);
    }

    /// Un forziere appena piazzato in `pos`: crea il suo block entity e, se accanto (a sinistra o a destra) ce n'è uno con lo stesso
    /// orientamento non ancora abbinato, li abbina (Dragonfly `Chest.UseOnBlock`).
    pub(crate) fn chest_placed(&mut self, pos: [i32; 3]) {
        let Some(hash) = self.chest_hash_at(pos) else { return };
        let mut mine = ChestData::empty();
        if let Some(facing) = facing_of(hash) {
            for dir in [facing.rotate_left(), facing.rotate_right()] {
                let other = dir.step(pos);
                let Some(other_hash) = self.chest_hash_at(other) else { continue };
                let mut theirs = self.chest_data(other);
                let free = theirs.pair.is_none_or(|p| p == [pos[0], pos[2]]);
                if facing_of(other_hash) == Some(facing) && free {
                    mine.pair = Some([other[0], other[2]]);
                    theirs.pair = Some([pos[0], pos[2]]);
                    self.save_chest(other, &theirs, true);
                    break;
                }
            }
        }
        self.save_chest(pos, &mine, true);
    }

    /// I blocchi che compongono il forziere in `pos`, nell'ordine delle caselle della finestra: uno, o due se è doppio (il primo ha le
    /// prime 27 caselle: il forziere dal lato destro di chi guarda la parte anteriore, come Dragonfly `Chest.pair`). Un abbinamento
    /// che non regge più (l'altro forziere è sparito o ruotato) si scioglie.
    fn chest_parts(&mut self, pos: [i32; 3]) -> Vec<[i32; 3]> {
        let Some(hash) = self.chest_hash_at(pos) else { return vec![pos] };
        let mut mine = self.chest_data(pos);
        let Some([px, pz]) = mine.pair else { return vec![pos] };
        let other = [px, pos[1], pz];
        let facing = facing_of(hash);
        let beside = facing.is_some_and(|f| other == f.rotate_left().step(pos) || other == f.rotate_right().step(pos));
        let mut theirs = self.chest_data(other);
        let valid = beside && self.chest_hash_at(other).and_then(facing_of) == facing && theirs.pair.is_none_or(|p| p == [pos[0], pos[2]]);
        if !valid {
            mine.pair = None;
            self.save_chest(pos, &mine, true);
            return vec![pos];
        }
        if theirs.pair.is_none() {
            theirs.pair = Some([pos[0], pos[2]]);
            self.save_chest(other, &theirs, true);
        }
        match facing {
            Some(f) if other == f.rotate_right().step(pos) => vec![other, pos],
            _ => vec![pos, other],
        }
    }

    /// Chi ha aperto un forziere che include `pos`.
    fn chest_viewers(&self, pos: [i32; 3]) -> Vec<u64> {
        self.players.iter().filter(|(_, p)| p.inventory.chest().is_some_and(|c| c.parts.contains(&pos))).map(|(&pid, _)| pid).collect()
    }

    /// L'animazione di apertura o chiusura (e il suono) su ogni blocco del forziere, per chi vede il chunk (Dragonfly `Chest.open/close`).
    fn chest_animation(&self, parts: &[[i32; 3]], open: bool) {
        for &part in parts {
            let packet: Packet =
                BlockEvent { position: BlockPos { x: part[0], y: part[1], z: part[2] }, event_type: CHANGE_CHEST_STATE, event_data: i32::from(open) }.packet();
            let chunk = (part[0].div_euclid(16), part[2].div_euclid(16));
            for (&viewer, _) in self.players.iter().filter(|(_, p)| p.sent.contains(&chunk)) {
                self.send(viewer, std::slice::from_ref(&packet));
            }
        }
        if let Some(&first) = parts.first() {
            let hash = self.block_at(Dim::Overworld, first).map_or(0, |b| b.0);
            self.play_sound(if open { "chest.open" } else { "chest.closed" }, first, hash);
        }
    }

    /// Apre il forziere in `pos` a `pid`: chiude ciò che aveva aperto, manda `ContainerOpen` (tipo container, finestra nuova) e il contenuto.
    /// Il primo che lo apre ne fa partire l'animazione. Se è già aperto lì non rimanda nulla.
    pub(crate) fn open_chest(&mut self, pid: u64, pos: [i32; 3]) {
        let Some(p) = self.players.get(&pid) else { return };
        if p.inventory.chest().is_some_and(|c| c.parts.contains(&pos)) {
            return;
        }
        self.close_window(pid);
        let parts = self.chest_parts(pos);
        let items: Vec<Item> = parts.iter().flat_map(|&part| self.chest_data(part).items).collect();
        let first = parts.iter().all(|&part| self.chest_viewers(part).is_empty());
        if first {
            self.chest_animation(&parts, true);
        }
        let Some(p) = self.players.get_mut(&pid) else { return };
        let window = p.inventory.open_chest(parts.clone(), items);
        log!("[#{pid}] apre il forziere {pos:?} ({} blocchi, finestra {window})", parts.len());
        let position = BlockPos { x: pos[0], y: pos[1], z: pos[2] };
        let mut packets = vec![ContainerOpen { container_id: window, container_type: CONTAINER_TYPE_CONTAINER, position, target_actor_id: -1 }.packet()];
        packets.extend(p.inventory.chest_content_packet());
        self.send(pid, &packets);
    }

    /// Toglie a `pid` la finestra del forziere (senza avvisare il client) e, se era l'ultimo a guardarlo, ne chiude l'animazione.
    pub(crate) fn close_chest_view(&mut self, pid: u64) -> Option<u8> {
        let window = self.players.get_mut(&pid)?.inventory.close_chest()?;
        if window.parts.iter().all(|&part| self.chest_viewers(part).is_empty()) {
            self.chest_animation(&window.parts, false);
        }
        Some(window.window)
    }

    /// Chiude per iniziativa del server ciò che `pid` ha aperto (banco o forziere): griglia e cursore tornano nell'inventario e il client
    /// riceve `ContainerClose` con `server_initiated_close` (come Dragonfly `closeWindow(false)`). Non fa nulla se non c'è nulla di aperto.
    pub(crate) fn close_window(&mut self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let table = p.inventory.table_window();
        let had_chest = p.inventory.chest().is_some();
        let chest = self.close_chest_view(pid);
        if table.is_some() || had_chest {
            self.return_ui_items(pid);
        }
        let mut packets = Vec::new();
        if let Some(window) = table {
            packets.push(
                ContainerClose { container_id: window, container_type: crate::inventory::CONTAINER_TYPE_WORKBENCH, server_initiated_close: true }.packet(),
            );
        }
        if let Some(window) = chest {
            packets.push(ContainerClose { container_id: window, container_type: CONTAINER_TYPE_CONTAINER, server_initiated_close: true }.packet());
        }
        if !packets.is_empty() {
            self.send(pid, &packets);
        }
    }

    /// Il blocco in `pos` ha cambiato nome (rotto o sostituito): chi aveva lì un banco o un forziere aperto viene chiuso.
    pub(crate) fn close_windows_at(&mut self, pos: [i32; 3]) {
        let viewers: Vec<u64> = self
            .players
            .iter()
            .filter(|(_, p)| p.inventory.table() == Some(pos) || p.inventory.chest().is_some_and(|c| c.parts.contains(&pos)))
            .map(|(&pid, _)| pid)
            .collect();
        for pid in viewers {
            log!("[#{pid}] il blocco {pos:?} è sparito: finestra chiusa");
            self.close_window(pid);
        }
    }

    /// Un forziere in `pos` non c'è più (il suo block entity è già tolto): il contenuto cade a terra e l'altra metà, se era doppio, torna singola.
    pub(crate) fn chest_removed(&mut self, pos: [i32; 3], nbt: &[u8]) {
        let data = ChestData::parse(nbt);
        let center = [pos[0] as f32 + 0.5, pos[1] as f32 + 0.5, pos[2] as f32 + 0.5];
        for item in data.items.into_iter().filter(|i| !i.is_empty()) {
            let velocity = [(self.roll() * 2.0 - 1.0) * DROP_SPREAD, DROP_LIFT, (self.roll() * 2.0 - 1.0) * DROP_SPREAD];
            self.spawn_item(item, center, velocity, DROP_PICKUP_DELAY);
        }
        if let Some([px, pz]) = data.pair {
            let other = [px, pos[1], pz];
            let mut theirs = self.chest_data(other);
            if self.chest_hash_at(other).is_some() && theirs.pair == Some([pos[0], pos[2]]) {
                theirs.pair = None;
                self.save_chest(other, &theirs, true);
            }
        }
    }

    /// Dopo una richiesta di pila di `pid`: se ha un forziere aperto ne riscrive i block entity e manda le caselle cambiate agli altri
    /// che lo hanno aperto.
    pub(crate) fn sync_chest(&mut self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let Some(window) = p.inventory.chest().cloned() else { return };
        let items = p.inventory.chest_items();
        for (k, &part) in window.parts.iter().enumerate() {
            let mut data = self.chest_data(part);
            let mine = &items[k * CHEST_SLOTS..(k + 1) * CHEST_SLOTS];
            if data.items != mine {
                data.items = mine.to_vec();
                self.save_chest(part, &data, false);
            }
        }
        let others: Vec<u64> = self
            .players
            .iter()
            .filter(|(&other, o)| other != pid && o.inventory.chest().is_some_and(|c| c.parts.iter().any(|part| window.parts.contains(part))))
            .map(|(&other, _)| other)
            .collect();
        for other in others {
            let Some(parts) = self.players.get(&other).and_then(|o| o.inventory.chest()).map(|c| c.parts.clone()) else { continue };
            let current: Vec<Vec<Item>> = parts.iter().map(|&part| self.chest_data(part).items).collect();
            let Some(o) = self.players.get_mut(&other) else { continue };
            let packets: Vec<Packet> = current
                .into_iter()
                .enumerate()
                .flat_map(|(k, items)| items.into_iter().enumerate().map(move |(i, item)| (k * CHEST_SLOTS + i, item)))
                .filter_map(|(i, item)| o.inventory.refresh_chest_slot(i, item))
                .collect();
            if !packets.is_empty() {
                self.send(other, &packets);
            }
        }
    }

    /// Il giocatore preme sul forziere in `pos`: lo apre se può (modalità, portata, niente di solido sopra); altrimenti il clic è comunque consumato.
    pub(crate) fn use_chest(&mut self, pid: u64, pos: [i32; 3], mode: GameMode, eyes: [f32; 3]) {
        if mode == GameMode::Spectator || !in_reach(eyes, pos, mode == GameMode::Creative) {
            return log!("[#{pid}] apertura del forziere {pos:?} rifiutata: modalità o portata");
        }
        let parts = self.chest_parts(pos);
        let covered = |at: &[i32; 3]| {
            let above = [at[0], at[1] + 1, at[2]];
            self.block_at(Dim::Overworld, above).is_some_and(|(hash, _)| neighbors::solid_face(hash, DOWN))
        };
        if parts.iter().any(covered) {
            return log!("[#{pid}] forziere {pos:?} coperto: non si apre");
        }
        self.open_chest(pid, pos);
    }
}
