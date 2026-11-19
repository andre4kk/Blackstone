//! Piazzamento dei blocchi: il client dice dove ha cliccato e cosa ha in mano, ma è il server a decidere (modalità, portata,
//! oggetto in mano, spazio libero, ganci) e a rimandare lo stato vero se rifiuta. Qui c'è il ramo generico (D§5): lo stato
//! del blocco si ricava dal clic con le regole di `rules`; i blocchi con regole proprie (porte, letti, mezze lastre, torce, piante...)
//! passano da `special`, e `neighbors` aggiorna i blocchi accanto a ogni modifica (appoggi, connessioni, angoli).
use crate::{
    breaking::in_reach,
    entity::armor_of,
    events::{BlockChange, WorldAction},
    inventory::{Item, HOTBAR_SLOTS},
    registry::{self, blocks},
    GameMode, Server,
};
use blackstone_net::debug;
use blackstone_protocol::input::UseItem;
use vgen::Dim;

pub(crate) mod neighbors;
pub(crate) mod rules;
mod special;

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

/// Le celle `cells` e i loro sei vicini, senza doppioni e nell'ordine in cui compaiono (ogni cella seguita dai suoi vicini).
pub(crate) fn with_neighbors(cells: &[[i32; 3]]) -> Vec<[i32; 3]> {
    let mut out: Vec<[i32; 3]> = Vec::with_capacity(cells.len() * 7);
    for &cell in cells {
        for pos in std::iter::once(cell).chain((DOWN..=EAST).map(|face| side(cell, face))) {
            if !out.contains(&pos) {
                out.push(pos);
            }
        }
    }
    out
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

/// Blocchi senza ingombro (o quasi) che non impediscono di piazzare sotto i piedi o la testa di qualcuno; porte e letti
/// si piazzano comunque (`IgnoreBBox`, D§4.1 e D§4.2). Approssimazione per nome finché non ci sono i modelli dei blocchi:
/// l'errore è sempre dalla parte di rifiutare di meno.
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
    SUFFIXES.iter().any(|s| name.ends_with(s))
        || EXACT.contains(&name)
        || name.ends_with("_sapling")
        || name.ends_with("_tulip")
        || name.ends_with("coral_fan")
        || name.ends_with("_door")
        || name == "bed"
        || neighbors::plant_kind(name).is_some_and(|plant| plant != neighbors::Plant::Cactus)
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

/// Un clic su un blocco sostituibile lo rimpiazza sul posto, come se si fosse cliccata la faccia superiore (`firstReplaceable`);
/// altrimenti il blocco va accanto a quello cliccato. `None` se il blocco cliccato non è nel mondo caricato.
fn normalize(ctx: &PlaceCtx, view: &impl BlockView) -> Option<PlaceCtx> {
    let clicked = view.block(ctx.clicked)?;
    Some(if replaceable(clicked) { PlaceCtx { clicked: side(ctx.clicked, DOWN), face: UP, ..*ctx } } else { *ctx })
}

impl PlaceCtx {
    /// Lo stesso clic con un'altra faccia che porta alla stessa cella (la torcia che si appoggia a un altro lato, la lanterna appesa...).
    fn with_face(&self, face: u8) -> PlaceCtx {
        PlaceCtx { clicked: side(self.target(), face ^ 1), face, ..*self }
    }
}

/// Blocchi che possono tenere l'acqua nel secondo strato (waterlogged, D§2 e D§4.7). Un blocco normale che si piazza in una
/// cella con acqua la sostituisce; la tengono solo questi: mezze lastre (non le doppie), scale, recinzioni (non i cancelletti),
/// muretti, pannelli di vetro e sbarre, botole, cartelli (anche appesi), scale a pioli, lanterne, catene, coralli e ventagli di
/// corallo, alghe e erba marina, cetrioli di mare, candele, bauli, falò, impalcature, parafulmini, campane, vasi decorati,
/// sensori sculk, licheni, stalattiti e foglie di dripleaf. Elenco per nome (come `passable`) finché non ci sono le proprietà dei blocchi.
fn waterloggable(name: &str) -> bool {
    const EXACT: [&str; 21] = [
        "ladder",
        "kelp",
        "seagrass",
        "sea_pickle",
        "conduit",
        "chest",
        "trapped_chest",
        "ender_chest",
        "campfire",
        "soul_campfire",
        "scaffolding",
        "lightning_rod",
        "pointed_dripstone",
        "bell",
        "decorated_pot",
        "glow_lichen",
        "sculk_vein",
        "sculk_shrieker",
        "small_dripleaf_block",
        "big_dripleaf",
        "fence",
    ];
    const SUFFIXES: [&str; 16] = [
        "_stairs",
        "_fence",
        "_wall",
        "_pane",
        "_bars",
        "trapdoor",
        "_sign",
        "chain",
        "_coral",
        "coral_fan",
        "coral_wall_fan",
        "candle",
        "sculk_sensor",
        "propagule",
        "lantern",
        "_slab",
    ];
    if name.contains("double_") || name == "sea_lantern" {
        return false;
    }
    EXACT.contains(&name) || name == "glass_pane" || name == "iron_bars" || SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// Blocchi che l'acqua (o la lava) porta via e che quindi non si piazzano in una cella con un liquido (`LiquidRemovable` di
/// Dragonfly, D§4.7): piante e fiori, alberelli, torce, polvere di redstone, ragnatele e fili, tappeti, colture, cacao e simili.
fn removed_by_liquid(name: &str) -> bool {
    const EXACT: [&str; 17] = [
        "redstone_wire",
        "web",
        "tripwire",
        "string",
        "cocoa",
        "nether_wart",
        "spore_blossom",
        "skull",
        "wheat",
        "carrots",
        "potatoes",
        "beetroot",
        "pumpkin_stem",
        "melon_stem",
        "torchflower_crop",
        "pitcher_crop",
        "fire",
    ];
    neighbors::plant_kind(name).is_some_and(|plant| plant != neighbors::Plant::Cactus)
        || EXACT.contains(&name)
        || name.ends_with("torch")
        || name.ends_with("carpet")
        || name.ends_with("_mushroom")
}

fn is_lava(hash: i32) -> bool {
    name_of(hash).is_some_and(|name| matches!(name, "lava" | "flowing_lava"))
}

/// La modifica di `pos` a `hash`. Se la cella ha acqua (nel secondo strato o come blocco) l'acqua resta nel secondo strato solo
/// per i blocchi che la possono tenere (`waterloggable`); gli altri la sostituiscono. `None` se il blocco è di quelli che
/// l'acqua o la lava portano via (`removed_by_liquid`): non si piazza in un liquido.
fn change_at(view: &impl BlockView, pos: [i32; 3], existing: i32, hash: i32) -> Option<BlockChange> {
    let name = name_of(hash).unwrap_or_default();
    let wet = is_water(existing) || view.water(pos);
    if (wet || is_lava(existing)) && removed_by_liquid(name) {
        return None;
    }
    Some(BlockChange { pos, hash, water: wet && waterloggable(name) })
}

/// Piazza il blocco di `ctx.block` nella cella giusta: `None` se si rifiuta, altrimenti le celle da cambiare.
/// Si rifiuta se: lo stato dell'oggetto è sconosciuto, il clic non è sul mondo caricato, la faccia non va bene per quel blocco,
/// la cella non è libera (D§2: o è sostituibile, o si piazza accanto), c'è il giocatore, o il blocco ha due metà che nessun
/// caso speciale sa piazzare. I blocchi con regole proprie (D§4) le applicano in `special`. Se la cella ha acqua, solo i blocchi
/// waterloggabili la tengono nel secondo strato (vedi `change_at`).
pub(crate) fn place(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    if ctx.face > EAST || !ctx.click.iter().chain(&ctx.feet).chain([&ctx.yaw]).all(|v| v.is_finite()) {
        return None;
    }
    let state = blocks().state(ctx.block)?;
    if let Some(done) = special::place_special(ctx, view) {
        return done;
    }
    if state.get("upper_block_bit").is_some() || state.get("head_piece_bit").is_some() {
        return None;
    }
    place_generic(&normalize(ctx, view)?, view)
}

/// Il ramo generico su un clic già normalizzato: un solo blocco, orientato dalle regole di `rules`.
fn place_generic(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>> {
    let state = blocks().state(ctx.block)?;
    let target = ctx.target();
    let existing = view.block(target)?;
    if !replaceable(existing) || !rules::face_allowed(&state.name, ctx.face) {
        return None;
    }
    let placed = rules::orient(state, ctx);
    let hash = placed.hash();
    if hash == existing || obstructs(hash, target, ctx.feet) {
        return None;
    }
    Some(vec![change_at(view, target, existing, hash)?])
}

/// Il mondo di una dimensione, per `place` e per gli aggiornamenti dei vicini.
pub(crate) struct WorldView<'a> {
    pub(crate) server: &'a Server,
    pub(crate) dim: Dim,
}

impl BlockView for WorldView<'_> {
    fn block(&self, pos: [i32; 3]) -> Option<i32> {
        self.server.block_at(self.dim, pos).map(|(hash, _)| hash)
    }

    fn water(&self, pos: [i32; 3]) -> bool {
        self.server.block_at(self.dim, pos).is_some_and(|(_, water)| water)
    }
}

/// Il blocco che l'oggetto in mano piazza: lo stato che porta, oppure (per porte e letti, che arrivano dal menu creativo senza
/// stato, e per la canna da zucchero, il cui blocco ha un altro nome) lo stato di default del blocco corrispondente.
/// `None` per la mano vuota e per gli oggetti che non piazzano nulla (il grano non è il suo seme).
fn held_block(held: &Item) -> Option<i32> {
    if held.is_empty() {
        return None;
    }
    held.block.or_else(|| {
        let name = registry::items().name(held.id)?;
        let block = if name == "minecraft:sugar_cane" { "minecraft:reeds" } else { name };
        let hash = registry::items().block_of(block)?;
        let two_halves = blocks().state(hash).is_some_and(|s| s.get("upper_block_bit").is_some() || s.get("head_piece_bit").is_some());
        (two_halves || block != name).then_some(hash)
    })
}

impl Server {
    /// Il giocatore usa l'oggetto in mano su un blocco con l'azione Place. La casella della barra che il client dichiara
    /// (`hotbar_slot`) diventa quella in mano, come `UpdateHeldSlot` di Dragonfly. Se il blocco cliccato è interattivo (porta,
    /// leva...) e il giocatore non è accovacciato con qualcosa in mano, lo si attiva (vedi `interact`) e non si piazza nulla,
    /// qualunque cosa il client creda di avere in mano. Altrimenti, se in mano ha un blocco e tutti i controlli passano
    /// (modalità, oggetto uguale a quello del client, portata, spazio libero, ganci) il blocco si piazza: chi vede il chunk
    /// riceve l'aggiornamento e il suono. In sopravvivenza il piazzamento accettato toglie un oggetto dalla casella in mano (in creativa nessuno) e al
    /// client si rimanda la casella col conteggio vero. All'autore si rimandano anche la
    /// cella cliccata e quella prevista, che possono differire dall'esito (mezze lastre che si uniscono, torce spostate).
    /// Se si rifiuta, all'autore si rimanda lo stato vero delle celle toccate e dei loro vicini. Una mano senza blocchi non fa nulla.
    pub(crate) fn place_item(&mut self, pid: u64, u: &UseItem) {
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.spawned) else { return };
        if u.face > EAST {
            return debug!("[#{pid}] piazzamento con la faccia {} inesistente: ignorato", u.face);
        }
        if (0..HOTBAR_SLOTS as i32).contains(&u.hotbar_slot) {
            p.inventory.selected = u.hotbar_slot as u8;
        }
        let (mode, eyes, yaw, sneaking) = (p.mode, p.pos, p.yaw, p.sneaking);
        let held = p.inventory.held().clone();
        let dim = Dim::Overworld;
        let feet = [eyes[0], eyes[1] - EYE_HEIGHT, eyes[2]];
        let block = held_block(&held);
        let ctx = PlaceCtx { clicked: u.pos, face: u.face, click: u.click_pos, yaw, feet, block: block.unwrap_or_default(), sneaking };
        // Un blocco interattivo (porta, leva...) si attiva invece di piazzare, a meno che si sia accovacciati con qualcosa in mano (D§5).
        if (!ctx.sneaking || held.is_empty()) && self.interact(pid, u, &ctx, mode, eyes) {
            return;
        }
        // Il client deve avere in mano ciò che il server crede, altrimenti si fida del server (D§5, Review Focus 4).
        if !held.same_kind(&Item::kind_of(&u.held)) {
            self.refuse_place(pid, u, "l'oggetto in mano non è quello del server");
            return self.resync_held(pid);
        }
        if block.is_none() {
            return;
        }
        let creative = mode == GameMode::Creative;
        if !matches!(mode, GameMode::Creative | GameMode::Survival) {
            return self.refuse_place(pid, u, "modalità senza modifica del mondo");
        }
        if !in_reach(eyes, u.pos, creative) {
            return self.refuse_place(pid, u, "fuori portata");
        }
        let Some(changes) = place(&ctx, &WorldView { server: self, dim }) else {
            return self.refuse_place(pid, u, "blocco non piazzabile lì");
        };
        // gli spettatori non hanno corpo: non sono d'intralcio
        let others_in_the_way = self
            .players
            .iter()
            .filter(|(&other, o)| other != pid && o.spawned && o.mode != GameMode::Spectator)
            .any(|(_, o)| changes.iter().any(|c| obstructs(c.hash, c.pos, [o.pos[0], o.pos[1] - EYE_HEIGHT, o.pos[2]])));
        if others_in_the_way || changes.iter().any(|c| !in_reach(eyes, c.pos, creative)) {
            return self.refuse_place(pid, u, "cella fuori portata o occupata da un giocatore");
        }
        if !self.allow(&WorldAction::Place { pid, changes: changes.clone() }) {
            return self.refuse_place(pid, u, "annullato da un gancio");
        }
        self.set_blocks(dim, &changes);
        // il colore del letto sta nel suo block entity (D§4.2): è l'aux dell'oggetto
        for c in changes.iter().filter(|c| name_of(c.hash) == Some("bed")) {
            self.put_block_entity(dim, special::bed_entity(c.pos, held.aux));
        }
        // il forziere ha il suo block entity (e si abbina a quello accanto)
        for c in changes.iter().filter(|c| crate::chest::is_chest(c.hash)) {
            self.chest_placed(c.pos);
        }
        // la fornace pure
        for c in changes.iter().filter(|c| crate::furnace::is_furnace(c.hash)) {
            self.furnace_placed(c.pos);
        }
        for p in self.players.values_mut() {
            // una rottura chiesta due volte nello stesso tick vale una, ma non se in mezzo la cella è stata riempita
            if p.last_break.is_some_and(|(_, pos)| changes.iter().any(|c| c.pos == pos)) {
                p.last_break = None;
            }
        }
        for c in &changes {
            self.play_sound("place", c.pos, c.hash);
        }
        // Il client ha previsto il blocco nella cella accanto a quella cliccata: se l'esito è un altro (lastre unite, torcia su un'altra
        // faccia) la cella prevista va corretta; la cella cliccata si rimanda per lo stesso motivo.
        self.resend_blocks(pid, dim, &[u.pos, side(u.pos, u.face)]);
        if !creative {
            // Dragonfly `UseItemOnBlock`: `SubtractItem(1)` se non creativo (anche per porte, letti e blocchi doppi: un solo oggetto per piazzamento).
            // Il client ha previsto il consumo, ma l'ID di pila nuovo e il conteggio vero glieli dice il server; l'ultimo oggetto svuota la casella.
            if let Some(p) = self.players.get_mut(&pid) {
                let (before, armor) = (held.stack(0), armor_of(p));
                let slot = p.inventory.selected as usize;
                let left = held.count.saturating_sub(1);
                p.inventory.set_main(slot, if left > 0 { Item { count: left, ..held } } else { Item::default() });
                self.send_equipment_changes(pid, &before, &armor);
            }
            self.resync_held(pid);
        }
    }

    /// Rimanda al giocatore la casella in mano e quale casella è selezionata, com'è per il server.
    fn resync_held(&self, pid: u64) {
        if let Some(p) = self.players.get(&pid) {
            self.send(pid, &[p.inventory.held_slot_packet(), p.inventory.hotbar_packet()]);
        }
    }

    /// Piazzamento rifiutato: si dice perché nel log e si rimanda all'autore lo stato vero del blocco cliccato, di quello accanto
    /// e dei loro vicini (il client prevede anche le seconde metà di porte, letti e piante alte).
    fn refuse_place(&self, pid: u64, u: &UseItem, why: &str) {
        debug!("[#{pid}] piazzamento su {:?} (faccia {}) rifiutato: {why}", u.pos, u.face);
        self.resend_around(pid, Dim::Overworld, &[u.pos, side(u.pos, u.face)]);
    }
}

