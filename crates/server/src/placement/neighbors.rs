//! Dipendenze dai vicini (D§4): chi regge cosa. Dopo ogni modifica del mondo `updates` guarda le celle cambiate e quelle accanto
//! e dice quali blocchi devono sparire (la torcia senza il muro, la metà alta di una porta senza la bassa, il fiore senza terra)
//! o cambiare stato (le connessioni dei muri, gli angoli delle scale). Qui ci sono anche i predicati di appoggio che il
//! piazzamento (`special`) usa per decidere se un blocco può stare in una cella. Nessuna di queste sparizioni lascia oggetti (tappa 1).
//!
//! Senza i modelli dei blocchi la solidità di una faccia si ricava dal nome: ciò che non è in un elenco di "non pieni" è un cubo
//! pieno. È un'approssimazione larga apposta: sbagliare per eccesso di solidità lascia in piedi un blocco, sbagliare per difetto
//! farebbe sparire blocchi che stanno bene.
use super::{
    is_water, name_of,
    rules::{put, Dir, FACE_NAMES, V},
    side, BlockView, DOWN, EAST, NORTH, SOUTH, UP, WEST,
};
use crate::{
    events::BlockChange,
    registry::{blocks, BlockState, StateValue},
};
use std::{collections::BTreeSet, sync::OnceLock};

/// Le quattro facce laterali.
pub(super) const HORIZONTAL: [u8; 4] = [NORTH, SOUTH, WEST, EAST];

/// Famiglia di una pianta, per sapere su che terreno sta (D§4.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plant {
    /// Erba, felce, erba alta, felce grande, arbusto rosso.
    Grass,
    DeadBush,
    /// Fiori e fiori alti.
    Flower,
    WitherRose,
    NetherSprouts,
    PinkPetals,
    SugarCane,
    Bamboo,
    Sapling,
    Cactus,
}

/// La famiglia della pianta con quel nome (senza `minecraft:`); `None` se non è una pianta con requisiti sul terreno.
pub(crate) fn plant_kind(name: &str) -> Option<Plant> {
    Some(match name {
        "short_grass" | "fern" | "tall_grass" | "large_fern" | "red_shrub" => Plant::Grass,
        "deadbush" => Plant::DeadBush,
        "poppy" | "dandelion" | "golden_dandelion" | "blue_orchid" | "allium" | "azure_bluet" | "red_tulip" | "orange_tulip" | "white_tulip" | "pink_tulip"
        | "oxeye_daisy" | "cornflower" | "lily_of_the_valley" | "torchflower" | "open_eyeblossom" | "closed_eyeblossom" | "sunflower" | "lilac"
        | "rose_bush" | "peony" => Plant::Flower,
        "wither_rose" => Plant::WitherRose,
        "nether_sprouts" => Plant::NetherSprouts,
        "pink_petals" => Plant::PinkPetals,
        "reeds" => Plant::SugarCane,
        "bamboo" | "bamboo_sapling" => Plant::Bamboo,
        "cactus" => Plant::Cactus,
        _ if name.ends_with("_sapling") => Plant::Sapling,
        _ => return None,
    })
}

/// Vero se il terreno `soil` regge la pianta (le liste di `SoilFor` di Dragonfly). Il rosa di wither si pianta anche dove si piantano
/// i fiori, ma solo lui sta su sabbia delle anime e netherrack.
fn soil_accepts(soil: &str, plant: Plant) -> bool {
    use Plant::*;
    let plant = match (soil, plant) {
        ("soul_sand" | "netherrack", plant) => return plant == WitherRose,
        (_, WitherRose) => Flower,
        (_, plant) => plant,
    };
    match soil {
        "dirt" | "dirt_with_roots" => plant != Cactus,
        "coarse_dirt" => !matches!(plant, Cactus | Grass | DeadBush),
        "grass_block" => plant != Cactus,
        "podzol" => !matches!(plant, Cactus | PinkPetals),
        "mycelium" => !matches!(plant, Cactus | Sapling),
        "mud" => !matches!(plant, Cactus | SugarCane),
        "muddy_mangrove_roots" => matches!(plant, Grass | Flower | NetherSprouts | PinkPetals | Bamboo | Sapling),
        "farmland" => matches!(plant, Grass | Flower | NetherSprouts | PinkPetals | DeadBush | Sapling),
        "sand" | "red_sand" => matches!(plant, Cactus | DeadBush | SugarCane | Bamboo),
        "gravel" => plant == Bamboo,
        "soul_soil" => plant == NetherSprouts,
        _ if soil == "hardened_clay" || (soil.ends_with("terracotta") && !soil.contains("glazed")) => plant == DeadBush,
        _ => false,
    }
}

/// Nome senza `minecraft:`.
fn short(state: &BlockState) -> &str {
    state.name.strip_prefix("minecraft:").unwrap_or(&state.name)
}

fn int(state: &BlockState, key: &str) -> Option<i32> {
    match state.get(key)? {
        StateValue::Int(v) => Some(*v),
        StateValue::Byte(v) => Some(*v as i32),
        StateValue::Str(_) => None,
    }
}

fn text<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    match state.get(key)? {
        StateValue::Str(v) => Some(v),
        _ => None,
    }
}

fn flag(state: &BlockState, key: &str) -> bool {
    int(state, key) == Some(1)
}

fn air() -> i32 {
    static AIR: OnceLock<i32> = OnceLock::new();
    *AIR.get_or_init(|| blocks().default_hash("minecraft:air").expect("l'aria esiste"))
}

/// Blocchi che non hanno nessuna faccia piena (o che non reggono nulla): fiori, torce, porte, letti...
fn hollow(name: &str) -> bool {
    const EXACT: &[&str] = &[
        "air",
        "water",
        "flowing_water",
        "lava",
        "flowing_lava",
        "fire",
        "soul_fire",
        "web",
        "lever",
        "ladder",
        "vine",
        "redstone_wire",
        "tripwire",
        "tripwire_hook",
        "snow_layer",
        "powder_snow",
        "frame",
        "glow_frame",
        "flower_pot",
        "bed",
        "cake",
        "anvil",
        "chest",
        "trapped_chest",
        "ender_chest",
        "hopper",
        "brewing_stand",
        "enchanting_table",
        "end_rod",
        "lightning_rod",
        "waterlily",
        "kelp",
        "seagrass",
        "brown_mushroom",
        "red_mushroom",
        "sweet_berry_bush",
        "bush",
        "firefly_bush",
        "leaf_litter",
        "wildflowers",
        "short_dry_grass",
        "tall_dry_grass",
        "hanging_roots",
        "crimson_roots",
        "warped_roots",
        "crimson_fungus",
        "warped_fungus",
        "cave_vines",
        "weeping_vines",
        "twisting_vines",
        "campfire",
        "soul_campfire",
    ];
    const SUFFIXES: &[&str] = &[
        "torch",
        "sign",
        "banner",
        "button",
        "rail",
        "carpet",
        "pressure_plate",
        "door",
        "fence_gate",
        "chain",
        "lantern",
        "candle",
        "skull",
        "coral",
        "coral_fan",
        "wall_fan",
        "propagule",
    ];
    // la lanterna di mare è un cubo pieno
    name != "sea_lantern"
        && (EXACT.contains(&name) || SUFFIXES.iter().any(|s| name.ends_with(s)) || name.starts_with("light_block") || plant_kind(name).is_some())
}

