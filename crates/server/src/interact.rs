//! Interazioni col tasto destro (D§5 delle note su Dragonfly): porte, botole, cancelletti, leve e pulsanti. Se il blocco cliccato
//! è interattivo e il giocatore non è accovacciato (o ha la mano vuota) il clic lo attiva invece di piazzare; altrimenti si piazza.
//! Le porte di ferro e le botole di ferro non si aprono a mano (regola vanilla): per loro `activate` risponde `None` e il clic
//! piazza come su un blocco qualunque.
//!
//! Suoni (nomi dell'enum `LevelSoundEvent` di Mojang, come li manda gophertunnel): `door.open`/`door.close`,
//! `trapdoor.open`/`trapdoor.close`, `fence_gate.open`/`fence_gate.close`, `power.on`/`power.off` (leva, come Dragonfly) e
//! `button.click_on`/`button.click_off` (Dragonfly non ha pulsanti). Il dato del suono è l'hash del nuovo stato del blocco.
use crate::{
    breaking::in_reach,
    events::{BlockChange, WorldAction},
    inventory::CONTAINER_TYPE_WORKBENCH,
    placement::{rules::Dir, BlockView, PlaceCtx, WorldView},
    registry::{blocks, BlockState, StateValue},
    GameMode, Server,
};
use bedrock_net::log;
use bedrock_protocol::{
    generated::{BlockPos, ContainerOpen, LevelSoundEvent, Vec3},
    input::UseItem,
    wire::ServerPacket,
};
use vgen::Dim;

/// Dopo quanti tick un pulsante di legno torna su (e uno di pietra: `STONE_BUTTON_TICKS`). Valori vanilla, non di Dragonfly.
const WOODEN_BUTTON_TICKS: u64 = 30;
const STONE_BUTTON_TICKS: u64 = 20;

/// Una modifica di blocco programmata per un tick futuro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scheduled {
    due: u64,
    dim: Dim,
    pos: [i32; 3],
    /// Stato che il blocco deve avere ancora a scadenza: se è cambiato nel frattempo la modifica non si applica.
    expect: i32,
    then: i32,
}

fn short(state: &BlockState) -> &str {
    state.name.strip_prefix("minecraft:").unwrap_or(&state.name)
}

/// Valore di un bit di stato (byte o int); `None` se il blocco non ha quello stato.
fn bit(state: &BlockState, key: &str) -> Option<bool> {
    match state.get(key)? {
        StateValue::Byte(v) => Some(*v != 0),
        StateValue::Int(v) => Some(*v != 0),
        StateValue::Str(_) => None,
    }
}

/// Lo stesso stato con il bit `key` a `on`, nello stesso tipo di tag.
fn with_bit(state: &BlockState, key: &str, on: bool) -> BlockState {
    state.with(key, if matches!(state.get(key), Some(StateValue::Int(_))) { StateValue::Int(on as i32) } else { StateValue::Byte(on as u8) })
}

fn is_door(state: &BlockState) -> bool {
    state.get("upper_block_bit").is_some() && state.get("door_hinge_bit").is_some() && state.get("open_bit").is_some()
}

fn is_trapdoor(state: &BlockState) -> bool {
    short(state).ends_with("trapdoor") && state.get("open_bit").is_some()
}

fn is_fence_gate(state: &BlockState) -> bool {
    let name = short(state);
    (name == "fence_gate" || name.ends_with("_fence_gate")) && state.get("open_bit").is_some()
}

fn is_button(state: &BlockState) -> bool {
    short(state).ends_with("button") && state.get("button_pressed_bit").is_some()
}

/// Il blocco a mano non si apre (ferro).
fn iron(state: &BlockState) -> bool {
    short(state).starts_with("iron_")
}

