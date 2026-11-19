//! Vita, danno e cura del giocatore, sul modello di Dragonfly (`player.go`: `Hurt`, `Heal`, `addHealth`).
use crate::{
    entity::{armor_of, EntityKind},
    events::WorldAction,
    inventory::{Item, HOTBAR_SLOTS},
    join::vitals_attributes,
    playerdata::EYE_HEIGHT,
    registry, tables, GameMode, Player, Server,
};
use bedrock_net::log;
use bedrock_protocol::{
    actor::set_actor_data,
    entity::{actor_event, actor_event_type, move_player_teleport, remove_actor, respawn, set_actor_motion},
    generated::UpdateAttributes,
    input::{respawn_state, ReleaseItem, UseItem},
    item::ItemStack,
    packets::{text_type, Text},
    wire::ServerPacket,
};
use vgen::Dim;

/// Vita massima (20 = dieci cuori) e fame massima.
pub(crate) const MAX_HEALTH: f32 = 20.0;
pub(crate) const MAX_FOOD: u8 = 20;
/// Saturazione massima (come la fame) e quella di partenza (vanilla e Dragonfly).
pub(crate) const MAX_SATURATION: f32 = 20.0;
pub(crate) const START_SATURATION: f32 = 5.0;
/// Fame minima per rigenerare (Dragonfly `canRegenerate`: 18-20) e fame fino a cui non si corre (`canSprint`: serve più di 6).
const REGEN_FOOD: u8 = 18;
const NO_SPRINT_FOOD: u8 = 6;
/// A difficoltà normale la fame toglie vita solo sopra questo valore (vanilla: si arriva a mezzo cuore; il 2 di Dragonfly è una sua scelta).
const STARVATION_HEALTH_LIMIT: f32 = 1.0;
/// Dopo un colpo si è invulnerabili per mezzo secondo (Dragonfly: `immunity := time.Second / 2`).
pub(crate) const IMMUNITY_TICKS: u64 = 10;
/// Tick prima che gli oggetti lasciati da chi muore si possano raccogliere (mezzo secondo).
const DEATH_PICKUP_DELAY: u64 = 10;
/// Quanto resta visibile il corpo di chi è morto: 1,1 s come Dragonfly (`finishDying`).
pub(crate) const BODY_TICKS: u64 = 22;

/// Quanto serve per mangiare: 32 tick (Dragonfly `DefaultConsumeDuration`, 1,61 s). L'alga secca metà, la bottiglia di miele un quarto in più (come Dragonfly).
const CONSUME_TICKS: u64 = 32;
/// Ogni quanti tick di uso compaiono le particelle del cibo (Dragonfly: `current%4 == 0`).
const EAT_EFFECT_TICKS: u64 = 4;

/// Tick di uso per finire `name`.
fn consume_ticks(name: &str) -> u64 {
    match name {
        "minecraft:dried_kelp" => CONSUME_TICKS / 2,
        "minecraft:honey_bottle" => CONSUME_TICKS * 5 / 4,
        _ => CONSUME_TICKS,
    }
}

/// Cosa resta in mano dopo aver mangiato `name` (le zuppe lasciano la ciotola, il miele la bottiglia: `Consume` di Dragonfly).
fn leftover(name: &str) -> Option<&'static str> {
    match name {
        "minecraft:mushroom_stew" | "minecraft:rabbit_stew" | "minecraft:beetroot_soup" | "minecraft:suspicious_stew" => Some("minecraft:bowl"),
        "minecraft:honey_bottle" => Some("minecraft:glass_bottle"),
        _ => None,
    }
}

/// Nome e valori dell'oggetto se è un cibo (e non è vuoto).
fn edible(item: &Item) -> Option<(&'static str, &'static tables::Food)> {
    if item.is_empty() {
        return None;
    }
    let name = registry::items().name(item.id)?;
    Some((name, tables::food(name)?))
}

