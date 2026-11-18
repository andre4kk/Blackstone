//! Rottura dei blocchi: il client dice cosa ha rotto, ma è il server a decidere (modalità, portata, ganci) e a rimandare
//! lo stato vero se rifiuta. In questa tappa la rottura è istantanea e senza oggetti lasciati a terra.
use crate::{
    events::{BlockChange, WorldAction},
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
                    self.resend_blocks(pid, Dim::Overworld, &[pos]);
                }
            }
            player_action::CREATIVE_DESTROY if p.mode == GameMode::Creative => self.break_block(pid, pos),
            player_action::CREATIVE_DESTROY => {
                log!("[#{pid}] rottura creativa fuori dalla modalità creativa: rifiutata");
                self.resend_blocks(pid, Dim::Overworld, &[pos]);
            }
            _ => {}
        }
    }

    /// Il giocatore rompe il blocco in `pos`. Se non può (modalità, portata, blocco indistruttibile, gancio che annulla)
    /// si rimanda lo stato vero a lui solo. Altrimenti il blocco diventa aria (l'acqua del secondo strato resta) e chi vede
    /// il chunk riceve l'aggiornamento e l'effetto di rottura. L'aria e i blocchi non caricati non fanno nulla.
    pub(crate) fn break_block(&mut self, pid: u64, pos: [i32; 3]) {
        let tick = self.tick_count;
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.spawned) else { return };
        // Il client può chiedere la stessa rottura per due strade nello stesso tick (PlayerAction e UseItem): vale una volta.
        if p.last_break == Some((tick, pos)) {
            return;
        }
        p.last_break = Some((tick, pos));
        let (mode, eyes) = (p.mode, p.pos);
        let dim = Dim::Overworld;
        let Some((old, water)) = self.block_at(dim, pos) else { return };
        if old == block_hash("minecraft:air", &[]) {
            return;
        }
        let creative = mode == GameMode::Creative;
        let editable = matches!(mode, GameMode::Creative | GameMode::Survival);
        if !editable || !in_reach(eyes, pos, creative) || hand_proof(old, creative) || !self.allow(&WorldAction::Break { pid, pos, old }) {
            log!("[#{pid}] rottura in {pos:?} rifiutata");
            return self.resend_blocks(pid, dim, &[pos]);
        }
        self.set_blocks(dim, &[BlockChange { pos, hash: block_hash("minecraft:air", &[]), water }]);
        let effect = LevelEvent {
            event_id: PARTICLES_DESTROY_BLOCK,
            position: Vec3 { x: pos[0] as f32 + 0.5, y: pos[1] as f32 + 0.5, z: pos[2] as f32 + 0.5 },
            data: old,
        }
        .packet();
        let chunk = (pos[0].div_euclid(16), pos[2].div_euclid(16));
        for (&viewer, _) in self.players.iter().filter(|(_, p)| p.sent.contains(&chunk)) {
            self.send(viewer, std::slice::from_ref(&effect));
        }
    }
}
