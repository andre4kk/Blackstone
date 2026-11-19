//! Giocatore finto (`/bot`): permette di provare entità, animazioni e oggetti a terra con un solo account.
//! Il bot è un'entità giocatore senza connessione (`pid: None`) con la skin di chi lo crea; gira in cerchio attorno al punto di creazione,
//! salta, agita il braccio, si accovaccia, cambia l'oggetto in mano e lancia una pietra (dal nulla, scelta del design).
//! Il bot resta nel mondo anche se chi l'ha creato esce: si toglie solo con `/bot rimuovi`.
//! Altezza: il bot tiene la quota di chi l'ha creato (non cerca il terreno sotto il suo percorso); il salto è un breve arco sopra quella quota.
use crate::{
    entity::{Entity, EntityKind},
    inventory::{window, Item},
    registry,
    survival::{IMMUNITY_TICKS, MAX_HEALTH},
    Server,
};
use blackstone_net::log;
use blackstone_protocol::{
    actor::set_actor_data,
    entity::{actor_event, actor_event_type, animate_swing, player_list_remove},
    generated::MobEquipment,
    skin::Skin,
    wire::ServerPacket,
    Packet,
};
use std::f32::consts::{PI, TAU};

/// Raggio del cerchio, in blocchi.
const RADIUS: f32 = 3.0;
/// Tick per un giro completo.
const LAP_TICKS: u64 = 200;
/// Ogni quanti tick salta, e per quanti tick sta in aria.
const JUMP_EVERY: u64 = 60;
const JUMP_TICKS: u64 = 10;
/// Altezza massima del salto.
const JUMP_HEIGHT: f32 = 1.0;
const SWING_EVERY: u64 = 40;
/// Ogni quanti tick si accovaccia, e per quanti tick resta così.
const SNEAK_EVERY: u64 = 100;
const SNEAK_TICKS: u64 = 20;
const HOLD_EVERY: u64 = 100;
const THROW_EVERY: u64 = 200;
/// Attesa prima che la pietra lanciata si possa raccogliere, e velocità del lancio.
const THROW_PICKUP_DELAY: u64 = 40;
const THROW_SPEED: f32 = 0.3;
/// Quanti bot al massimo, e quanti caratteri al massimo nel nome (come un gamertag Xbox).
pub(crate) const MAX_BOTS: usize = 8;
const MAX_NAME_CHARS: usize = 16;
/// Altezza degli occhi sopra i piedi.
const EYE_HEIGHT: f32 = 1.62;
/// Oggetti che il bot tiene in mano a turno (quelli che esistono nel registro).
const HELD_ITEMS: &[&str] = &["minecraft:stone", "minecraft:diamond_sword", "minecraft:apple", "minecraft:iron_pickaxe"];

pub(crate) struct Bot {
    pub id: u64,
    /// Skin di chi l'ha creato: serve alla sua voce nella `PlayerList`.
    pub skin: Skin,
    /// Il centro del cerchio: la posizione degli occhi alla creazione.
    center: [f32; 3],
    /// Il tick di creazione.
    born: u64,
    pub sneaking: bool,
    pub held: Item,
    /// Vita (0..=20): a 0 il bot sparisce come con `/bot rimuovi`.
    pub health: f32,
    /// Fino a questo tick (escluso) il bot è invulnerabile dopo un colpo; `last_damage` è il danno di quel colpo (come i giocatori).
    invulnerable_until: u64,
    last_damage: f32,
}

fn held_item(n: usize) -> Item {
    let ids: Vec<i16> = HELD_ITEMS.iter().filter_map(|name| registry::items().id(name)).collect();
    if ids.is_empty() {
        return Item::default();
    }
    Item { id: ids[n % ids.len()], count: 1, ..Default::default() }
}

/// L'UUID del bot, stabile: ricavato dal suo ID di entità.
pub(crate) fn bot_uuid(id: u64) -> [u8; 16] {
    let mut uuid = [0u8; 16];
    uuid[..8].copy_from_slice(b"BLKSTONE");
    uuid[8..].copy_from_slice(&id.to_be_bytes());
    uuid
}