/// Vero se la faccia `face` del blocco con quell'hash è piena, cioè può reggere una torcia, un pulsante, una porta...
/// Mezze lastre e scale hanno le facce di Dragonfly (`model.Slab`, `model.Stair`); muri e recinzioni sono pieni su e giù, i vetri
/// sottili solo sotto. Le foglie contano come cubi pieni (vi si appoggiano i rampicanti nel mondo vero).
pub(super) fn solid_face(hash: i32, face: u8) -> bool {
    let Some(state) = blocks().state(hash) else { return false };
    let name = short(state);
    if name.ends_with("slab") && state.get("minecraft:vertical_half").is_some() {
        if name.contains("double_") {
            return true;
        }
        return if text(state, "minecraft:vertical_half") == Some("top") { face == UP } else { face == DOWN };
    }
    if let Some(corner) = text(state, "minecraft:corner") {
        return stairs_face_solid(state, corner, face);
    }
    if state.get("wall_post_bit").is_some() || (name.ends_with("fence") && !hollow(name)) {
        return face <= UP;
    }
    if name.ends_with("pane") || name.ends_with("bars") {
        return face == DOWN;
    }
    !hollow(name)
}

/// Facce piene di una scala (`model.Stair.FaceSolid`): il lato del gradino pieno più, negli angoli interni, quello dell'angolo.
fn stairs_face_solid(state: &BlockState, corner: &str, face: u8) -> bool {
    let upside_down = flag(state, "upside_down_bit");
    if (face == UP && upside_down) || (face == DOWN && !upside_down) {
        return true;
    }
    let Some(facing) = stairs_facing(state) else { return false };
    match corner {
        "none" => face == facing.face(),
        "inner_left" => face == facing.face() || face == facing.rotate_left().face(),
        "inner_right" => face == facing.face() || face == facing.rotate_right().face(),
        _ => false,
    }
}

/// Il lato pieno di una scala: `weirdo_direction` è 3 meno l'indice della direzione di Dragonfly.
fn stairs_facing(state: &BlockState) -> Option<Dir> {
    Dir::from_index(3 - int(state, "weirdo_direction")?)
}

/// Se la faccia `face` del blocco in `pos` è piena; `None` se la cella non è nel mondo caricato.
pub(super) fn solid(view: &impl BlockView, pos: [i32; 3], face: u8) -> Option<bool> {
    Some(solid_face(view.block(pos)?, face))
}

/// Vero se il blocco riempie tutta la cella e non lascia passare la luce: conta come "muro" per la cerniera delle porte.
pub(super) fn opaque_cube(hash: i32) -> bool {
    let transparent = name_of(hash).is_none_or(|n| n.ends_with("glass") || n.contains("leaves") || n.ends_with("ice") || n == "slime" || n == "honey_block");
    !transparent && (0..6).all(|face| solid_face(hash, face))
}

fn is_chain(hash: i32) -> bool {
    name_of(hash).is_some_and(|n| n.ends_with("chain"))
}

/// Vero se la cella ha acqua (come blocco o nel secondo strato); `None` se non è nel mondo caricato.
fn has_water(view: &impl BlockView, pos: [i32; 3]) -> Option<bool> {
    Some(view.water(pos) || is_water(view.block(pos)?))
}

/// Una pila di canne, cactus o bambù: il nome conta come "dello stesso tipo" per camminare lungo la colonna.
fn stacked(name: &str) -> bool {
    matches!(name, "reeds" | "cactus" | "bamboo" | "bamboo_sapling")
}

/// Un blocco che regge qualcosa in senso strutturale: non è aria, liquido o erba sostituibile, né un blocco non pieno noto.
fn structural(hash: i32) -> bool {
    !super::replaceable(hash) && name_of(hash).is_none_or(|n| !hollow(n))
}

/// Vero se la pianta può stare in `pos`; `None` se serve una cella fuori dal mondo caricato. Con `strict` (il piazzamento) il
/// terreno deve essere nella lista di Dragonfly (D§4.6); senza (le reazioni dei vicini, anche su piante nate col mondo) basta che
/// sotto ci sia un blocco che regge, così un fiore sul muschio non sparisce per un vicino. Per la canna conta anche l'acqua accanto al
/// terreno, per il cactus l'aria ai quattro lati; per canna, cactus e bambù il terreno è quello della base della pila.
pub(super) fn plant_supported(view: &impl BlockView, pos: [i32; 3], plant: Plant, strict: bool) -> Option<bool> {
    // quanto può essere alta una pila prima di arrendersi (il mondo è alto 384 blocchi)
    const STACK: usize = 400;
    // la base della pila che comincia sotto `pos`: la cella più in basso che ha ancora un blocco dello stesso tipo sotto di sé
    let base_of = |same: fn(&str) -> bool| -> Option<[i32; 3]> {
        let mut base = pos;
        for _ in 0..STACK {
            if !name_of(view.block(side(base, DOWN))?).is_some_and(same) {
                break;
            }
            base = side(base, DOWN);
        }
        Some(base)
    };
    // il terreno sotto la cella `p` regge la pianta
    let holds = |p: [i32; 3]| -> Option<bool> {
        let below = view.block(side(p, DOWN))?;
        Some(if strict { name_of(below).is_some_and(|n| soil_accepts(n, plant)) } else { structural(below) })
    };
    match plant {
        Plant::SugarCane => {
            let base = base_of(|n| n == "reeds")?;
            if !holds(base)? {
                return Some(false);
            }
            // l'acqua deve toccare il terreno su cui poggia la base
            let soil_pos = side(base, DOWN);
            let mut unknown = false;
            for face in HORIZONTAL {
                match has_water(view, side(soil_pos, face)) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unknown = true,
                }
            }
            (!unknown).then_some(false)
        }
        Plant::Cactus => {
            // ogni cella della pila ha l'aria ai quattro lati
            let base = base_of(|n| n == "cactus")?;
            let mut cell = pos;
            loop {
                for face in HORIZONTAL {
                    if name_of(view.block(side(cell, face))?) != Some("air") {
                        return Some(false);
                    }
                }
                if cell == base {
                    break;
                }
                cell = side(cell, DOWN);
            }
            holds(base)
        }
        Plant::Bamboo => holds(base_of(|n| matches!(n, "bamboo" | "bamboo_sapling"))?),
        _ => holds(pos),
    }
}

