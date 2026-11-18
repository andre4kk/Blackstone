//! Regole di orientamento: da un blocco di partenza (lo stato dell'oggetto in mano) e dal clic si ricava lo stato da piazzare.
//! Ogni famiglia ha la sua convenzione per lo stesso nome di stato (D§3 delle note su Dragonfly): porte, botole, scale, letti...
//! Un valore che il blocco non ammette (o uno stato che non ha) non si scrive: lo stato resta quello di partenza.
use super::{PlaceCtx, DOWN, EAST, NORTH, SOUTH, UP, WEST};
use crate::registry::{blocks, BlockState, StateValue};

/// Direzione orizzontale, nell'ordine di Dragonfly (`cube.Direction`): nord, sud, ovest, est.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dir {
    North,
    South,
    West,
    East,
}

impl Dir {
    /// Verso dove guarda un giocatore con quella rotazione (0 = sud, 90 = ovest, -90 = est, 180 = nord), come `Rotation.Direction`.
    /// Gli angoli si riportano a [0, 360): 135 è ovest, 136 nord, 315 est (-45), 316 sud.
    pub(super) fn from_yaw(yaw: f32) -> Dir {
        let y = yaw.rem_euclid(360.0);
        if y <= 45.0 || y > 315.0 {
            Dir::South
        } else if y <= 135.0 {
            Dir::West
        } else if y <= 225.0 {
            Dir::North
        } else {
            Dir::East
        }
    }

    /// La direzione di una faccia laterale (nord 2, sud 3, ovest 4, est 5); `None` per su e giù.
    pub(super) fn from_face(face: u8) -> Option<Dir> {
        match face {
            NORTH => Some(Dir::North),
            SOUTH => Some(Dir::South),
            WEST => Some(Dir::West),
            EAST => Some(Dir::East),
            _ => None,
        }
    }

    pub(super) fn opposite(self) -> Dir {
        match self {
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    pub(super) fn rotate_right(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
        }
    }

    pub(super) fn rotate_left(self) -> Dir {
        self.rotate_right().opposite()
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }

    /// Indice nell'ordine di Dragonfly (nord 0, sud 1, ovest 2, est 3).
    pub(super) fn index(self) -> i32 {
        self as i32
    }

    /// La direzione con quell'indice di Dragonfly (0..3); `None` oltre.
    pub(super) fn from_index(index: i32) -> Option<Dir> {
        [Dir::North, Dir::South, Dir::West, Dir::East].get(usize::try_from(index).ok()?).copied()
    }

    /// ID della faccia (2..5).
    pub(super) fn face(self) -> u8 {
        self as u8 + 2
    }

    /// Il valore "direction" di Bedrock (sud 0, ovest 1, nord 2, est 3).
    pub(super) fn legacy(self) -> i32 {
        match self {
            Dir::South => 0,
            Dir::West => 1,
            Dir::North => 2,
            Dir::East => 3,
        }
    }

    /// Vero per est e ovest.
    pub(super) fn is_x(self) -> bool {
        matches!(self, Dir::West | Dir::East)
    }
}

pub(super) const FACE_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// Asse della faccia: su e giù Y, ovest ed est X, nord e sud Z.
fn face_axis(face: u8) -> &'static str {
    match face {
        DOWN | UP => "y",
        WEST | EAST => "x",
        _ => "z",
    }
}

/// Uno dei 16 settori di rotazione (0 = sud, 4 = ovest, 8 = nord, 12 = est), come `Rotation.Orientation`.
fn orientation(yaw: f32) -> i32 {
    ((yaw.rem_euclid(360.0) / 22.5).round() as i32).rem_euclid(16)
}

/// Un valore da scrivere: numero (int o byte) o testo.
pub(super) enum V<'a> {
    N(i32),
    S(&'a str),
}