impl Server {
    /// Il nome da dare a un bot: senza caratteri di controllo e senza `§`, senza spazi ai bordi, al massimo 16 caratteri ("Bot" se resta vuoto).
    /// Rifiuta (con il messaggio per chi scrive) un nome uguale, senza badare alle maiuscole, a quello di un giocatore o di un altro bot, e il bot oltre il limite.
    pub(crate) fn bot_name(&self, raw: &str) -> Result<String, String> {
        if self.bots.len() >= MAX_BOTS {
            return Err(format!("§cCi sono già {MAX_BOTS} bot: toglili con /bot rimuovi."));
        }
        let cleaned: String = raw.chars().filter(|c| !c.is_control() && *c != '§').collect();
        let cleaned: String = cleaned.trim().chars().take(MAX_NAME_CHARS).collect();
        let name = cleaned.trim_end().to_string();
        let name = if name.is_empty() { "Bot".to_string() } else { name };
        let taken = self.players.iter().filter(|(_, p)| p.spawned).any(|(&id, _)| self.name(id).to_lowercase() == name.to_lowercase())
            || self.bots.iter().any(
                |b| matches!(self.entities.get(b.id).map(|e| &e.kind), Some(EntityKind::Player { name: n, .. }) if n.to_lowercase() == name.to_lowercase()),
            );
        if taken {
            return Err(format!("§cIl nome {name} è già in uso da un giocatore o da un bot."));
        }
        Ok(name)
    }

    /// Crea un bot con `name` nel punto di `by`, con la sua skin. Restituisce l'ID dell'entità (0 se `by` non è nel mondo).
    /// Lo vedranno dal prossimo `visibility_tick` i giocatori che hanno il chunk; la voce nella `PlayerList` va subito a tutti.
    pub(crate) fn spawn_bot(&mut self, by: u64, name: &str) -> u64 {
        let Some(p) = self.players.get(&by).filter(|p| p.spawned) else { return 0 };
        let Some(info) = p.info.as_ref() else { return 0 };
        let (skin, center, yaw) = (info.skin.clone(), p.pos, p.yaw);
        let id = self.entities.alloc();
        self.entities.insert(Entity {
            id,
            kind: EntityKind::Player { pid: None, name: name.to_string(), uuid: bot_uuid(id) },
            pos: [center[0] + RADIUS, center[1], center[2]],
            pitch: 0.0,
            yaw,
            head_yaw: yaw,
            on_ground: true,
            moved: false,
        });
        self.bots.push(Bot {
            id,
            skin,
            center,
            born: self.tick_count,
            sneaking: false,
            held: held_item(0),
            health: MAX_HEALTH,
            invulnerable_until: 0,
            last_damage: 0.0,
        });
        if let Some(list) = self.player_list_add_packet(&[], &[id]) {
            self.send_to_spawned(&list);
        }
        log!("[bot] #{id} \"{name}\" creato da #{by}");
        id
    }

    /// Toglie tutti i bot: spariscono a chi li vedeva e dalla `PlayerList` di tutti.
    pub(crate) fn remove_bots(&mut self) {
        let bots = std::mem::take(&mut self.bots);
        if bots.is_empty() {
            return;
        }
        for b in &bots {
            self.despawn(b.id);
        }
        let uuids: Vec<[u8; 16]> = bots.iter().map(|b| bot_uuid(b.id)).collect();
        self.send_to_spawned(&player_list_remove(&uuids));
        log!("[bot] rimossi {} bot", bots.len());
    }