/// Un mondo finto per i test del piazzamento e dei vicini.
#[cfg(test)]
pub(super) mod fake {
    use super::*;
    use crate::world::block_hash;
    use std::collections::{HashMap, HashSet};

    /// Un mondo di prova: le celle non elencate sono chunk non caricati.
    #[derive(Default)]
    pub(crate) struct Fake {
        pub(crate) blocks: HashMap<[i32; 3], i32>,
        pub(crate) water: HashSet<[i32; 3]>,
    }

    impl BlockView for Fake {
        fn block(&self, pos: [i32; 3]) -> Option<i32> {
            self.blocks.get(&pos).copied()
        }

        fn water(&self, pos: [i32; 3]) -> bool {
            self.water.contains(&pos)
        }
    }

    impl Fake {
        /// Applica le modifiche (come `Server::set_blocks`, ma senza vicini).
        pub(crate) fn apply(&mut self, changes: &[BlockChange]) {
            for c in changes {
                self.blocks.insert(c.pos, c.hash);
                if c.water {
                    self.water.insert(c.pos);
                } else {
                    self.water.remove(&c.pos);
                }
            }
        }

        /// Lo stato del blocco in `pos`, senza il prefisso `minecraft:` nel nome.
        pub(crate) fn name(&self, pos: [i32; 3]) -> String {
            let state = blocks().state(self.blocks[&pos]).expect("stato noto");
            state.name.strip_prefix("minecraft:").unwrap_or(&state.name).to_string()
        }
    }

