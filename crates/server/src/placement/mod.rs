//! Piazzamento dei blocchi: il client dice dove ha cliccato e cosa ha in mano, ma è il server a decidere (modalità, portata,
//! oggetto in mano, spazio libero, ganci) e a rimandare lo stato vero se rifiuta. Qui c'è il ramo generico (D§5): lo stato
//! del blocco si ricava dal clic con le regole di `rules`. Porte, letti e gli altri casi speciali arrivano con il loro modulo.
use crate::{
    breaking::in_reach,
    events::{BlockChange, WorldAction},
    inventory::Item,
    registry::blocks,
    GameMode, Server,
};
use bedrock_net::log;
use bedrock_protocol::{
    generated::{LevelSoundEvent, Vec3},
    input::UseItem,
    wire::ServerPacket,
};
use vgen::Dim;

pub(crate) mod rules;

/// Facce, nell'ordine di Bedrock e di Dragonfly (`cube.Face`).
pub(crate) const DOWN: u8 = 0;
pub(crate) const UP: u8 = 1;
pub(crate) const NORTH: u8 = 2;
pub(crate) const SOUTH: u8 = 3;
pub(crate) const WEST: u8 = 4;
pub(crate) const EAST: u8 = 5;

/// Altezza degli occhi sopra i piedi (in piedi, D§2) e misure del giocatore per l'ingombro (D§6).
const EYE_HEIGHT: f32 = 1.62;
const PLAYER_WIDTH: f64 = 0.6;
const PLAYER_HEIGHT: f64 = 1.8;
/// Gli ingombri si restringono di questo margine, così chi tocca solo il bordo non ostruisce.
const MARGIN: f64 = 1e-4;

/// Il clic che piazza un blocco.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlaceCtx {
    /// Blocco cliccato. Il blocco va sempre nella cella `clicked + face` (vedi [`PlaceCtx::target`]): se si clicca un blocco
    /// sostituibile, `place` sposta il clic sulla cella sotto e la faccia su `UP` prima di chiamare le regole.
    pub clicked: [i32; 3],
    /// Faccia cliccata: 0 giù, 1 su, 2 nord, 3 sud, 4 ovest, 5 est.
    pub face: u8,
    /// Punto cliccato, relativo al blocco (0..1).
    pub click: [f32; 3],
    /// Rotazione del giocatore (0 = sud, 90 = ovest, -90 = est, 180 = nord).
    pub yaw: f32,
    /// Piedi del giocatore.
    pub feet: [f32; 3],
    /// Hash dello stato del blocco dell'oggetto in mano.
    pub block: i32,
    #[allow(dead_code)] // lo leggono le interazioni (accovacciato: si piazza invece di aprire)
    pub sneaking: bool,
}

impl PlaceCtx {
    /// La cella dove va il blocco.
    pub fn target(&self) -> [i32; 3] {
        side(self.clicked, self.face)
    }
}

/// Il mondo com'è, visto dal piazzamento.
pub(crate) trait BlockView {
    /// Hash del blocco nello strato 0; `None` se la cella è fuori dal mondo o in un chunk non caricato.
    fn block(&self, pos: [i32; 3]) -> Option<i32>;

    /// Vero se nella cella c'è acqua nel secondo strato.
    fn water(&self, _pos: [i32; 3]) -> bool {
        false
    }
}

/// La cella accanto a `pos` dalla parte di `face` (saturando ai limiti: una cella inventata non esiste nel mondo).
fn side(pos: [i32; 3], face: u8) -> [i32; 3] {
    const STEP: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
    let step = STEP.get(face as usize).copied().unwrap_or_default();
    [pos[0].saturating_add(step[0]), pos[1].saturating_add(step[1]), pos[2].saturating_add(step[2])]
}

/// Nome del blocco con quell'hash senza `minecraft:`.
fn name_of(hash: i32) -> Option<&'static str> {
    blocks().state(hash).map(|s| s.name.strip_prefix("minecraft:").unwrap_or(&s.name))
}