    /// Il bot `id` subisce `amount` di danno. Restituisce `true` se il colpo è andato a segno (`false` se il bot non esiste, la quantità non è
    /// positiva o è dentro l'invulnerabilità: come per i giocatori, 10 tick dopo un colpo conta solo la parte che supera l'ultimo danno).
    /// Chi lo vede riceve l'animazione del colpo; a vita 0 il bot sparisce (corpo e voce nella `PlayerList`) come con `/bot rimuovi`, senza oggetti a terra.
    pub(crate) fn hurt_bot(&mut self, id: u64, amount: f32) -> bool {
        let tick = self.tick_count;
        let Some(bot) = self.bots.iter_mut().find(|b| b.id == id) else { return false };
        if amount.is_nan() || amount <= 0.0 {
            return false;
        }
        let mut left = amount;
        if tick < bot.invulnerable_until {
            left -= bot.last_damage;
            if left <= 0.0 {
                return false;
            }
        }
        bot.invulnerable_until = tick + IMMUNITY_TICKS;
        bot.last_damage = amount;
        bot.health = (bot.health - left).max(0.0);
        let dead = bot.health <= f32::EPSILON;
        self.broadcast_to_viewers(id, &[actor_event(id, actor_event_type::HURT, 0)]);
        if dead {
            self.remove_bot(id);
        }
        true
    }

    /// Toglie il bot `id`: sparisce a chi lo vedeva e dalla `PlayerList` di tutti.
    fn remove_bot(&mut self, id: u64) {
        self.bots.retain(|b| b.id != id);
        self.despawn(id);
        self.send_to_spawned(&player_list_remove(&[bot_uuid(id)]));
        log!("[bot] #{id} morto");
    }

    fn send_to_spawned(&self, packet: &Packet) {
        for (&pid, _) in self.players.iter().filter(|(_, p)| p.spawned) {
            self.send(pid, std::slice::from_ref(packet));
        }
    }

    /// Un tick per ogni bot: il passo sul cerchio e le azioni a tempo (salto, braccio, accovacciamento, oggetto in mano, lancio).
    /// Il movimento lo manda `movement_tick`.
    pub(crate) fn bots_tick(&mut self) {
        for i in 0..self.bots.len() {
            let (id, center, age) = (self.bots[i].id, self.bots[i].center, self.tick_count + 1 - self.bots[i].born);
            let angle = TAU * (age % LAP_TICKS) as f32 / LAP_TICKS as f32;
            // Il bot gira in senso antiorario: la direzione del moto (-sin, cos) corrisponde, con 0 = sud e 90 = ovest, a un'imbardata pari all'angolo.
            let yaw = (angle.to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
            let phase = age % JUMP_EVERY;
            let jump = if phase < JUMP_TICKS { JUMP_HEIGHT * (PI * phase as f32 / JUMP_TICKS as f32).sin() } else { 0.0 };
            let at = [center[0] + RADIUS * angle.cos(), center[1] + jump, center[2] + RADIUS * angle.sin()];
            if let Some(e) = self.entities.get_mut(id) {
                e.pos = at;
                e.yaw = yaw;
                e.head_yaw = yaw;
                e.on_ground = jump == 0.0;
                e.moved = true;
            }
            if age % SWING_EVERY == 0 {
                self.broadcast_to_viewers(id, &[animate_swing(id)]);
            }
            let sneaking = age % SNEAK_EVERY < SNEAK_TICKS;
            if sneaking != self.bots[i].sneaking {
                self.bots[i].sneaking = sneaking;
                self.broadcast_to_viewers(id, &[set_actor_data(id, &self.entity_data(id))]);
            }
            if age % HOLD_EVERY == 0 {
                let held = held_item((age / HOLD_EVERY) as usize);
                let packet =
                    MobEquipment { target_runtime_id: id, item: held.stack(0), slot: 0, selected_slot: 0, container_id: window::INVENTORY as u8 }.packet();
                self.bots[i].held = held;
                self.broadcast_to_viewers(id, &[packet]);
            }
            if age % THROW_EVERY == 0 {
                let feet = [at[0], at[1] - EYE_HEIGHT + 0.5, at[2]];
                let rad = yaw.to_radians();
                let velocity = [-rad.sin() * THROW_SPEED, 0.2, rad.cos() * THROW_SPEED];
                let stone = Item { id: registry::items().id("minecraft:stone").unwrap_or(0), count: 1, ..Default::default() };
                self.spawn_item(stone, feet, velocity, THROW_PICKUP_DELAY);
            }
        }
    }
}