fn change(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> BlockChange {
    BlockChange { pos, hash: state.hash(), water: view.water(pos) }
}

/// Attiva il blocco `state` in `pos` con quel clic: le celle da cambiare, oppure `None` se il blocco non è interattivo (e il clic
/// piazza). `Some` con nessuna modifica vuol dire "attivato, ma non cambia nulla" (il pulsante già premuto): il clic è consumato.
pub(crate) fn activate(state: &BlockState, pos: [i32; 3], ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    if iron(state) {
        return None;
    }
    if is_door(state) {
        let open = !bit(state, "open_bit")?;
        let mut changes = vec![change(view, pos, &with_bit(state, "open_bit", open))];
        // l'altra metà segue (se c'è e se è l'altra metà della stessa porta)
        let upper = bit(state, "upper_block_bit")?;
        let other_pos = [pos[0], if upper { pos[1].saturating_sub(1) } else { pos[1].saturating_add(1) }, pos[2]];
        let other = view.block(other_pos).and_then(|hash| blocks().state(hash)).filter(|o| o.name == state.name && bit(o, "upper_block_bit") == Some(!upper));
        changes.extend(other.map(|o| change(view, other_pos, &with_bit(o, "open_bit", open))));
        Some(changes)
    } else if is_trapdoor(state) {
        Some(vec![change(view, pos, &with_bit(state, "open_bit", !bit(state, "open_bit")?))])
    } else if is_fence_gate(state) {
        let open = !bit(state, "open_bit")?;
        let mut gate = with_bit(state, "open_bit", open);
        // aprendo da davanti si apre verso chi sta dall'altra parte: il cancelletto prende la direzione di chi lo apre
        let player = Dir::from_yaw(ctx.yaw);
        if open && matches!(state.get("minecraft:cardinal_direction"), Some(StateValue::Str(f)) if *f == player.opposite().name()) {
            gate = gate.with("minecraft:cardinal_direction", StateValue::Str(player.name().into()));
        }
        Some(vec![change(view, pos, &gate)])
    } else if short(state) == "lever" {
        Some(vec![change(view, pos, &with_bit(state, "open_bit", !bit(state, "open_bit")?))])
    } else if is_button(state) {
        // un pulsante già premuto non fa nulla finché non torna su
        Some(if bit(state, "button_pressed_bit")? { Vec::new() } else { vec![change(view, pos, &with_bit(state, "button_pressed_bit", true))] })
    } else {
        None
    }
}

/// Il suono di un blocco appena attivato, dal suo nuovo stato.
fn sound(new: &BlockState) -> Option<&'static str> {
    let on = |key| bit(new, key).unwrap_or(false);
    Some(if is_door(new) {
        if on("open_bit") {
            "door.open"
        } else {
            "door.close"
        }
    } else if is_trapdoor(new) {
        if on("open_bit") {
            "trapdoor.open"
        } else {
            "trapdoor.close"
        }
    } else if is_fence_gate(new) {
        if on("open_bit") {
            "fence_gate.open"
        } else {
            "fence_gate.close"
        }
    } else if short(new) == "lever" {
        if on("open_bit") {
            "power.on"
        } else {
            "power.off"
        }
    } else if is_button(new) {
        if on("button_pressed_bit") {
            "button.click_on"
        } else {
            "button.click_off"
        }
    } else {
        return None;
    })
}

/// Dopo quanti tick un pulsante premuto torna su: 20 per quelli di pietra e di pietranera lucida, 30 per gli altri (legno, bambù, funghi).
fn button_ticks(state: &BlockState) -> Option<u64> {
    (is_button(state) && bit(state, "button_pressed_bit") == Some(true)).then(|| {
        if matches!(short(state), "stone_button" | "polished_blackstone_button") {
            STONE_BUTTON_TICKS
        } else {
            WOODEN_BUTTON_TICKS
        }
    })
}

