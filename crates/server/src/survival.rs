//! Vita, danno e cura del giocatore, sul modello di Dragonfly (`player.go`: `Hurt`, `Heal`, `addHealth`).
// finché i task successivi (morte, fame, combattimento) non lo chiamano, è usato solo dai test
#![allow(dead_code)]
use crate::{events::WorldAction, join::vitals_attributes, playerdata::EYE_HEIGHT, GameMode, Server};
use bedrock_net::log;
use bedrock_protocol::{
    entity::{actor_event, actor_event_type},
    generated::UpdateAttributes,
    wire::ServerPacket,
};
use vgen::Dim;

/// Vita massima (20 = dieci cuori) e fame massima.
pub(crate) const MAX_HEALTH: f32 = 20.0;
pub(crate) const MAX_FOOD: u8 = 20;
/// Saturazione massima (come la fame) e quella di partenza (vanilla e Dragonfly).
pub(crate) const MAX_SATURATION: f32 = 20.0;
pub(crate) const START_SATURATION: f32 = 5.0;
/// Dopo un colpo si è invulnerabili per mezzo secondo (Dragonfly: `immunity := time.Second / 2`).
const IMMUNITY_TICKS: u64 = 10;

/// Da cosa viene un danno.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DamageCause {
    /// Caduta da troppo in alto.
    Fall,
    /// Fuori dal mondo (sotto il fondo).
    Void,
    /// Fame a zero.
    Starvation,
    /// Colpito dall'entità `attacker`.
    Attack { attacker: u64 },
    /// Comando `/kill`.
    Kill,
}

impl DamageCause {
    /// Vuoto e `/kill` colpiscono anche in creativa e ignorano l'invulnerabilità dopo un colpo (vanilla).
    fn unavoidable(&self) -> bool {
        matches!(self, DamageCause::Void | DamageCause::Kill)
    }
}

impl Server {
    /// Fa subire `amount` di danno a `pid`. Restituisce `true` se il colpo è andato a segno.
    ///
    /// Come Dragonfly `Hurt`: niente danno a chi è morto, in creativa o da spettatore (il vuoto e `/kill` colpiscono anche in creativa),
    /// né per quantità non positive. Dentro l'invulnerabilità (10 tick dopo un colpo) conta solo la parte che supera l'ultimo danno,
    /// e se non la supera il colpo è ignorato. I ganci vedono quanto danno resta davvero (`WorldAction::Damage`) e possono annullarlo.
    /// A vita 0 chiama [`Server::die`].
    pub(crate) fn hurt(&mut self, pid: u64, amount: f32, cause: DamageCause) -> bool {
        let tick = self.tick_count;
        let Some(p) = self.players.get(&pid) else { return false };
        let vulnerable = match p.mode {
            GameMode::Survival | GameMode::Adventure => true,
            GameMode::Creative => cause.unavoidable(),
            GameMode::Spectator => false,
        };
        if p.dead || !vulnerable || amount.is_nan() || amount <= 0.0 {
            return false;
        }
        let mut left = amount;
        if !cause.unavoidable() && tick < p.invulnerable_until {
            left -= p.last_damage;
            if left <= 0.0 {
                return false;
            }
        }
        if !self.allow(&WorldAction::Damage { pid, amount: left, cause: cause.clone() }) {
            return false;
        }
        let Some(p) = self.players.get_mut(&pid) else { return false };
        p.invulnerable_until = tick + IMMUNITY_TICKS;
        p.last_damage = amount;
        p.health = (p.health - left).max(0.0);
        let dead = p.health <= f32::EPSILON;
        if dead {
            p.health = 0.0;
        }
        self.send_vitals(pid);
        // l'animazione del colpo la vedono lui e chi lo guarda (Dragonfly: `viewer.ViewEntityAction(p, HurtAction{})`)
        let event = actor_event(pid, actor_event_type::HURT, 0);
        self.send(pid, std::slice::from_ref(&event));
        self.broadcast_to_viewers(pid, std::slice::from_ref(&event));
        if dead {
            self.die(pid, cause);
        }
        true
    }