// ---------- muri ----------

/// Direzioni laterali nell'ordine delle chiavi dello stato del muro.
const WALL_SIDES: [(Dir, &str); 4] = [
    (Dir::North, "wall_connection_type_north"),
    (Dir::East, "wall_connection_type_east"),
    (Dir::South, "wall_connection_type_south"),
    (Dir::West, "wall_connection_type_west"),
];

fn is_wall(state: &BlockState) -> bool {
    state.get("wall_post_bit").is_some()
}

/// Come `wall.go:calculateConnections`: a ogni lato il muro si collega se di fronte c'è una faccia piena, un altro muro, un
/// cancelletto di fianco o un vetro sottile; il braccio è alto se sopra c'è qualcosa che parte da terra e lo copre.
/// Dove la cella accanto non è nel mondo caricato resta il valore che c'era.
fn wall_connection(view: &impl BlockView, pos: [i32; 3], dir: Dir, current: &str) -> String {
    let at = side(pos, dir.face());
    let Some(hash) = view.block(at) else { return current.to_string() };
    let Some(state) = blocks().state(hash) else { return "none".to_string() };
    let name = short(state);
    let connected = solid_face(hash, dir.opposite().face())
        || is_wall(state)
        || (name.ends_with("fence_gate")
            && text(state, "minecraft:cardinal_direction").and_then(dir_from_name).is_some_and(|facing| facing.is_x() != dir.is_x()))
        || name.ends_with("pane")
        || name.ends_with("bars");
    if !connected {
        return "none".to_string();
    }
    let tall = view.block(side(pos, UP)).is_some_and(|above| tall_over(above, dir));
    if tall { "tall" } else { "short" }.to_string()
}

fn dir_from_name(name: &str) -> Option<Dir> {
    [Dir::North, Dir::South, Dir::West, Dir::East].into_iter().find(|d| d.name() == name)
}

/// Se il blocco sopra un muro lo allunga da quel lato (la sua forma parte da terra e copre il braccio). Approssimato: un cubo
/// pieno, una mezza lastra bassa o doppia o una scala dritta coprono tutti i lati, un altro muro solo dove ha un braccio.
fn tall_over(above: i32, dir: Dir) -> bool {
    let Some(state) = blocks().state(above) else { return false };
    if is_wall(state) {
        return WALL_SIDES.iter().find(|(d, _)| *d == dir).is_some_and(|(_, key)| text(state, key).is_some_and(|c| c != "none"));
    }
    if text(state, "minecraft:corner").is_some() {
        return !flag(state, "upside_down_bit");
    }
    if short(state).ends_with("slab") && state.get("minecraft:vertical_half").is_some() {
        return short(state).contains("double_") || text(state, "minecraft:vertical_half") == Some("bottom");
    }
    (0..6).all(|face| solid_face(above, face))
}

/// Come `wall.go:calculatePost`: il palo si vede se il muro ha meno di due bracci o se non sono in linea; ciò che sta sopra
/// (lanterna dritta, torcia in piedi, cartello, un altro palo) lo impone.
fn wall_post(view: &impl BlockView, pos: [i32; 3], connections: &[bool; 4]) -> bool {
    let forced = view.block(side(pos, UP)).and_then(|h| blocks().state(h)).is_some_and(|above| {
        let name = short(above);
        if name.ends_with("lantern") && above.get("hanging").is_some() {
            !flag(above, "hanging")
        } else if above.get("torch_facing_direction").is_some() {
            text(above, "torch_facing_direction") == Some("top")
        } else if name.contains("standing_sign") || name.contains("wall_sign") {
            true
        } else {
            is_wall(above) && flag(above, "wall_post_bit")
        }
    });
    if forced {
        return true;
    }
    let [north, east, south, west] = *connections;
    if connections.iter().filter(|&&c| c).count() < 2 {
        true
    } else if north && south {
        east || west
    } else if east && west {
        north || south
    } else {
        true
    }
}

