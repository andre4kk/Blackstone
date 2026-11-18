//! Registro delle entità (giocatori visti dagli altri, oggetti a terra) e visibilità per chunk.
//! L'ID unico di ogni entità coincide con quello di runtime (come lo `StartGame` dei giocatori, dove `entity_id = pid`).
use crate::{inventory, join::ability_layers, playerdata, streaming::chunk_of, GameMode, Server};
use bedrock_protocol::{
    actor::{flag, key, DataValue},
    entity::{add_item_actor, add_player, player_list_add, player_list_remove, remove_actor, AddPlayerData, PlayerListEntry},
    item::ItemStack,
    skin::uuid_bytes,
    Packet,
};
use std::collections::{HashMap, HashSet};

/// Il primo ID delle entità che non sono giocatori: sopra ogni `pid` (che parte da 1 e cresce piano), così gli ID non si scontrano.
#[allow(dead_code)] // serve ai compiti successivi della tappa 2 (oggetti a terra, movimento)
pub(crate) const FIRST_ENTITY_ID: u64 = 1 << 32;

pub(crate) enum EntityKind {
    /// `pid: None` è un bot (nessuna connessione).
    Player { pid: Option<u64>, name: String, uuid: [u8; 16] },
    #[allow(dead_code)] // serve ai compiti successivi della tappa 2 (oggetti a terra, movimento)
    Item { item: inventory::Item, pickup_at: u64, despawn_at: u64, velocity: [f32; 3] },
}

pub(crate) struct Entity {
    pub id: u64,
    pub kind: EntityKind,
    /// Per i giocatori sono gli occhi, come `Player.pos`.
    pub pos: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub head_yaw: f32,
    #[allow(dead_code)] // letto dall'invio del movimento (compito successivo)
    pub on_ground: bool,
    /// Da mandare agli spettatori al prossimo giro.
    #[allow(dead_code)]
    pub moved: bool,
}

#[derive(Default)]
pub(crate) struct Entities {
    map: HashMap<u64, Entity>,
    #[allow(dead_code)]
    next: u64,
}

impl Entities {
    /// Un ID libero per un'entità che non è un giocatore.
    #[allow(dead_code)]
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

impl Server {
    /// Per ogni giocatore nel mondo confronta le entità che vede (`visible`) con quelle nei chunk che ha ricevuto (`sent`):
    /// le nuove si mostrano, quelle uscite (o sparite dal registro) si tolgono. Un giocatore non vede mai la propria entità.
    pub(crate) fn visibility_tick(&mut self) {
        let pids: Vec<u64> = self.players.iter().filter(|(_, p)| p.spawned).map(|(&pid, _)| pid).collect();
        for pid in pids {
            let p = &self.players[&pid];
            let should: HashSet<u64> = self
                .entities
                .iter()
                .filter(|e| !matches!(e.kind, EntityKind::Player { pid: Some(own), .. } if own == pid) && p.sent.contains(&chunk_of(e.pos[0], e.pos[2])))
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

    /// I pacchetti che fanno comparire `e` a uno spettatore. Il giocatore richiede prima la sua voce nella `PlayerList` (compito del chiamante).
    pub(crate) fn spawn_packets(&self, e: &Entity) -> Vec<Packet> {
        match &e.kind {
            EntityKind::Player { pid, name, uuid } => {
                let mode = pid.and_then(|pid| self.players.get(&pid)).map_or(GameMode::default(), |p| p.mode);
                let flags = [flag::HAS_GRAVITY, flag::HAS_COLLISION, flag::CAN_CLIMB, flag::BREATHING].iter().fold(0i64, |f, b| f | 1 << b);
                vec![add_player(&AddPlayerData {
                    uuid: *uuid,
                    name: name.clone(),
                    runtime_id: e.id,
                    position: e.pos,
                    pitch: e.pitch,
                    yaw: e.yaw,
                    head_yaw: e.head_yaw,
                    held: ItemStack::air(),
                    game_type: mode as i32,
                    data: vec![
                        (key::FLAGS, DataValue::Int64(flags)),
                        (key::BOUNDING_BOX_WIDTH, DataValue::Float(0.6)),
                        (key::BOUNDING_BOX_HEIGHT, DataValue::Float(1.8)),
                    ],
                    layers: ability_layers(mode, false),
                    ..Default::default()
                })]
            }
            EntityKind::Item { item, velocity, .. } => vec![add_item_actor(e.id as i64, e.id, &item.stack(0), e.pos, *velocity, &[])],
        }
    }

    /// La `PlayerList` che aggiunge questi giocatori (ordinati per `pid`); chi non è nel mondo o non ha fatto il login si salta.
    fn player_list_add_packet(&self, pids: &[u64]) -> Option<Packet> {
        let mut pids = pids.to_vec();
        pids.sort_unstable();
        let entries: Vec<PlayerListEntry> = pids
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
        if let Some(all) = self.player_list_add_packet(&inside) {
            self.send(pid, &[all]);
        }
        if let Some(own) = self.player_list_add_packet(&[pid]) {
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