    /// Aggiorna la caduta di `pid` dopo un `PlayerAuthInput` con i piedi a `feet_y`. Va chiamato prima di aggiornare `p.pos`:
    /// la posizione precedente serve a capire se sta scendendo.
    ///
    /// Idea di Dragonfly (`updateFallState`/`fall`): scendendo si accumula la distanza; salendo o fermi in aria la si azzera; all'atterraggio
    /// il danno è `ceil(distanza - 3)` se `distanza - 3 >= 0.5`. Qui la distanza è dal punto più alto della discesa (`fall_start_y`).
    /// Il client non dice "sono a terra" in modo esplicito, ma manda `VERTICAL_COLLISION` quando urta in verticale: è a terra se il
    /// flag c'è e non sta salendo (il flag vale anche per la testa contro un soffitto, ma lì si sale). Si azzera anche in acqua,
    /// in volo, in planata e fuori da sopravvivenza/avventura.
    pub(crate) fn fall_tick(&mut self, pid: u64, feet_y: f32, on_ground: bool) {
        let in_water = self.feet_resets_fall(pid, feet_y);
        let Some(p) = self.players.get_mut(&pid) else { return };
        let before = p.pos[1] - EYE_HEIGHT;
        let falls = matches!(p.mode, GameMode::Survival | GameMode::Adventure) && !p.dead && !p.flying && !p.gliding && !in_water;
        if !falls {
            p.fall_start_y = None;
            return;
        }
        if on_ground && feet_y <= before {
            let distance = p.fall_start_y.take().unwrap_or(before) - feet_y;
            let damage = distance - 3.0;
            if damage >= 0.5 {
                self.hurt(pid, damage.ceil(), DamageCause::Fall);
            }
        } else if feet_y < before {
            p.fall_start_y.get_or_insert(before);
        } else {
            p.fall_start_y = None;
        }
    }

    /// Vero se la cella dei piedi di `pid` (a `feet_y`) azzera la caduta. Come Dragonfly (`fallDistanceEntity` di acqua, lava, scala, rampicante,
    /// ragnatela): acqua (strato dei liquidi o blocco), lava, scala, vite e ragnatela. Il client si arrampica già da solo.
    fn feet_resets_fall(&self, pid: u64, feet_y: f32) -> bool {
        const BLOCKS: [&str; 7] =
            ["minecraft:water", "minecraft:flowing_water", "minecraft:lava", "minecraft:flowing_lava", "minecraft:ladder", "minecraft:vine", "minecraft:web"];
        let Some(p) = self.players.get(&pid) else { return false };
        let cell = [p.pos[0].floor() as i32, feet_y.floor() as i32, p.pos[2].floor() as i32];
        self.block_at(Dim::Overworld, cell)
            .is_some_and(|(hash, layer)| layer || crate::registry::blocks().state(hash).is_some_and(|s| BLOCKS.contains(&s.name.as_str())))
    }

    /// Chi ha i piedi sotto il fondo del mondo (come Dragonfly: `y < range[0]`, -64 nel mondo) subisce 4 di danno, anche in creativa
    /// (non da spettatore). Ogni 10 tick: il vuoto ignora l'invulnerabilità, quindi il ritmo lo dà questo controllo.
    pub(crate) fn void_tick(&mut self) {
        if !self.tick_count.is_multiple_of(IMMUNITY_TICKS) {
            return;
        }
        let bottom = Dim::Overworld.height_range().0 as f32;
        let below: Vec<u64> = self.players.iter().filter(|(_, p)| p.spawned && p.pos[1] - EYE_HEIGHT < bottom).map(|(&pid, _)| pid).collect();
        for pid in below {
            self.hurt(pid, 4.0, DamageCause::Void);
        }
    }

    /// Cura `pid` di `amount` (senza superare la vita massima). Come Dragonfly `Heal`: nulla per i morti, per chi non può subire
    /// danno (creativa, spettatore) e per quantità negative o NaN; i ganci non vedono la cura (non c'è un'azione dedicata).
    pub(crate) fn heal(&mut self, pid: u64, amount: f32) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        if p.dead || !matches!(p.mode, GameMode::Survival | GameMode::Adventure) || amount.is_nan() || amount < 0.0 {
            return;
        }
        let before = p.health;
        p.health = (p.health + amount).min(MAX_HEALTH);
        if p.health != before {
            self.send_vitals(pid);
        }
    }

    /// Manda a `pid` vita, fame, saturazione ed esaurimento attuali (`UpdateAttributes`, stessi nomi e massimi dello spawn).
    pub(crate) fn send_vitals(&self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let packet = UpdateAttributes { target_runtime_id: pid, attribute_list: vitals_attributes(p), tick: 0 }.packet();
        self.send(pid, &[packet]);
    }

    /// La vita di `pid` è arrivata a 0. Per ora segna solo che è morto: il resto (messaggio, oggetti a terra, rinascita) è del Task 5.
    pub(crate) fn die(&mut self, pid: u64, cause: DamageCause) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        p.dead = true;
        log!("[#{pid}] morto: {cause:?}");
    }
}
