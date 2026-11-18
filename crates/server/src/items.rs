//! Oggetti a terra: il lancio (da `Q` o trascinando fuori dall'inventario), la fisica (gravità, attrito, appoggio) e la scomparsa.
//! I valori sono quelli di Dragonfly (`entity/item_behaviour.go`, `entity/movement.go`). La posizione di un oggetto è quella dei suoi piedi
//! (il riquadro è 0.25 x 0.25); il client la vede 0.125 più in alto (vedi `Entity::network_pos`).
use crate::{
    entity::{Entity, EntityKind},
    inventory::Item,
    placement::{neighbors::solid_face, UP},
    Server,
};
use bedrock_protocol::entity::set_actor_motion;
use vgen::Dim;

/// Quanto la velocità verticale cala a ogni tick.
const GRAVITY: f32 = 0.04;
/// Ogni tick la velocità si moltiplica per `1 - 0.02` (aria).
const AIR_DRAG: f32 = 0.98;
/// A terra la velocità orizzontale si moltiplica anche per questo (nessun blocco ha un attrito diverso nel nostro mondo).
const GROUND_FRICTION: f32 = 0.6;
/// Tick prima che un oggetto lanciato dal giocatore si possa raccogliere (2 secondi).
pub(crate) const THROW_PICKUP_DELAY: u64 = 40;
/// Tick di vita di un oggetto a terra (5 minuti).
pub(crate) const LIFETIME: u64 = 6000;
/// Sotto questa quota (il fondo del mondo) l'oggetto sparisce.
const WORLD_BOTTOM: f32 = -64.0;
/// Il lancio parte 0.3 sotto gli occhi, con questa velocità nella direzione dello sguardo.
const THROW_DROP: f32 = 0.3;
const THROW_SPEED: f32 = 0.3;
/// Mezza larghezza e altezza del riquadro.
const HALF_WIDTH: f32 = 0.125;
const HEIGHT: f32 = 0.25;
/// Sotto questa velocità un asse si ferma (la soglia di Dragonfly).
const EPSILON: f32 = 0.001;

/// Dove va e con che velocità un oggetto dopo un tick, e se poggia su un blocco.
#[derive(Debug, PartialEq)]
struct Step {
    pos: [f32; 3],
    velocity: [f32; 3],
    on_ground: bool,
}

/// Un tick di fisica. `top(cella)` dice se la faccia superiore del blocco regge l'oggetto, `full(cella)` se il blocco è un cubo pieno.
/// Una cella che non si conosce (chunk non in memoria) conta come aria. Il centro dell'oggetto decide su quale colonna sta.
fn step(pos: [f32; 3], mut v: [f32; 3], was_on_ground: bool, top: impl Fn([i32; 3]) -> bool, full: impl Fn([i32; 3]) -> bool) -> Step {
    v[1] = (v[1] - GRAVITY) * AIR_DRAG;
    let friction = AIR_DRAG * if was_on_ground { GROUND_FRICTION } else { 1.0 };
    (v[0], v[2]) = (v[0] * friction, v[2] * friction);
    let [x, y, z] = pos;
    let (cx, cz) = (x.floor() as i32, z.floor() as i32);
    // Verticale: in discesa si cerca da dove si è partiti verso il basso la prima faccia superiore piena, in salita il soffitto.
    let mut ny = y + v[1];
    let mut on_ground = false;
    if v[1] < 0.0 {
        for cy in (ny.floor() as i32..=y.floor() as i32).rev() {
            let surface = (cy + 1) as f32;
            if top([cx, cy, cz]) && surface <= y + EPSILON && surface >= ny {
                (ny, v[1], on_ground) = (surface, 0.0, true);
                break;
            }
        }
    } else if v[1] > 0.0 {
        let head = ny + HEIGHT;
        if full([cx, head.floor() as i32, cz]) {
            (ny, v[1]) = (head.floor() - HEIGHT - EPSILON, 0.0);
        }
    }
    // Orizzontale: il bordo del riquadro non entra in un cubo pieno all'altezza del corpo.
    let body = (ny + HEIGHT / 2.0).floor() as i32;
    let mut nx = x + v[0];
    if v[0] != 0.0 && full([(nx + HALF_WIDTH.copysign(v[0])).floor() as i32, body, cz]) {
        (nx, v[0]) = (x, 0.0);
    }
    let mut nz = z + v[2];
    if v[2] != 0.0 && full([cx, body, (nz + HALF_WIDTH.copysign(v[2])).floor() as i32]) {
        (nz, v[2]) = (z, 0.0);
    }
    for axis in [0, 2] {
        if v[axis].abs() < EPSILON {
            v[axis] = 0.0;
        }
    }
    Step { pos: [nx, ny, nz], velocity: v, on_ground }
}

/// Vero se la velocità è cambiata di direzione (un asse ha cambiato segno o si è fermato): allora la sa anche chi guarda.
fn direction_changed(before: [f32; 3], after: [f32; 3]) -> bool {
    let sign = |v: f32| {
        if v == 0.0 {
            0
        } else if v > 0.0 {
            1
        } else {
            -1
        }
    };
    before.iter().zip(after).any(|(&b, a)| sign(b) != sign(a))
}

impl Server {
    /// Il giocatore butta `item`: parte 0.3 sotto gli occhi e va nella direzione dello sguardo (come `Player.Drop` di Dragonfly).
    /// I ganci si sono già pronunciati (`WorldAction::DropItem`).
    pub(crate) fn drop_item(&mut self, pid: u64, item: Item) {
        let Some(p) = self.players.get(&pid) else { return };
        let (yaw, pitch) = (p.yaw.to_radians(), p.pitch.to_radians());
        let look = [-yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos()];
        let pos = [p.pos[0], p.pos[1] - THROW_DROP, p.pos[2]];
        self.spawn_item(item, pos, look.map(|c| c * THROW_SPEED), THROW_PICKUP_DELAY);
    }

