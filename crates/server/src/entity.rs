//! Registro delle entità (giocatori visti dagli altri, oggetti a terra) e visibilità per chunk.
//! L'ID unico di ogni entità coincide con quello di runtime (come lo `StartGame` dei giocatori, dove `entity_id = pid`).
use crate::{
    inventory::{self, window},
    join::ability_layers,
    playerdata,
    streaming::chunk_of,
    GameMode, Server,
};
use blackstone_protocol::{
    actor::{flag, key, set_actor_data, DataValue},
    entity::{
        add_item_actor, add_player, mob_armor_equipment, move_actor_absolute, player_list_add, player_list_remove, remove_actor, AddPlayerData, PlayerListEntry,
    },
    generated::MobEquipment,
    item::ItemStack,
    skin::uuid_bytes,
    wire::ServerPacket,
    Packet,
};
use std::collections::{HashMap, HashSet};

/// Il primo ID delle entità che non sono giocatori: sopra ogni `pid` (che parte da 1 e cresce piano), così gli ID non si scontrano.
pub(crate) const FIRST_ENTITY_ID: u64 = 1 << 32;

pub(crate) enum EntityKind {
    /// `pid: None` è un bot (nessuna connessione).
    Player {
        pid: Option<u64>,
        name: String,
        uuid: [u8; 16],
    },
    Item {
        item: inventory::Item,
        pickup_at: u64,
        despawn_at: u64,
        velocity: [f32; 3],
    },
}

pub(crate) struct Entity {
    pub id: u64,
    pub kind: EntityKind,
    /// Per i giocatori sono gli occhi, come `Player.pos`.
    pub pos: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub head_yaw: f32,
    pub on_ground: bool,
    /// Da mandare agli spettatori al prossimo giro.
    pub moved: bool,
}

impl Entity {
    /// La posizione come la vede il client: un oggetto a terra ha il riquadro 0.125 più in basso della sua origine (come Dragonfly).
    pub fn network_pos(&self) -> [f32; 3] {
        match self.kind {
            EntityKind::Item { .. } => [self.pos[0], self.pos[1] + ITEM_NETWORK_OFFSET, self.pos[2]],
            EntityKind::Player { .. } => self.pos,
        }
    }
}

/// Di quanto il client vede più in alto un oggetto a terra (`NetworkOffset` di Dragonfly).
const ITEM_NETWORK_OFFSET: f32 = 0.125;

#[derive(Default)]
pub(crate) struct Entities {
    map: HashMap<u64, Entity>,
    next: u64,
}

impl Entities {
    /// Un ID libero per un'entità che non è un giocatore.
    pub fn alloc(&mut self) -> u64 {
        self.next = self.next.max(FIRST_ENTITY_ID - 1) + 1;
        self.next
    }

    pub fn insert(&mut self, e: Entity) {
        self.map.insert(e.id, e);
    }

    pub fn remove(&mut self, id: u64) -> Option<Entity> {
        self.map.remove(&id)
    }