/// Lo stato del muro `state` in `pos` con connessioni e palo ricalcolati.
fn settled_wall(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> BlockState {
    let mut next = state.clone();
    let mut connected = [false; 4];
    for (i, (dir, key)) in WALL_SIDES.iter().enumerate() {
        let value = wall_connection(view, pos, *dir, text(state, key).unwrap_or("none"));
        connected[i] = value != "none";
        put(&mut next, key, V::S(&value));
    }
    put(&mut next, "wall_post_bit", V::N(wall_post(view, pos, &connected) as i32));
    next
}

// ---------- scale ----------

/// Come `stairs.go:calculateCorner`: la scala si piega se davanti o dietro c'è una scala di lato, a meno che dal lato opposto
/// la fila non continui. Se una cella attorno non è nel mondo caricato resta l'angolo che c'era.
fn stairs_corner(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> String {
    let current = text(state, "minecraft:corner").unwrap_or("none").to_string();
    let (Some(facing), upside_down) = (stairs_facing(state), flag(state, "upside_down_bit")) else { return current };
    let (left, right) = (facing.rotate_left(), facing.rotate_right());
    let near = |dir: Dir| -> Option<Option<(Dir, i32)>> {
        let hash = view.block(side(pos, dir.face()))?;
        Some(
            blocks()
                .state(hash)
                .filter(|s| text(s, "minecraft:corner").is_some() && flag(s, "upside_down_bit") == upside_down)
                .and_then(|s| stairs_facing(s).map(|f| (f, hash))),
        )
    };
    let (Some(front), Some(back), Some(at_left), Some(at_right)) = (near(facing), near(facing.opposite()), near(left), near(right)) else { return current };
    // la fila continua da quel lato se lì c'è una scala dello stesso tipo, rivolta allo stesso modo
    let continued = |neighbor: Option<(Dir, i32)>| neighbor.is_some_and(|(f, hash)| f == facing && name_of(hash) == Some(short(state)));
    let (continued_left, continued_right) = (continued(at_left), continued(at_right));
    if let Some((f, _)) = front {
        if f == left && !continued_right {
            return "outer_left".into();
        } else if f == right && !continued_left {
            return "outer_right".into();
        }
    }
    if let Some((f, _)) = back {
        if f == left && !continued_left {
            return "inner_left".into();
        } else if f == right && !continued_right {
            return "inner_right".into();
        }
    }
    "none".into()
}

/// Lo stato della scala `state` in `pos` con l'angolo ricalcolato.
fn settled_stairs(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> BlockState {
    let mut next = state.clone();
    put(&mut next, "minecraft:corner", V::S(&stairs_corner(view, pos, state)));
    next
}

/// Vero se `state` è un blocco il cui stato dipende dai vicini (muro o scala): `settled` lo ricalcola.
pub(super) fn depends_on_neighbors(state: &BlockState) -> bool {
    is_wall(state) || text(state, "minecraft:corner").is_some()
}

/// Lo stato di `hash` in `pos` con le parti che dipendono dai vicini ricalcolate (muri, scale); per gli altri blocchi `hash`.
pub(super) fn settled(view: &impl BlockView, pos: [i32; 3], hash: i32) -> i32 {
    match blocks().state(hash) {
        Some(state) if is_wall(state) => settled_wall(view, pos, state).hash(),
        Some(state) if text(state, "minecraft:corner").is_some() => settled_stairs(view, pos, state).hash(),
        _ => hash,
    }
}

// ---------- sopravvivenza ----------

/// Cosa succede a un blocco dopo che un vicino è cambiato.
enum Fate {
    Stay,
    /// Sparisce, senza oggetti.
    Gone,
    Becomes(i32),
}

/// La faccia verso cui guarda uno stato che porta un indice di faccia (scala a pioli, pulsante).
fn facing_face(state: &BlockState) -> Option<u8> {
    int(state, "facing_direction").and_then(|f| u8::try_from(f).ok()).filter(|&f| f <= EAST)
}

/// Il blocco in `pos` regge un oggetto attaccato dalla faccia `face` (il supporto sta dalla parte opposta, a `pos.side(face ^ 1)`).
fn attached(view: &impl BlockView, pos: [i32; 3], face: u8) -> Option<bool> {
    solid(view, side(pos, face ^ 1), face)
}

fn torch(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> Option<bool> {
    // il valore dice da che parte sta il blocco che la regge ("top": sotto, sulla faccia superiore)
    let toward = match text(state, "torch_facing_direction")? {
        "top" => DOWN,
        "unknown" => return None,
        direction => FACE_NAMES.iter().position(|n| *n == direction).map(|i| i as u8)?,
    };
    solid(view, side(pos, toward), toward ^ 1)
}

fn lever(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> Option<bool> {
    // "up_north_south", "down_east_west" o la faccia sola ("east"): conta la prima parola
    let head = text(state, "lever_direction")?.split('_').next()?;
    let face = FACE_NAMES.iter().position(|n| *n == head)? as u8;
    attached(view, pos, face)
}

fn lantern(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> Option<bool> {
    if flag(state, "hanging") {
        let up = side(pos, UP);
        Some(is_chain(view.block(up)?) || solid(view, up, DOWN)?)
    } else {
        solid(view, side(pos, DOWN), UP)
    }
}

fn door(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> Option<bool> {
    let partner = |p: [i32; 3], upper: bool| -> Option<bool> {
        let other = blocks().state(view.block(p)?);
        Some(other.is_some_and(|o| o.name == state.name && flag(o, "upper_block_bit") == upper))
    };
    if flag(state, "upper_block_bit") {
        partner(side(pos, DOWN), false)
    } else {
        Some(solid(view, side(pos, DOWN), UP)? && partner(side(pos, UP), true)?)
    }
}

fn bed(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> Option<bool> {
    let facing = match int(state, "direction")? {
        0 => Dir::South,
        1 => Dir::West,
        2 => Dir::North,
        _ => Dir::East,
    };
    let head = flag(state, "head_piece_bit");
    // la testa sta davanti ai piedi
    let other = side(pos, if head { facing.opposite().face() } else { facing.face() });
    Some(blocks().state(view.block(other)?).is_some_and(|o| o.name == state.name && flag(o, "head_piece_bit") != head))
}

fn double_plant(view: &impl BlockView, pos: [i32; 3], state: &BlockState, plant: Plant) -> Option<bool> {
    let partner = |p: [i32; 3], upper: bool| -> Option<bool> {
        let other = blocks().state(view.block(p)?);
        Some(other.is_some_and(|o| o.name == state.name && flag(o, "upper_block_bit") == upper))
    };
    if flag(state, "upper_block_bit") {
        partner(side(pos, DOWN), false)
    } else {
        Some(partner(side(pos, UP), true)? && plant_supported(view, pos, plant, false)?)
    }
}

/// Il rampicante perde i lati senza un cubo pieno (a meno che il rampicante sopra non li tenga) e sparisce senza lati.
/// Un rampicante senza lati (appeso solo a un soffitto) non si tocca.
fn vine(view: &impl BlockView, pos: [i32; 3], state: &BlockState) -> Option<Fate> {
    let bits = int(state, "vine_direction_bits")?;
    let above = blocks().state(view.block(side(pos, UP))?).filter(|s| short(s) == "vine").and_then(|s| int(s, "vine_direction_bits")).unwrap_or(0);
    let mut kept = bits;
    // un bit per lato: sud 1, ovest 2, nord 4, est 8
    for (bit, face) in [(1, SOUTH), (2, WEST), (4, NORTH), (8, EAST)] {
        if bits & bit != 0 && above & bit == 0 && !solid(view, side(pos, face), face ^ 1)? {
            kept &= !bit;
        }
    }
    Some(match kept {
        k if k == bits => Fate::Stay,
        0 => Fate::Gone,
        k => Fate::Becomes(state.with("vine_direction_bits", StateValue::Int(k)).hash()),
    })
}

/// Cosa succede al blocco `hash` in `pos` dopo il cambio di un vicino. Una cella ai margini del mondo caricato lascia il blocco com'è.
fn fate(view: &impl BlockView, pos: [i32; 3], hash: i32) -> Fate {
    let Some(state) = blocks().state(hash) else { return Fate::Stay };
    let name = short(state);
    let keep = |supported: Option<bool>| if supported.unwrap_or(true) { Fate::Stay } else { Fate::Gone };
    if state.get("torch_facing_direction").is_some() {
        keep(torch(view, pos, state))
    } else if name == "lever" {
        keep(lever(view, pos, state))
    } else if (name.ends_with("button") && state.get("button_pressed_bit").is_some()) || name == "ladder" {
        keep(facing_face(state).and_then(|face| attached(view, pos, face)))
    } else if name.ends_with("lantern") && state.get("hanging").is_some() {
        keep(lantern(view, pos, state))
    } else if state.get("upper_block_bit").is_some() && state.get("door_hinge_bit").is_some() {
        keep(door(view, pos, state))
    } else if name == "bed" {
        keep(bed(view, pos, state))
    } else if let Some(plant) = plant_kind(name) {
        if state.get("upper_block_bit").is_some() {
            keep(double_plant(view, pos, state, plant))
        } else {
            keep(plant_supported(view, pos, plant, false))
        }
    } else if name == "vine" {
        vine(view, pos, state).unwrap_or(Fate::Stay)
    } else if depends_on_neighbors(state) {
        match settled(view, pos, hash) {
            next if next == hash => Fate::Stay,
            next => Fate::Becomes(next),
        }
    } else {
        Fate::Stay
    }
}

/// I blocchi che devono cambiare o sparire perché è cambiato qualcosa nelle celle `changed`: si guardano quelle celle e le sei
/// accanto. Le sparizioni lasciano l'acqua del secondo strato. Il chiamante applica il risultato e richiama per i giri successivi.
pub(crate) fn updates(changed: &[[i32; 3]], view: &impl BlockView) -> Vec<BlockChange> {
    let mut cells = BTreeSet::new();
    for &cell in changed {
        cells.insert(cell);
        cells.extend((0..=EAST).map(|face| side(cell, face)));
    }
    // una pila di canne, cactus o bambù regge o cade tutta insieme: si guardano anche le celle uguali sopra quelle già in esame
    // (nei due giri a disposizione una colonna alta non arriverebbe in fondo un vicino alla volta)
    for pos in cells.clone() {
        let mut above = pos;
        while view.block(above).and_then(name_of).is_some_and(stacked) {
            above = side(above, UP);
            if !view.block(above).and_then(name_of).is_some_and(stacked) || !cells.insert(above) {
                break;
            }
        }
    }
    cells
        .into_iter()
        .filter_map(|pos| {
            let hash = view.block(pos)?;
            let next = match fate(view, pos, hash) {
                Fate::Stay => return None,
                Fate::Gone => air(),
                Fate::Becomes(next) => next,
            };
            (next != hash).then(|| BlockChange { pos, hash: next, water: view.water(pos) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{
        fake::{air, click, floor, hash, Fake},
        place,
    };

    /// Quanti giri servono al massimo nei test (come `NEIGHBOR_ROUNDS` del server).
    const ROUNDS: usize = 2;

    /// Cambia le celle e fa reagire i vicini come `Server::set_blocks`: restituisce tutto ciò che è cambiato per reazione.
    fn set(f: &mut Fake, changes: &[BlockChange]) -> Vec<BlockChange> {
        f.apply(changes);
        let (mut cells, mut reactions): (Vec<[i32; 3]>, Vec<BlockChange>) = (changes.iter().map(|c| c.pos).collect(), Vec::new());
        for _ in 0..ROUNDS {
            let extra = updates(&cells, f);
            if extra.is_empty() {
                break;
            }
            f.apply(&extra);
            cells = extra.iter().map(|c| c.pos).collect();
            reactions.extend(extra);
        }
        reactions
    }

    fn remove(f: &mut Fake, pos: [i32; 3]) -> Vec<BlockChange> {
        let water = f.water.contains(&pos);
        set(f, &[BlockChange { pos, hash: air(), water }])
    }

    fn put_all(f: &mut Fake, pos: [i32; 3], name: &str) -> Vec<BlockChange> {
        set(f, &[BlockChange { pos, hash: hash(name), water: false }])
    }

    /// Piazza `name` cliccando la faccia `face` del blocco `at` e applica il risultato con i suoi vicini.
    fn build(f: &mut Fake, at: [i32; 3], face: u8, name: &str) {
        let changes = place(&click(at, face, name), f).unwrap_or_else(|| panic!("{name} non si piazza"));
        set(f, &changes);
    }

    fn text(f: &Fake, pos: [i32; 3], key: &str) -> String {
        match blocks().state(f.blocks[&pos]).unwrap().get(key) {
            Some(StateValue::Str(v)) => v.clone(),
            other => panic!("{key}: {other:?}"),
        }
    }

    fn int(f: &Fake, pos: [i32; 3], key: &str) -> i32 {
        match blocks().state(f.blocks[&pos]).unwrap().get(key) {
            Some(StateValue::Int(v)) => *v,
            Some(StateValue::Byte(v)) => *v as i32,
            other => panic!("{key}: {other:?}"),
        }
    }

    #[test]
    fn torch_breaks_when_support_removed() {
        let mut f = floor();
        build(&mut f, [0, 9, 0], crate::placement::UP, "minecraft:torch");
        assert_eq!(f.name([0, 10, 0]), "torch");
        // un blocco che cambia accanto, ma che non la regge, non la tocca
        assert!(put_all(&mut f, [3, 10, 3], "minecraft:stone").is_empty());
        assert_eq!(f.name([0, 10, 0]), "torch");
        // via il pavimento: la torcia sparisce
        let gone = remove(&mut f, [0, 9, 0]);
        assert_eq!(gone, [BlockChange { pos: [0, 10, 0], hash: air(), water: false }]);
        assert_eq!(f.name([0, 10, 0]), "air");
        // una torcia a muro regge finché c'è il muro; con un altro blocco al suo posto resta
        let mut wall = floor();
        put_all(&mut wall, [1, 10, 0], "minecraft:stone");
        build(&mut wall, [1, 10, 0], crate::placement::WEST, "minecraft:torch");
        assert!(remove(&mut wall, [0, 9, 0]).is_empty(), "il pavimento sotto non la regge");
        assert!(put_all(&mut wall, [1, 10, 0], "minecraft:cobblestone").is_empty());
        assert_eq!(wall.name([0, 10, 0]), "torch");
        assert_eq!(remove(&mut wall, [1, 10, 0]).len(), 1);
        assert_eq!(wall.name([0, 10, 0]), "air");
        // in acqua: sparisce ma l'acqua resta
        let mut wet = floor();
        build(&mut wet, [0, 9, 0], crate::placement::UP, "minecraft:torch");
        wet.water.insert([0, 10, 0]);
        let gone = remove(&mut wet, [0, 9, 0]);
        assert_eq!(gone, [BlockChange { pos: [0, 10, 0], hash: air(), water: true }]);
    }

    #[test]
    fn lever_button_lantern_ladder_fall_without_support() {
        for (name, face) in [
            ("minecraft:lever", crate::placement::UP),
            ("minecraft:stone_button", crate::placement::UP),
            ("minecraft:lantern", crate::placement::UP),
            ("minecraft:soul_lantern", crate::placement::UP),
        ] {
            let mut f = floor();
            build(&mut f, [0, 9, 0], face, name);
            assert_ne!(f.name([0, 10, 0]), "air", "{name}");
            remove(&mut f, [0, 9, 0]);
            assert_eq!(f.name([0, 10, 0]), "air", "{name}");
        }
        // lanterna appesa: via il soffitto
        let mut f = floor();
        put_all(&mut f, [0, 11, 0], "minecraft:stone");
        build(&mut f, [0, 11, 0], crate::placement::DOWN, "minecraft:lantern");
        assert_eq!(int(&f, [0, 10, 0], "hanging"), 1);
        remove(&mut f, [0, 11, 0]);
        assert_eq!(f.name([0, 10, 0]), "air");
        // scala a pioli e pulsante a muro
        let mut wall = floor();
        put_all(&mut wall, [1, 10, 0], "minecraft:stone");
        build(&mut wall, [1, 10, 0], crate::placement::WEST, "minecraft:ladder");
        build(&mut wall, [1, 10, 0], crate::placement::NORTH, "minecraft:stone_button");
        assert_eq!((wall.name([0, 10, 0]), wall.name([1, 10, -1])), ("ladder".into(), "stone_button".into()));
        remove(&mut wall, [1, 10, 0]);
        assert_eq!((wall.name([0, 10, 0]), wall.name([1, 10, -1])), ("air".into(), "air".into()));
    }

    #[test]
    fn door_upper_half_removed_with_lower() {
        let mut f = floor();
        build(&mut f, [0, 9, 0], crate::placement::UP, "minecraft:wooden_door");
        assert_eq!((f.name([0, 10, 0]), f.name([0, 11, 0])), ("wooden_door".into(), "wooden_door".into()));
        // la porta intera sta in piedi dopo il piazzamento, e un vicino qualsiasi non la tocca
        assert!(put_all(&mut f, [2, 10, 0], "minecraft:stone").is_empty());
        // rotta la metà bassa: la alta sparisce al primo giro
        let reactions = remove(&mut f, [0, 10, 0]);
        assert_eq!(reactions, [BlockChange { pos: [0, 11, 0], hash: air(), water: false }]);
        // rotta la metà alta: la bassa sparisce
        let mut g = floor();
        build(&mut g, [0, 9, 0], crate::placement::UP, "minecraft:wooden_door");
        remove(&mut g, [0, 11, 0]);
        assert_eq!((g.name([0, 10, 0]), g.name([0, 11, 0])), ("air".into(), "air".into()));
        // via il pavimento: la bassa sparisce al primo giro, la alta al secondo
        let mut h = floor();
        build(&mut h, [0, 9, 0], crate::placement::UP, "minecraft:wooden_door");
        let reactions = remove(&mut h, [0, 9, 0]);
        assert_eq!(reactions.iter().map(|c| c.pos).collect::<Vec<_>>(), [[0, 10, 0], [0, 11, 0]]);
        assert_eq!((h.name([0, 10, 0]), h.name([0, 11, 0])), ("air".into(), "air".into()));
    }

    #[test]
    fn bed_loses_both_halves_together() {
        let mut f = floor();
        build(&mut f, [0, 9, 0], crate::placement::UP, "minecraft:bed");
        assert_eq!((f.name([0, 10, 0]), f.name([0, 10, 1])), ("bed".into(), "bed".into()));
        assert!(put_all(&mut f, [2, 10, 0], "minecraft:stone").is_empty());
        remove(&mut f, [0, 10, 1]);
        assert_eq!(f.name([0, 10, 0]), "air", "senza la testa spariscono anche i piedi");
        let mut g = floor();
        build(&mut g, [0, 9, 0], crate::placement::UP, "minecraft:bed");
        remove(&mut g, [0, 10, 0]);
        assert_eq!(g.name([0, 10, 1]), "air", "e viceversa");
    }

    #[test]
    fn wall_updates_when_neighbor_placed() {
        let mut f = floor();
        build(&mut f, [0, 9, 0], crate::placement::UP, "minecraft:cobblestone_wall");
        assert_eq!((text(&f, [0, 10, 0], "wall_connection_type_east"), int(&f, [0, 10, 0], "wall_post_bit")), ("none".into(), 1));
        // un blocco pieno a est: il muro si collega
        let reactions = put_all(&mut f, [1, 10, 0], "minecraft:stone");
        assert_eq!(reactions.len(), 1);
        assert_eq!(text(&f, [0, 10, 0], "wall_connection_type_east"), "short");
        // un secondo muro a ovest: due bracci in linea, senza palo
        let mut g = floor();
        build(&mut g, [0, 9, 0], crate::placement::UP, "minecraft:cobblestone_wall");
        build(&mut g, [-1, 9, 0], crate::placement::UP, "minecraft:cobblestone_wall");
        assert_eq!(text(&g, [0, 10, 0], "wall_connection_type_west"), "short", "il muro di prima si è collegato al nuovo");
        assert_eq!(text(&g, [-1, 10, 0], "wall_connection_type_east"), "short", "e il nuovo a lui");
        put_all(&mut g, [1, 10, 0], "minecraft:stone");
        assert_eq!(int(&g, [0, 10, 0], "wall_post_bit"), 0, "est e ovest in linea");
        // tolto il blocco, il braccio sparisce e il palo torna
        remove(&mut g, [1, 10, 0]);
        assert_eq!(text(&g, [0, 10, 0], "wall_connection_type_east"), "none");
        assert_eq!(int(&g, [0, 10, 0], "wall_post_bit"), 1);
        // un blocco sopra allunga i bracci: tall
        put_all(&mut g, [0, 11, 0], "minecraft:stone");
        assert_eq!(text(&g, [0, 10, 0], "wall_connection_type_west"), "tall");
        // un blocco in diagonale non cambia nulla
        assert!(put_all(&mut g, [1, 10, 1], "minecraft:stone").is_empty());
    }

    #[test]
    fn stairs_corner_follows_neighbors() {
        let mut f = floor();
        build(&mut f, [0, 9, 0], crate::placement::UP, "minecraft:oak_stairs"); // rivolta a sud (weirdo 2)
        assert_eq!(text(&f, [0, 10, 0], "minecraft:corner"), "none");
        // una scala rivolta a est piazzata davanti (z + 1): quella di prima diventa un angolo esterno sinistro
        let east = blocks().state(hash("minecraft:oak_stairs")).unwrap().with("weirdo_direction", StateValue::Int(0)).hash();
        let reactions = set(&mut f, &[BlockChange { pos: [0, 10, 1], hash: east, water: false }]);
        assert_eq!(reactions.len(), 1);
        assert_eq!(text(&f, [0, 10, 0], "minecraft:corner"), "outer_left");
        // tolta, torna dritta
        remove(&mut f, [0, 10, 1]);
        assert_eq!(text(&f, [0, 10, 0], "minecraft:corner"), "none");
    }

    #[test]
    fn plants_die_without_their_soil() {
        let mut f = floor();
        put_all(&mut f, [0, 9, 0], "minecraft:grass_block");
        build(&mut f, [0, 9, 0], crate::placement::UP, "minecraft:poppy");
        assert_eq!(f.name([0, 10, 0]), "poppy");
        assert!(put_all(&mut f, [1, 10, 0], "minecraft:stone").is_empty(), "un vicino non la tocca");
        remove(&mut f, [0, 9, 0]);
        assert_eq!(f.name([0, 10, 0]), "air");
        // la terra che diventa pietra regge ancora (le reazioni chiedono solo un appoggio vero); diventando un blocco che non regge no
        let mut g = floor();
        put_all(&mut g, [0, 9, 0], "minecraft:dirt");
        build(&mut g, [0, 9, 0], crate::placement::UP, "minecraft:oak_sapling");
        put_all(&mut g, [0, 9, 0], "minecraft:stone");
        assert_eq!(g.name([0, 10, 0]), "oak_sapling");
        put_all(&mut g, [0, 9, 0], "minecraft:torch");
        assert_eq!(g.name([0, 10, 0]), "air");
        // piante alte: via la metà bassa sparisce l'alta, e viceversa; senza terra spariscono tutt'e due (secondo giro)
        for (gone, kept) in [([0, 10, 0], [0, 11, 0]), ([0, 11, 0], [0, 10, 0])] {
            let mut t = floor();
            put_all(&mut t, [0, 9, 0], "minecraft:grass_block");
            build(&mut t, [0, 9, 0], crate::placement::UP, "minecraft:sunflower");
            remove(&mut t, gone);
            assert_eq!(t.name(kept), "air");
        }
        let mut t = floor();
        put_all(&mut t, [0, 9, 0], "minecraft:grass_block");
        build(&mut t, [0, 9, 0], crate::placement::UP, "minecraft:tall_grass");
        remove(&mut t, [0, 9, 0]);
        assert_eq!((t.name([0, 10, 0]), t.name([0, 11, 0])), ("air".into(), "air".into()));
        // canna: sparisce se l'acqua viene tolta, e la pila si regge sulla base
        let mut c = floor();
        put_all(&mut c, [0, 9, 0], "minecraft:sand");
        put_all(&mut c, [1, 9, 0], "minecraft:water");
        build(&mut c, [0, 9, 0], crate::placement::UP, "minecraft:reeds");
        build(&mut c, [0, 10, 0], crate::placement::UP, "minecraft:reeds");
        assert_eq!((c.name([0, 10, 0]), c.name([0, 11, 0])), ("reeds".into(), "reeds".into()));
        // l'acqua tolta non è un vicino della canna: nessuna reazione (in Dragonfly se ne accorge solo a un tick casuale)
        assert!(remove(&mut c, [1, 9, 0]).is_empty());
        // la sabbia tolta: sparisce la canna sopra, e al secondo giro quella sopra ancora
        remove(&mut c, [0, 9, 0]);
        assert_eq!((c.name([0, 10, 0]), c.name([0, 11, 0])), ("air".into(), "air".into()), "al secondo giro anche la canna sopra");
        // il cactus sparisce se gli si mette un blocco accanto
        let mut k = floor();
        put_all(&mut k, [0, 9, 0], "minecraft:sand");
        build(&mut k, [0, 9, 0], crate::placement::UP, "minecraft:cactus");
        put_all(&mut k, [1, 10, 0], "minecraft:stone");
        assert_eq!(k.name([0, 10, 0]), "air");
    }

    /// Alza una colonna di `name` di `height` celle sopra `soil_pos`, direttamente nel mondo (senza piazzarla).
    fn column(f: &mut Fake, soil_pos: [i32; 3], name: &str, height: i32) {
        for y in 1..=height {
            put_all(f, [soil_pos[0], soil_pos[1] + y, soil_pos[2]], name);
        }
    }

    #[test]
    fn tall_stacks_fall_together() {
        // canna da tre, cactus da tre, bambù da cinque: tolto il terreno cade tutta la colonna, non solo le prime due celle
        for (soil, name, height) in
            [("minecraft:sand", "minecraft:reeds", 3), ("minecraft:sand", "minecraft:cactus", 3), ("minecraft:dirt", "minecraft:bamboo", 5)]
        {
            let mut f = floor();
            put_all(&mut f, [0, 9, 0], soil);
            put_all(&mut f, [1, 9, 0], "minecraft:water");
            column(&mut f, [0, 9, 0], name, height);
            assert_eq!(f.name([0, 9 + height, 0]), name.trim_start_matches("minecraft:"), "{name}: la colonna c'è");
            // un vicino che cambia non la tocca
            assert!(put_all(&mut f, [3, 10, 3], "minecraft:stone").is_empty(), "{name}");
            let gone = remove(&mut f, [0, 9, 0]);
            assert_eq!(gone.len() as i32, height, "{name}: tutte le celle");
            for y in 10..10 + height {
                assert_eq!(f.name([0, y, 0]), "air", "{name} in y {y}");
            }
        }
        // spezzata nel mezzo: restano quelle sotto, cadono quelle sopra (anche oltre la prima sopra)
        let mut f = floor();
        put_all(&mut f, [0, 9, 0], "minecraft:dirt");
        column(&mut f, [0, 9, 0], "minecraft:bamboo", 5);
        remove(&mut f, [0, 11, 0]);
        assert_eq!(
            (f.name([0, 10, 0]), f.name([0, 12, 0]), f.name([0, 13, 0]), f.name([0, 14, 0])),
            ("bamboo".into(), "air".into(), "air".into(), "air".into())
        );
    }

    #[test]
    fn plants_from_the_world_survive_unusual_soil() {
        // un fiore nato col mondo su muschio (non è nelle liste di piazzamento): resta quando un vicino cambia...
        let mut f = floor();
        put_all(&mut f, [0, 9, 0], "minecraft:moss_block");
        put_all(&mut f, [0, 10, 0], "minecraft:poppy");
        assert!(put_all(&mut f, [1, 10, 0], "minecraft:stone").is_empty());
        assert!(remove(&mut f, [1, 10, 0]).is_empty());
        assert!(remove(&mut f, [1, 9, 0]).is_empty(), "il terreno accanto non conta");
        assert_eq!(f.name([0, 10, 0]), "poppy");
        // ...e anche se si cambia il blocco sotto con un altro che regge
        assert!(put_all(&mut f, [0, 9, 0], "minecraft:clay").is_empty());
        assert_eq!(f.name([0, 10, 0]), "poppy");
        // ma muore se il muschio sparisce
        remove(&mut f, [0, 9, 0]);
        assert_eq!(f.name([0, 10, 0]), "air");
        // la pianta alta nata col mondo su terra radicata sta in piedi
        let mut h = floor();
        put_all(&mut h, [0, 9, 0], "minecraft:dirt_with_roots");
        build_free(&mut h, [0, 10, 0], "minecraft:tall_grass");
        assert!(put_all(&mut h, [1, 10, 0], "minecraft:stone").is_empty());
        assert_eq!((h.name([0, 10, 0]), h.name([0, 11, 0])), ("tall_grass".into(), "tall_grass".into()));
    }

    /// Una pianta alta nata col mondo: le due metà messe direttamente, senza passare dal piazzamento.
    fn build_free(f: &mut Fake, pos: [i32; 3], name: &str) {
        let lower = blocks().state(hash(name)).unwrap().with("upper_block_bit", StateValue::Byte(0)).hash();
        let upper = blocks().state(hash(name)).unwrap().with("upper_block_bit", StateValue::Byte(1)).hash();
        set(f, &[BlockChange { pos, hash: lower, water: false }, BlockChange { pos: [pos[0], pos[1] + 1, pos[2]], hash: upper, water: false }]);
    }

    #[test]
    fn vine_loses_unsupported_sides() {
        let mut f = floor();
        put_all(&mut f, [1, 10, 0], "minecraft:stone");
        put_all(&mut f, [0, 10, 1], "minecraft:stone");
        // un rampicante appeso a est e a sud (bit 8 + 1)
        let both = blocks().state(hash("minecraft:vine")).unwrap().with("vine_direction_bits", StateValue::Int(9)).hash();
        set(&mut f, &[BlockChange { pos: [0, 10, 0], hash: both, water: false }]);
        assert_eq!(int(&f, [0, 10, 0], "vine_direction_bits"), 9);
        remove(&mut f, [1, 10, 0]);
        assert_eq!(int(&f, [0, 10, 0], "vine_direction_bits"), 1, "resta solo il lato sud");
        remove(&mut f, [0, 10, 1]);
        assert_eq!(f.name([0, 10, 0]), "air");
        // un rampicante sotto un altro che tiene lo stesso lato non lo perde
        let mut g = floor();
        put_all(&mut g, [1, 11, 0], "minecraft:stone");
        let east = blocks().state(hash("minecraft:vine")).unwrap().with("vine_direction_bits", StateValue::Int(8)).hash();
        set(&mut g, &[BlockChange { pos: [0, 11, 0], hash: east, water: false }, BlockChange { pos: [0, 10, 0], hash: east, water: false }]);
        assert_eq!((int(&g, [0, 10, 0], "vine_direction_bits"), int(&g, [0, 11, 0], "vine_direction_bits")), (8, 8));
        // un rampicante senza lati (appeso al soffitto) non si tocca
        let mut h = floor();
        let roof = blocks().state(hash("minecraft:vine")).unwrap().with("vine_direction_bits", StateValue::Int(0)).hash();
        set(&mut h, &[BlockChange { pos: [0, 10, 0], hash: roof, water: false }]);
        assert!(put_all(&mut h, [1, 10, 0], "minecraft:stone").is_empty());
    }

    #[test]
    fn unloaded_cells_leave_blocks_alone() {
        // una torcia con il pavimento in un chunk non caricato non sparisce
        let mut f = Fake::default();
        f.blocks.insert([0, 10, 0], hash("minecraft:torch"));
        f.blocks.insert([1, 10, 0], air());
        assert!(updates(&[[1, 10, 0]], &f).is_empty());
        // lo stesso per un muro con un lato sconosciuto: tiene il valore che aveva
        let wall = blocks().state(hash("minecraft:cobblestone_wall")).unwrap().with("wall_connection_type_west", StateValue::Str("short".into())).hash();
        f.blocks.insert([0, 10, 0], wall);
        assert!(updates(&[[1, 10, 0]], &f).iter().all(|c| c.hash != air()));
        let after = updates(&[[1, 10, 0]], &f);
        for c in after {
            assert_eq!(text_of(c.hash, "wall_connection_type_west"), "short");
        }
        // celle fuori dal mondo e liste vuote
        assert!(updates(&[], &f).is_empty());
        assert!(updates(&[[i32::MAX, 10, i32::MAX]], &f).is_empty());
    }

    fn text_of(hash: i32, key: &str) -> String {
        match blocks().state(hash).unwrap().get(key) {
            Some(StateValue::Str(v)) => v.clone(),
            other => panic!("{key}: {other:?}"),
        }
    }

    #[test]
    fn solid_faces_follow_the_models() {
        use crate::placement::{DOWN, EAST, NORTH, UP};
        let solid = |name: &str, face: u8| solid_face(hash(name), face);
        assert!((0..6).all(|f| solid("minecraft:stone", f)));
        assert!((0..6).all(|f| solid("minecraft:oak_leaves", f)), "le foglie reggono i rampicanti");
        assert!((0..6).all(|f| solid("minecraft:sea_lantern", f)), "la lanterna di mare è un cubo");
        assert!(!(0..6).any(|f| solid("minecraft:air", f) || solid("minecraft:water", f) || solid("minecraft:torch", f) || solid("minecraft:poppy", f)));
        assert!(
            !(0..6).any(|f| solid("minecraft:wooden_door", f) || solid("minecraft:bed", f) || solid("minecraft:trapdoor", f) || solid("minecraft:chest", f))
        );
        // mezze lastre: bassa solo sotto, alta solo sopra, doppia ovunque
        assert!(solid("minecraft:oak_slab", DOWN) && !solid("minecraft:oak_slab", UP) && !solid("minecraft:oak_slab", EAST));
        assert!((0..6).all(|f| solid("minecraft:oak_double_slab", f)));
        // scale: il fondo (o la cima se capovolte) e il lato pieno
        assert!(solid("minecraft:oak_stairs", DOWN) && !solid("minecraft:oak_stairs", UP));
        let stairs = |weirdo: i32, corner: &str| {
            solid_face(
                blocks()
                    .state(hash("minecraft:oak_stairs"))
                    .unwrap()
                    .with("weirdo_direction", StateValue::Int(weirdo))
                    .with("minecraft:corner", StateValue::Str(corner.into()))
                    .hash(),
                NORTH,
            )
        };
        assert!(stairs(3, "none"), "weirdo 3 guarda a nord: il lato pieno è a nord");
        assert!(!stairs(2, "none") && !stairs(3, "outer_left"));
        // muri e recinzioni: pieni su e giù; vetri sottili e sbarre solo sotto
        assert!(solid("minecraft:cobblestone_wall", UP) && solid("minecraft:cobblestone_wall", DOWN) && !solid("minecraft:cobblestone_wall", EAST));
        assert!(solid("minecraft:oak_fence", UP) && !solid("minecraft:oak_fence", NORTH) && !solid("minecraft:fence_gate", UP));
        assert!(solid("minecraft:glass_pane", DOWN) && !solid("minecraft:glass_pane", UP) && !solid("minecraft:iron_bars", EAST));
        assert!(!solid_face(12345, UP), "uno stato sconosciuto non regge nulla");
    }

    #[test]
    fn reactions_are_bounded() {
        // una porta con il pavimento tolto ha bisogno di due giri: con un solo giro la metà alta resterebbe
        let mut f = floor();
        build(&mut f, [0, 9, 0], crate::placement::UP, "minecraft:wooden_door");
        f.apply(&[BlockChange { pos: [0, 9, 0], hash: air(), water: false }]);
        let first = updates(&[[0, 9, 0]], &f);
        assert_eq!(first.iter().map(|c| c.pos).collect::<Vec<_>>(), [[0, 10, 0]]);
        // senza cambiamenti non c'è nessuna reazione
        assert!(updates(&[], &f).is_empty());
        // le reazioni non toccano le celle che stanno bene
        let mut g = floor();
        build(&mut g, [0, 9, 0], crate::placement::UP, "minecraft:wooden_door");
        assert!(updates(&[[0, 10, 0], [0, 11, 0]], &g).is_empty());
    }
}