/// Da cosa viene un danno.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DamageCause {
    /// Caduta da troppo in alto.
    Fall,
    /// Fuori dal mondo (sotto il fondo).
    Void,
    /// Fame a zero.
    Starvation,
    /// Colpito dall'entità `attacker` (la userà il combattimento, Task 8; per ora i test).
    #[allow(dead_code)]
    Attack { attacker: u64 },
    /// Comando `/kill`.
    Kill,
}

impl DamageCause {
    /// Vuoto e `/kill` colpiscono anche in creativa e ignorano l'invulnerabilità dopo un colpo (vanilla).
    fn unavoidable(&self) -> bool {
        matches!(self, DamageCause::Void | DamageCause::Kill)
    }

    /// Dragonfly `Hurt`: solo i danni ridotti dall'armatura (`ReducedByArmour`) costano 0,1 di esaurimento. Fame, vuoto e `/kill` no.
    fn reduced_by_armour(&self) -> bool {
        matches!(self, DamageCause::Fall | DamageCause::Attack { .. })
    }
}

impl Player {
    /// Dragonfly `hungerManager.exhaust`: somma l'esaurimento e, a ogni 4 punti, toglie 1 di saturazione o, a saturazione 0, 1 di fame
    /// (`desaturate`). Non guarda modalità né morte: lo fa [`Server::exhaust`].
    fn exhaust(&mut self, points: f32) {
        self.exhaustion += points;
        while self.exhaustion >= 4.0 {
            self.exhaustion -= 4.0;
            if self.saturation <= 0.0 && self.food != 0 {
                self.food -= 1;
            } else if self.saturation > 0.0 {
                self.saturation = (self.saturation - 1.0).max(0.0);
            }
        }
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
        if cause.reduced_by_armour() {
            self.exhaust(pid, 0.1); // prima dell'invio: l'ultimo `UpdateAttributes` porta anche l'esaurimento nuovo
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
        // Elenco di Dragonfly più i rampicanti che Dragonfly non ha ma il client fa scalare (vite del Nether, impalcatura).
        const BLOCKS: [&str; 12] = [
            "minecraft:water",
            "minecraft:flowing_water",
            "minecraft:lava",
            "minecraft:flowing_lava",
            "minecraft:ladder",
            "minecraft:vine",
            "minecraft:web",
            "minecraft:twisting_vines",
            "minecraft:weeping_vines",
            "minecraft:cave_vines",
            "minecraft:cave_vines_body_with_berries",
            "minecraft:scaffolding",
        ];
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

    /// Cura `pid` di `amount` (senza superare la vita massima) e restituisce quanta vita è tornata davvero. Come Dragonfly `Heal`: nulla per i
    /// morti, per chi non può subire danno (creativa, spettatore) e per quantità negative o NaN; i ganci non vedono la cura (non c'è un'azione dedicata).
    pub(crate) fn heal(&mut self, pid: u64, amount: f32) -> f32 {
        let Some(p) = self.players.get_mut(&pid) else { return 0.0 };
        if p.dead || !matches!(p.mode, GameMode::Survival | GameMode::Adventure) || amount.is_nan() || amount < 0.0 {
            return 0.0;
        }
        let before = p.health;
        p.health = (p.health + amount).min(MAX_HEALTH);
        let healed = p.health - before;
        if healed != 0.0 {
            self.send_vitals(pid);
        }
        healed
    }

    /// Stanca `pid` di `points` (Dragonfly `Exhaust`): nulla per chi non subisce danno (creativa, spettatore) e per i morti. A ogni 4 punti di
    /// esaurimento si toglie 1 di saturazione o, a saturazione 0, 1 di fame. `UpdateAttributes` parte solo se fame o saturazione sono cambiate
    /// (l'esaurimento da solo no: cambia a ogni passo e il client non ne ha bisogno finché non cambia la barra). Se la fame scende a 6 o meno
    /// la corsa si ferma, come Dragonfly (`StopSprinting`). Non c'è l'evento `HandleFoodLoss`: i ganci non vedono la perdita di fame.
    pub(crate) fn exhaust(&mut self, pid: u64, points: f32) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        if p.dead || !matches!(p.mode, GameMode::Survival | GameMode::Adventure) {
            return;
        }
        let before = (p.food, p.saturation);
        p.exhaust(points);
        if before != (p.food, p.saturation) {
            self.stop_sprint_if_too_hungry(pid);
            self.send_vitals(pid);
        }
    }

    /// Dragonfly `canSprint`: con fame 6 o meno non si corre. Ferma la corsa e lo dice a chi guarda.
    fn stop_sprint_if_too_hungry(&mut self, pid: u64) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        if p.food <= NO_SPRINT_FOOD && p.sprinting {
            p.sprinting = false;
            self.send_entity_state(pid);
        }
    }