/// Scrive `value` nello stato `key` se il blocco ha quella chiave e ammette quel valore; il tipo (byte, int, stringa) è quello dello stato.
pub(super) fn put(state: &mut BlockState, key: &str, value: V) {
    let new = match (state.get(key), value) {
        (Some(StateValue::Int(_)), V::N(n)) => StateValue::Int(n),
        (Some(StateValue::Byte(_)), V::N(n)) => match u8::try_from(n) {
            Ok(n) => StateValue::Byte(n),
            Err(_) => return,
        },
        (Some(StateValue::Str(_)), V::S(s)) => StateValue::Str(s.to_string()),
        _ => return,
    };
    if blocks().values(&state.name, key).is_some_and(|allowed| allowed.contains(&new)) {
        *state = state.with(key, new);
    }
}

/// Faccia della botte (`calculateFace` di Dragonfly): su o giù se il giocatore è entro un blocco in orizzontale e gli occhi sono
/// molto più in alto o più in basso, altrimenti rivolta verso il giocatore.
fn barrel_face(ctx: &PlaceCtx, dir: Dir) -> i32 {
    let target = ctx.target();
    let (px, pz) = (ctx.feet[0].floor() as i64, ctx.feet[2].floor() as i64);
    if (px - target[0] as i64).abs() < 2 && (pz - target[2] as i64).abs() < 2 {
        let eyes = ctx.feet[1] as f64 + 1.62;
        if eyes - target[1] as f64 > 2.0 {
            return UP as i32;
        } else if (target[1] as f64) - eyes > 0.0 {
            return DOWN as i32;
        }
    }
    dir.opposite().face() as i32
}

/// Blocchi con `facing_direction` che il clic non orienta: tengono lo stato di partenza.
const KEEP_FACING: [&str; 5] = ["melon_stem", "pumpkin_stem", "piston_arm_collision", "sticky_piston_arm_collision", "jigsaw"];
/// Blocchi con un "davanti" rivolto al giocatore. Senza l'inclinazione della testa nel contesto guardano solo in orizzontale.
const FACE_PLAYER: [&str; 7] = ["piston", "sticky_piston", "dropper", "dispenser", "command_block", "chain_command_block", "repeating_command_block"];
/// Blocchi con `pillar_axis` che ignorano la faccia cliccata e restano con l'asse verticale (D§3).
const ALWAYS_Y: [&str; 6] =
    ["quartz_block", "smooth_quartz", "chiseled_quartz_block", "purpur_block", "deprecated_purpur_block_1", "deprecated_purpur_block_2"];

