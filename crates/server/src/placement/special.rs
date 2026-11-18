//! Casi speciali di piazzamento (D§4): ciò che il ramo generico non sa fare perché il blocco ha due metà (porte, letti, piante
//! alte), si fonde con un altro (mezze lastre) o ha bisogno di un appoggio che si cerca attorno (torce, leve, pulsanti,
//! lanterne, scale a pioli, rampicanti, piante). Ogni caso traduce il `UseOnBlock` del blocco in Dragonfly.
use super::{
    change_at, neighbors, normalize, place_generic, replaceable,
    rules::{self, put, Dir, V},
    side, BlockView, PlaceCtx, DOWN, EAST, NORTH, SOUTH, UP, WEST,
};
use crate::{
    events::BlockChange,
    nbt::{write_root, Enc, Tag},
    registry::{blocks, BlockState, StateValue},
};
use vgen::BlockEntity;

/// Il blocco di `ctx.block` ha regole proprie? `None` se no (si usa il ramo generico), altrimenti `Some` con l'esito:
/// `None` se si rifiuta, le celle da cambiare se si piazza.
pub(super) fn place_special(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Option<Vec<BlockChange>>> {
    if ctx.face > EAST {
        return None;
    }
    let state = blocks().state(ctx.block)?;
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    if name.ends_with("slab") && state.get("minecraft:vertical_half").is_some() {
        return slab(ctx, view, state);
    }
    let placed = if state.get("upper_block_bit").is_some() && state.get("door_hinge_bit").is_some() {
        door(ctx, view, state)
    } else if name == "bed" {
        bed(ctx, view, state)
    } else if state.get("torch_facing_direction").is_some() {
        torch(ctx, view)
    } else if name == "lever" || (name.ends_with("button") && state.get("button_pressed_bit").is_some()) {
        on_support(ctx, view)
    } else if name.ends_with("lantern") && state.get("hanging").is_some() {
        lantern(ctx, view)
    } else if name == "ladder" {
        ladder(ctx, view)
    } else if name == "vine" {
        vine(ctx, view)
    } else if let Some(plant) = neighbors::plant_kind(name) {
        if state.get("upper_block_bit").is_some() {
            double_plant(ctx, view, state, plant)
        } else {
            single_plant(ctx, view, plant)
        }
    } else if state.get("wall_post_bit").is_some() || state.get("minecraft:corner").is_some() {
        derived(ctx, view)
    } else {
        return None;
    };
    Some(placed)
}

/// La lastra doppia di una mezza lastra, con la stessa metà: `X_slab` -> `X_double_slab` (rame: `cut_copper_slab` -> `double_cut_copper_slab`).
fn double_slab(name: &str, half: &StateValue) -> Option<i32> {
    let double = match name.strip_suffix("cut_copper_slab") {
        Some(prefix) => format!("{prefix}double_cut_copper_slab"),
        None => format!("{}_double_slab", name.strip_suffix("_slab")?),
    };
    let next = blocks().state(blocks().default_hash(&double)?)?.with("minecraft:vertical_half", half.clone());
    blocks().is_valid(&next).then(|| next.hash())
}

/// Una mezza lastra (non doppia) dello stesso tipo di `name`: la sua metà.
fn same_half_slab(hash: i32, name: &str) -> Option<StateValue> {
    let state = blocks().state(hash)?;
    (state.name == name && !name.contains("double_")).then(|| state.get("minecraft:vertical_half").cloned()).flatten()
}

/// Mezze lastre (D§4.3): cliccando la faccia giusta di una mezza lastra uguale, o il blocco accanto a una mezza lastra uguale,
/// diventa una lastra doppia; altrimenti `None`: la lastra si piazza come un blocco qualsiasi (la metà la decide `rules`).
fn slab(ctx: &PlaceCtx, view: &impl BlockView, state: &BlockState) -> Option<Option<Vec<BlockChange>>> {
    if state.name.contains("double_") {
        return None;
    }
    let merge = |pos: [i32; 3], half: &StateValue| {
        let hash = double_slab(&state.name, half)?;
        // la lastra doppia non regge l'acqua nel secondo strato
        Some(vec![BlockChange { pos, hash, water: false }])
    };
    let clicked = view.block(ctx.clicked)?;
    if let Some(half) = same_half_slab(clicked, &state.name) {
        let top = half == StateValue::Str("top".into());
        if (ctx.face == UP && !top) || (ctx.face == DOWN && top) {
            return Some(merge(ctx.clicked, &half));
        }
    }
    // il blocco cliccato non è sostituibile e accanto, dalla parte cliccata, c'è una mezza lastra uguale
    let next = ctx.target();
    if !replaceable(clicked) {
        if let Some(half) = view.block(next).and_then(|h| same_half_slab(h, &state.name)) {
            return Some(merge(next, &half));
        }
    }
    None
}

/// Cerniera della porta (`doorHingeRight`): a destra se a sinistra c'è una porta e a destra no, o se a destra ci sono più
/// blocchi pieni opachi che a sinistra (contando le due metà).
fn hinge_right(view: &impl BlockView, pos: [i32; 3], facing: Dir) -> bool {
    let count = |face: u8| {
        let beside = side(pos, face);
        let (mut solid, mut door) = (0, false);
        for cell in [beside, side(beside, UP)] {
            let Some(hash) = view.block(cell) else { continue };
            door |= blocks().state(hash).is_some_and(|s| s.get("door_hinge_bit").is_some());
            solid += neighbors::opaque_cube(hash) as i32;
        }
        (solid, door)
    };
    let ((left_solid, left_door), (right_solid, right_door)) = (count(facing.rotate_left().face()), count(facing.rotate_right().face()));
    (left_door && !right_door) || right_solid > left_solid
}

/// Un blocco con un appoggio pieno sotto (`supportedFromBelow`).
fn supported_below(view: &impl BlockView, pos: [i32; 3]) -> bool {
    neighbors::solid(view, side(pos, DOWN), UP) == Some(true)
}

/// La cella `pos` si può riempire con `hash`: è sostituibile e non c'è già lo stesso blocco. Restituisce il cambio.
fn free_cell(view: &impl BlockView, pos: [i32; 3], hash: i32) -> Option<BlockChange> {
    let existing = view.block(pos)?;
    if replaceable(existing) && existing != hash {
        change_at(view, pos, existing, hash)
    } else {
        None
    }
}

/// Porte (D§4.1): due metà, solo cliccando la faccia superiore (o un blocco sostituibile) con aria sopra e un appoggio pieno
/// sotto. La porta ignora l'ingombro del giocatore (`IgnoreBBox`).
fn door(ctx: &PlaceCtx, view: &impl BlockView, state: &BlockState) -> Option<Vec<BlockChange>> {
    let ctx = normalize(ctx, view)?;
    let target = ctx.target();
    if ctx.face != UP || !supported_below(view, target) {
        return None;
    }
    let facing = Dir::from_yaw(ctx.yaw);
    let mut lower = rules::orient(state, &ctx);
    put(&mut lower, "open_bit", V::N(0));
    put(&mut lower, "upper_block_bit", V::N(0));
    put(&mut lower, "door_hinge_bit", V::N(hinge_right(view, target, facing) as i32));
    let mut upper = lower.clone();
    put(&mut upper, "upper_block_bit", V::N(1));
    Some(vec![free_cell(view, target, lower.hash())?, free_cell(view, side(target, UP), upper.hash())?])
}

/// Letti (D§4.2): i piedi nella cella cliccata, la testa davanti al giocatore; tutte e due le celle libere e con un appoggio pieno sotto.
/// Il colore è nel block entity (vedi [`bed_entity`]).
fn bed(ctx: &PlaceCtx, view: &impl BlockView, state: &BlockState) -> Option<Vec<BlockChange>> {
    let ctx = normalize(ctx, view)?;
    let foot_pos = ctx.target();
    let head_pos = side(foot_pos, Dir::from_yaw(ctx.yaw).face());
    if !supported_below(view, foot_pos) || !supported_below(view, head_pos) {
        return None;
    }
    let mut foot = rules::orient(state, &ctx);
    put(&mut foot, "head_piece_bit", V::N(0));
    put(&mut foot, "occupied_bit", V::N(0));
    let mut head = foot.clone();
    put(&mut head, "head_piece_bit", V::N(1));
    // prima la testa, poi i piedi, come Dragonfly
    let head = free_cell(view, head_pos, head.hash())?;
    Some(vec![head, free_cell(view, foot_pos, foot.hash())?])
}

/// Il block entity di un letto con quel colore (l'aux dell'oggetto, 0..15): il colore sta solo lì (D§4.2).
pub(super) fn bed_entity(pos: [i32; 3], color: u32) -> BlockEntity {
    let color = if color < 16 { color as i8 } else { 0 };
    let nbt = Tag::Compound(vec![
        ("id".into(), Tag::String("Bed".into())),
        ("color".into(), Tag::Byte(color)),
        ("x".into(), Tag::Int(pos[0])),
        ("y".into(), Tag::Int(pos[1])),
        ("z".into(), Tag::Int(pos[2])),
    ]);
    BlockEntity { x: pos[0], y: pos[1], z: pos[2], nbt: write_root("", &nbt, Enc::Network) }
}

/// Torce (D§4.4): non da sotto; se il lato cliccato non regge, si cerca un appoggio tra sud, ovest, nord, est e sotto.
fn torch(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    let ctx = normalize(ctx, view)?;
    if ctx.face == DOWN {
        return None;
    }
    let target = ctx.target();
    let holds = |toward: u8| neighbors::solid(view, side(target, toward), toward ^ 1) == Some(true);
    let face = if holds(ctx.face ^ 1) { ctx.face } else { [SOUTH, WEST, NORTH, EAST, DOWN].into_iter().find(|&toward| holds(toward))? ^ 1 };
    place_generic(&ctx.with_face(face), view)
}

/// Leve e pulsanti (D§4.4): si attaccano alla faccia cliccata (anche su e giù) e il blocco cliccato deve essere pieno da quel lato.
fn on_support(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    let ctx = normalize(ctx, view)?;
    if neighbors::solid(view, side(ctx.target(), ctx.face ^ 1), ctx.face) != Some(true) {
        return None;
    }
    place_generic(&ctx, view)
}

/// Lanterne (D§4.4): cliccando sotto un blocco si appendono, se lì c'è un blocco pieno o una catena (altrimenti si appoggiano);
/// in piedi servono un appoggio pieno sotto.
fn lantern(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    let mut ctx = normalize(ctx, view)?;
    let target = ctx.target();
    if ctx.face == DOWN {
        let up = side(target, UP);
        let chain = view.block(up).and_then(super::name_of).is_some_and(|n| n.ends_with("chain"));
        if !chain && neighbors::solid(view, up, DOWN) != Some(true) {
            ctx = ctx.with_face(UP);
        }
    }
    if ctx.face != DOWN && !supported_below(view, target) {
        return None;
    }
    place_generic(&ctx, view)
}

/// Scale a pioli (D§4.4): solo di lato; se il blocco dietro non regge, si cerca un muro attorno (sud, nord, est, ovest).
fn ladder(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    let ctx = normalize(ctx, view)?;
    if ctx.face <= UP {
        return None;
    }
    let target = ctx.target();
    let holds = |toward: u8| neighbors::solid(view, side(target, toward), toward ^ 1) == Some(true);
    let face = if holds(ctx.face ^ 1) { ctx.face } else { [SOUTH, NORTH, EAST, WEST].into_iter().find(|&toward| holds(toward))? ^ 1 };
    place_generic(&ctx.with_face(face), view)
}

/// Rampicanti (D§4.4): solo di lato, su un cubo pieno, e non sopra un altro rampicante.
fn vine(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    if ctx.face <= UP || neighbors::solid(view, ctx.clicked, ctx.face) != Some(true) {
        return None;
    }
    let existing = view.block(ctx.target())?;
    if super::name_of(existing) == Some("vine") {
        return None;
    }
    place_generic(ctx, view)
}

/// Piante basse (D§4.6): serve il terreno giusto sotto (e per la canna l'acqua accanto, per il cactus l'aria attorno).
fn single_plant(ctx: &PlaceCtx, view: &impl BlockView, plant: neighbors::Plant) -> Option<Vec<BlockChange>> {
    let ctx = normalize(ctx, view)?;
    if neighbors::plant_supported(view, ctx.target(), plant, true) != Some(true) {
        return None;
    }
    place_generic(&ctx, view)
}

/// Piante alte (erba alta, girasole...): due celle libere e il terreno sotto la prima.
fn double_plant(ctx: &PlaceCtx, view: &impl BlockView, state: &BlockState, plant: neighbors::Plant) -> Option<Vec<BlockChange>> {
    let ctx = normalize(ctx, view)?;
    let target = ctx.target();
    if neighbors::plant_supported(view, target, plant, true) != Some(true) {
        return None;
    }
    let mut lower = rules::orient(state, &ctx);
    put(&mut lower, "upper_block_bit", V::N(0));
    let mut upper = lower.clone();
    put(&mut upper, "upper_block_bit", V::N(1));
    Some(vec![free_cell(view, target, lower.hash())?, free_cell(view, side(target, UP), upper.hash())?])
}

/// Muri e scale: il blocco si piazza come gli altri, ma connessioni e angoli si calcolano dai vicini (D§4.5, D§3 `minecraft:corner`).
fn derived(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    let ctx = normalize(ctx, view)?;
    let mut changes = place_generic(&ctx, view)?;
    changes[0].hash = neighbors::settled(view, changes[0].pos, changes[0].hash);
    Some(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{
        fake::{air, click, floor, hash, Fake},
        place, DOWN, EAST, NORTH, UP, WEST,
    };

    fn put_block(f: &mut Fake, pos: [i32; 3], name: &str) {
        f.blocks.insert(pos, hash(name));
    }

    fn state_at(changes: &[BlockChange], pos: [i32; 3]) -> BlockState {
        let c = changes.iter().find(|c| c.pos == pos).unwrap_or_else(|| panic!("nessun cambio in {pos:?}: {changes:?}"));
        blocks().state(c.hash).expect("stato noto").clone()
    }

    fn val(state: &BlockState, key: &str) -> StateValue {
        state.get(key).cloned().unwrap_or_else(|| panic!("{key} manca in {state:?}"))
    }

    fn text(state: &BlockState, key: &str) -> String {
        match val(state, key) {
            StateValue::Str(v) => v,
            other => panic!("{key}: {other:?}"),
        }
    }

    fn int(state: &BlockState, key: &str) -> i32 {
        match val(state, key) {
            StateValue::Int(v) => v,
            StateValue::Byte(v) => v as i32,
            other => panic!("{key}: {other:?}"),
        }
    }

    /// Una mezza lastra alta di quel tipo.
    fn top_slab(name: &str) -> i32 {
        blocks().state(hash(name)).unwrap().with("minecraft:vertical_half", StateValue::Str("top".into())).hash()
    }

    /// Cella (0, 10, 0) sul pavimento: il clic è sul blocco sotto, faccia superiore.
    fn on_floor(held: &str) -> PlaceCtx {
        click([0, 9, 0], UP, held)
    }

    #[test]
    fn door_places_two_halves_with_hinge() {
        let mut f = floor();
        let changes = place(&on_floor("minecraft:wooden_door"), &f).expect("porta piazzata");
        assert_eq!(changes.len(), 2);
        let (lower, upper) = (state_at(&changes, [0, 10, 0]), state_at(&changes, [0, 11, 0]));
        assert_eq!((int(&lower, "upper_block_bit"), int(&upper, "upper_block_bit")), (0, 1));
        // il giocatore guarda a sud: la porta è "west" (D§3, direzione ruotata a destra)
        assert_eq!((text(&lower, "minecraft:cardinal_direction"), text(&upper, "minecraft:cardinal_direction")), ("west".into(), "west".into()));
        assert_eq!((int(&lower, "open_bit"), int(&upper, "open_bit")), (0, 0), "si piazza chiusa");
        assert_eq!((int(&lower, "door_hinge_bit"), int(&upper, "door_hinge_bit")), (0, 0), "nessun vicino: cerniera a sinistra");

        // a sud la destra del giocatore è l'ovest: due blocchi pieni a ovest spostano la cerniera a destra (su entrambe le metà)
        put_block(&mut f, [-1, 10, 0], "minecraft:stone");
        put_block(&mut f, [-1, 11, 0], "minecraft:stone");
        let changes = place(&on_floor("minecraft:wooden_door"), &f).unwrap();
        assert_eq!((int(&state_at(&changes, [0, 10, 0]), "door_hinge_bit"), int(&state_at(&changes, [0, 11, 0]), "door_hinge_bit")), (1, 1));
        // due blocchi pieni anche a est: pari, resta a sinistra
        put_block(&mut f, [1, 10, 0], "minecraft:stone");
        put_block(&mut f, [1, 11, 0], "minecraft:stone");
        assert_eq!(int(&state_at(&place(&on_floor("minecraft:wooden_door"), &f).unwrap(), [0, 10, 0]), "door_hinge_bit"), 0);

        // una porta a sinistra (est) e nessuna a destra: cerniera a destra, anche senza blocchi pieni
        let mut g = floor();
        put_block(&mut g, [1, 10, 0], "minecraft:wooden_door");
        assert_eq!(int(&state_at(&place(&on_floor("minecraft:wooden_door"), &g).unwrap(), [0, 10, 0]), "door_hinge_bit"), 1);
        // il vetro non conta come muro (non è opaco)
        let mut h = floor();
        put_block(&mut h, [-1, 10, 0], "minecraft:glass");
        assert_eq!(int(&state_at(&place(&on_floor("minecraft:wooden_door"), &h).unwrap(), [0, 10, 0]), "door_hinge_bit"), 0);
    }

    #[test]
    fn door_family_uses_the_same_rules() {
        let f = floor();
        for name in ["minecraft:iron_door", "minecraft:spruce_door", "minecraft:copper_door", "minecraft:bamboo_door"] {
            let changes = place(&on_floor(name), &f).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(changes.len(), 2, "{name}");
            for c in &changes {
                assert!(blocks().is_valid(blocks().state(c.hash).unwrap()), "{name}");
            }
        }
        // orientazione dal giocatore: nord -> "east"
        let mut north = on_floor("minecraft:wooden_door");
        north.yaw = 180.0;
        assert_eq!(text(&state_at(&place(&north, &f).unwrap(), [0, 10, 0]), "minecraft:cardinal_direction"), "east");
    }

    #[test]
    fn door_needs_solid_floor_and_free_top() {
        let f = floor();
        // cliccando sul fianco di un blocco la porta non si piazza (D§4.1: solo faccia superiore)
        let mut wall = floor();
        put_block(&mut wall, [1, 10, 0], "minecraft:stone");
        assert!(place(&click([1, 10, 0], WEST, "minecraft:wooden_door"), &wall).is_none());
        assert!(place(&click([1, 10, 0], UP, "minecraft:wooden_door"), &wall).is_some(), "sopra il muro sì");
        // niente pavimento sotto
        assert!(place(&click([0, 12, 0], UP, "minecraft:wooden_door"), &f).is_none(), "clic nell'aria: la porta pende nel vuoto");
        // sopra occupato
        let mut low = floor();
        put_block(&mut low, [0, 11, 0], "minecraft:stone");
        assert!(place(&on_floor("minecraft:wooden_door"), &low).is_none());
        // sopra c'è un blocco sostituibile: va bene
        let mut grass = floor();
        put_block(&mut grass, [0, 11, 0], "minecraft:short_grass");
        assert!(place(&on_floor("minecraft:wooden_door"), &grass).is_some());
        // la cella della porta occupata
        let mut busy = floor();
        put_block(&mut busy, [0, 10, 0], "minecraft:stone");
        assert!(place(&on_floor("minecraft:wooden_door"), &busy).is_none());
        // appoggio: una mezza lastra bassa non regge (solo la faccia inferiore è piena), una alta sì
        let mut slab = floor();
        put_block(&mut slab, [0, 9, 0], "minecraft:oak_slab");
        assert!(place(&on_floor("minecraft:wooden_door"), &slab).is_none());
        slab.blocks.insert([0, 9, 0], top_slab("minecraft:oak_slab"));
        assert!(place(&on_floor("minecraft:wooden_door"), &slab).is_some());
        // si piazza anche addosso al giocatore (IgnoreBBox)
        let mut inside = on_floor("minecraft:wooden_door");
        inside.feet = [0.5, 10.0, 0.5];
        assert!(place(&inside, &f).is_some());
        // un blocco sostituibile cliccato conta come faccia superiore: la porta va in quella cella
        let mut flowers = floor();
        put_block(&mut flowers, [2, 10, 2], "minecraft:short_grass");
        let changes = place(&click([2, 10, 2], NORTH, "minecraft:wooden_door"), &flowers).unwrap();
        assert_eq!(changes[0].pos, [2, 10, 2]);
        // acqua: la porta la sostituisce (non si può "waterlogged")
        let mut wet = floor();
        wet.water.insert([0, 10, 0]);
        let changes = place(&on_floor("minecraft:wooden_door"), &wet).unwrap();
        assert!(changes.iter().any(|c| c.pos == [0, 10, 0]) && changes.iter().all(|c| !c.water));
    }

    #[test]
    fn bed_places_head_and_foot() {
        let f = floor();
        // il giocatore guarda a sud: i piedi nella cella cliccata, la testa davanti in z + 1
        let changes = place(&on_floor("minecraft:bed"), &f).expect("letto piazzato");
        assert_eq!(changes.len(), 2);
        let (foot, head) = (state_at(&changes, [0, 10, 0]), state_at(&changes, [0, 10, 1]));
        assert_eq!((int(&foot, "head_piece_bit"), int(&head, "head_piece_bit")), (0, 1));
        assert_eq!((int(&foot, "direction"), int(&head, "direction")), (0, 0), "sud");
        assert_eq!((int(&foot, "occupied_bit"), int(&head, "occupied_bit")), (0, 0));
        // verso ovest la testa sta in x - 1
        let mut west = on_floor("minecraft:bed");
        west.yaw = 90.0;
        let changes = place(&west, &f).unwrap();
        assert_eq!(int(&state_at(&changes, [-1, 10, 0]), "head_piece_bit"), 1);
        assert_eq!(int(&state_at(&changes, [0, 10, 0]), "direction"), 1);
        // serve appoggio sotto entrambe le celle
        let mut hole = floor();
        put_block(&mut hole, [0, 9, 1], "minecraft:air");
        assert!(place(&on_floor("minecraft:bed"), &hole).is_none(), "niente sotto la testa");
        let mut hole = floor();
        put_block(&mut hole, [0, 9, 0], "minecraft:air");
        put_block(&mut hole, [1, 10, 0], "minecraft:stone");
        assert!(place(&click([1, 10, 0], WEST, "minecraft:bed"), &hole).is_none(), "niente sotto i piedi");
        // e le due celle libere
        let mut busy = floor();
        put_block(&mut busy, [0, 10, 1], "minecraft:stone");
        assert!(place(&on_floor("minecraft:bed"), &busy).is_none());
        // si piazza anche addosso al giocatore
        let mut inside = on_floor("minecraft:bed");
        inside.feet = [0.5, 10.0, 0.5];
        assert!(place(&inside, &f).is_some());
    }

    #[test]
    fn bed_entity_carries_the_color() {
        let entity = bed_entity([3, 10, 4], 8);
        assert_eq!((entity.x, entity.y, entity.z), (3, 10, 4));
        let (name, tag) = crate::nbt::Reader::new(&entity.nbt, Enc::Network).root().expect("NBT di rete leggibile");
        assert_eq!(name, "");
        assert_eq!(tag.get("id").and_then(Tag::as_str), Some("Bed"));
        assert_eq!(tag.get("color"), Some(&Tag::Byte(8)));
        assert_eq!(tag.get("y").and_then(Tag::as_i32), Some(10));
        let wild = crate::nbt::Reader::new(&bed_entity([0, 0, 0], 99).nbt, Enc::Network).root().unwrap().1;
        assert_eq!(wild.get("color"), Some(&Tag::Byte(0)), "un colore fuori scala diventa bianco");
    }

    #[test]
    fn slab_on_same_slab_becomes_double() {
        let mut f = floor();
        put_block(&mut f, [0, 10, 0], "minecraft:oak_slab");
        // la faccia superiore di una mezza lastra bassa: diventa doppia nello stesso posto
        let changes = place(&click([0, 10, 0], UP, "minecraft:oak_slab"), &f).unwrap();
        assert_eq!(changes.len(), 1);
        let state = state_at(&changes, [0, 10, 0]);
        assert_eq!(
            (state.name.as_str(), text(&state, "minecraft:vertical_half"), changes[0].water),
            ("minecraft:oak_double_slab", "bottom".to_string(), false)
        );
        assert!(blocks().is_valid(&state));
        // dalla faccia inferiore si piazza sotto come un blocco qualsiasi: là c'è il pavimento, quindi niente
        assert!(place(&click([0, 10, 0], DOWN, "minecraft:oak_slab"), &f).is_none());
        // una mezza lastra alta si completa cliccando sotto
        f.blocks.insert([0, 10, 0], top_slab("minecraft:oak_slab"));
        let state = state_at(&place(&click([0, 10, 0], DOWN, "minecraft:oak_slab"), &f).unwrap(), [0, 10, 0]);
        assert_eq!((state.name.as_str(), text(&state, "minecraft:vertical_half")), ("minecraft:oak_double_slab", "top".to_string()));
        // e dall'alto no: si piazza sopra
        assert_eq!(place(&click([0, 10, 0], UP, "minecraft:oak_slab"), &f).unwrap()[0].pos, [0, 11, 0]);
        // un'altra lastra non si fonde: va sopra, nella cella accanto
        f.blocks.insert([0, 10, 0], hash("minecraft:oak_slab"));
        let changes = place(&click([0, 10, 0], UP, "minecraft:spruce_slab"), &f).unwrap();
        assert_eq!((changes[0].pos, blocks().state(changes[0].hash).unwrap().name.as_str()), ([0, 11, 0], "minecraft:spruce_slab"));
        // il blocco cliccato non è sostituibile e accanto c'è una mezza lastra uguale: diventa doppia lì
        let mut beside = floor();
        put_block(&mut beside, [0, 10, 0], "minecraft:oak_slab");
        let changes = place(&click([0, 9, 0], UP, "minecraft:oak_slab"), &beside).unwrap();
        assert_eq!(changes.iter().map(|c| c.pos).collect::<Vec<_>>(), [[0, 10, 0]]);
        assert_eq!(blocks().state(changes[0].hash).unwrap().name, "minecraft:oak_double_slab");
        // una lastra doppia non si fonde con niente
        let mut full = floor();
        put_block(&mut full, [0, 10, 0], "minecraft:oak_double_slab");
        assert_eq!(place(&click([0, 10, 0], UP, "minecraft:oak_slab"), &full).unwrap()[0].pos, [0, 11, 0]);
        // l'acqua della cella non resta in una lastra doppia
        let mut wet = floor();
        put_block(&mut wet, [0, 10, 0], "minecraft:oak_slab");
        wet.water.insert([0, 10, 0]);
        assert!(!place(&click([0, 10, 0], UP, "minecraft:oak_slab"), &wet).unwrap()[0].water);
    }

    #[test]
    fn every_half_slab_has_its_double() {
        let mut checked = 0;
        for name in blocks().names().filter(|n| n.ends_with("_slab") && !n.contains("double_")) {
            let mut f = floor();
            f.blocks.insert([0, 10, 0], hash(name));
            let changes = place(&click([0, 10, 0], UP, name), &f).unwrap_or_else(|| panic!("{name}"));
            let placed = blocks().state(changes[0].hash).unwrap();
            assert!(placed.name.contains("double_"), "{name} -> {}", placed.name);
            assert!(blocks().is_valid(placed), "{name}");
            checked += 1;
        }
        assert!(checked > 100, "{checked} lastre");
        // il rame ha il nome della lastra doppia con "double" davanti al "cut"
        let mut f = floor();
        f.blocks.insert([0, 10, 0], hash("minecraft:waxed_exposed_cut_copper_slab"));
        let changes = place(&click([0, 10, 0], UP, "minecraft:waxed_exposed_cut_copper_slab"), &f).unwrap();
        assert_eq!(blocks().state(changes[0].hash).unwrap().name, "minecraft:waxed_exposed_double_cut_copper_slab");
    }

    #[test]
    fn torch_on_wall_and_floor_refused_on_ceiling() {
        let mut f = floor();
        let facing = |changes: &[BlockChange]| text(&state_at(changes, changes[0].pos), "torch_facing_direction");
        // a terra
        assert_eq!(facing(&place(&on_floor("minecraft:torch"), &f).unwrap()), "top");
        // su un muro: la torcia sta a ovest del muro e il blocco è a est
        put_block(&mut f, [1, 10, 0], "minecraft:stone");
        let changes = place(&click([1, 10, 0], WEST, "minecraft:torch"), &f).unwrap();
        assert_eq!((changes[0].pos, facing(&changes)), ([0, 10, 0], "east".to_string()));
        // sul soffitto no
        put_block(&mut f, [0, 11, 0], "minecraft:stone");
        assert!(place(&click([0, 11, 0], DOWN, "minecraft:torch"), &f).is_none());
        // anche la torcia dell'anima
        assert!(place(&on_floor("minecraft:soul_torch"), &f).is_some());
        // sopra un blocco che non regge si rifiuta: la faccia alta di una lastra bassa, un fiore
        let mut weak = floor();
        put_block(&mut weak, [0, 10, 0], "minecraft:oak_slab");
        assert!(place(&click([0, 10, 0], UP, "minecraft:torch"), &weak).is_none());
        weak.blocks.insert([0, 10, 0], top_slab("minecraft:oak_slab"));
        assert!(place(&click([0, 10, 0], UP, "minecraft:torch"), &weak).is_some(), "una lastra alta sì");
        put_block(&mut weak, [0, 10, 0], "minecraft:poppy");
        assert!(place(&click([0, 10, 0], UP, "minecraft:torch"), &weak).is_none(), "un fiore non regge nulla");
        // un clic su un blocco sostituibile senza niente sotto cerca un appoggio attorno: sud, ovest, nord, est, sotto
        let mut grass = floor();
        put_block(&mut grass, [0, 9, 0], "minecraft:air");
        put_block(&mut grass, [0, 10, 0], "minecraft:short_grass");
        put_block(&mut grass, [-1, 10, 0], "minecraft:stone");
        let changes = place(&click([0, 10, 0], NORTH, "minecraft:torch"), &grass).unwrap();
        assert_eq!((changes[0].pos, facing(&changes)), ([0, 10, 0], "west".to_string()));
        put_block(&mut grass, [0, 10, 1], "minecraft:stone");
        assert_eq!(facing(&place(&click([0, 10, 0], NORTH, "minecraft:torch"), &grass).unwrap()), "south");
        // nessun appoggio: rifiutata
        let mut nothing = floor();
        put_block(&mut nothing, [0, 9, 0], "minecraft:air");
        put_block(&mut nothing, [0, 8, 0], "minecraft:air");
        put_block(&mut nothing, [0, 10, 0], "minecraft:short_grass");
        assert!(place(&click([0, 10, 0], NORTH, "minecraft:torch"), &nothing).is_none());
    }

    #[test]
    fn lever_on_ceiling_axis() {
        let mut f = floor();
        put_block(&mut f, [0, 11, 0], "minecraft:stone");
        put_block(&mut f, [1, 10, 0], "minecraft:stone");
        let direction = |click: PlaceCtx| text(&state_at(&place(&click, &f).unwrap(), click.target()), "lever_direction");
        // soffitto: l'asse dipende da dove guarda il giocatore (sud/nord: nord-sud; ovest/est: est-ovest)
        let ceiling = |yaw: f32| PlaceCtx { yaw, ..click([0, 11, 0], DOWN, "minecraft:lever") };
        assert_eq!([direction(ceiling(0.0)), direction(ceiling(180.0))], ["down_north_south", "down_north_south"]);
        assert_eq!([direction(ceiling(90.0)), direction(ceiling(-90.0))], ["down_east_west", "down_east_west"]);
        // pavimento
        let floor_click = |yaw: f32| PlaceCtx { yaw, ..on_floor("minecraft:lever") };
        assert_eq!([direction(floor_click(0.0)), direction(floor_click(90.0))], ["up_north_south", "up_east_west"]);
        // muro: la faccia cliccata
        assert_eq!(direction(click([1, 10, 0], WEST, "minecraft:lever")), "west");
        // non si attacca a ciò che non regge
        let mut hollow = floor();
        put_block(&mut hollow, [1, 10, 0], "minecraft:glass_pane");
        assert!(place(&click([1, 10, 0], WEST, "minecraft:lever"), &hollow).is_none(), "il vetro sottile non è pieno di lato");
        put_block(&mut hollow, [1, 10, 0], "minecraft:torch");
        assert!(place(&click([1, 10, 0], WEST, "minecraft:lever"), &hollow).is_none());
        // un clic su un blocco sostituibile vale "dall'alto": la leva va sul pavimento in quella cella
        let mut grass = floor();
        put_block(&mut grass, [0, 10, 0], "minecraft:short_grass");
        assert_eq!(text(&state_at(&place(&click([0, 10, 0], NORTH, "minecraft:lever"), &grass).unwrap(), [0, 10, 0]), "lever_direction"), "up_north_south");
    }

    #[test]
    fn lantern_hanging_from_block() {
        let mut f = floor();
        put_block(&mut f, [0, 11, 0], "minecraft:stone");
        let hanging = |c: &PlaceCtx, f: &Fake| int(&state_at(&place(c, f).unwrap(), c.target()), "hanging");
        assert_eq!(hanging(&click([0, 11, 0], DOWN, "minecraft:lantern"), &f), 1, "appesa al soffitto");
        assert_eq!(hanging(&on_floor("minecraft:lantern"), &f), 0, "in piedi");
        assert_eq!(hanging(&on_floor("minecraft:soul_lantern"), &f), 0);
        // una catena regge la lanterna appesa
        let mut chain = floor();
        put_block(&mut chain, [0, 11, 0], "minecraft:iron_chain");
        assert_eq!(hanging(&click([0, 11, 0], DOWN, "minecraft:lantern"), &chain), 1);
        // sotto un blocco che non regge dall'alto si appoggia a terra (se sotto c'è un pavimento)
        let mut chest = floor();
        put_block(&mut chest, [2, 11, 0], "minecraft:chest");
        let changes = place(&click([2, 11, 0], DOWN, "minecraft:lantern"), &chest).unwrap();
        assert_eq!((changes[0].pos, int(&state_at(&changes, [2, 10, 0]), "hanging")), ([2, 10, 0], 0));
        // senza niente sotto si rifiuta
        let mut empty = floor();
        put_block(&mut empty, [0, 9, 0], "minecraft:air");
        put_block(&mut empty, [0, 8, 0], "minecraft:air");
        assert!(place(&on_floor("minecraft:lantern"), &empty).is_none());
        // di lato serve lo stesso l'appoggio sotto
        put_block(&mut empty, [1, 10, 0], "minecraft:stone");
        assert!(place(&click([1, 10, 0], WEST, "minecraft:lantern"), &empty).is_none());
        put_block(&mut f, [1, 10, 0], "minecraft:stone");
        assert_eq!(hanging(&click([1, 10, 0], WEST, "minecraft:lantern"), &f), 0, "con il pavimento sì, e non appesa");
    }

    #[test]
    fn button_attaches_to_face() {
        let mut f = floor();
        put_block(&mut f, [1, 10, 0], "minecraft:stone");
        put_block(&mut f, [0, 11, 0], "minecraft:stone");
        let facing = |c: PlaceCtx| {
            let changes = place(&c, &f).unwrap();
            let state = state_at(&changes, changes[0].pos);
            (changes[0].pos, int(&state, "facing_direction"), int(&state, "button_pressed_bit"))
        };
        assert_eq!(facing(click([1, 10, 0], WEST, "minecraft:stone_button")), ([0, 10, 0], 4, 0), "a muro: la faccia cliccata");
        assert_eq!(facing(on_floor("minecraft:wooden_button")), ([0, 10, 0], 1, 0), "a terra");
        assert_eq!(facing(click([0, 11, 0], DOWN, "minecraft:birch_button")), ([0, 10, 0], 0, 0), "al soffitto");
        // non si attacca a ciò che non regge
        let mut hollow = floor();
        put_block(&mut hollow, [1, 10, 0], "minecraft:fence_gate");
        assert!(place(&click([1, 10, 0], WEST, "minecraft:stone_button"), &hollow).is_none());
        // di lato a una mezza lastra non regge
        let mut slab = floor();
        put_block(&mut slab, [1, 10, 0], "minecraft:oak_slab");
        assert!(place(&click([1, 10, 0], WEST, "minecraft:stone_button"), &slab).is_none());
    }

    #[test]
    fn wall_connects_to_neighbors_and_post() {
        let mut f = floor();
        let wall = |f: &Fake| {
            let changes = place(&on_floor("minecraft:cobblestone_wall"), f).unwrap();
            blocks().state(changes[0].hash).unwrap().clone()
        };
        let sides = |s: &BlockState| ["north", "east", "south", "west"].map(|d| text(s, &format!("wall_connection_type_{d}")));
        // da solo: nessun braccio e il palo
        let alone = wall(&f);
        assert_eq!(sides(&alone), ["none"; 4]);
        assert_eq!(int(&alone, "wall_post_bit"), 1);
        // un blocco pieno a est e uno a ovest: due bracci in linea, senza palo
        put_block(&mut f, [1, 10, 0], "minecraft:stone");
        put_block(&mut f, [-1, 10, 0], "minecraft:stone");
        let line = wall(&f);
        assert_eq!(sides(&line), ["none", "short", "none", "short"]);
        assert_eq!(int(&line, "wall_post_bit"), 0);
        // più un braccio a nord (un altro muro): angolo a T, palo
        put_block(&mut f, [0, 10, -1], "minecraft:cobblestone_wall");
        let tee = wall(&f);
        assert_eq!(sides(&tee), ["short", "short", "none", "short"]);
        assert_eq!(int(&tee, "wall_post_bit"), 1);
        // un angolo (nord ed est) ha il palo
        let mut corner = floor();
        put_block(&mut corner, [0, 10, -1], "minecraft:stone");
        put_block(&mut corner, [1, 10, 0], "minecraft:stone");
        assert_eq!(int(&wall(&corner), "wall_post_bit"), 1);
        // un vetro sottile si collega, un fiore no
        let mut thin = floor();
        put_block(&mut thin, [0, 10, 1], "minecraft:glass_pane");
        put_block(&mut thin, [0, 10, -1], "minecraft:poppy");
        assert_eq!(sides(&wall(&thin)), ["none", "none", "short", "none"]);
        // un blocco sopra allunga i bracci
        put_block(&mut f, [0, 11, 0], "minecraft:stone");
        put_block(&mut f, [0, 10, -1], "minecraft:air");
        assert_eq!(sides(&wall(&f)), ["none", "tall", "none", "tall"]);
        // il cancelletto si collega solo di fianco: a est un cancelletto rivolto a nord sì, rivolto a est no
        let mut gate = floor();
        let gate_facing =
            |dir: &str| blocks().state(hash("minecraft:fence_gate")).unwrap().with("minecraft:cardinal_direction", StateValue::Str(dir.into())).hash();
        gate.blocks.insert([1, 10, 0], gate_facing("north"));
        assert_eq!(sides(&wall(&gate))[1], "short");
        gate.blocks.insert([1, 10, 0], gate_facing("east"));
        assert_eq!(sides(&wall(&gate))[1], "none");
        // una lanterna dritta sopra impone il palo anche con due bracci in linea
        let mut lit = floor();
        put_block(&mut lit, [1, 10, 0], "minecraft:stone");
        put_block(&mut lit, [-1, 10, 0], "minecraft:stone");
        put_block(&mut lit, [0, 11, 0], "minecraft:lantern");
        assert_eq!(int(&wall(&lit), "wall_post_bit"), 1);
    }

    #[test]
    fn stairs_corner_outer_left() {
        let mut f = floor();
        let stairs_facing = |weirdo: i32| blocks().state(hash("minecraft:oak_stairs")).unwrap().with("weirdo_direction", StateValue::Int(weirdo));
        let corner = |f: &Fake| text(&state_at(&place(&on_floor("minecraft:oak_stairs"), f).unwrap(), [0, 10, 0]), "minecraft:corner");
        assert_eq!(corner(&f), "none");
        // il giocatore guarda a sud: davanti è z + 1; a sinistra del sud c'è l'est (weirdo 0): scala davanti rivolta a est = angolo esterno sinistro
        f.blocks.insert([0, 10, 1], stairs_facing(0).hash());
        assert_eq!(corner(&f), "outer_left");
        // rivolta a ovest (weirdo 1): esterno destro
        f.blocks.insert([0, 10, 1], stairs_facing(1).hash());
        assert_eq!(corner(&f), "outer_right");
        // rivolta come la nuova: nessun angolo
        f.blocks.insert([0, 10, 1], stairs_facing(2).hash());
        assert_eq!(corner(&f), "none");
        // dietro (z - 1) rivolta a est: angolo interno sinistro; a ovest: interno destro
        let mut back = floor();
        back.blocks.insert([0, 10, -1], stairs_facing(0).hash());
        assert_eq!(corner(&back), "inner_left");
        back.blocks.insert([0, 10, -1], stairs_facing(1).hash());
        assert_eq!(corner(&back), "inner_right");
        // la fila che continua sul lato opposto cancella l'angolo: davanti a est, a destra (ovest) una scala come la nuova
        f.blocks.insert([0, 10, 1], stairs_facing(0).hash());
        f.blocks.insert([-1, 10, 0], stairs_facing(2).hash());
        assert_eq!(corner(&f), "none", "la fila continua a destra");
        // una scala capovolta davanti non conta
        let mut flipped = floor();
        flipped.blocks.insert([0, 10, 1], stairs_facing(0).with("upside_down_bit", StateValue::Byte(1)).hash());
        assert_eq!(corner(&flipped), "none");
    }

    #[test]
    fn flower_needs_soil() {
        let mut f = floor();
        let on = |soil: &str, plant: &str, f: &mut Fake| {
            f.blocks.insert([0, 9, 0], hash(soil));
            place(&on_floor(plant), f).is_some()
        };
        for soil in ["minecraft:grass_block", "minecraft:dirt", "minecraft:podzol", "minecraft:mud", "minecraft:farmland", "minecraft:coarse_dirt"] {
            assert!(on(soil, "minecraft:poppy", &mut f), "{soil}");
        }
        for soil in ["minecraft:stone", "minecraft:sand", "minecraft:gravel", "minecraft:oak_planks", "minecraft:air"] {
            assert!(!on(soil, "minecraft:poppy", &mut f), "{soil}");
        }
        // l'erba non cresce sulla terra grossa; l'arbusto morto sì su sabbia e argilla cotta
        assert!(on("minecraft:dirt", "minecraft:short_grass", &mut f) && !on("minecraft:coarse_dirt", "minecraft:short_grass", &mut f));
        assert!(on("minecraft:sand", "minecraft:deadbush", &mut f) && on("minecraft:hardened_clay", "minecraft:deadbush", &mut f));
        assert!(!on("minecraft:grass_block", "minecraft:cactus", &mut f), "il cactus vuole la sabbia");
        assert!(on("minecraft:sand", "minecraft:cactus", &mut f));
        // la rosa di wither sta anche sulla sabbia delle anime; gli altri fiori no
        assert!(on("minecraft:soul_sand", "minecraft:wither_rose", &mut f) && !on("minecraft:soul_sand", "minecraft:poppy", &mut f));
        assert!(on("minecraft:grass_block", "minecraft:wither_rose", &mut f));
        assert!(!on("minecraft:oak_log", "minecraft:oak_sapling", &mut f) && on("minecraft:dirt", "minecraft:oak_sapling", &mut f));
        // il cactus rifiuta un blocco accanto
        f.blocks.insert([0, 9, 0], hash("minecraft:sand"));
        f.blocks.insert([1, 10, 0], hash("minecraft:stone"));
        assert!(place(&on_floor("minecraft:cactus"), &f).is_none());
        // piante alte: due celle libere e il terreno sotto la prima
        let mut tall = floor();
        tall.blocks.insert([0, 9, 0], hash("minecraft:grass_block"));
        let changes = place(&on_floor("minecraft:sunflower"), &tall).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!((int(&state_at(&changes, [0, 10, 0]), "upper_block_bit"), int(&state_at(&changes, [0, 11, 0]), "upper_block_bit")), (0, 1));
        for name in ["minecraft:tall_grass", "minecraft:large_fern", "minecraft:rose_bush", "minecraft:lilac", "minecraft:peony"] {
            assert_eq!(place(&on_floor(name), &tall).unwrap_or_else(|| panic!("{name}")).len(), 2, "{name}");
        }
        tall.blocks.insert([0, 11, 0], hash("minecraft:stone"));
        assert!(place(&on_floor("minecraft:sunflower"), &tall).is_none(), "sopra occupato");
        tall.blocks.insert([0, 11, 0], air());
        tall.blocks.insert([0, 9, 0], hash("minecraft:stone"));
        assert!(place(&on_floor("minecraft:sunflower"), &tall).is_none(), "sulla pietra no");
        // un blocco sostituibile (erba) si rimpiazza
        let mut grass = floor();
        grass.blocks.insert([0, 9, 0], hash("minecraft:grass_block"));
        grass.blocks.insert([0, 10, 0], hash("minecraft:short_grass"));
        assert!(place(&click([0, 10, 0], NORTH, "minecraft:poppy"), &grass).is_some());
        // i blocchi a due metà che il server non sa ancora piazzare restano rifiutati
        assert!(place(&on_floor("minecraft:pitcher_plant"), &grass).is_none());
    }

    #[test]
    fn sugar_cane_on_sand() {
        let mut f = floor();
        f.blocks.insert([0, 9, 0], hash("minecraft:sand"));
        assert!(place(&on_floor("minecraft:reeds"), &f).is_none(), "senza acqua accanto");
        // l'acqua come blocco vicino alla sabbia
        f.blocks.insert([1, 9, 0], hash("minecraft:water"));
        assert!(place(&on_floor("minecraft:reeds"), &f).is_some(), "acqua accanto alla sabbia");
        // acqua solo nel secondo strato, dall'altro lato
        let mut layered = floor();
        layered.blocks.insert([0, 9, 0], hash("minecraft:sand"));
        layered.water.insert([0, 9, -1]);
        assert!(place(&on_floor("minecraft:reeds"), &layered).is_some());
        // l'acqua sopra o in diagonale non conta
        let mut far = floor();
        far.blocks.insert([0, 9, 0], hash("minecraft:sand"));
        far.blocks.insert([1, 9, 1], hash("minecraft:water"));
        far.blocks.insert([1, 10, 0], hash("minecraft:water"));
        assert!(place(&on_floor("minecraft:reeds"), &far).is_none());
        // altri terreni
        for (soil, ok) in
            [("minecraft:dirt", true), ("minecraft:grass_block", true), ("minecraft:red_sand", true), ("minecraft:stone", false), ("minecraft:mud", false)]
        {
            let mut g = floor();
            g.blocks.insert([0, 9, 0], hash(soil));
            g.blocks.insert([1, 9, 0], hash("minecraft:water"));
            assert_eq!(place(&on_floor("minecraft:reeds"), &g).is_some(), ok, "{soil}");
        }
        // sopra un'altra canna: il terreno è quello della base
        let mut stack = floor();
        stack.blocks.insert([0, 9, 0], hash("minecraft:sand"));
        stack.blocks.insert([1, 9, 0], hash("minecraft:water"));
        stack.blocks.insert([0, 10, 0], hash("minecraft:reeds"));
        assert!(place(&click([0, 10, 0], UP, "minecraft:reeds"), &stack).is_some());
    }

    #[test]
    fn ladder_needs_wall() {
        let mut f = floor();
        put_block(&mut f, [1, 10, 0], "minecraft:stone");
        let changes = place(&click([1, 10, 0], WEST, "minecraft:ladder"), &f).unwrap();
        assert_eq!((changes[0].pos, int(&state_at(&changes, [0, 10, 0]), "facing_direction")), ([0, 10, 0], 4));
        // non dall'alto né dal basso
        assert!(place(&on_floor("minecraft:ladder"), &f).is_none());
        put_block(&mut f, [0, 11, 0], "minecraft:stone");
        assert!(place(&click([0, 11, 0], DOWN, "minecraft:ladder"), &f).is_none());
        // un clic su un blocco sostituibile vale "dall'alto": rifiutata
        let mut grass = floor();
        put_block(&mut grass, [0, 10, 0], "minecraft:short_grass");
        assert!(place(&click([0, 10, 0], NORTH, "minecraft:ladder"), &grass).is_none());
        // il blocco dietro non regge (un vetro sottile): si cerca un muro attorno alla cella, prima sud poi nord, est, ovest
        let mut pane = floor();
        put_block(&mut pane, [0, 10, 0], "minecraft:glass_pane");
        put_block(&mut pane, [1, 10, 1], "minecraft:stone");
        let changes = place(&click([0, 10, 0], EAST, "minecraft:ladder"), &pane).unwrap();
        assert_eq!((changes[0].pos, int(&state_at(&changes, [1, 10, 0]), "facing_direction")), ([1, 10, 0], 2), "appoggiata al muro a sud: guarda a nord");
        // nessun muro: rifiutata
        let mut bare = floor();
        put_block(&mut bare, [0, 10, 0], "minecraft:glass_pane");
        assert!(place(&click([0, 10, 0], EAST, "minecraft:ladder"), &bare).is_none());
    }

    #[test]
    fn vine_attaches_to_full_blocks_on_the_side() {
        let mut f = floor();
        put_block(&mut f, [1, 10, 0], "minecraft:stone");
        let bits = |c: PlaceCtx, f: &Fake| int(&state_at(&place(&c, f).unwrap(), c.target()), "vine_direction_bits");
        assert_eq!(bits(click([1, 10, 0], WEST, "minecraft:vine"), &f), 8, "il blocco è a est: bit est");
        assert!(place(&click([1, 10, 0], UP, "minecraft:vine"), &f).is_none(), "non da sopra");
        assert!(place(&click([0, 10, 0], WEST, "minecraft:vine"), &f).is_none(), "l'aria non regge");
        put_block(&mut f, [0, 10, 0], "minecraft:vine");
        assert!(place(&click([1, 10, 0], WEST, "minecraft:vine"), &f).is_none(), "non si sovrascrive un rampicante");
    }
}