    /// Esaurimento da movimento orizzontale (Dragonfly `Move`: 0,01 per blocco a nuoto, 0,1 per blocco in corsa), dalla distanza tra la
    /// posizione attuale e quella nuova (occhi, quindi la differenza verticale non conta). Spostamenti oltre `MAX_STEP` in un tick sono
    /// teletrasporti, non passi.
    pub(crate) fn exhaust_by_movement(&mut self, pid: u64, new_eyes: [f32; 3]) {
        const MAX_STEP: f32 = 8.0;
        let Some(p) = self.players.get(&pid) else { return };
        let distance = (new_eyes[0] - p.pos[0]).hypot(new_eyes[2] - p.pos[2]);
        if !distance.is_finite() || distance > MAX_STEP {
            return;
        }
        if p.swimming {
            self.exhaust(pid, 0.01 * distance);
        } else if p.sprinting {
            self.exhaust(pid, 0.1 * distance);
        }
    }

    /// Un tick di fame per ogni giocatore vivo in sopravvivenza o avventura (Dragonfly `tickFood`, difficoltà fissa "normale"):
    /// a `food_tick` 1 con fame 18 o più si recupera mezzo cuore (e si spende 6 di esaurimento), a fame 0 si perde mezzo cuore
    /// finché ne resta almeno uno; con fame 20 e saturazione si recupera mezzo cuore anche ogni 20 tick (la regola rapida di Dragonfly,
    /// che nel codice scatta a `foodTick % 10 == 0 && % 20 == 0`, cioè ogni 20 tick e non ogni 10 come dice il suo commento). Il contatore va da 1 a 80.
    pub(crate) fn hunger_tick(&mut self) {
        let pids: Vec<u64> = self
            .players
            .iter()
            .filter(|(_, p)| p.spawned && !p.dead && matches!(p.mode, GameMode::Survival | GameMode::Adventure))
            .map(|(&pid, _)| pid)
            .collect();
        for pid in pids {
            let Some(p) = self.players.get(&pid) else { continue };
            let (tick, food, saturation) = (p.food_tick, p.food, p.saturation);
            if tick % 20 == 0 && food == MAX_FOOD && saturation > 0.0 {
                self.regenerate(pid);
            }
            if tick == 1 {
                let food = self.players.get(&pid).map_or(0, |p| p.food);
                if food >= REGEN_FOOD {
                    self.regenerate(pid);
                } else if food == 0 {
                    self.starve(pid);
                }
            }
            self.stop_sprint_if_too_hungry(pid);
            if let Some(p) = self.players.get_mut(&pid) {
                p.food_tick = if p.food_tick >= 80 { 1 } else { p.food_tick + 1 };
            }
        }
    }

    /// Dragonfly `regenerate`: mezzo cuore di vita, e se è tornato davvero costa 6 di esaurimento. Niente a vita piena.
    fn regenerate(&mut self, pid: u64) {
        if self.players.get(&pid).is_none_or(|p| p.health >= MAX_HEALTH) {
            return;
        }
        if self.heal(pid, 1.0) > 0.0 {
            self.exhaust(pid, 6.0);
        }
    }

