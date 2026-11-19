//! Rottura dei blocchi: il client dice cosa ha rotto, ma è il server a decidere (modalità, portata, ganci) e a rimandare
//! lo stato vero se rifiuta. La rottura è istantanea; in sopravvivenza il blocco lascia a terra gli oggetti dovuti (`drops_for`).
use crate::{
    events::{BlockChange, WorldAction},
    inventory::Item,
    registry::{self, BlockState, StateValue},
    tables::{block_drops, block_hardness, tool, Drop, FALLBACK_HARDNESS},
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

/// `LevelEvent` delle crepe di rottura (gophertunnel `LevelEventStartBlockCracking`, `Stop`, `Update`): la posizione e' l'angolo del blocco
/// (Dragonfly `pos.Vec3()`), il dato di Start e Update e' `65535 / tick di rottura`.
const START_BLOCK_CRACKING: i32 = 3600;
const STOP_BLOCK_CRACKING: i32 = 3601;
const UPDATE_BLOCK_CRACKING: i32 = 3602;

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

/// Gli oggetti che il blocco `block` lascia se rotto con `held` in mano; `roll` dà numeri in [0, 1).
/// Senza voce nella tabella il blocco lascia se stesso (1), ma solo se il suo nome è anche un oggetto: quelli che in vanilla lasciano
/// altro (cartelli, ripetitori, canna da zucchero, colture) hanno una voce in `tools/extract_survival.py`, gli altri non lasciano niente.
/// Con voce: serve lo strumento del tipo giusto e di livello sufficiente, altrimenti niente. Le voci con `when` valgono solo se lo stato
/// del blocco (crescita, altezza) è nell'intervallo. Se le probabilità delle voci valide sommano 1 è un tiro solo (ghiaia: selce o
/// ghiaia), altrimenti ogni voce tira per conto suo.
pub(crate) fn drops_for(block: &BlockState, held: &Item, roll: &mut impl FnMut() -> f32) -> Vec<Item> {
    let Some(rules) = block_drops(&block.name) else {
        return Item::from_name(&block.name, 1).into_iter().collect();
    };
    if let Some(kind) = rules.tool {
        let tier = registry::items().name(held.id).and_then(tool).filter(|t| t.kind == kind).map(|t| t.tier);
        if !tier.is_some_and(|t| t >= rules.min_tier) {
            return Vec::new();
        }
    }
    let state_of = |key: &str| match block.get(key) {
        Some(StateValue::Int(v)) => Some(*v),
        Some(StateValue::Byte(v)) => Some(i32::from(*v)),
        _ => None,
    };
    // una voce con condizione su uno stato che il blocco non ha non vale mai (meglio niente di qualcosa di sbagliato)
    let valid: Vec<&Drop> =
        rules.drops.iter().filter(|d| d.when.as_ref().is_none_or(|w| state_of(&w.state).is_some_and(|v| (w.min..=w.max).contains(&v)))).collect();
    let exclusive = (valid.iter().map(|d| d.chance).sum::<f32>() - 1.0).abs() < 1e-4;
    let mut chosen = Vec::new();
    if exclusive {
        let mut r = roll();
        for d in &valid {
            if r < d.chance {
                chosen.push(d);
                break;
            }
            r -= d.chance;
        }
    } else {
        for d in &valid {
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

/// Quanti tick ci vogliono a rompere `block` con `held` in mano, come `BreakDuration` di Dragonfly: durezza del blocco, velocita' dello strumento
/// se e' quello efficace (e abbastanza forte per raccogliere: altrimenti 1), divisa per 5 sott'acqua; il danno per tick e' velocita' / durezza / 30
/// (/ 100 se lo strumento non basta per raccogliere); 0 se arriva a 1 in un tick. Non ci sono effetti ne' incantesimi, e si suppone il giocatore a
/// terra (il server non tiene `on_ground`): chi e' in aria rompe 5 volte piu' piano in Dragonfly, qui le crepe sarebbero un po' troppo rapide.
/// Un blocco senza voce in tabella ha durezza `FALLBACK_HARDNESS` e nessuno strumento efficace.
pub(crate) fn break_ticks(block: &BlockState, held: &Item, underwater: bool) -> u32 {
    let info = block_hardness(&block.name);
    let hardness = info.map_or(FALLBACK_HARDNESS, |i| i.hardness);
    if hardness <= 0.0 {
        return 0;
    }
    let held_tool = registry::items().name(held.id).and_then(tool);
    let harvestable = match block_drops(&block.name).and_then(|r| r.tool.map(|kind| (kind, r.min_tier))) {
        None => true,
        Some((kind, min_tier)) => held_tool.is_some_and(|t| t.kind == kind && t.tier >= min_tier),
    };
    let mut speed = 1.0f64;
    if let (Some(t), Some(effective)) = (held_tool, info.and_then(|i| i.effective)) {
        if t.kind == effective && harvestable {
            speed = f64::from(t.efficiency);
        }
    }
    if underwater {
        speed /= 5.0;
    }
    let damage = speed / hardness / if harvestable { 30.0 } else { 100.0 };
    if damage >= 1.0 {
        0
    } else {
        (1.0 / damage).ceil() as u32
    }
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
    /// Start e Continue scelgono il blocco che si sta rompendo (e in sopravvivenza ne mostrano le crepe: vedi `start_crack`), Abort lo
    /// dimentica, Stop e Predict lo rompono (solo se è quello scelto, come Dragonfly: D§7) e CreativeDestroy lo rompe subito, ma solo
    /// in creativo. Le altre azioni non servono.
    pub(crate) fn block_action(&mut self, pid: u64, action: i32, pos: [i32; 3]) {
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.spawned) else { return };
        match action {
            player_action::START_BREAK => self.start_breaking(pid, pos),
            // un Continue su un altro blocco da quello scelto è un nuovo inizio; sullo stesso blocco aggiorna la velocità delle crepe
            player_action::CONTINUE_DESTROY if p.breaking == Some(pos) => self.update_crack(pid, pos),
            player_action::CONTINUE_DESTROY => self.start_breaking(pid, pos),
            player_action::ABORT_BREAK => {
                p.breaking = None;
                self.stop_crack(pid);
            }
            player_action::STOP_BREAK | player_action::PREDICT_DESTROY => {
                let chosen = p.breaking.take() == Some(pos);
                self.stop_crack(pid);
                if chosen {
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

    /// Il giocatore comincia a rompere `pos` (Dragonfly `StartBreaking`: prima abbandona quello di prima, poi sceglie il nuovo).
    fn start_breaking(&mut self, pid: u64, pos: [i32; 3]) {
        self.stop_crack(pid);
        if let Some(p) = self.players.get_mut(&pid) {
            p.breaking = Some(pos);
        }
        self.start_crack(pid, pos);
    }

    /// Quanto ci mette `pid` a rompere il blocco in `pos` con quel che ha in mano; `None` se non si rompe a mano (aria, liquidi, roccia di
    /// fondo, chunk non caricato) o se la modalità non è sopravvivenza/avventura (in creativa la rottura è istantanea e non ha crepe).
    fn break_time(&self, pid: u64, pos: [i32; 3]) -> Option<u32> {
        let p = self.players.get(&pid)?;
        if !matches!(p.mode, GameMode::Survival | GameMode::Adventure) || !in_reach(p.pos, pos, false) {
            return None;
        }
        let (hash, _) = self.block_at(Dim::Overworld, pos)?;
        if hash == block_hash("minecraft:air", &[]) || hand_proof(hash, false) {
            return None;
        }
        let state = registry::blocks().state(hash)?;
        // sott'acqua se gli occhi sono in acqua (blocco d'acqua o secondo strato)
        let cell = [p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32];
        let underwater = self.block_at(Dim::Overworld, cell).is_some_and(|(hash, layer)| {
            layer || registry::blocks().state(hash).is_some_and(|s| matches!(s.name.as_str(), "minecraft:water" | "minecraft:flowing_water"))
        });
        Some(break_ticks(state, p.inventory.held(), underwater))
    }

    /// Le crepe cominciano: `LevelEvent` 3600 con `65535 / tick di rottura` a chi rompe e a chi vede il chunk (Dragonfly `StartCrackAction`).
    /// Una rottura istantanea (0 tick) non ha crepe da animare.
    fn start_crack(&mut self, pid: u64, pos: [i32; 3]) {
        let Some(ticks) = self.break_time(pid, pos).filter(|&t| t > 0) else { return };
        self.send_crack(pos, START_BLOCK_CRACKING, ticks);
        if let Some(p) = self.players.get_mut(&pid) {
            p.crack = Some((pos, ticks));
        }
    }

    /// Il tempo di rottura è cambiato mentre si rompe (altro strumento in mano, acqua): `LevelEvent` 3602 con la nuova velocità
    /// (Dragonfly `ContinueCrackAction`). Senza crepe in corso, o con una rottura ormai istantanea, non si manda niente.
    fn update_crack(&mut self, pid: u64, pos: [i32; 3]) {
        let Some(old) = self.players.get(&pid).and_then(|p| p.crack).filter(|&(at, _)| at == pos).map(|(_, ticks)| ticks) else { return };
        let Some(ticks) = self.break_time(pid, pos).filter(|&t| t > 0 && t != old) else { return };
        self.send_crack(pos, UPDATE_BLOCK_CRACKING, ticks);
        if let Some(p) = self.players.get_mut(&pid) {
            p.crack = Some((pos, ticks));
        }
    }

    /// Le crepe finiscono (rottura abbandonata, finita o interrotta dalla morte): `LevelEvent` 3601 (Dragonfly `StopCrackAction`). Se non
    /// ce n'erano non si manda niente.
    pub(crate) fn stop_crack(&mut self, pid: u64) {
        if let Some((pos, _)) = self.players.get_mut(&pid).and_then(|p| p.crack.take()) {
            self.send_crack(pos, STOP_BLOCK_CRACKING, 0);
        }
    }

    /// Manda il `LevelEvent` delle crepe a chi ha il chunk del blocco (chi rompe e gli spettatori).
    fn send_crack(&self, pos: [i32; 3], event_id: i32, ticks: u32) {
        let data = if ticks == 0 { 0 } else { (65535.0 / f64::from(ticks)) as i32 };
        let packet = LevelEvent { event_id, position: Vec3 { x: pos[0] as f32, y: pos[1] as f32, z: pos[2] as f32 }, data }.packet();
        let chunk = (pos[0].div_euclid(16), pos[2].div_euclid(16));
        for (&viewer, _) in self.players.iter().filter(|(_, p)| p.sent.contains(&chunk)) {
            self.send(viewer, std::slice::from_ref(&packet));
        }
    }

    /// Il giocatore rompe il blocco in `pos`. Se non può (modalità, portata, blocco indistruttibile, gancio che annulla)
    /// si rimanda lo stato vero a lui solo, per la cella e per i suoi vicini (l'altra metà di una porta o di un letto, la pianta alta). Altrimenti il blocco diventa aria (l'acqua del secondo strato resta) e chi vede
    /// il chunk riceve l'aggiornamento e l'effetto di rottura. L'aria e i blocchi non caricati non fanno nulla.
    pub(crate) fn break_block(&mut self, pid: u64, pos: [i32; 3]) {
        self.stop_crack(pid); // rotto o rifiutato, le crepe finiscono (arriva anche da `InventoryTransaction` Destroy, senza Stop prima)
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
                let drops = drops_for(state, &held, &mut || self.roll());
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
        // Dragonfly `BreakBlock`: `p.Exhaust(0.005)` (nulla in creativa: `exhaust` lo esclude)
        self.exhaust(pid, 0.005);
    }
}
