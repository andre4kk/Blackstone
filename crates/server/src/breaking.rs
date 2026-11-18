//! Rottura dei blocchi: il client dice cosa ha rotto, ma è il server a decidere (modalità, portata, ganci) e a rimandare
//! lo stato vero se rifiuta. La rottura è istantanea; in sopravvivenza il blocco lascia a terra gli oggetti dovuti (`drops_for`).
use crate::{
    events::{BlockChange, WorldAction},
    inventory::Item,
    registry,
    tables::{block_drops, tool},
    world::{block_hash, State},
    GameMode, Server,
};
use bedrock_net::log;
use bedrock_protocol::{
    generated::{LevelEvent, Vec3},
    input::player_action,
    wire::ServerPacket,
};
use std::{collections::HashSet, sync::OnceLock};
use vgen::Dim;

/// `LevelEvent` con suono e particelle di rottura; il dato è l'hash del blocco rotto (P§7.3).
const PARTICLES_DESTROY_BLOCK: i32 = 2001;

/// Portata massima tra gli occhi e il centro del blocco (D§6).
const REACH: f64 = 8.0;
const REACH_CREATIVE: f64 = 14.0;

/// Vero se il centro del blocco `pos` dista dagli occhi al massimo la portata (14 blocchi in creativo, 8 altrimenti).
/// Valori non finiti non sono mai a portata.
pub(crate) fn in_reach(eyes: [f32; 3], pos: [i32; 3], creative: bool) -> bool {
    let limit = if creative { REACH_CREATIVE } else { REACH };
    let squared: f64 = (0..3)
        .map(|i| {
            let delta = pos[i] as f64 + 0.5 - eyes[i] as f64;
            delta * delta
        })
        .sum();
    squared <= limit * limit // con un NaN il confronto è falso
}

/// Spinta casuale degli oggetti lasciati da un blocco (Dragonfly `BreakBlock`: x/z in ±0,1, y 0,2) e attesa prima di poterli raccogliere.
const DROP_SPREAD: f32 = 0.1;
const DROP_LIFT: f32 = 0.2;
const DROP_PICKUP_DELAY: u64 = 10;

/// Gli oggetti che il blocco `block_name` lascia se rotto con `held` in mano; `roll` dà numeri in [0, 1).
/// Senza voce nella tabella il blocco lascia se stesso (1), ma solo se il suo nome è anche un oggetto (`redstone_wire` no).
/// Con voce: serve lo strumento del tipo giusto e di livello sufficiente, altrimenti niente. Se le probabilità delle voci
/// sommano 1 è un tiro solo (ghiaia: selce o ghiaia), altrimenti ogni voce tira per conto suo.
pub(crate) fn drops_for(block_name: &str, held: &Item, roll: &mut impl FnMut() -> f32) -> Vec<Item> {
    let Some(rules) = block_drops(block_name) else {
        return Item::from_name(block_name, 1).into_iter().collect();
    };
    if let Some(kind) = rules.tool {
        let tier = registry::items().name(held.id).and_then(tool).filter(|t| t.kind == kind).map(|t| t.tier);
        if !tier.is_some_and(|t| t >= rules.min_tier) {
            return Vec::new();
        }
    }
    let exclusive = (rules.drops.iter().map(|d| d.chance).sum::<f32>() - 1.0).abs() < 1e-4;
    let mut chosen = Vec::new();
    if exclusive {
        let mut r = roll();
        for d in &rules.drops {
            if r < d.chance {
                chosen.push(d);
                break;
            }
            r -= d.chance;
        }
    } else {
        for d in &rules.drops {
            if roll() < d.chance {
                chosen.push(d);
            }
        }
    }
    let mut out = Vec::new();
    for d in chosen {
        let spread = d.max.saturating_sub(d.min);
        let count = d.min + ((roll() * (u32::from(spread) + 1) as f32) as u16).min(spread);
        if count > 0 {
            out.extend(Item::from_name(&d.item, count)); // un tiro di quantità 0 (arbusto secco: 0-2 bastoni) non lascia niente
        }
    }
    out
}

/// Hash dei blocchi che nessuno rompe a mano: i liquidi, e in sopravvivenza anche la roccia di fondo.
fn hand_proof(hash: i32, creative: bool) -> bool {
    static SETS: OnceLock<(HashSet<i32>, HashSet<i32>)> = OnceLock::new();
    let (liquids, bedrock) = SETS.get_or_init(|| {
        let liquids = ["minecraft:water", "minecraft:flowing_water", "minecraft:lava", "minecraft:flowing_lava"]
            .iter()
            .flat_map(|name| (0..16).map(move |depth| block_hash(name, &[("liquid_depth", State::Int(depth))])))
            .collect();
        let bedrock = (0..2).map(|bit| block_hash("minecraft:bedrock", &[("infiniburn_bit", State::Byte(bit))])).collect();
        (liquids, bedrock)
    });
    liquids.contains(&hash) || (!creative && bedrock.contains(&hash))
}