    /// Dragonfly `starve`: un punto di danno da fame (a difficoltà normale solo se resta più di 1 di vita: la fame non uccide).
    fn starve(&mut self, pid: u64) {
        if self.players.get(&pid).is_some_and(|p| p.health > STARVATION_HEALTH_LIMIT) {
            self.hurt(pid, 1.0, DamageCause::Starvation);
        }
    }

    /// Il client usa l'oggetto in mano nell'aria (`InventoryTransaction` ItemUse, azione ClickAir). Sul modello di Dragonfly `UseItem` per i
    /// cibi (`item.Consumable`): la casella dichiarata dal client diventa quella in mano (se l'oggetto non è quello del server si rimanda la
    /// casella e non si fa nulla, come `VerifyAndSetHeldSlot`); fuori dai cibi non succede niente. Con la pancia piena (fame 20, in
    /// sopravvivenza o avventura) un cibo normale non si mangia (la mela d'oro e il frutto del coro sì); la creativa mangia sempre. La prima
    /// volta comincia a mangiare (`eating_since`, flag "usa oggetto" a chi guarda); gli usi seguenti, mentre mangia, finiscono il pasto
    /// se sono passati abbastanza tick e se no non fanno nulla. Gli spettatori non usano oggetti.
    pub(crate) fn use_item_in_air(&mut self, pid: u64, u: &UseItem) {
        if !self.players.get(&pid).is_some_and(|p| p.spawned && !p.dead && p.mode != GameMode::Spectator) || !self.adopt_held_slot(pid, u.hotbar_slot, &u.held)
        {
            return;
        }
        let Some(p) = self.players.get(&pid) else { return };
        let Some((name, food)) = edible(p.inventory.held()) else { return self.stop_eating(pid) };
        if !food.always_edible && matches!(p.mode, GameMode::Survival | GameMode::Adventure) && p.food >= MAX_FOOD {
            return self.stop_eating(pid); // Dragonfly: `ReleaseItem()` senza consumare
        }
        let Some(since) = p.eating_since else {
            if let Some(p) = self.players.get_mut(&pid) {
                p.eating_since = Some(self.tick_count);
            }
            return self.send_entity_state(pid);
        };
        if self.tick_count.saturating_sub(since) >= consume_ticks(name) {
            self.finish_eating(pid);
        }
    }

    /// Il client rilascia l'oggetto che teneva premuto (`InventoryTransaction` ReleaseItem). Dragonfly lo tratta come un annullamento del cibo
    /// (il pasto finisce con un altro uso, vedi [`Server::use_item_in_air`]); qui, se il client lo manda al posto di quell'uso e i tick sono
    /// passati, il pasto finisce comunque: il rilascio senza abbastanza tick lo annulla. La mano secondaria non mangia (si rimanda l'inventario).
    pub(crate) fn release_item(&mut self, pid: u64, r: &ReleaseItem) {
        if !self.players.get(&pid).is_some_and(|p| p.spawned && !p.dead) {
            return;
        }
        if r.hand != 0 {
            return self.send_inventory(pid);
        }
        if !self.adopt_held_slot(pid, r.hotbar_slot, &r.held) {
            return;
        }
        let Some(p) = self.players.get(&pid) else { return };
        if let (Some(since), Some((name, _)), true) = (p.eating_since, edible(p.inventory.held()), p.mode != GameMode::Spectator) {
            if self.tick_count.saturating_sub(since) >= consume_ticks(name) {
                self.finish_eating(pid);
            }
        }
        self.stop_eating(pid);
    }

    /// La casella della barra dichiarata dal client diventa quella in mano (se valida; cambiare casella interrompe il pasto) e deve contenere
    /// quello che il server crede: altrimenti si rimanda la casella al client e si restituisce `false`.
    fn adopt_held_slot(&mut self, pid: u64, hotbar_slot: i32, claimed: &ItemStack) -> bool {
        let Some(p) = self.players.get_mut(&pid) else { return false };
        if (0..HOTBAR_SLOTS as i32).contains(&hotbar_slot) && p.inventory.selected as i32 != hotbar_slot {
            let (held, armor) = (p.inventory.held().stack(0), armor_of(p));
            p.inventory.selected = hotbar_slot as u8;
            self.send_equipment_changes(pid, &held, &armor);
            self.stop_eating(pid);
        }
        let Some(p) = self.players.get(&pid) else { return false };
        if p.inventory.held().same_kind(&Item::kind_of(claimed)) {
            return true;
        }
        self.send(pid, &[p.inventory.held_slot_packet(), p.inventory.hotbar_packet()]);
        false
    }