    pub fn get(&self, id: u64) -> Option<&Entity> {
        self.map.get(&id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Entity> {
        self.map.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.map.values()
    }
}

/// L'armatura indossata (elmo, corazza, gambali, stivali) come la vedono gli altri (senza ID di pila).
pub(crate) fn armor_of(p: &crate::Player) -> [ItemStack; 4] {
    std::array::from_fn(|i| p.inventory.armor(i).stack(0))
}

impl Server {
    /// Per ogni giocatore nel mondo confronta le entità che vede (`visible`) con quelle nei chunk che ha ricevuto (`sent`):
    /// le nuove si mostrano, quelle uscite (o sparite dal registro) si tolgono. Un giocatore non vede mai la propria entità.
    pub(crate) fn visibility_tick(&mut self) {
        let pids: Vec<u64> = self.players.iter().filter(|(_, p)| p.spawned).map(|(&pid, _)| pid).collect();
        for pid in pids {
            let p = &self.players[&pid];
            // Il corpo di chi è morto lo vede solo chi già lo vedeva, e solo per `BODY_TICKS` (come Dragonfly `finishDying`); alla rinascita torna.
            let tick = self.tick_count;
            let hidden = |e: &Entity| match e.kind {
                EntityKind::Player { pid: Some(other), .. } => {
                    self.players.get(&other).is_some_and(|o| o.dead && (!p.visible.contains(&e.id) || tick >= o.death_tick + crate::survival::BODY_TICKS))
                }
                _ => false,
            };
            let should: HashSet<u64> = self
                .entities
                .iter()
                .filter(|e| {
                    !hidden(e) && !matches!(e.kind, EntityKind::Player { pid: Some(own), .. } if own == pid) && p.sent.contains(&chunk_of(e.pos[0], e.pos[2]))
                })
                .map(|e| e.id)
                .collect();
            let mut out: Vec<Packet> = Vec::new();
            let mut added: Vec<u64> = should.difference(&p.visible).copied().collect();
            added.sort_unstable();
            for id in added {
                out.extend(self.spawn_packets(&self.entities.map[&id]));
            }
            let mut gone: Vec<u64> = p.visible.difference(&should).copied().collect();
            gone.sort_unstable();
            out.extend(gone.into_iter().map(|id| remove_actor(id as i64)));
            if !out.is_empty() {
                self.send(pid, &out);
            }
            self.players.get_mut(&pid).unwrap().visible = should;
        }
    }

    /// Manda a ogni spettatore, al massimo una volta per giro, la posizione e le rotazioni (corpo e testa) delle entità che si sono mosse.
    pub(crate) fn movement_tick(&mut self) {
        let mut moved: Vec<(u64, Packet)> = Vec::new();
        for e in self.entities.map.values_mut().filter(|e| e.moved) {
            e.moved = false;
            moved.push((e.id, move_actor_absolute(e.id, e.network_pos(), e.pitch, e.yaw, e.head_yaw, e.on_ground, false)));
        }
        if moved.is_empty() {
            return;
        }
        moved.sort_unstable_by_key(|(id, _)| *id);
        let pids: Vec<u64> = self.players.iter().filter(|(_, p)| p.spawned && !p.visible.is_empty()).map(|(&pid, _)| pid).collect();
        for pid in pids {
            let out: Vec<Packet> = moved.iter().filter(|(id, _)| self.players[&pid].visible.contains(id)).map(|(_, p)| p.clone()).collect();
            if !out.is_empty() {
                self.send(pid, &out);
            }
        }
    }

    /// I pacchetti che fanno comparire `e` a uno spettatore. Il giocatore richiede prima la sua voce nella `PlayerList` (compito del chiamante).
    pub(crate) fn spawn_packets(&self, e: &Entity) -> Vec<Packet> {
        match &e.kind {
            EntityKind::Player { pid, name, uuid } => {
                let player = pid.and_then(|pid| self.players.get(&pid));
                let mode = player.map_or(GameMode::default(), |p| p.mode);
                let held = match player {
                    Some(p) => p.inventory.held().stack(0),
                    None => self.bots.iter().find(|b| b.id == e.id).map_or_else(ItemStack::air, |b| b.held.stack(0)),
                };
                let mut packets = vec![add_player(&AddPlayerData {
                    uuid: *uuid,
                    name: name.clone(),
                    runtime_id: e.id,
                    position: e.pos,
                    pitch: e.pitch,
                    yaw: e.yaw,
                    head_yaw: e.head_yaw,
                    held,
                    game_type: mode as i32,
                    data: self.entity_data(e.id),
                    layers: ability_layers(mode, false),
                    // 0 non è una piattaforma valida ("invalid enum value" sul client): -1 = sconosciuta, come Dragonfly.
                    build_platform: player.and_then(|p| p.info.as_ref()).and_then(|i| i.device_os).map_or(-1, |d| d as i32),
                    ..Default::default()
                })];
                if let Some(armor) = player.map(armor_of).filter(|a| a.iter().any(|s| s.id != 0)) {
                    packets.push(mob_armor_equipment(e.id, &armor));
                }
                packets
            }
            EntityKind::Item { item, velocity, .. } => {
                vec![add_item_actor(e.id as i64, e.id, &item.stack(0), e.network_pos(), *velocity, &self.entity_data(e.id))]
            }
        }
    }

    /// I dati dell'entità (`SetActorData` e `AddPlayer`): per un giocatore i flag di base (gravità, collisioni, arrampicata, respiro)
    /// più lo stato attuale (accovacciato, in corsa, a nuoto, in planata), il nome e il riquadro del corpo secondo lo stato. Un oggetto a terra ha gravità, collisioni e il riquadro 0.25 x 0.25.
    pub(crate) fn entity_data(&self, id: u64) -> Vec<(u32, DataValue)> {
        let bot_sneaking = self.bots.iter().any(|b| b.id == id && b.sneaking);
        let (state, name) = match self.entities.get(id).map(|e| &e.kind) {
            Some(EntityKind::Player { pid, name, .. }) => (pid.and_then(|pid| self.players.get(&pid)), name.clone()),
            // il giocatore che sta entrando non ha ancora la sua entità
            None => (self.players.get(&id), self.players.get(&id).and_then(|p| p.info.as_ref()).map_or_else(String::new, |i| i.name.clone())),
            Some(EntityKind::Item { .. }) => {
                let flags = [flag::HAS_GRAVITY, flag::HAS_COLLISION].iter().fold(0i64, |f, b| f | 1 << b);
                return vec![
                    (key::FLAGS, DataValue::Int64(flags)),
                    (key::BOUNDING_BOX_WIDTH, DataValue::Float(0.25)),
                    (key::BOUNDING_BOX_HEIGHT, DataValue::Float(0.25)),
                ];
            }
        };
        let mut flags = [flag::HAS_GRAVITY, flag::HAS_COLLISION, flag::CAN_CLIMB, flag::BREATHING].iter().fold(0i64, |f, b| f | 1 << b);
        flags |= (bot_sneaking as i64) << flag::SNEAKING;
        let (mut sneaking, mut crawling) = (bot_sneaking, false);
        if let Some(p) = state {
            let using = p.eating_since.is_some();
            for (on, bit) in [
                (p.sneaking, flag::SNEAKING),
                (p.sprinting, flag::SPRINTING),
                (p.swimming, flag::SWIMMING),
                (p.gliding, flag::GLIDING),
                (using, flag::USING_ITEM),
            ] {
                flags |= (on as i64) << bit;
            }
            sneaking |= p.sneaking;
            crawling = p.swimming || p.gliding;
        }
        // Il nome sopra la testa, sempre visibile (come `writeNameTagMetadata` di Dragonfly).
        let show = !name.is_empty();
        flags |= (show as i64) << flag::SHOW_NAME | (show as i64) << flag::ALWAYS_SHOW_NAME;
        // Il riquadro del corpo (`ptype.BBox` di Dragonfly): 0.6 a nuoto o in planata, 1.5 accovacciato, 1.8 altrimenti.
        let height = if crawling {
            0.6
        } else if sneaking {
            1.5
        } else {
            1.8
        };
        vec![
            (key::FLAGS, DataValue::Int64(flags)),
            (key::NAME, DataValue::Str(name)),
            (key::BOUNDING_BOX_WIDTH, DataValue::Float(0.6)),
            (key::BOUNDING_BOX_HEIGHT, DataValue::Float(height)),
            (key::ALWAYS_SHOW_NAMETAG, DataValue::Byte(show as u8)),
        ]
    }

    /// Manda `packets` a chi vede l'entità `id` (non a lei stessa).
    pub(crate) fn broadcast_to_viewers(&self, id: u64, packets: &[Packet]) {
        for (&pid, _) in self.players.iter().filter(|(_, p)| p.visible.contains(&id)) {
            self.send(pid, packets);
        }
    }

    /// Lo stato del giocatore è cambiato: lo vedono gli spettatori e lui stesso (come Dragonfly).
    pub(crate) fn send_entity_state(&self, pid: u64) {
        let packet = set_actor_data(pid, &self.entity_data(pid));
        self.broadcast_to_viewers(pid, std::slice::from_ref(&packet));
        self.send(pid, &[packet]);
    }

    /// Cambiata la mano o l'armatura di `pid` rispetto a prima (`held`, `armor` sono i valori di prima): lo si dice agli spettatori.
    pub(crate) fn send_equipment_changes(&self, pid: u64, held: &ItemStack, armor: &[ItemStack; 4]) {
        let Some(p) = self.players.get(&pid) else { return };
        let mut out = Vec::new();
        let now = p.inventory.held().stack(0);
        if now != *held {
            out.push(
                MobEquipment {
                    target_runtime_id: pid,
                    item: now,
                    slot: p.inventory.selected,
                    selected_slot: p.inventory.selected,
                    container_id: window::INVENTORY as u8,
                }
                .packet(),
            );
        }
        let worn = armor_of(p);
        if worn != *armor {
            out.push(mob_armor_equipment(pid, &worn));
        }
        if !out.is_empty() {
            self.broadcast_to_viewers(pid, &out);
        }
    }

    /// La `PlayerList` che aggiunge questi giocatori (ordinati per `pid`) e questi bot; chi non è nel mondo o non ha fatto il login si salta.
    pub(crate) fn player_list_add_packet(&self, pids: &[u64], bots: &[u64]) -> Option<Packet> {
        let mut pids = pids.to_vec();
        pids.sort_unstable();
        let mut entries: Vec<PlayerListEntry> = pids
            .iter()
            .filter_map(|pid| {
                let info = self.players.get(pid)?.info.as_ref()?;
                let EntityKind::Player { uuid, .. } = self.entities.get(*pid)?.kind else { return None };
                Some(PlayerListEntry {
                    uuid,
                    unique_id: *pid as i64,
                    name: info.name.clone(),
                    xuid: info.xuid.clone().unwrap_or_default(),
                    platform_chat_id: String::new(),
                    build_platform: info.device_os.map_or(-1, |d| d as i32),
                    skin: &info.skin,
                    teacher: false,
                    host: false,
                    sub_client: false,
                })
            })
            .collect();
        entries.extend(self.bots.iter().filter(|b| bots.contains(&b.id)).filter_map(|b| {
            let EntityKind::Player { uuid, name, .. } = &self.entities.get(b.id)?.kind else { return None };
            Some(PlayerListEntry {
                uuid: *uuid,
                unique_id: b.id as i64,
                name: name.clone(),
                xuid: String::new(),
                platform_chat_id: String::new(),
                build_platform: -1,
                skin: &b.skin,
                teacher: false,
                host: false,
                sub_client: false,
            })
        }));
        (!entries.is_empty()).then(|| player_list_add(&entries))
    }

    /// Il giocatore entra nel mondo: diventa un'entità (ID = `pid`, posizione = occhi), riceve la lista completa dei giocatori
    /// e gli altri ricevono la sua voce. Il corpo lo vedranno dal `visibility_tick`, quando i chunk saranno inviati.
    pub(crate) fn add_player_entity(&mut self, pid: u64) {
        let Some(p) = self.players.get(&pid) else { return };
        let Some(info) = p.info.as_ref() else { return };
        let uuid = uuid_bytes(info, &playerdata::identity(info));
        let entity = Entity {
            id: pid,
            kind: EntityKind::Player { pid: Some(pid), name: info.name.clone(), uuid },
            pos: p.pos,
            pitch: p.pitch,
            yaw: p.yaw,
            head_yaw: p.yaw,
            on_ground: true,
            moved: false,
        };
        self.entities.insert(entity);
        let inside: Vec<u64> = self.players.iter().filter(|(_, p)| p.spawned).map(|(&id, _)| id).collect();
        let bots: Vec<u64> = self.bots.iter().map(|b| b.id).collect();
        if let Some(all) = self.player_list_add_packet(&inside, &bots) {
            self.send(pid, &[all]);
        }
        if let Some(own) = self.player_list_add_packet(&[pid], &[]) {
            for other in inside.into_iter().filter(|&id| id != pid) {
                self.send(other, std::slice::from_ref(&own));
            }
        }
    }

    /// Il giocatore esce: sparisce dal registro e dal corpo di chi lo vedeva, poi la sua voce lascia la `PlayerList` di tutti.
    /// Da chiamare dopo averlo tolto da `players`.
    pub(crate) fn remove_player_entity(&mut self, pid: u64) {
        let Some(EntityKind::Player { uuid, .. }) = self.entities.get(pid).map(|e| &e.kind) else { return };
        let uuid = *uuid;
        self.despawn(pid);
        let list = player_list_remove(&[uuid]);
        let inside: Vec<u64> = self.players.iter().filter(|(_, p)| p.spawned).map(|(&id, _)| id).collect();
        for other in inside {
            self.send(other, std::slice::from_ref(&list));
        }
    }

    /// Toglie l'entità dal registro e la fa sparire a chi la vedeva.
    pub(crate) fn despawn(&mut self, id: u64) {
        self.entities.remove(id);
        let watchers: Vec<u64> = self.players.iter().filter(|(_, p)| p.visible.contains(&id)).map(|(&pid, _)| pid).collect();
        for pid in watchers {
            self.send(pid, &[remove_actor(id as i64)]);
            self.players.get_mut(&pid).unwrap().visible.remove(&id);
        }
    }
}