impl Server {
    /// Il giocatore usa l'oggetto in mano sul blocco cliccato: se è interattivo lo attiva e restituisce `true` (anche se rifiuta:
    /// il clic è consumato e non piazza nulla), altrimenti `false` e il chiamante piazza. Si rifiuta (rimandando lo stato vero
    /// all'autore) per spettatori, fuori portata e gancio che annulla; l'avventura può usare porte e leve (vanilla).
    pub(crate) fn interact(&mut self, pid: u64, u: &UseItem, ctx: &PlaceCtx, mode: GameMode, eyes: [f32; 3]) -> bool {
        let dim = Dim::Overworld;
        let view = WorldView { server: self, dim };
        let Some(state) = view.block(u.pos).and_then(|hash| blocks().state(hash)) else { return false };
        if short(state) == "crafting_table" {
            // Banco da lavoro (Dragonfly `CraftingTable.Activate`): apre la finestra 3x3. Fuori portata o da spettatore il clic è consumato e basta.
            if mode != GameMode::Spectator && in_reach(eyes, u.pos, mode == GameMode::Creative) {
                self.open_crafting_table(pid, u.pos);
            } else {
                log!("[#{pid}] apertura del banco {:?} rifiutata: modalità o portata", u.pos);
            }
            return true;
        }
        let Some(changes) = activate(state, u.pos, ctx, &view) else { return false };
        let creative = mode == GameMode::Creative;
        let others: Vec<[i32; 3]> = changes.iter().map(|c| c.pos).filter(|&p| p != u.pos).collect();
        if mode == GameMode::Spectator || !in_reach(eyes, u.pos, creative) || changes.iter().any(|c| !in_reach(eyes, c.pos, creative)) {
            log!("[#{pid}] uso di {:?} rifiutato: modalità o portata", u.pos);
            self.resend_blocks(pid, dim, &[&[u.pos], others.as_slice()].concat());
            return true;
        }
        if changes.is_empty() {
            self.keep_button_scheduled(dim, u.pos);
            return true;
        }
        if !self.allow(&WorldAction::Interact { pid, changes: changes.clone() }) {
            log!("[#{pid}] uso di {:?} annullato da un gancio", u.pos);
            self.resend_blocks(pid, dim, &[&[u.pos], others.as_slice()].concat());
            return true;
        }
        self.set_blocks(dim, &changes);
        for c in changes.iter().filter(|c| c.pos == u.pos) {
            if let Some(new) = blocks().state(c.hash) {
                if let Some(name) = sound(new) {
                    self.play_sound(name, c.pos, c.hash);
                }
                if let Some(ticks) = button_ticks(new) {
                    self.schedule(self.tick_count + ticks, dim, c.pos, c.hash, released(new));
                }
            }
        }
        true
    }

    /// Apre il banco da lavoro in `pos` a `pid`: `ContainerOpen` di tipo banco con una finestra nuova (1..=99). Se è già aperto lì non rimanda
    /// nulla (un secondo `ContainerOpen` farebbe crashare il client); se era aperto un altro banco, prima ne restituisce la griglia.
    pub(crate) fn open_crafting_table(&mut self, pid: u64, pos: [i32; 3]) {
        let Some(p) = self.players.get(&pid) else { return };
        if p.inventory.table() == Some(pos) {
            return;
        }
        self.return_ui_items(pid);
        let Some(p) = self.players.get_mut(&pid) else { return };
        let window = p.inventory.open_table(pos);
        log!("[#{pid}] apre il banco da lavoro {pos:?} (finestra {window})");
        let position = BlockPos { x: pos[0], y: pos[1], z: pos[2] };
        self.send(pid, &[ContainerOpen { container_id: window, container_type: CONTAINER_TYPE_WORKBENCH, position, target_actor_id: -1 }.packet()]);
    }