    /// Smette di mangiare (cambio di casella, rilascio, morte, spettatore, oggetto che non è più un cibo) e lo dice a chi guarda.
    pub(crate) fn stop_eating(&mut self, pid: u64) {
        if self.players.get_mut(&pid).is_some_and(|p| p.eating_since.take().is_some()) {
            self.send_entity_state(pid);
        }
    }

    /// Il pasto è finito (Dragonfly `UseItem` ramo `Consumable`, durata raggiunta). Il conto riparte subito per il cibo seguente della pila, come
    /// in Dragonfly; i ganci vedono `WorldAction::Eat` (un'unità) e possono annullare (non si consuma nulla). Altrimenti fame e saturazione
    /// crescono (`Saturate`: la saturazione non supera mai la fame, `hungerManager.saturate`), in sopravvivenza e avventura si toglie un
    /// cibo dalla casella (restano ciotola o bottiglia dove previsto) e il client riceve vitali e casella; in creativa il cibo resta (Dragonfly:
    /// `subtractItem` non toglie in creativa) ma fame e saturazione salgono comunque. Gli effetti (rigenerazione, veleno, fame...) e il
    /// ruttino non ci sono: il server non ha effetti né suoni di giocatore.
    fn finish_eating(&mut self, pid: u64) {
        let tick = self.tick_count;
        let Some(p) = self.players.get_mut(&pid) else { return };
        p.eating_since = Some(tick);
        let held = p.inventory.held().clone();
        let Some((name, food)) = edible(&held) else { return };
        let (points, saturation) = (food.points, food.saturation);
        if !self.allow(&WorldAction::Eat { pid, item: Item { count: 1, ..held.clone() } }) {
            return;
        }
        let Some(p) = self.players.get_mut(&pid) else { return };
        let (before_held, armor) = (held.stack(0), armor_of(p));
        p.food = (p.food + points).min(MAX_FOOD);
        p.saturation = (p.saturation + saturation).clamp(0.0, p.food as f32);
        let mut packets = Vec::new();
        let (mut overflow, mut emptied) = (Item::default(), false);
        if p.mode != GameMode::Creative {
            let slot = p.inventory.selected as usize;
            let left = held.count - 1;
            let rest = leftover(name).and_then(|n| Item::from_name(n, 1)).unwrap_or_default();
            // la mano che si svuota riceve il resto (Dragonfly `addNewItem`); se la pila continua, il resto va nell'inventario
            p.inventory.set_main(slot, if left > 0 { Item { count: left, ..held } } else { rest.clone() });
            packets.push(p.inventory.held_slot_packet());
            if left > 0 && !rest.is_empty() {
                let (over, changed) = p.inventory.add(rest);
                packets.extend(changed.iter().filter_map(|&(container, s)| p.inventory.slot_packet_at(container, s)));
                overflow = over;
            }
            if p.inventory.held().is_empty() {
                p.eating_since = None; // niente più da mangiare
                emptied = true;
            }
        }
        self.send_vitals(pid);
        self.send(pid, &packets);
        self.send_equipment_changes(pid, &before_held, &armor);
        if emptied {
            self.send_entity_state(pid);
        }
        if !overflow.is_empty() {
            self.drop_item(pid, overflow); // inventario pieno: il resto finisce a terra
        }
    }