/// Blocchi che un piazzamento qualsiasi sostituisce (D§4.7): aria, liquidi, erbe, fuoco, luce, rampicanti.
pub(crate) fn replaceable(hash: i32) -> bool {
    name_of(hash).is_some_and(|name| {
        matches!(
            name,
            "air"
                | "water"
                | "flowing_water"
                | "lava"
                | "flowing_lava"
                | "short_grass"
                | "tall_grass"
                | "fern"
                | "large_fern"
                | "deadbush"
                | "fire"
                | "soul_fire"
                | "nether_sprouts"
                | "vine"
                | "red_shrub"
        ) || name.starts_with("light_block")
    })
}

fn is_water(hash: i32) -> bool {
    name_of(hash).is_some_and(|name| matches!(name, "water" | "flowing_water"))
}

/// Blocchi senza ingombro (o quasi) che non impediscono di piazzare sotto i piedi o la testa di qualcuno.
/// Approssimazione per nome finché non ci sono i modelli dei blocchi: l'errore è sempre dalla parte di rifiutare di meno.
fn passable(name: &str) -> bool {
    const SUFFIXES: [&str; 11] = ["torch", "sign", "banner", "button", "lever", "ladder", "vine", "vines", "rail", "carpet", "pressure_plate"];
    const EXACT: [&str; 14] = [
        "short_grass",
        "tall_grass",
        "fern",
        "large_fern",
        "deadbush",
        "bush",
        "sapling",
        "brown_mushroom",
        "red_mushroom",
        "tripwire",
        "tripwire_hook",
        "redstone_wire",
        "frame",
        "glow_frame",
    ];
    SUFFIXES.iter().any(|s| name.ends_with(s)) || EXACT.contains(&name) || name.ends_with("_sapling") || name.ends_with("_tulip") || name.ends_with("coral_fan")
}

/// Vero se il blocco `hash` messo in `pos` si sovrappone al giocatore con quei piedi (0.6 × 1.8, D§6).
pub(crate) fn obstructs(hash: i32, pos: [i32; 3], feet: [f32; 3]) -> bool {
    if name_of(hash).is_some_and(passable) {
        return false;
    }
    let half = PLAYER_WIDTH / 2.0 - MARGIN;
    let overlaps = |center: f32, half: f64, cell: i32| (center as f64 - half) < cell as f64 + 1.0 && (center as f64 + half) > cell as f64;
    overlaps(feet[0], half, pos[0])
        && overlaps(feet[2], half, pos[2])
        && feet[1] as f64 + MARGIN < pos[1] as f64 + 1.0
        && feet[1] as f64 + PLAYER_HEIGHT - MARGIN > pos[1] as f64
}

/// Piazza il blocco di `ctx.block` nella cella giusta: `None` se si rifiuta, altrimenti le celle da cambiare.
/// Si rifiuta se: lo stato dell'oggetto è sconosciuto, il clic non è sul mondo caricato, la faccia non va bene per quel blocco,
/// la cella non è libera (D§2: o è sostituibile, o si piazza accanto), c'è il giocatore, o il blocco è un caso speciale
/// (due metà: porte, letti, piante alte) che il ramo generico non sa piazzare. Se la cella ha acqua, il blocco la tiene nel secondo strato.
pub(crate) fn place(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    if ctx.face > EAST || !ctx.click.iter().chain(&ctx.feet).chain([&ctx.yaw]).all(|v| v.is_finite()) {
        return None;
    }
    let state = blocks().state(ctx.block)?;
    if state.get("upper_block_bit").is_some() || state.get("head_piece_bit").is_some() {
        return None;
    }
    // Un blocco sostituibile si rimpiazza sul posto, come se si fosse cliccata la faccia superiore (`firstReplaceable`).
    let clicked = view.block(ctx.clicked)?;
    let ctx = if replaceable(clicked) { PlaceCtx { clicked: side(ctx.clicked, DOWN), face: UP, ..*ctx } } else { *ctx };
    let target = ctx.target();
    let existing = view.block(target)?;
    if !replaceable(existing) || !rules::face_allowed(&state.name, ctx.face) {
        return None;
    }
    let placed = rules::orient(state, &ctx);
    let hash = placed.hash();
    if hash == existing || obstructs(hash, target, ctx.feet) {
        return None;
    }
    Some(vec![BlockChange { pos: target, hash, water: is_water(existing) || view.water(target) }])
}