impl Server {
    /// Un'azione di blocco del giocatore (da `PlayerAction` o dalle azioni di `PlayerAuthInput`).
    /// Start e Continue scelgono il blocco che si sta rompendo, Abort lo dimentica, Stop e Predict lo rompono (solo se è
    /// quello scelto, come Dragonfly: D§7) e CreativeDestroy lo rompe subito, ma solo in creativo. Le altre azioni non servono.
    pub(crate) fn block_action(&mut self, pid: u64, action: i32, pos: [i32; 3]) {
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.spawned) else { return };
        match action {
            player_action::START_BREAK | player_action::CONTINUE_DESTROY => p.breaking = Some(pos),
            player_action::ABORT_BREAK => p.breaking = None,
            player_action::STOP_BREAK | player_action::PREDICT_DESTROY => {
                if p.breaking.take() == Some(pos) {
                    self.break_block(pid, pos);
                } else {
                    // il client ha rotto un blocco che non stava rompendo: gli si dice com'è davvero
                    self.resend_around(pid, Dim::Overworld, &[pos]);
                }
            }
            player_action::CREATIVE_DESTROY if p.mode == GameMode::Creative => self.break_block(pid, pos),
            player_action::CREATIVE_DESTROY => {
                log!("[#{pid}] rottura creativa fuori dalla modalità creativa: rifiutata");
                self.resend_around(pid, Dim::Overworld, &[pos]);
            }
            _ => {}
        }
    }

    /// Il giocatore rompe il blocco in `pos`. Se non può (modalità, portata, blocco indistruttibile, gancio che annulla)
    /// si rimanda lo stato vero a lui solo, per la cella e per i suoi vicini (l'altra metà di una porta o di un letto, la pianta alta). Altrimenti il blocco diventa aria (l'acqua del secondo strato resta) e chi vede
    /// il chunk riceve l'aggiornamento e l'effetto di rottura. L'aria e i blocchi non caricati non fanno nulla.
    pub(crate) fn break_block(&mut self, pid: u64, pos: [i32; 3]) {
        let tick = self.tick_count;
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.spawned) else { return };
        // Il client può chiedere la stessa rottura per due strade nello stesso tick (PlayerAction e UseItem): vale una volta.
        if p.last_break == Some((tick, pos)) {
            return;
        }
        p.last_break = Some((tick, pos));
        let (mode, eyes, held) = (p.mode, p.pos, p.inventory.held().clone());
        let dim = Dim::Overworld;
        let Some((old, water)) = self.block_at(dim, pos) else { return };
        if old == block_hash("minecraft:air", &[]) {
            return;
        }
        let creative = mode == GameMode::Creative;
        let editable = matches!(mode, GameMode::Creative | GameMode::Survival);
        if !editable || !in_reach(eyes, pos, creative) || hand_proof(old, creative) || !self.allow(&WorldAction::Break { pid, pos, old }) {
            log!("[#{pid}] rottura in {pos:?} rifiutata");
            return self.resend_around(pid, dim, &[pos]);
        }
        self.set_blocks(dim, &[BlockChange { pos, hash: block_hash("minecraft:air", &[]), water }]);
        let effect = LevelEvent {
            event_id: PARTICLES_DESTROY_BLOCK,
            position: Vec3 { x: pos[0] as f32 + 0.5, y: pos[1] as f32 + 0.5, z: pos[2] as f32 + 0.5 },
            data: old,
        }
        .packet();
        if mode == GameMode::Survival {
            if let Some(state) = registry::blocks().state(old) {
                let drops = drops_for(&state.name, &held, &mut || self.roll());
                for item in drops {
                    let velocity = [(self.roll() * 2.0 - 1.0) * DROP_SPREAD, DROP_LIFT, (self.roll() * 2.0 - 1.0) * DROP_SPREAD];
                    self.spawn_item(item, [pos[0] as f32 + 0.5, pos[1] as f32 + 0.5, pos[2] as f32 + 0.5], velocity, DROP_PICKUP_DELAY);
                }
            }
        }
        let chunk = (pos[0].div_euclid(16), pos[2].div_euclid(16));
        for (&viewer, _) in self.players.iter().filter(|(_, p)| p.sent.contains(&chunk)) {
            self.send(viewer, std::slice::from_ref(&effect));
        }
    }
}