    /// Un tick per chi sta mangiando: se non può più (morto, spettatore, in mano non c'è più un cibo) smette; ogni `EAT_EFFECT_TICKS` tick
    /// manda a chi guarda, e a lui, l'evento "mangia" (`EatAction` di Dragonfly: `ActorEventFeed`, dato `(ID oggetto << 16) | aux`).
    pub(crate) fn eating_tick(&mut self) {
        let eating: Vec<(u64, bool, i32)> = self
            .players
            .iter()
            .filter(|(_, p)| p.eating_since.is_some())
            .map(|(&pid, p)| {
                let held = p.inventory.held();
                let can = p.spawned && !p.dead && p.mode != GameMode::Spectator && edible(held).is_some();
                (pid, can, ((held.id as i32) << 16) | (held.aux & 0xffff) as i32)
            })
            .collect();
        for (pid, can, data) in eating {
            if !can {
                self.stop_eating(pid);
            } else if self.tick_count.is_multiple_of(EAT_EFFECT_TICKS) {
                let event = actor_event(pid, actor_event_type::EATING_ITEM, data);
                self.send(pid, std::slice::from_ref(&event));
                self.broadcast_to_viewers(pid, std::slice::from_ref(&event));
            }
        }
    }

    /// Manda a `pid` vita, fame, saturazione ed esaurimento attuali (`UpdateAttributes`, stessi nomi e massimi dello spawn).
    pub(crate) fn send_vitals(&self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let packet = UpdateAttributes { target_runtime_id: pid, attribute_list: vitals_attributes(p), tick: 0 }.packet();
        self.send(pid, &[packet]);
    }