/// Il mondo di una dimensione, per `place`.
struct WorldView<'a> {
    server: &'a Server,
    dim: Dim,
}

impl BlockView for WorldView<'_> {
    fn block(&self, pos: [i32; 3]) -> Option<i32> {
        self.server.block_at(self.dim, pos).map(|(hash, _)| hash)
    }

    fn water(&self, pos: [i32; 3]) -> bool {
        self.server.block_at(self.dim, pos).is_some_and(|(_, water)| water)
    }
}

impl Server {
    /// Il giocatore usa l'oggetto in mano su un blocco con l'azione Place. Se in mano ha un blocco e tutti i controlli
    /// passano (modalità, oggetto uguale a quello del client, portata, spazio libero, ganci) il blocco si piazza: chi vede
    /// il chunk riceve l'aggiornamento e il suono; in sopravvivenza si consuma un oggetto, in creativo no. Altrimenti si
    /// rimanda al solo autore lo stato vero del blocco cliccato e di quello accanto. Una mano senza blocchi non fa nulla.
    pub(crate) fn place_item(&mut self, pid: u64, u: &UseItem) {
        let Some(p) = self.players.get(&pid).filter(|p| p.spawned) else { return };
        if u.face > EAST {
            return log!("[#{pid}] piazzamento con la faccia {} inesistente: ignorato", u.face);
        }
        let (mode, eyes, yaw, sneaking) = (p.mode, p.pos, p.yaw, p.sneaking);
        let held = p.inventory.held().clone();
        // Il client deve avere in mano ciò che il server crede, altrimenti si fida del server (D§5, Review Focus 4).
        if !held.same_kind(&Item::kind_of(&u.held)) {
            return self.refuse_place(pid, u, "l'oggetto in mano non è quello del server");
        }
        let Some(block) = held.block.filter(|_| !held.is_empty()) else { return };
        let creative = mode == GameMode::Creative;
        if !matches!(mode, GameMode::Creative | GameMode::Survival) {
            return self.refuse_place(pid, u, "modalità senza modifica del mondo");
        }
        if !in_reach(eyes, u.pos, creative) {
            return self.refuse_place(pid, u, "fuori portata");
        }
        let dim = Dim::Overworld;
        let feet = [eyes[0], eyes[1] - EYE_HEIGHT, eyes[2]];
        let ctx = PlaceCtx { clicked: u.pos, face: u.face, click: u.click_pos, yaw, feet, block, sneaking };
        let Some(changes) = place(&ctx, &WorldView { server: self, dim }) else {
            return self.refuse_place(pid, u, "blocco non piazzabile lì");
        };
        let others_in_the_way = self
            .players
            .iter()
            .filter(|(&other, o)| other != pid && o.spawned)
            .any(|(_, o)| changes.iter().any(|c| obstructs(c.hash, c.pos, [o.pos[0], o.pos[1] - EYE_HEIGHT, o.pos[2]])));
        if others_in_the_way || changes.iter().any(|c| !in_reach(eyes, c.pos, creative)) {
            return self.refuse_place(pid, u, "cella fuori portata o occupata da un giocatore");
        }
        if !self.allow(&WorldAction::Place { pid, changes: changes.clone() }) {
            return self.refuse_place(pid, u, "annullato da un gancio");
        }
        self.set_blocks(dim, &changes);
        for p in self.players.values_mut() {
            // una rottura chiesta due volte nello stesso tick vale una, ma non se in mezzo la cella è stata riempita
            if p.last_break.is_some_and(|(_, pos)| changes.iter().any(|c| c.pos == pos)) {
                p.last_break = None;
            }
        }
        for c in &changes {
            let sound = LevelSoundEvent {
                sound_event: "place".into(),
                position: Vec3 { x: c.pos[0] as f32 + 0.5, y: c.pos[1] as f32 + 0.5, z: c.pos[2] as f32 + 0.5 },
                data: c.hash,
                actor_identifier: String::new(),
                is_baby: false,
                is_global: false,
                actor_unique_id: -1,
                fire_at_position: None,
            }
            .packet();
            let chunk = (c.pos[0].div_euclid(16), c.pos[2].div_euclid(16));
            for (&viewer, _) in self.players.iter().filter(|(_, p)| p.sent.contains(&chunk)) {
                self.send(viewer, std::slice::from_ref(&sound));
            }
        }
        if !creative {
            if let Some(packet) = self.players.get_mut(&pid).and_then(|p| p.inventory.consume_held()) {
                self.send(pid, &[packet]);
            }
        }
    }