    /// Chiude il banco e restituisce all'inventario gli oggetti rimasti nella griglia e sul cursore (Dragonfly `MoveItemsToInventory`);
    /// quelli che non ci stanno cadono a terra. Il client riceve le caselle cambiate.
    pub(crate) fn return_ui_items(&mut self, pid: u64) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        let mut changed = Vec::new();
        let mut leftover = Vec::new();
        for item in p.inventory.release_ui() {
            let (rest, slots) = p.inventory.add(item);
            changed.extend(slots);
            if !rest.is_empty() {
                leftover.push(rest);
            }
        }
        changed.sort_unstable();
        changed.dedup();
        let packets: Vec<_> = changed.iter().filter_map(|&(c, s)| p.inventory.slot_packet_at(c, s)).collect();
        if !packets.is_empty() {
            self.send(pid, &packets);
        }
        for item in leftover {
            self.drop_item(pid, item);
        }
    }

    /// Un pulsante premuto che non ha un appuntamento per risalire (per esempio salvato premuto: la memoria dei tick non sopravvive
    /// al riavvio) ne riceve uno quando lo si clicca, così non resta giù per sempre. Se l'appuntamento c'è già per questo stato
    /// resta quello: la risalita si conta dalla prima pressione.
    fn keep_button_scheduled(&mut self, dim: Dim, pos: [i32; 3]) {
        let Some((hash, _)) = self.block_at(dim, pos) else { return };
        if self.scheduled.iter().any(|s| s.dim == dim && s.pos == pos && s.expect == hash) {
            return;
        }
        if let Some(state) = blocks().state(hash).filter(|s| button_ticks(s).is_some()) {
            self.schedule(self.tick_count + button_ticks(state).unwrap_or(WOODEN_BUTTON_TICKS), dim, pos, hash, released(state));
        }
    }

    /// Programma il cambio di `pos` a `then` per il tick `due_tick`, se a quel punto il blocco è ancora `expect`. Una cella ha
    /// un solo appuntamento: uno nuovo sostituisce il vecchio (che apparteneva a un blocco ormai rotto o sostituito).
    pub(crate) fn schedule(&mut self, due_tick: u64, dim: Dim, pos: [i32; 3], expect: i32, then: i32) {
        self.scheduled.retain(|s| !(s.dim == dim && s.pos == pos));
        self.scheduled.push(Scheduled { due: due_tick, dim, pos, expect, then });
    }

    /// Applica le modifiche programmate arrivate a scadenza. Se la cella non è in memoria (chunk scaricato) l'appuntamento resta e
    /// si riprova al tick dopo; se il blocco non è più quello atteso (rotto, sostituito) l'appuntamento cade.
    pub(crate) fn run_scheduled(&mut self) {
        if self.scheduled.is_empty() {
            return;
        }
        let now = self.tick_count;
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.scheduled).into_iter().partition(|s| s.due <= now);
        self.scheduled = later;
        for s in due {
            match self.block_at(s.dim, s.pos) {
                None => self.scheduled.push(s),
                Some((hash, water)) if hash == s.expect => {
                    self.set_blocks(s.dim, &[BlockChange { pos: s.pos, hash: s.then, water }]);
                    if let Some(name) = blocks().state(s.then).and_then(sound) {
                        self.play_sound(name, s.pos, s.then);
                    }
                }
                Some(_) => {}
            }
        }
    }

    /// Manda il suono `name` (al centro di `pos`, con `data`) a chi vede il chunk.
    pub(crate) fn play_sound(&self, name: &str, pos: [i32; 3], data: i32) {
        let packet = LevelSoundEvent {
            sound_event: name.into(),
            position: Vec3 { x: pos[0] as f32 + 0.5, y: pos[1] as f32 + 0.5, z: pos[2] as f32 + 0.5 },
            data,
            actor_identifier: String::new(),
            is_baby: false,
            is_global: false,
            actor_unique_id: -1,
            fire_at_position: None,
        }
        .packet();
        let chunk = (pos[0].div_euclid(16), pos[2].div_euclid(16));
        for (&viewer, _) in self.players.iter().filter(|(_, p)| p.sent.contains(&chunk)) {
            self.send(viewer, std::slice::from_ref(&packet));
        }
    }
}