    /// La vita di `pid` è arrivata a 0 (da [`Server::hurt`]). Sul modello di Dragonfly `kill`: animazione di morte a chi lo vede, fine di corsa e
    /// accovacciamento, inventario (armatura e mano secondaria comprese) a terra salvo `keep_inventory` e salvo lo spettatore, messaggio in chat per tutti.
    /// Il giocatore resta morto (non si muove, non agisce, non subisce danno) finché il client non chiede di rinascere (vedi [`Server::respawn`]).
    ///
    /// La morte non si annulla: i ganci vedono solo `WorldAction::Death` e la loro risposta è ignorata (a vita 0 non c'è un valore sensato a cui
    /// tornare; Dragonfly lascia ai ganci solo `keepInv`). Dragonfly toglie anche il corpo dopo 1,1 s; qui il client fa da solo la sua
    /// animazione e il corpo sparisce a chi lo guarda alla rinascita.
    pub(crate) fn die(&mut self, pid: u64, cause: DamageCause) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        p.dead = true;
        p.death_tick = self.tick_count;
        p.health = 0.0;
        p.breaking = None;
        p.fall_start_y = None;
        p.flying = false;
        let before = (p.sneaking, p.sprinting, p.swimming, p.gliding);
        (p.sneaking, p.sprinting, p.swimming, p.gliding) = (false, false, false, false);
        let state_changed = before != (false, false, false, false);
        let (mode, feet) = (p.mode, [p.pos[0], p.pos[1] - EYE_HEIGHT, p.pos[2]]);
        log!("[#{pid}] morto: {cause:?}");
        self.allow(&WorldAction::Death { pid }); // solo osservazione: la morte non si annulla
        self.stop_eating(pid);
        if state_changed {
            self.send_entity_state(pid); // come Dragonfly `StopSneaking`/`StopSprinting`: `updateState`
        }
        let event = actor_event(pid, actor_event_type::DEATH, 0);
        self.send(pid, std::slice::from_ref(&event));
        self.broadcast_to_viewers(pid, std::slice::from_ref(&event));
        if !self.keep_inventory && mode != GameMode::Spectator {
            self.drop_inventory(pid, feet);
        }
        let name = self.name(pid);
        let message = match cause {
            DamageCause::Fall => format!("{name} è caduto da troppo in alto"),
            DamageCause::Void => format!("{name} è caduto fuori dal mondo"),
            DamageCause::Starvation => format!("{name} è morto di fame"),
            DamageCause::Attack { attacker } => {
                let killer = match self.entities.get(attacker).map(|e| &e.kind) {
                    Some(EntityKind::Player { name, .. }) => name.clone(),
                    _ => "qualcosa".to_string(),
                };
                format!("{name} è stato ucciso da {killer}")
            }
            DamageCause::Kill => format!("{name} è morto"),
        };
        self.broadcast(Text::new(text_type::RAW, message));
    }

    /// Svuota inventario, armatura e mano secondaria di `pid` e ne fa oggetti a terra in `feet` (uno per casella non vuota, con una piccola
    /// spinta casuale come Dragonfly `dropItems`: `[rand*0.2-0.1, 0.2, rand*0.2-0.1]`). Poi rimanda il contenuto vuoto a lui e l'armatura tolta a chi lo vede.
    fn drop_inventory(&mut self, pid: u64, feet: [f32; 3]) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        let (held, armor) = (p.inventory.held().stack(0), armor_of(p));
        for item in p.inventory.take_all() {
            let velocity = [self.roll() * 0.2 - 0.1, 0.2, self.roll() * 0.2 - 0.1];
            self.spawn_item(item, feet, velocity, DEATH_PICKUP_DELAY);
        }
        self.send_inventory(pid);
        self.send_equipment_changes(pid, &held, &armor);
    }

    /// Il client morto chiede di rinascere. Sul modello di Dragonfly `Respawn`: vita, fame e saturazione di partenza (20, 20, 5, esaurimento 0),
    /// posizione al punto di partenza del mondo (lo stesso di `StartGame`: non ci sono letti), teletrasporto del client e poi `Respawn`
    /// con lo stato `ReadyToSpawn` (`SendRespawn`). Chi lo vedeva perde il vecchio corpo e ne vede uno nuovo al prossimo `visibility_tick`.
    /// Chi non è morto non fa nulla.
    pub(crate) fn respawn(&mut self, pid: u64) {
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.dead) else { return };
        settle_dead(p, self.spawn);
        let (eyes, pitch, yaw) = (p.pos, p.pitch, p.yaw);
        log!("[#{pid}] rinasce in [{:.1}, {:.1}, {:.1}]", eyes[0], eyes[1], eyes[2]);
        self.on_move(pid, eyes);
        // il vecchio corpo sparisce a chi lo vedeva; il `visibility_tick` mostra quello nuovo
        let watchers: Vec<u64> = self.players.iter().filter(|(&other, p)| other != pid && p.visible.contains(&pid)).map(|(&other, _)| other).collect();
        for other in watchers {
            self.send(other, &[remove_actor(pid as i64)]);
            if let Some(p) = self.players.get_mut(&other) {
                p.visible.remove(&pid);
            }
        }
        self.send_vitals(pid);
        self.send_only_abilities(pid);
        self.send(pid, &[set_actor_data(pid, &self.entity_data(pid))]);
        self.send(
            pid,
            &[set_actor_motion(pid, [0.0; 3]), move_player_teleport(pid, eyes, pitch, yaw, self.tick_count), respawn(eyes, respawn_state::READY_TO_SPAWN, pid)],
        );
    }
}

/// Dopo la morte il giocatore riparte dal punto di partenza del mondo con vita, fame e saturazione di partenza. Vale alla rinascita e a chi
/// esce da morto (rientra vivo: il suo inventario è già a terra, o intero con `keep_inventory`). Chi non è morto non cambia.
pub(crate) fn settle_dead(p: &mut Player, spawn: Option<(i32, i32, i32)>) {
    if !p.dead {
        return;
    }
    p.dead = false;
    (p.health, p.food, p.saturation, p.exhaustion) = (MAX_HEALTH, MAX_FOOD, START_SATURATION, 0.0);
    (p.invulnerable_until, p.last_damage, p.fall_start_y, p.food_tick) = (0, 0.0, None, 1);
    if let Some((x, y, z)) = spawn {
        p.pos = [x as f32 + 0.5, y as f32 + EYE_HEIGHT, z as f32 + 0.5];
    }
}