/// Regola base per stato più eccezioni per famiglia. `ctx` deve essere normalizzato (vedi `PlaceCtx`).
pub(crate) fn orient(state: &BlockState, ctx: &PlaceCtx) -> BlockState {
    let mut s = state.clone();
    if ctx.face > EAST {
        return s; // faccia inesistente: nessuna regola
    }
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let (dir, face) = (Dir::from_yaw(ctx.yaw), ctx.face);
    let side = Dir::from_face(face);
    // metà alta: faccia inferiore, oppure punto cliccato nella metà alta di una faccia laterale
    let top = face == DOWN || (ctx.click[1] > 0.5 && face != UP);
    for (key, _) in &state.states {
        let key = key.as_str();
        match key {
            "pillar_axis" => put(&mut s, key, V::S(if ALWAYS_Y.contains(&name) { "y" } else { face_axis(face) })),
            "minecraft:cardinal_direction" => {
                let facing = if name.contains("anvil") {
                    dir.rotate_left()
                } else if name == "wooden_door" || name.ends_with("_door") {
                    dir.rotate_right()
                } else if name == "fence_gate" || name.ends_with("_fence_gate") {
                    dir
                } else if name == "shelf_mushroom" {
                    side.unwrap_or(dir.opposite())
                } else {
                    dir.opposite()
                };
                put(&mut s, key, V::S(facing.name()));
            }
            "facing_direction" if KEEP_FACING.contains(&name) => {}
            "facing_direction" => {
                let value = if name == "hopper" {
                    if face == DOWN {
                        DOWN as i32
                    } else {
                        (face ^ 1) as i32
                    }
                } else if name == "barrel" {
                    barrel_face(ctx, dir)
                } else if name.ends_with("_glazed_terracotta") || FACE_PLAYER.contains(&name) {
                    dir.opposite().face() as i32
                } else if name == "end_rod" {
                    // sull'asse verticale vale la faccia, sugli altri la faccia opposta
                    if face <= UP {
                        face as i32
                    } else {
                        (face ^ 1) as i32
                    }
                } else {
                    // scala, cartelli e stendardi a muro, pulsanti, teste, cornici, parafulmini: la faccia cliccata
                    face as i32
                };
                put(&mut s, key, V::N(value));
            }
            "direction" => {
                let facing = if name == "bed" {
                    dir.legacy()
                } else if name == "grindstone" {
                    side.unwrap_or(dir.opposite()).legacy()
                } else if name.ends_with("trapdoor") {
                    3 - dir.opposite().index()
                } else if name == "cocoa" {
                    side.map_or(dir, Dir::opposite).legacy()
                } else {
                    // telaio, vaso decorato, alveare e il resto: il davanti guarda il giocatore
                    dir.opposite().legacy()
                };
                put(&mut s, key, V::N(facing));
            }
            "attachment" => put(
                &mut s,
                key,
                V::S(if face == UP {
                    "standing"
                } else if face == DOWN {
                    "hanging"
                } else {
                    "side"
                }),
            ),
            "weirdo_direction" => put(&mut s, key, V::N(3 - dir.index())),
            "upside_down_bit" => put(&mut s, key, V::N(top as i32)),
            "minecraft:vertical_half" => put(&mut s, key, V::S(if top { "top" } else { "bottom" })),
            "torch_facing_direction" => {
                let facing = match face {
                    UP => "top",
                    DOWN => "unknown", // non si piazza: `face_allowed` la rifiuta
                    _ => FACE_NAMES[(face ^ 1) as usize],
                };
                put(&mut s, key, V::S(facing));
            }
            "lever_direction" => {
                let value = if face <= UP {
                    format!("{}_{}", FACE_NAMES[face as usize], if dir.is_x() { "east_west" } else { "north_south" })
                } else {
                    FACE_NAMES[face as usize].to_string()
                };
                put(&mut s, key, V::S(&value));
            }
            "ground_sign_direction" => put(&mut s, key, V::N((orientation(ctx.yaw) + 8) % 16)),
            // un bit per lato (sud 1, ovest 2, nord 4, est 8): il rampicante si attacca al lato opposto alla faccia cliccata
            "vine_direction_bits" => {
                if let Some(attach) = side.map(Dir::opposite) {
                    let bit = match attach {
                        Dir::South => 1,
                        Dir::West => 2,
                        Dir::North => 4,
                        Dir::East => 8,
                    };
                    put(&mut s, key, V::N(bit));
                }
            }
            // un bit per faccia (giù, su, nord, sud, ovest, est) attaccata al blocco cliccato, cioè alla faccia opposta (UNVERIFIED)
            "multi_face_direction_bits" => put(&mut s, key, V::N(1 << (face ^ 1))),
            "hanging" if name.ends_with("lantern") => put(&mut s, key, V::N((face == DOWN) as i32)),
            // UNVERIFIED (nessuna implementazione in Dragonfly): germogli e cristalli guardano via dal blocco cliccato, l'osservatore verso il giocatore
            "minecraft:block_face" => put(&mut s, key, V::S(FACE_NAMES[face as usize])),
            "minecraft:facing_direction" => put(&mut s, key, V::S(dir.opposite().name())),
            _ => {}
        }
    }
    s
}