    pub(crate) fn hash(name: &str) -> i32 {
        blocks().default_hash(name).unwrap_or_else(|| panic!("{name} non esiste"))
    }

    pub(crate) fn air() -> i32 {
        block_hash("minecraft:air", &[])
    }

    /// Una pietra in (0, 10, 0), aria attorno (anche sopra e accanto), un giocatore lontano.
    pub(crate) fn world() -> Fake {
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

    /// Un pavimento di pietra fino a y = 9 (le celle libere cominciano a y = 10), da -4 a 4 su x e z, aria sopra fino a y = 14.
    pub(crate) fn floor() -> Fake {
        let mut f = Fake::default();
        for x in -4..=4 {
            for z in -4..=4 {
                for y in 6..=14 {
                    f.blocks.insert([x, y, z], if y <= 9 { hash("minecraft:stone") } else { air() });
                }
            }
        }
        f
    }

    /// Un clic con quell'oggetto in mano, il giocatore lontano e rivolto a sud.
    pub(crate) fn click(clicked: [i32; 3], face: u8, held: &str) -> PlaceCtx {
        PlaceCtx { clicked, face, click: [0.5, 0.5, 0.5], yaw: 0.0, feet: [30.5, 10.0, 30.5], block: hash(held), sneaking: false }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{air, hash, world, Fake};
    use super::*;
    use crate::world::{block_hash, State};

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
    fn only_waterloggable_blocks_keep_the_water() {
        let mut f = world();
        f.water.insert([1, 10, 0]);
        f.water.insert([0, 11, 0]);
        let at = |f: &Fake, face: u8, held: &str| place(&click([0, 10, 0], face, held), f).map(|c| (c[0].pos, c[0].water));
        let dry = place(&click([0, 10, 0], NORTH, "minecraft:cobblestone"), &f).unwrap();
        assert!(!dry[0].water);
        // un blocco normale sostituisce l'acqua (D§4.7)
        for held in ["minecraft:cobblestone", "minecraft:dirt", "minecraft:oak_planks", "minecraft:oak_door", "minecraft:oak_double_slab"] {
            if let Some(hash) = blocks().default_hash(held) {
                let ctx = PlaceCtx { block: hash, ..click([0, 10, 0], EAST, "minecraft:cobblestone") };
                assert_eq!(place(&ctx, &f).map(|c| (c[0].pos, c[0].water)), Some(([1, 10, 0], false)), "{held}: sostituisce l'acqua");
            }
        }
        // mezzi blocchi, scale, recinzioni, muretti, botole, cartelli, scale a pioli, lanterne, catene, pannelli: la tengono
        for held in [
            "minecraft:oak_slab",
            "minecraft:oak_stairs",
            "minecraft:oak_fence",
            "minecraft:cobblestone_wall",
            "minecraft:glass_pane",
            "minecraft:iron_bars",
            "minecraft:trapdoor",
            "minecraft:ladder",
            "minecraft:lantern",
            "minecraft:iron_chain",
            "minecraft:tube_coral",
            "minecraft:tube_coral_fan",
            "minecraft:sea_pickle",
            "minecraft:chest",
        ] {
            if blocks().default_hash(held).is_some() {
                // di lato o, per quelli che non si appoggiano di lato (lanterne, coralli), sopra
                let got = at(&f, EAST, held).or_else(|| at(&f, UP, held));
                assert!(matches!(got, Some((_, true))), "{held}: tiene l'acqua nel secondo strato ({got:?})");
            }
        }
        // acqua come blocco nello strato 0: il blocco la sostituisce, e solo i waterloggabili ne tengono nel secondo strato
        f.blocks.insert([0, 10, 1], block_hash("minecraft:water", &[("liquid_depth", State::Int(0))]));
        assert_eq!(at(&f, SOUTH, "minecraft:cobblestone"), Some(([0, 10, 1], false)));
        assert_eq!(at(&f, SOUTH, "minecraft:oak_slab"), Some(([0, 10, 1], true)));
        // la lava no
        f.blocks.insert([-1, 10, 0], block_hash("minecraft:lava", &[("liquid_depth", State::Int(0))]));
        assert!(!place(&click([0, 10, 0], WEST, "minecraft:cobblestone"), &f).unwrap()[0].water);
        assert!(!place(&click([0, 10, 0], WEST, "minecraft:oak_slab"), &f).unwrap()[0].water);
    }

    #[test]
    fn plants_and_torches_are_not_placed_in_water() {
        // i fiori, l'erba e gli alberelli li porta via l'acqua: non si piazzano in una cella con l'acqua (LiquidRemovable)
        let mut f = world();
        // terra sotto le tre celle, così le piante mancano solo per colpa del liquido
        let grass = hash("minecraft:grass_block");
        for pos in [[1, 9, 0], [0, 9, 1], [-1, 9, 0], [0, 9, -1], [0, 10, 0]] {
            f.blocks.insert(pos, grass);
        }
        f.water.insert([1, 10, 0]); // acqua nel secondo strato
        f.blocks.insert([0, 10, 1], block_hash("minecraft:water", &[("liquid_depth", State::Int(0))])); // acqua come blocco
        f.blocks.insert([-1, 10, 0], block_hash("minecraft:lava", &[("liquid_depth", State::Int(0))]));
        for held in ["minecraft:poppy", "minecraft:dandelion", "minecraft:short_grass", "minecraft:oak_sapling", "minecraft:deadbush", "minecraft:torch"] {
            if blocks().default_hash(held).is_none() {
                continue;
            }
            for face in [EAST, SOUTH, WEST] {
                assert!(place(&click([0, 10, 0], face, held), &f).is_none(), "{held} (faccia {face}) in un liquido");
            }
            // all'asciutto sì (la cella a nord ha la terra sotto e niente acqua)
            assert!(place(&click([0, 10, 0], NORTH, held), &f).is_some(), "{held} all'asciutto");
        }
    }

    #[test]
    fn items_without_a_state_place_their_default_block_only_when_needed() {
        let item = |name: &str| Item { id: registry::items().id(name).unwrap(), count: 1, aux: 0, block: None, nbt: None };
        let block = |name: &str| held_block(&item(name)).and_then(|h| blocks().state(h)).map(|s| s.name.clone());
        assert_eq!(block("minecraft:wooden_door").as_deref(), Some("minecraft:wooden_door"));
        assert_eq!(block("minecraft:bed").as_deref(), Some("minecraft:bed"));
        assert_eq!(block("minecraft:sugar_cane").as_deref(), Some("minecraft:reeds"));
        assert_eq!(block("minecraft:wheat"), None, "il grano non è il suo seme");
        assert_eq!(block("minecraft:stick"), None);
        let stone = Item { block: Some(hash("minecraft:dirt")), ..item("minecraft:stone") };
        assert_eq!(held_block(&stone), Some(hash("minecraft:dirt")), "lo stato dell'oggetto vince");
        assert_eq!(held_block(&Item::default()), None);
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
    fn blocks_with_two_halves_need_a_special_case() {
        let f = world();
        // quelli che hanno un caso speciale lo seguono (porte, letti, piante alte): qui un appoggio c'è per la porta, non per la testa del letto
        assert_eq!(place(&click([0, 10, 0], UP, "minecraft:wooden_door"), &f).map(|c| c.len()), Some(2));
        assert!(place(&click([0, 10, 0], UP, "minecraft:bed"), &f).is_none());
        // ...gli altri il ramo generico li rifiuta
        for name in ["minecraft:pitcher_plant", "minecraft:pale_moss_carpet", "minecraft:small_dripleaf_block", "minecraft:straw_bed"] {
            assert!(place(&click([0, 10, 0], UP, name), &f).is_none(), "{name}: nessun caso speciale");
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