    /// Crea un oggetto a terra in `pos` (i piedi) con `velocity`; si può raccogliere dopo `pickup_delay` tick e sparisce dopo `LIFETIME`.
    /// Chi ha il chunk lo vede dal prossimo `visibility_tick`. Restituisce l'ID dell'entità.
    pub(crate) fn spawn_item(&mut self, item: Item, pos: [f32; 3], velocity: [f32; 3], pickup_delay: u64) -> u64 {
        let id = self.entities.alloc();
        self.entities.insert(Entity {
            id,
            kind: EntityKind::Item { item, pickup_at: self.tick_count + pickup_delay, despawn_at: self.tick_count + LIFETIME, velocity },
            pos,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            on_ground: false,
            moved: false,
        });
        id
    }

    /// Un tick per tutti gli oggetti a terra: sparisce chi è scaduto o è sotto il fondo del mondo, gli altri cadono e si fermano sui blocchi.
    /// Chi si è spostato lo manda `movement_tick`; un cambio di direzione della velocità lo manda subito a chi lo vede.
    pub(crate) fn items_tick(&mut self) {
        let mut ids: Vec<u64> = self.entities.iter().filter(|e| matches!(e.kind, EntityKind::Item { .. })).map(|e| e.id).collect();
        ids.sort_unstable();
        for id in ids {
            let Some(Entity { kind: EntityKind::Item { despawn_at, velocity, .. }, pos, on_ground, .. }) = self.entities.get(id) else { continue };
            let (despawn_at, velocity, pos, on_ground) = (*despawn_at, *velocity, *pos, *on_ground);
            if self.tick_count >= despawn_at || pos[1] < WORLD_BOTTOM {
                self.despawn(id);
                continue;
            }
            let cell = |c: [i32; 3]| self.block_at(Dim::Overworld, c);
            let top = |c: [i32; 3]| cell(c).is_some_and(|(hash, _)| solid_face(hash, UP));
            let full = |c: [i32; 3]| cell(c).is_some_and(|(hash, _)| (0..6).all(|face| solid_face(hash, face)));
            let result = step(pos, velocity, on_ground, top, full);
            let e = self.entities.get_mut(id).unwrap();
            e.moved |= result.pos != e.pos || result.on_ground != e.on_ground;
            (e.pos, e.on_ground) = (result.pos, result.on_ground);
            let EntityKind::Item { velocity: v, .. } = &mut e.kind else { continue };
            *v = result.velocity;
            if direction_changed(velocity, result.velocity) {
                self.broadcast_to_viewers(id, &[set_actor_motion(id, result.velocity)]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un mondo di prova: pietra piena fino a y = 70 (compreso).
    fn floor(c: [i32; 3]) -> bool {
        c[1] <= 70
    }

    #[test]
    fn falling_item_gains_speed_with_gravity_and_drag() {
        let s = step([0.5, 80.0, 0.5], [0.0; 3], false, floor, floor);
        assert!((s.velocity[1] - (-0.04 * 0.98)).abs() < 1e-6);
        assert!(!s.on_ground && s.pos[1] < 80.0);
    }

    #[test]
    fn item_lands_on_the_top_face_and_stops() {
        let mut s = Step { pos: [0.5, 72.0, 0.5], velocity: [0.0; 3], on_ground: false };
        for _ in 0..40 {
            s = step(s.pos, s.velocity, s.on_ground, floor, floor);
        }
        assert_eq!((s.pos[1], s.velocity[1], s.on_ground), (71.0, 0.0, true));
    }

    #[test]
    fn ground_friction_slows_the_horizontal_motion() {
        let air = step([0.5, 80.0, 0.5], [0.3, 0.0, 0.0], false, floor, floor);
        let ground = step([0.5, 71.0, 0.5], [0.3, 0.0, 0.0], true, floor, floor);
        assert!((air.velocity[0] - 0.3 * 0.98).abs() < 1e-6);
        assert!((ground.velocity[0] - 0.3 * 0.98 * 0.6).abs() < 1e-6);
    }

    #[test]
    fn a_wall_stops_the_item() {
        let wall = |c: [i32; 3]| c[1] <= 70 || c[0] >= 1;
        let s = step([0.8, 71.0, 0.5], [0.3, 0.0, 0.0], true, wall, wall);
        assert_eq!((s.pos[0], s.velocity[0]), (0.8, 0.0));
    }

    #[test]
    fn a_fast_fall_does_not_tunnel_through_the_floor() {
        let s = step([0.5, 75.0, 0.5], [0.0, -5.0, 0.0], false, floor, floor);
        assert_eq!((s.pos[1], s.on_ground), (71.0, true));
    }

    #[test]
    fn ceiling_stops_a_rising_item() {
        let ceiling = |c: [i32; 3]| c[1] <= 70 || c[1] >= 73;
        let s = step([0.5, 72.0, 0.5], [0.0, 1.0, 0.0], false, ceiling, ceiling);
        assert_eq!(s.velocity[1], 0.0);
        assert!(s.pos[1] + HEIGHT <= 73.0);
    }

    #[test]
    fn direction_changes_are_detected() {
        assert!(direction_changed([0.0; 3], [0.0, -0.04, 0.0]));
        assert!(direction_changed([0.1, 0.0, 0.0], [0.0; 3]));
        assert!(!direction_changed([0.1, -0.3, 0.0], [0.05, -0.4, 0.0]));
    }
}