/// Il pulsante `pressed` com'è dopo essere risalito.
fn released(pressed: &BlockState) -> i32 {
    with_bit(pressed, "button_pressed_bit", false).hash()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{
        fake::{click, floor, hash, Fake},
        EAST, UP,
    };

    /// Lo stato di default del blocco con quegli stati cambiati.
    fn state(name: &str, set: &[(&str, StateValue)]) -> i32 {
        let base = blocks().state(hash(name)).expect("stato noto");
        set.iter().fold(base.clone(), |s, (k, v)| s.with(k, v.clone())).hash()
    }

    fn byte(v: u8) -> StateValue {
        StateValue::Byte(v)
    }

    fn st(hash: i32) -> BlockState {
        blocks().state(hash).expect("stato noto").clone()
    }

    fn bit_at(f: &Fake, pos: [i32; 3], key: &str) -> bool {
        bit(&st(f.blocks[&pos]), key).unwrap_or_else(|| panic!("{key} manca"))
    }

    /// Il clic (vedi `placement::fake::click`) con un oggetto qualunque in mano, rivolto a sud.
    fn on(pos: [i32; 3]) -> PlaceCtx {
        click(pos, EAST, "minecraft:stone")
    }

    fn activate_at(f: &Fake, pos: [i32; 3], ctx: &PlaceCtx) -> Option<Vec<BlockChange>> {
        activate(&st(f.blocks[&pos]), pos, ctx, f)
    }

    fn door_world(name: &str) -> Fake {
        let mut f = floor();
        let lower = state(name, &[("upper_block_bit", byte(0))]);
        f.blocks.insert([0, 10, 0], lower);
        f.blocks.insert([0, 11, 0], state(name, &[("upper_block_bit", byte(1))]));
        f
    }

    #[test]
    fn door_toggles_both_halves() {
        for name in ["minecraft:wooden_door", "minecraft:spruce_door", "minecraft:bamboo_door", "minecraft:copper_door", "minecraft:waxed_oxidized_copper_door"]
        {
            let mut f = door_world(name);
            // clic sulla metà bassa: si aprono tutte e due
            let changes = activate_at(&f, [0, 10, 0], &on([0, 10, 0])).unwrap_or_else(|| panic!("{name} interattiva"));
            assert_eq!(changes.iter().map(|c| c.pos).collect::<Vec<_>>(), [[0, 10, 0], [0, 11, 0]], "{name}");
            f.apply(&changes);
            assert!(bit_at(&f, [0, 10, 0], "open_bit") && bit_at(&f, [0, 11, 0], "open_bit"), "{name} aperta");
            // le altre proprietà non cambiano (metà, direzione, cerniera)
            assert!(!bit_at(&f, [0, 10, 0], "upper_block_bit") && bit_at(&f, [0, 11, 0], "upper_block_bit"), "{name}");
            // clic sulla metà alta: si chiudono tutte e due
            let changes = activate_at(&f, [0, 11, 0], &on([0, 11, 0])).unwrap();
            f.apply(&changes);
            assert!(!bit_at(&f, [0, 10, 0], "open_bit") && !bit_at(&f, [0, 11, 0], "open_bit"), "{name} chiusa");
        }
        // una metà sola (l'altra è sparita): si apre quella cliccata e basta; l'acqua del secondo strato resta
        let mut f = door_world("minecraft:wooden_door");
        f.blocks.insert([0, 11, 0], hash("minecraft:stone"));
        f.water.insert([0, 10, 0]);
        let changes = activate_at(&f, [0, 10, 0], &on([0, 10, 0])).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].water);
        // una porta di un altro tipo sopra non è la sua metà
        f.blocks.insert([0, 11, 0], state("minecraft:spruce_door", &[("upper_block_bit", byte(1))]));
        assert_eq!(activate_at(&f, [0, 10, 0], &on([0, 10, 0])).unwrap().len(), 1);
    }

    #[test]
    fn iron_door_and_iron_trapdoor_ignore_hand() {
        let f = door_world("minecraft:iron_door");
        assert_eq!(activate_at(&f, [0, 10, 0], &on([0, 10, 0])), None, "la porta di ferro non si apre a mano: il clic piazza");
        assert_eq!(activate_at(&f, [0, 11, 0], &on([0, 11, 0])), None);
        let mut g = floor();
        g.blocks.insert([0, 10, 0], hash("minecraft:iron_trapdoor"));
        assert_eq!(activate_at(&g, [0, 10, 0], &on([0, 10, 0])), None);
        // il resto del ferro non è interattivo comunque
        g.blocks.insert([0, 10, 0], hash("minecraft:iron_block"));
        assert_eq!(activate_at(&g, [0, 10, 0], &on([0, 10, 0])), None);
    }

    #[test]
    fn trapdoor_and_fence_gate_toggle() {
        let mut f = floor();
        for name in ["minecraft:trapdoor", "minecraft:spruce_trapdoor", "minecraft:copper_trapdoor", "minecraft:waxed_weathered_copper_trapdoor"] {
            f.blocks.insert([0, 10, 0], hash(name));
            let changes = activate_at(&f, [0, 10, 0], &on([0, 10, 0])).unwrap_or_else(|| panic!("{name} interattiva"));
            assert_eq!(changes.len(), 1);
            f.apply(&changes);
            assert!(bit_at(&f, [0, 10, 0], "open_bit"), "{name} aperta");
            f.apply(&activate_at(&f, [0, 10, 0], &on([0, 10, 0])).unwrap());
            assert!(!bit_at(&f, [0, 10, 0], "open_bit"), "{name} chiusa");
        }
        // cancelletto rivolto a sud
        let gate = |name: &str, facing: &str, open: u8| {
            state(name, &[("minecraft:cardinal_direction", StateValue::Str(facing.into())), ("open_bit", byte(open)), ("in_wall_bit", byte(1))])
        };
        let facing = |f: &Fake| match st(f.blocks[&[0, 10, 0]]).get("minecraft:cardinal_direction") {
            Some(StateValue::Str(v)) => v.clone(),
            other => panic!("{other:?}"),
        };
        for name in ["minecraft:fence_gate", "minecraft:spruce_fence_gate", "minecraft:crimson_fence_gate"] {
            // dal lato davanti (guarda a sud, come il cancelletto): la direzione resta
            f.blocks.insert([0, 10, 0], gate(name, "south", 0));
            let from_front = PlaceCtx { yaw: 0.0, ..on([0, 10, 0]) };
            f.apply(&activate_at(&f, [0, 10, 0], &from_front).unwrap());
            assert!(bit_at(&f, [0, 10, 0], "open_bit"), "{name}");
            assert_eq!(facing(&f), "south", "{name}");
            assert!(bit_at(&f, [0, 10, 0], "in_wall_bit"), "{name}: il muro accanto non cambia");
            // dal lato opposto (guarda a nord) la direzione si inverte, ma solo aprendo
            f.blocks.insert([0, 10, 0], gate(name, "south", 0));
            let from_behind = PlaceCtx { yaw: 180.0, ..on([0, 10, 0]) };
            f.apply(&activate_at(&f, [0, 10, 0], &from_behind).unwrap());
            assert_eq!((bit_at(&f, [0, 10, 0], "open_bit"), facing(&f).as_str()), (true, "north"), "{name}");
            f.apply(&activate_at(&f, [0, 10, 0], &PlaceCtx { yaw: 0.0, ..on([0, 10, 0]) }).unwrap());
            assert_eq!((bit_at(&f, [0, 10, 0], "open_bit"), facing(&f).as_str()), (false, "north"), "chiudendo non si inverte");
        }
    }

    #[test]
    fn lever_toggles() {
        let mut f = floor();
        f.blocks.insert([0, 10, 0], hash("minecraft:lever"));
        let flip = |f: &mut Fake| {
            let changes = activate_at(f, [0, 10, 0], &on([0, 10, 0])).expect("leva interattiva");
            assert_eq!(changes.len(), 1);
            f.apply(&changes);
            bit_at(f, [0, 10, 0], "open_bit")
        };
        assert_eq!([flip(&mut f), flip(&mut f), flip(&mut f)], [true, false, true]);
        // la direzione della leva non cambia
        assert_eq!(st(f.blocks[&[0, 10, 0]]).get("lever_direction"), st(hash("minecraft:lever")).get("lever_direction"));
    }

    #[test]
    fn button_presses_once_and_waits() {
        let mut f = floor();
        for (name, ticks) in [
            ("minecraft:stone_button", 20),
            ("minecraft:polished_blackstone_button", 20),
            ("minecraft:wooden_button", 30),
            ("minecraft:spruce_button", 30),
            ("minecraft:bamboo_button", 30),
            ("minecraft:crimson_button", 30),
        ] {
            f.blocks.insert([0, 10, 0], hash(name));
            assert_eq!(button_ticks(&st(f.blocks[&[0, 10, 0]])), None, "{name}: su non programma niente");
            let changes = activate_at(&f, [0, 10, 0], &on([0, 10, 0])).unwrap_or_else(|| panic!("{name} interattivo"));
            f.apply(&changes);
            assert!(bit_at(&f, [0, 10, 0], "button_pressed_bit"), "{name}");
            let pressed = st(f.blocks[&[0, 10, 0]]);
            assert_eq!(button_ticks(&pressed), Some(ticks), "{name}");
            assert_eq!(released(&pressed), hash(name), "{name}: risale allo stato di partenza");
            assert_eq!(activate_at(&f, [0, 10, 0], &on([0, 10, 0])), Some(Vec::new()), "{name}: già premuto, il clic è consumato e non cambia nulla");
        }
        // un blocco qualunque non è interattivo
        f.blocks.insert([0, 10, 0], hash("minecraft:stone"));
        assert_eq!(activate_at(&f, [0, 10, 0], &click([0, 10, 0], UP, "minecraft:stone")), None);
    }

    #[test]
    fn sounds_follow_the_new_state() {
        let on_off = |name: &str, key: &str| {
            let open = st(state(name, &[(key, byte(1))]));
            let closed = st(state(name, &[(key, byte(0))]));
            (sound(&open), sound(&closed))
        };
        assert_eq!(on_off("minecraft:wooden_door", "open_bit"), (Some("door.open"), Some("door.close")));
        assert_eq!(on_off("minecraft:trapdoor", "open_bit"), (Some("trapdoor.open"), Some("trapdoor.close")));
        assert_eq!(on_off("minecraft:fence_gate", "open_bit"), (Some("fence_gate.open"), Some("fence_gate.close")));
        assert_eq!(on_off("minecraft:lever", "open_bit"), (Some("power.on"), Some("power.off")));
        assert_eq!(on_off("minecraft:stone_button", "button_pressed_bit"), (Some("button.click_on"), Some("button.click_off")));
        assert_eq!(sound(&st(hash("minecraft:stone"))), None);
    }
}