    /// Piazzamento rifiutato: si dice perché nel log e si rimanda all'autore lo stato vero del blocco cliccato e di quello accanto.
    fn refuse_place(&self, pid: u64, u: &UseItem, why: &str) {
        log!("[#{pid}] piazzamento su {:?} (faccia {}) rifiutato: {why}", u.pos, u.face);
        self.resend_blocks(pid, Dim::Overworld, &[u.pos, side(u.pos, u.face)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{block_hash, State};
    use std::collections::{HashMap, HashSet};

    /// Un mondo di prova: le celle non elencate sono chunk non caricati.
    #[derive(Default)]
    struct Fake {
        blocks: HashMap<[i32; 3], i32>,
        water: HashSet<[i32; 3]>,
    }

    impl BlockView for Fake {
        fn block(&self, pos: [i32; 3]) -> Option<i32> {
            self.blocks.get(&pos).copied()
        }

        fn water(&self, pos: [i32; 3]) -> bool {
            self.water.contains(&pos)
        }
    }

    fn hash(name: &str) -> i32 {
        blocks().default_hash(name).unwrap_or_else(|| panic!("{name} non esiste"))
    }

    fn air() -> i32 {
        block_hash("minecraft:air", &[])
    }

    /// Una pietra in (0, 10, 0), aria attorno (anche sopra e accanto), un giocatore lontano.
    fn world() -> Fake {
        let mut f = Fake::default();
        for x in -3..=3 {
            for y in 6..=14 {
                for z in -3..=3 {
                    f.blocks.insert([x, y, z], air());
                }
            }
        }
        f.blocks.insert([0, 10, 0], hash("minecraft:stone"));
        f
    }

    fn click(clicked: [i32; 3], face: u8, held: &str) -> PlaceCtx {
        PlaceCtx { clicked, face, click: [0.5, 0.5, 0.5], yaw: 0.0, feet: [3.5, 10.0, 3.5], block: hash(held), sneaking: false }
    }

    #[test]
    fn place_next_to_the_clicked_face() {
        let f = world();
        let at = |face: u8| place(&click([0, 10, 0], face, "minecraft:cobblestone"), &f).unwrap().remove(0).pos;
        assert_eq!([at(UP), at(DOWN), at(NORTH), at(SOUTH), at(WEST), at(EAST)], [[0, 11, 0], [0, 9, 0], [0, 10, -1], [0, 10, 1], [-1, 10, 0], [1, 10, 0]]);
        let change = place(&click([0, 10, 0], UP, "minecraft:cobblestone"), &f).unwrap().remove(0);
        assert_eq!((change.hash, change.water), (hash("minecraft:cobblestone"), false));
    }

    #[test]
    fn place_on_replaceable_grass_goes_in_place() {
        let mut f = world();
        f.blocks.insert([0, 10, 0], hash("minecraft:short_grass"));
        f.blocks.insert([0, 9, 0], hash("minecraft:dirt"));
        // si clicca il lato del filo d'erba: il blocco prende il suo posto e si orienta come se si fosse cliccata la faccia superiore
        let changes = place(&click([0, 10, 0], NORTH, "minecraft:oak_log"), &f).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].pos, [0, 10, 0]);
        let log = blocks().state(changes[0].hash).unwrap();
        assert_eq!(log.get("pillar_axis"), Some(&crate::registry::StateValue::Str("y".into())), "non z: la faccia è stata portata su UP");
    }

    #[test]
    fn replaceable_blocks_follow_the_notes() {
        for name in [
            "air",
            "water",
            "flowing_water",
            "lava",
            "flowing_lava",
            "short_grass",
            "tall_grass",
            "fern",
            "large_fern",
            "deadbush",
            "fire",
            "light_block_7",
            "nether_sprouts",
            "vine",
            "red_shrub",
        ] {
            assert!(replaceable(hash(&format!("minecraft:{name}"))), "{name}");
        }
        let water = block_hash("minecraft:water", &[("liquid_depth", State::Int(5))]);
        assert!(replaceable(water), "ogni livello d'acqua");
        for name in ["stone", "oak_slab", "snow", "short_dry_grass", "dirt"] {
            if let Some(h) = blocks().default_hash(&format!("minecraft:{name}")) {
                assert!(!replaceable(h), "{name}");
            }
        }
        assert!(!replaceable(12345), "uno stato sconosciuto non è sostituibile");
    }

    #[test]
    fn place_into_occupied_is_refused() {
        let mut f = world();
        f.blocks.insert([1, 10, 0], hash("minecraft:dirt"));
        assert!(place(&click([0, 10, 0], EAST, "minecraft:cobblestone"), &f).is_none());
        // lo stesso blocco sostituibile già presente non si rimpiazza con sé stesso
        f.blocks.insert([0, 11, 0], hash("minecraft:short_grass"));
        assert!(place(&click([0, 10, 0], UP, "minecraft:short_grass"), &f).is_none());
        assert!(place(&click([0, 10, 0], UP, "minecraft:cobblestone"), &f).is_some());
    }

    #[test]
    fn place_outside_the_loaded_world_is_refused() {
        let f = world();
        assert!(place(&click([9, 10, 9], UP, "minecraft:cobblestone"), &f).is_none(), "cliccato in un chunk non caricato");
        let mut edge = world();
        edge.blocks.insert([3, 10, 0], hash("minecraft:stone"));
        assert!(place(&click([3, 10, 0], EAST, "minecraft:cobblestone"), &edge).is_none(), "la cella accanto non è caricata");
        assert!(place(&click([i32::MAX, 10, 0], EAST, "minecraft:cobblestone"), &f).is_none(), "nessun overflow");
        assert!(place(&click([0, 10, 0], 6, "minecraft:cobblestone"), &f).is_none(), "faccia inesistente");
    }

    #[test]
    fn place_inside_player_is_refused() {
        // pietra in (0, 8, 0) e una colonna in x = 1, da y = 10 a y = 12; la cella (0, 10, 0) è libera
        let mut f = world();
        for pos in [[0, 8, 0], [1, 10, 0], [1, 11, 0], [1, 12, 0]] {
            f.blocks.insert(pos, hash("minecraft:stone"));
        }
        f.blocks.insert([0, 10, 0], air());
        let put = |feet: [f32; 3], clicked: [i32; 3], face: u8, held: &str| {
            let mut ctx = click(clicked, face, held);
            ctx.feet = feet;
            place(&ctx, &f)
        };
        let stone = "minecraft:cobblestone";
        assert!(put([0.5, 10.0, 0.5], [1, 10, 0], WEST, stone).is_none(), "nei piedi");
        assert!(put([0.5, 10.0, 0.5], [1, 11, 0], WEST, stone).is_none(), "nella testa, a 1.8 blocchi dai piedi");
        assert!(put([0.5, 10.0, 0.5], [1, 12, 0], WEST, stone).is_some(), "sopra la testa (finisce a 11.8)");
        assert!(put([0.5, 10.0, 0.5], [0, 8, 0], UP, stone).is_some(), "sotto i piedi");
        // l'ingombro è largo 0.6: chi sta nella cella accanto, a mezzo blocco dal bordo, non è d'intralcio
        assert!(put([1.3, 10.0, 0.5], [1, 10, 0], WEST, stone).is_some(), "x da 1.0 a 1.6: tocca solo il bordo");
        assert!(put([1.29, 10.0, 0.5], [1, 10, 0], WEST, stone).is_none(), "x da 0.99 a 1.59: entra di un soffio");
        // i blocchi senza ingombro non sono d'intralcio
        assert!(put([0.5, 10.0, 0.5], [1, 10, 0], WEST, "minecraft:torch").is_some());
    }

    #[test]
    fn place_in_water_waterlogs() {
        let mut f = world();
        f.water.insert([1, 10, 0]);
        let dry = place(&click([0, 10, 0], NORTH, "minecraft:cobblestone"), &f).unwrap();
        assert!(!dry[0].water);
        let wet = place(&click([0, 10, 0], EAST, "minecraft:cobblestone"), &f).unwrap();
        assert_eq!((wet[0].pos, wet[0].water), ([1, 10, 0], true), "acqua nel secondo strato: resta");
        // acqua come blocco nello strato 0: il blocco la sostituisce ma l'acqua resta nel secondo strato
        f.blocks.insert([0, 10, 1], block_hash("minecraft:water", &[("liquid_depth", State::Int(0))]));
        let source = place(&click([0, 10, 0], SOUTH, "minecraft:cobblestone"), &f).unwrap();
        assert_eq!((source[0].pos, source[0].water), ([0, 10, 1], true));
        // la lava no
        f.blocks.insert([-1, 10, 0], block_hash("minecraft:lava", &[("liquid_depth", State::Int(0))]));
        assert!(!place(&click([0, 10, 0], WEST, "minecraft:cobblestone"), &f).unwrap()[0].water);
    }

    #[test]
    fn unknown_held_item_is_refused() {
        let f = world();
        let mut ctx = click([0, 10, 0], UP, "minecraft:cobblestone");
        ctx.block = 0x1234_5678;
        assert!(place(&ctx, &f).is_none(), "uno stato di blocco che il registro non conosce");
        ctx.block = 0;
        assert!(place(&ctx, &f).is_none());
        ctx.block = hash("minecraft:cobblestone");
        assert!(place(&ctx, &f).is_some());
    }

    #[test]
    fn blocks_with_two_halves_are_not_generic() {
        let f = world();
        for name in ["minecraft:wooden_door", "minecraft:bed", "minecraft:tall_grass", "minecraft:sunflower"] {
            assert!(place(&click([0, 10, 0], UP, name), &f).is_none(), "{name}: lo piazzerà il caso speciale");
        }
    }

    #[test]
    fn faces_refused_by_the_block_are_refused() {
        let f = world();
        assert!(place(&click([0, 10, 0], UP, "minecraft:ladder"), &f).is_none());
        let side = place(&click([0, 10, 0], EAST, "minecraft:ladder"), &f).unwrap();
        assert_eq!(blocks().state(side[0].hash).unwrap().get("facing_direction"), Some(&crate::registry::StateValue::Int(5)));
        assert!(place(&click([0, 10, 0], DOWN, "minecraft:torch"), &f).is_none());
        assert!(place(&click([0, 10, 0], UP, "minecraft:torch"), &f).is_some());
    }

    #[test]
    fn non_finite_numbers_are_refused() {
        let f = world();
        let tweaks: [fn(&mut PlaceCtx); 3] = [|c| c.yaw = f32::NAN, |c| c.click[1] = f32::INFINITY, |c| c.feet[0] = f32::NAN];
        for tweak in tweaks {
            let mut ctx = click([0, 10, 0], UP, "minecraft:cobblestone");
            tweak(&mut ctx);
            assert!(place(&ctx, &f).is_none());
        }
    }
}