/// Vero se il blocco `name` (con o senza `minecraft:`) può essere piazzato cliccando su quella `face`:
/// la scala a pioli, il rampicante e il fungo a mensola solo di lato, le torce non da sotto (D§4.4).
pub(crate) fn face_allowed(name: &str, face: u8) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    match name {
        "ladder" | "vine" | "shelf_mushroom" => face > UP,
        _ if name.ends_with("torch") || name.starts_with("colored_torch") => face != DOWN,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        placement::{DOWN, EAST, NORTH, SOUTH, UP, WEST},
        registry::{blocks, StateValue},
    };

    const SOUTH_YAW: f32 = 0.0;
    const WEST_YAW: f32 = 90.0;
    const EAST_YAW: f32 = -90.0;
    const NORTH_YAW: f32 = 180.0;

    fn ctx(face: u8, yaw: f32) -> PlaceCtx {
        PlaceCtx { clicked: [0, 10, 0], face, click: [0.5, 0.5, 0.5], yaw, feet: [5.5, 10.0, 5.5], block: 0, sneaking: false }
    }

    fn start(name: &str) -> BlockState {
        let hash = blocks().default_hash(name).unwrap_or_else(|| panic!("{name} non esiste"));
        blocks().state(hash).unwrap().clone()
    }

    /// Il blocco `name` orientato, valido nel registro.
    fn placed(name: &str, ctx: &PlaceCtx) -> BlockState {
        let s = orient(&start(name), ctx);
        assert!(blocks().is_valid(&s), "{s:?} non è uno stato valido");
        s
    }

    fn int(s: &BlockState, key: &str) -> i32 {
        match s.get(key) {
            Some(StateValue::Int(v)) => *v,
            Some(StateValue::Byte(v)) => *v as i32,
            other => panic!("{key}: {other:?}"),
        }
    }

    fn text(s: &BlockState, key: &str) -> String {
        match s.get(key) {
            Some(StateValue::Str(v)) => v.clone(),
            other => panic!("{key}: {other:?}"),
        }
    }

    #[test]
    fn log_axis_from_face() {
        assert_eq!(text(&placed("minecraft:oak_log", &ctx(EAST, SOUTH_YAW)), "pillar_axis"), "x");
        assert_eq!(text(&placed("minecraft:oak_log", &ctx(WEST, SOUTH_YAW)), "pillar_axis"), "x");
        assert_eq!(text(&placed("minecraft:oak_log", &ctx(UP, SOUTH_YAW)), "pillar_axis"), "y");
        assert_eq!(text(&placed("minecraft:oak_log", &ctx(DOWN, SOUTH_YAW)), "pillar_axis"), "y");
        assert_eq!(text(&placed("minecraft:oak_log", &ctx(NORTH, SOUTH_YAW)), "pillar_axis"), "z");
        assert_eq!(text(&placed("minecraft:oak_log", &ctx(SOUTH, SOUTH_YAW)), "pillar_axis"), "z");
        // il blocco di quarzo liscio e simili non ruotano mai (D§3); il resto dello stato non si tocca
        assert_eq!(text(&placed("minecraft:quartz_block", &ctx(EAST, SOUTH_YAW)), "pillar_axis"), "y");
        assert_eq!(text(&placed("minecraft:quartz_pillar", &ctx(EAST, SOUTH_YAW)), "pillar_axis"), "x");
        assert_eq!(int(&placed("minecraft:hay_block", &ctx(EAST, SOUTH_YAW)), "deprecated"), 0);
    }

    #[test]
    fn yaw_maps_to_the_four_directions_with_wraparound() {
        // scale: weirdo_direction = 3 - direzione (nord 0, sud 1, ovest 2, est 3)
        let stairs = |yaw: f32| int(&placed("minecraft:oak_stairs", &ctx(UP, yaw)), "weirdo_direction");
        assert_eq!([stairs(0.0), stairs(90.0), stairs(-90.0), stairs(180.0)], [2, 1, 0, 3]);
        assert_eq!([stairs(45.0), stairs(46.0), stairs(-45.0), stairs(-46.0)], [2, 1, 0, 0]);
        assert_eq!([stairs(135.0), stairs(136.0), stairs(-135.0), stairs(-136.0)], [1, 3, 3, 3]);
        assert_eq!([stairs(360.0), stairs(450.0), stairs(-270.0), stairs(540.0), stairs(-180.0)], [2, 1, 1, 3, 3]);
    }

    #[test]
    fn stairs_face_player_and_flip() {
        assert_eq!(int(&placed("minecraft:oak_stairs", &ctx(UP, NORTH_YAW)), "weirdo_direction"), 3);
        let flip = |face: u8, y: f32| {
            let mut c = ctx(face, SOUTH_YAW);
            c.click[1] = y;
            int(&placed("minecraft:oak_stairs", &c), "upside_down_bit")
        };
        assert_eq!(flip(NORTH, 0.7), 1, "metà alta di una faccia laterale");
        assert_eq!(flip(NORTH, 0.3), 0);
        assert_eq!(flip(DOWN, 0.0), 1, "faccia inferiore: sempre capovolta");
        assert_eq!(flip(UP, 0.9), 0, "faccia superiore: mai capovolta");
        assert_eq!(text(&placed("minecraft:oak_stairs", &ctx(UP, SOUTH_YAW)), "minecraft:corner"), "none", "gli angoli li calcolano i vicini");
    }

    #[test]
    fn slab_top_half() {
        assert_eq!(text(&placed("minecraft:oak_slab", &ctx(DOWN, SOUTH_YAW)), "minecraft:vertical_half"), "top");
        assert_eq!(text(&placed("minecraft:oak_slab", &ctx(UP, SOUTH_YAW)), "minecraft:vertical_half"), "bottom");
        let mut high = ctx(EAST, SOUTH_YAW);
        high.click[1] = 0.8;
        assert_eq!(text(&placed("minecraft:oak_slab", &high), "minecraft:vertical_half"), "top");
        let mut low = ctx(EAST, SOUTH_YAW);
        low.click[1] = 0.2;
        assert_eq!(text(&placed("minecraft:oak_slab", &low), "minecraft:vertical_half"), "bottom");
    }

    #[test]
    fn chest_and_furnace_face_player() {
        for name in ["minecraft:chest", "minecraft:furnace", "minecraft:ender_chest", "minecraft:carved_pumpkin", "minecraft:lectern"] {
            let facing = |yaw: f32| text(&placed(name, &ctx(UP, yaw)), "minecraft:cardinal_direction");
            assert_eq!([facing(SOUTH_YAW), facing(NORTH_YAW), facing(WEST_YAW), facing(EAST_YAW)], ["north", "south", "east", "west"], "{name}");
        }
    }

    #[test]
    fn door_anvil_and_fence_gate_use_their_own_convention() {
        let of = |name: &str, yaw: f32| text(&placed(name, &ctx(UP, yaw)), "minecraft:cardinal_direction");
        // porta: la direzione del giocatore ruotata a destra (giocatore verso sud -> "west")
        assert_eq!([of("minecraft:wooden_door", SOUTH_YAW), of("minecraft:wooden_door", NORTH_YAW)], ["west", "east"]);
        assert_eq!([of("minecraft:wooden_door", WEST_YAW), of("minecraft:iron_door", EAST_YAW)], ["north", "south"]);
        // incudine: ruotata a sinistra
        assert_eq!([of("minecraft:anvil", SOUTH_YAW), of("minecraft:anvil", NORTH_YAW)], ["east", "west"]);
        // cancelletto: la direzione del giocatore, senza giri
        assert_eq!([of("minecraft:fence_gate", SOUTH_YAW), of("minecraft:fence_gate", WEST_YAW)], ["south", "west"]);
    }

    #[test]
    fn trapdoor_direction() {
        let dir = |yaw: f32| int(&placed("minecraft:trapdoor", &ctx(UP, yaw)), "direction");
        assert_eq!(dir(SOUTH_YAW), 3);
        assert_eq!([dir(NORTH_YAW), dir(WEST_YAW), dir(EAST_YAW)], [2, 0, 1]);
        let mut high = ctx(WEST, SOUTH_YAW);
        high.click[1] = 0.9;
        assert_eq!(int(&placed("minecraft:trapdoor", &high), "upside_down_bit"), 1);
        assert_eq!(int(&placed("minecraft:trapdoor", &ctx(DOWN, SOUTH_YAW)), "upside_down_bit"), 1);
        assert_eq!(int(&placed("minecraft:trapdoor", &ctx(UP, SOUTH_YAW)), "upside_down_bit"), 0);
        assert_eq!(int(&placed("minecraft:trapdoor", &ctx(UP, SOUTH_YAW)), "open_bit"), 0, "si piazza chiusa");
    }

    #[test]
    fn glazed_terracotta_and_ladder_and_wall_sign() {
        // terracotta smaltata: 2 + direzione opposta a quella del giocatore (sud -> nord = 2, nord -> sud = 3, ovest -> est = 5)
        let glazed = |yaw: f32| int(&placed("minecraft:white_glazed_terracotta", &ctx(UP, yaw)), "facing_direction");
        assert_eq!([glazed(SOUTH_YAW), glazed(NORTH_YAW), glazed(WEST_YAW), glazed(EAST_YAW)], [2, 3, 5, 4]);
        // scala a pioli, cartello a muro e pulsante: l'ID della faccia cliccata
        for name in ["minecraft:ladder", "minecraft:wall_sign", "minecraft:stone_button"] {
            for face in NORTH..=EAST {
                assert_eq!(int(&placed(name, &ctx(face, SOUTH_YAW)), "facing_direction"), face as i32, "{name}");
            }
        }
        // cornice: la faccia cliccata, anche su e giù
        assert_eq!(int(&placed("minecraft:frame", &ctx(DOWN, SOUTH_YAW)), "facing_direction"), 0);
    }

    #[test]
    fn hopper_end_rod_and_barrel() {
        let hopper = |face: u8| int(&placed("minecraft:hopper", &ctx(face, SOUTH_YAW)), "facing_direction");
        assert_eq!([hopper(DOWN), hopper(UP), hopper(NORTH), hopper(EAST)], [0, 0, 3, 4]);
        let rod = |face: u8| int(&placed("minecraft:end_rod", &ctx(face, SOUTH_YAW)), "facing_direction");
        assert_eq!([rod(UP), rod(DOWN), rod(NORTH), rod(EAST)], [1, 0, 3, 4]);
        // botte: i piedi sono a (5.5, 10, 5.5), gli occhi a y 11.62; il blocco va sopra la cella cliccata
        let barrel = |clicked: [i32; 3]| {
            let mut c = ctx(UP, SOUTH_YAW);
            c.clicked = clicked;
            int(&placed("minecraft:barrel", &c), "facing_direction")
        };
        assert_eq!(barrel([5, 10, 5]), 2, "quasi all'altezza degli occhi: verso il giocatore (giocatore a sud, botte a nord)");
        assert_eq!(barrel([5, 7, 5]), 1, "tre blocchi sotto gli occhi e vicino in orizzontale: guarda in su");
        assert_eq!(barrel([5, 13, 5]), 0, "sopra gli occhi: guarda in giù");
        assert_eq!(barrel([9, 7, 5]), 2, "lontano in orizzontale: verso il giocatore");
    }

    #[test]
    fn direction_families_differ() {
        let dir = |name: &str, yaw: f32| int(&placed(name, &ctx(UP, yaw)), "direction");
        // letto: la direzione del giocatore (sud 0, ovest 1, nord 2, est 3)
        assert_eq!(
            [dir("minecraft:bed", SOUTH_YAW), dir("minecraft:bed", WEST_YAW), dir("minecraft:bed", NORTH_YAW), dir("minecraft:bed", EAST_YAW)],
            [0, 1, 2, 3]
        );
        // telaio e vaso decorato: la direzione opposta
        for name in ["minecraft:loom", "minecraft:decorated_pot"] {
            assert_eq!([dir(name, SOUTH_YAW), dir(name, WEST_YAW), dir(name, NORTH_YAW), dir(name, EAST_YAW)], [2, 3, 0, 1], "{name}");
        }
        // mola: a muro prende la direzione della faccia cliccata, altrimenti sta in piedi o appesa
        let wall = placed("minecraft:grindstone", &ctx(EAST, SOUTH_YAW));
        assert_eq!((text(&wall, "attachment"), int(&wall, "direction")), ("side".to_string(), 3));
        assert_eq!(text(&placed("minecraft:grindstone", &ctx(DOWN, SOUTH_YAW)), "attachment"), "hanging");
        assert_eq!(text(&placed("minecraft:grindstone", &ctx(UP, SOUTH_YAW)), "attachment"), "standing");
    }

    #[test]
    fn torch_lever_lantern_vine_and_standing_sign() {
        let torch = |face: u8| text(&placed("minecraft:torch", &ctx(face, SOUTH_YAW)), "torch_facing_direction");
        assert_eq!([torch(UP), torch(NORTH), torch(SOUTH), torch(WEST), torch(EAST)], ["top", "south", "north", "east", "west"]);
        let lever = |face: u8, yaw: f32| text(&placed("minecraft:lever", &ctx(face, yaw)), "lever_direction");
        assert_eq!([lever(NORTH, SOUTH_YAW), lever(EAST, SOUTH_YAW)], ["north", "east"]);
        assert_eq!(
            [lever(UP, SOUTH_YAW), lever(UP, WEST_YAW), lever(DOWN, NORTH_YAW), lever(DOWN, EAST_YAW)],
            ["up_north_south", "up_east_west", "down_north_south", "down_east_west"]
        );
        assert_eq!(int(&placed("minecraft:lantern", &ctx(DOWN, SOUTH_YAW)), "hanging"), 1);
        assert_eq!(int(&placed("minecraft:lantern", &ctx(UP, SOUTH_YAW)), "hanging"), 0);
        // rampicante: attaccato al lato opposto alla faccia cliccata (sud 1, ovest 2, nord 4, est 8)
        let vine = |face: u8| int(&placed("minecraft:vine", &ctx(face, SOUTH_YAW)), "vine_direction_bits");
        assert_eq!([vine(NORTH), vine(SOUTH), vine(WEST), vine(EAST)], [1, 4, 8, 2]);
        // cartello in piedi: 16 settori, opposto al giocatore (sud 0 -> 8)
        let sign = |yaw: f32| int(&placed("minecraft:standing_sign", &ctx(UP, yaw)), "ground_sign_direction");
        assert_eq!([sign(SOUTH_YAW), sign(NORTH_YAW), sign(WEST_YAW), sign(EAST_YAW)], [8, 0, 12, 4]);
        assert_eq!([sign(11.0), sign(12.0), sign(-11.0), sign(-12.0), sign(360.0)], [8, 9, 8, 7, 8]);
    }

    #[test]
    fn orient_keeps_the_rest_of_the_state() {
        let start = start("minecraft:trapdoor").with("open_bit", StateValue::Byte(1));
        assert_eq!(int(&orient(&start, &ctx(UP, SOUTH_YAW)), "open_bit"), 1, "orient non tocca ciò che non gestisce");
        assert_eq!(orient(&start, &ctx(UP, SOUTH_YAW)).name, "minecraft:trapdoor");
    }

    #[test]
    fn some_blocks_refuse_some_faces() {
        assert!(!face_allowed("minecraft:ladder", UP) && !face_allowed("minecraft:ladder", DOWN) && face_allowed("minecraft:ladder", EAST));
        assert!(!face_allowed("minecraft:torch", DOWN) && face_allowed("minecraft:torch", UP) && !face_allowed("minecraft:soul_torch", DOWN));
        assert!(!face_allowed("minecraft:vine", UP) && face_allowed("minecraft:vine", NORTH));
        assert!(face_allowed("minecraft:stone", DOWN) && face_allowed("minecraft:lever", DOWN));
    }

    #[test]
    fn every_rule_yields_a_valid_state() {
        let mut checked = 0;
        for name in blocks().names() {
            let start = start(name);
            for yaw in [SOUTH_YAW, WEST_YAW, NORTH_YAW, EAST_YAW] {
                for face in DOWN..=EAST {
                    for y in [0.2, 0.8] {
                        let mut c = ctx(face, yaw);
                        c.click[1] = y;
                        let s = orient(&start, &c);
                        assert_eq!(s.name, name);
                        assert!(blocks().is_valid(&s), "{name} yaw {yaw} faccia {face}: {s:?} non è valido");
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 1000 * 48, "ha visto tutto il registro: {checked}");
    }
}
