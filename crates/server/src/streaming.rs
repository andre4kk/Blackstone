//! Streaming dei chunk col movimento del giocatore, ingresso dei chunk dal generatore e dal disco, scaricamento.
use crate::{disk::DiskEvent, gen_client::GenEvent, world::Chunk, ChunkKey, Server};
use bedrock_net::log;
use bedrock_protocol::{
    generated::{ChunkPos, LevelChunk},
    wire::ServerPacket,
    Packet,
};
use std::collections::HashSet;
use vgen::{ChunkData, Dim};

/// Chunk inviati al massimo a ogni giocatore per tick.
pub(crate) const CHUNKS_PER_TICK: usize = 8;
/// Chunk tenuti in memoria oltre il raggio dei giocatori, per non ricaricare chi fa avanti e indietro al bordo.
const UNLOAD_MARGIN: i32 = 2;

/// Il chunk (x, z) di una posizione (blocchi): `floor(x) >> 4`.
pub(crate) fn chunk_of(x: f32, z: f32) -> (i32, i32) {
    ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4)
}

/// Vero se il chunk a distanza (`dx`, `dz`) sta nell'area quasi circolare di raggio `radius`.
fn in_range(dx: i32, dz: i32, radius: i32) -> bool {
    dx * dx + dz * dz <= radius * radius + radius
}

/// Chunk da inviare a un giocatore in `center` con raggio `radius`, dal più vicino al più lontano.
pub(crate) fn wanted(center: (i32, i32), radius: i32) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> =
        (-radius..=radius).flat_map(|dx| (-radius..=radius).map(move |dz| (dx, dz))).filter(|&(dx, dz)| in_range(dx, dz, radius)).collect();
    out.sort_by_key(|&(dx, dz)| (dx * dx + dz * dz, dx, dz)); // a parità di distanza l'ordine è fisso
    out.into_iter().map(|(dx, dz)| (center.0 + dx, center.1 + dz)).collect()
}

impl Server {
    /// Il giocatore si è spostato (posizione degli occhi). Se cambia chunk: nuovo centro al client e via i chunk fuori raggio
    /// da `sent` (ai nuovi ci pensa `stream_tick`).
    pub(crate) fn on_move(&mut self, pid: u64, pos: [f32; 3]) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        p.pos = pos;
        let chunk = chunk_of(pos[0], pos[2]);
        if chunk == p.last_chunk || !p.spawned {
            return;
        }
        p.last_chunk = chunk;
        let radius = p.radius;
        let keep: HashSet<(i32, i32)> = wanted(chunk, radius).into_iter().collect();
        p.sent.retain(|c| keep.contains(c));
        if p.streaming {
            let packet = self.publisher_update(pos, radius);
            self.send(pid, &[packet]);
        }
    }

    /// Un giro di streaming: per ogni giocatore al massimo `CHUNKS_PER_TICK` chunk (dalla memoria); quelli che mancano si
    /// chiedono al disco e, se non ci sono, al generatore. Poi si scaricano i chunk lontani da tutti.
    pub(crate) fn stream_tick(&mut self) {
        let pids: Vec<u64> = self.players.iter().filter(|(_, p)| p.spawned && p.streaming).map(|(&id, _)| id).collect();
        for pid in pids {
            let Some(p) = self.players.get(&pid) else { continue };
            let (mut ready, mut missing) = (Vec::new(), Vec::new());
            for c in wanted(p.last_chunk, p.radius).into_iter().filter(|c| !p.sent.contains(c)) {
                if self.chunks.contains_key(&(Dim::Overworld as u8, c.0, c.1)) {
                    if ready.len() < CHUNKS_PER_TICK {
                        ready.push(c);
                    }
                } else {
                    missing.push(c);
                }
            }
            for (x, z) in missing {
                self.fetch(Dim::Overworld, x, z);
            }
            if ready.is_empty() {
                continue;
            }
            let packets: Vec<Packet> = ready
                .iter()
                .map(|&(x, z)| {
                    let chunk = &self.chunks[&(Dim::Overworld as u8, x, z)];
                    LevelChunk {
                        chunk_position: ChunkPos { x, z },
                        dimension_id: Dim::Overworld as i32,
                        sub_chunks_count: chunk.sub_chunk_count(),
                        serialized_chunk_data: chunk.network_bytes(),
                        ..Default::default()
                    }
                    .packet()
                })
                .collect();
            self.send(pid, &packets);
            if let Some(p) = self.players.get_mut(&pid) {
                p.sent.extend(ready);
            }
        }
        self.unload_far();
    }

    /// Toglie dalla memoria i chunk fuori dal raggio + `UNLOAD_MARGIN` di tutti i giocatori, salvando quelli modificati.
    fn unload_far(&mut self) {
        if self.chunks.is_empty() {
            return;
        }
        let near: Vec<((i32, i32), i32)> = self.players.values().filter(|p| p.spawned).map(|p| (p.last_chunk, p.radius + UNLOAD_MARGIN)).collect();
        let far: Vec<ChunkKey> = self.chunks.keys().filter(|&&(_, x, z)| !near.iter().any(|&(c, r)| in_range(x - c.0, z - c.1, r))).copied().collect();
        for key in far {
            if let Some(chunk) = self.chunks.remove(&key) {
                if chunk.dirty {
                    if let Some(disk) = &self.disk {
                        disk.save(chunk);
                    }
                }
            }
        }
    }

    /// Chiede un chunk che manca in memoria: prima al disco, poi al generatore. Non fa nulla se è già in arrivo.
    fn fetch(&mut self, dim: Dim, x: i32, z: i32) {
        let key = (dim as u8, x, z);
        if self.requested.contains(&key) || self.loading.contains(&key) {
            return;
        }
        match &self.disk {
            Some(disk) => {
                self.loading.insert(key);
                disk.load(dim, x, z);
            }
            None => self.generate(dim, x, z),
        }
    }

    /// Il chunk non è su disco: lo fa il generatore, o è piatto se il generatore non c'è.
    fn generate(&mut self, dim: Dim, x: i32, z: i32) {
        let key = (dim as u8, x, z);
        match (&self.gen, self.flat) {
            (Some(gen), false) => {
                self.requested.insert(key);
                gen.request_chunk(dim, x, z);
            }
            _ => {
                self.chunks.entry(key).or_insert_with(|| Chunk::flat(dim, x, z));
                // il piatto di ripiego non si salva
            }
        }
    }

    /// Un chunk dal generatore: lo salva su disco, lo tiene in memoria (lo `stream_tick` seguente lo invia a chi lo aspetta).
    pub(crate) fn on_chunk(&mut self, data: ChunkData) {
        let key = (data.dim as u8, data.x, data.z);
        self.requested.remove(&key);
        if self.chunks.contains_key(&key) {
            return; // già in memoria (anche modificato): non si sovrascrive
        }
        let chunk = Chunk::from_gen(data);
        if let Some(disk) = &self.disk {
            disk.save(chunk.clone());
        }
        self.chunks.insert(key, chunk);
    }

    pub(crate) fn on_gen_event(&mut self, event: GenEvent) {
        match event {
            GenEvent::Ready => {
                self.gen_ready = true;
                log!("[gen] generatore pronto");
            }
            GenEvent::Palette(entries) => {
                log!("[gen] palette dei blocchi ricevuta: {} voci", entries.len());
                if let Some(disk) = &self.disk {
                    disk.set_palette(entries);
                }
            }
            GenEvent::Chunk(data) => self.on_chunk(data),
            GenEvent::Spawn(Dim::Overworld, x, y, z) => {
                log!("[mondo] spawn calcolato dal generatore e salvato: {x}, {y}, {z}");
                self.spawn = Some((x, y, z));
                if let Some(disk) = &self.disk {
                    disk.set_spawn(x, y, z);
                }
                self.resume_waiting_logins();
            }
            GenEvent::Spawn(dim, ..) => log!("[gen] spawn di {dim:?} ignorato"),
            GenEvent::Down(reason) if !self.gen_ready && !self.flat => {
                log!("[gen] generatore assente ({reason}): uso il mondo piatto di ripiego; riavvia il server dopo aver sistemato il generatore");
                self.fall_back_to_flat();
            }
            GenEvent::Down(reason) => log!("[gen] generatore caduto ({reason}): il client lo riavvia e rimanda le richieste"),
        }
    }

    /// Passa al mondo piatto: i chunk attesi diventano piatti, lo spawn è quello piatto (non si salva), i login in coda partono.
    fn fall_back_to_flat(&mut self) {
        self.flat = true;
        self.gen = None;
        for (dim, x, z) in std::mem::take(&mut self.requested) {
            let dim = Dim::try_from(dim).unwrap_or(Dim::Overworld);
            self.chunks.entry((dim as u8, x, z)).or_insert_with(|| Chunk::flat(dim, x, z));
        }
        self.spawn = Some(crate::world::FLAT_SPAWN);
        self.resume_waiting_logins();
    }

    /// Lo spawn ora è noto: `StartGame` per chi lo aspettava.
    fn resume_waiting_logins(&mut self) {
        for pid in std::mem::take(&mut self.waiting_start) {
            if self.players.contains_key(&pid) {
                self.start_game(pid);
            }
        }
    }

    pub(crate) fn on_disk_event(&mut self, event: DiskEvent) {
        let DiskEvent::Loaded(dim, x, z, chunk) = event;
        let key = (dim as u8, x, z);
        self.loading.remove(&key);
        match chunk {
            Some(chunk) => {
                self.chunks.entry(key).or_insert(chunk);
            }
            None => self.generate(dim, x, z),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        disk::DiskEvent,
        gen_client::{GenClient, GenConfig, Launch, Pending},
        storage::World,
        world::Chunk,
        Player,
    };
    use bedrock_net::{Command, Sender};
    use bedrock_protocol::{generated::NetworkChunkPublisherUpdate, varint, Session};
    use std::{collections::HashSet, path::PathBuf, time::Duration};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    const R: i32 = 4;

    struct Rig {
        server: Server,
        /// Richieste che il server fa al generatore (finto).
        requests: UnboundedReceiver<Pending>,
        sent: UnboundedReceiver<Command>,
        disk_events: UnboundedReceiver<DiskEvent>,
    }

    /// Cartella temporanea di un test, rimossa alla fine.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("versmc-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Rig {
        /// Server con un generatore finto (che non risponde da solo) e senza disco.
        fn new() -> Rig {
            let (sender, sent) = Sender::for_tests();
            let (_, disk_events) = unbounded_channel();
            let (gen, requests) = GenClient::for_tests();
            let mut server = Server::new(sender);
            server.gen = Some(gen);
            server.flat = false;
            Rig { server, requests, sent, disk_events }
        }

        fn with_world(dir: &TempDir) -> Rig {
            let (sender, sent) = Sender::for_tests();
            let (tx, disk_events) = unbounded_channel();
            let (gen, requests) = GenClient::for_tests();
            let mut server = Server::with_world(sender, World::open_with(&dir.0, "Prova", Some(7)).unwrap(), tx);
            server.gen = Some(gen);
            server.flat = false;
            Rig { server, requests, sent, disk_events }
        }

        /// Un giocatore già nel mondo, con il centro in `chunk`.
        fn add_player(&mut self, pid: u64, radius: i32, chunk: (i32, i32)) {
            let pos = [chunk.0 as f32 * 16.0 + 8.0, 70.0, chunk.1 as f32 * 16.0 + 8.0];
            self.server.players.insert(pid, Player { spawned: true, streaming: true, radius, pos, last_chunk: chunk, ..Default::default() });
        }

        /// Richieste fatte al generatore da quando si è guardato l'ultima volta.
        fn requested(&mut self) -> Vec<(i32, i32)> {
            let mut out = Vec::new();
            while let Ok(p) = self.requests.try_recv() {
                match p {
                    Pending::Chunk(Dim::Overworld, x, z) => out.push((x, z)),
                    other => panic!("richiesta inattesa: {other:?}"),
                }
            }
            out
        }

        /// Il generatore risponde con un chunk vuoto.
        fn answer(&mut self, (x, z): (i32, i32)) {
            self.server.on_chunk(ChunkData { dim: Dim::Overworld, x, z, sections: vec![], block_entities: vec![] });
        }

        /// Chunk (x, z) di tutti i `LevelChunk` mandati a `pid`, e quanti `NetworkChunkPublisherUpdate`.
        fn sent_chunks(&mut self, pid: u64) -> (Vec<(i32, i32)>, usize) {
            let (mut chunks, mut publishers) = (Vec::new(), 0);
            while let Ok(cmd) = self.sent.try_recv() {
                let Command::Send(to, data) = cmd else { continue };
                for p in Session::default().decode(&data).unwrap().into_iter().filter(|_| to == pid) {
                    if p.id == LevelChunk::ID {
                        let mut b = &p.body[..];
                        chunks.push((varint::read_signed(&mut b).unwrap(), varint::read_signed(&mut b).unwrap()));
                    } else if p.id == NetworkChunkPublisherUpdate::ID {
                        publishers += 1;
                    }
                }
            }
            (chunks, publishers)
        }

        /// Pompa gli eventi del disco nel server (le letture finiscono in un thread a parte: si aspetta il primo).
        fn pump_disk(&mut self, expected: usize) {
            for _ in 0..expected {
                let event = self.disk_events.blocking_recv().expect("evento del disco");
                self.server.on_disk_event(event);
            }
        }
    }

    fn offset(set: Vec<(i32, i32)>, (dx, dz): (i32, i32)) -> HashSet<(i32, i32)> {
        set.into_iter().map(|(x, z)| (x + dx, z + dz)).collect()
    }

    #[test]
    fn wanted_is_sorted_by_distance_and_matches_old_count() {
        let w = wanted((0, 0), R);
        assert_eq!(w.len(), 69, "come il test end-to-end: punti con x² + z² <= r² + r");
        assert_eq!(w[0], (0, 0));
        assert_eq!(w.iter().collect::<HashSet<_>>().len(), 69, "nessun duplicato");
        assert!(w.windows(2).all(|p| p[0].0 * p[0].0 + p[0].1 * p[0].1 <= p[1].0 * p[1].0 + p[1].1 * p[1].1), "dal più vicino al più lontano");
        let shifted = wanted((10, -5), R);
        assert_eq!(shifted.iter().copied().collect::<HashSet<_>>(), offset(w, (10, -5)), "stessa forma attorno al centro");
        assert_eq!(chunk_of(0.0, 0.0), (0, 0));
        assert_eq!(chunk_of(-0.5, 15.99), (-1, 0));
        assert_eq!(chunk_of(-16.0, 16.0), (-1, 1));
    }

    #[test]
    fn moving_one_chunk_requests_only_the_new_edge() {
        let mut rig = Rig::new();
        rig.add_player(1, R, (0, 0));
        rig.server.stream_tick();
        let first = rig.requested();
        assert_eq!(first.iter().copied().collect::<HashSet<_>>(), wanted((0, 0), R).into_iter().collect(), "al primo giro tutta l'area");
        for c in first {
            rig.answer(c);
        }
        for _ in 0..10 {
            rig.server.stream_tick(); // 69 chunk a 8 per tick
        }
        let (sent, _) = rig.sent_chunks(1);
        assert_eq!(sent.len(), 69);
        assert!(rig.requested().is_empty());

        rig.server.on_move(1, [16.5, 70.0, 0.5]); // chunk (1, 0)
        rig.server.stream_tick();
        let edge: HashSet<_> = wanted((1, 0), R).into_iter().filter(|c| !wanted((0, 0), R).contains(c)).collect();
        let requested = rig.requested();
        assert_eq!(requested.len(), edge.len(), "senza duplicati");
        assert_eq!(requested.into_iter().collect::<HashSet<_>>(), edge, "solo il bordo nuovo");
        assert!(!edge.is_empty());
        let (_, publishers) = rig.sent_chunks(1);
        assert_eq!(publishers, 1, "il nuovo centro va al client");
    }

    #[test]
    fn fast_flight_never_requests_the_same_chunk_twice() {
        let mut rig = Rig::new();
        rig.add_player(1, R, (0, 0));
        let mut all = Vec::new();
        let mut slow: std::collections::VecDeque<Vec<(i32, i32)>> = Default::default();
        for step in 1..=40 {
            rig.server.on_move(1, [(step * 3) as f32 * 16.0 + 0.5, 70.0, 0.5]); // 3 chunk per mossa
            rig.server.stream_tick();
            let new = rig.requested();
            all.extend(new.iter().copied());
            slow.push_back(new);
            if slow.len() > 5 {
                slow.pop_front().unwrap().into_iter().for_each(|c| rig.answer(c));
                // il generatore risponde con 5 mosse di ritardo
            }
        }
        for c in slow.into_iter().flatten() {
            rig.answer(c);
        }
        assert_eq!(all.iter().collect::<HashSet<_>>().len(), all.len(), "nessun chunk chiesto due volte");
        assert!(all.len() > 500);
        let p = &rig.server.players[&1];
        assert!(p.sent.iter().all(|c| wanted(p.last_chunk, R).contains(c)), "sent contiene solo chunk nel raggio");
        rig.server.stream_tick();
        assert!(rig.server.chunks.len() < 150, "i chunk lasciati indietro vengono scaricati: {}", rig.server.chunks.len());
    }

    #[test]
    fn returning_to_a_chunk_resends_it() {
        let mut rig = Rig::new();
        rig.add_player(1, 2, (0, 0));
        rig.server.stream_tick();
        for c in rig.requested() {
            rig.answer(c);
        }
        for _ in 0..5 {
            rig.server.stream_tick();
        }
        assert!(rig.sent_chunks(1).0.contains(&(0, 0)));
        rig.server.on_move(1, [16.0 * 20.0, 70.0, 0.0]); // lontano
        assert!(!rig.server.players[&1].sent.contains(&(0, 0)));
        rig.server.on_move(1, [0.5, 70.0, 0.5]); // di ritorno: il chunk (0, 0) è ancora in memoria
        rig.server.stream_tick();
        assert!(rig.sent_chunks(1).0.contains(&(0, 0)), "rimandato");
    }

    #[test]
    fn chunks_left_behind_are_unloaded_and_saved_if_dirty() {
        let dir = TempDir::new("unload");
        let mut rig = Rig::with_world(&dir);
        rig.add_player(1, 2, (0, 0));
        let mut dirty = Chunk::flat(Dim::Overworld, 6, 0); // oltre raggio + 2 (6² > 4² + 4)
        dirty.dirty = true;
        let expected = dirty.network_bytes();
        let kept = Chunk::flat(Dim::Overworld, 4, 0); // 4² <= 4² + 4: dentro raggio + 2
        let mut kept_dirty = Chunk::flat(Dim::Overworld, 0, 0);
        kept_dirty.dirty = true;
        let clean_far = Chunk::flat(Dim::Overworld, -9, 9);
        rig.server.chunks.insert((0, 6, 0), dirty);
        rig.server.chunks.insert((0, 4, 0), kept);
        rig.server.chunks.insert((0, 0, 0), kept_dirty);
        rig.server.chunks.insert((0, -9, 9), clean_far);
        rig.server.stream_tick();
        assert!(!rig.server.chunks.contains_key(&(0, 6, 0)), "scaricato");
        assert!(!rig.server.chunks.contains_key(&(0, -9, 9)), "scaricato");
        assert!(rig.server.chunks.contains_key(&(0, 4, 0)) && rig.server.chunks.contains_key(&(0, 0, 0)), "ancora nel raggio + 2");
        assert!(rig.server.chunks[&(0, 0, 0)].dirty, "quello vicino resta modificato in memoria");
        drop(rig);

        let mut world = World::open(&dir.0).unwrap();
        assert_eq!(world.load(Dim::Overworld, 6, 0).expect("chunk modificato salvato").network_bytes(), expected);
        assert!(world.load(Dim::Overworld, -9, 9).is_none(), "il chunk pulito non viene scritto");
        assert!(world.load(Dim::Overworld, 0, 0).is_none(), "quello ancora caricato non è stato toccato");
    }

    #[test]
    fn chunk_from_the_generator_is_saved_and_a_stored_chunk_is_read_before_asking_the_generator() {
        let dir = TempDir::new("disk-first");
        let mut rig = Rig::with_world(&dir);
        rig.add_player(1, 1, (0, 0));
        rig.server.stream_tick();
        let first = rig.requested();
        // con un disco, prima si legge: nessuna richiesta al generatore finché il disco non dice "non c'è"
        assert!(first.is_empty());
        let n = rig.server.loading.len();
        assert_eq!(n, wanted((0, 0), 1).len());
        rig.pump_disk(n);
        assert_eq!(rig.requested().len(), n, "il disco è vuoto: tutti al generatore");
        rig.answer((0, 0)); // salvato
        rig.server.chunks.clear();
        rig.server.requested.clear();
        rig.server.stream_tick();
        rig.pump_disk(wanted((0, 0), 1).len());
        let again = rig.requested();
        assert!(!again.contains(&(0, 0)), "(0, 0) viene dal disco, non dal generatore");
        assert_eq!(again.len(), wanted((0, 0), 1).len() - 1);
        assert!(rig.server.chunks.contains_key(&(0, 0, 0)));
    }

    #[tokio::test]
    async fn missing_generator_falls_back_to_flat_world() {
        let cfg = GenConfig {
            launch: Launch::Process { program: PathBuf::from("questo-programma-non-esiste-xyz"), args_extra: vec![] },
            seed: 1,
            restart_delay: Duration::from_millis(50),
        };
        let (client, mut events) = GenClient::spawn(cfg);
        let (sender, _sent) = Sender::for_tests();
        let mut server = Server::new(sender);
        server.gen = Some(client);
        server.flat = false;
        server.spawn = None;
        server.players.insert(1, Player { spawned: true, streaming: true, radius: 2, ..Default::default() });
        server.stream_tick();
        assert_eq!(server.requested.len(), wanted((0, 0), 2).len(), "col generatore in avvio i chunk si chiedono");
        assert!(server.chunks.is_empty());

        let event = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
        assert!(matches!(event, GenEvent::Down(_)), "il programma manca");
        server.on_gen_event(event);
        assert!(server.flat && server.gen.is_none(), "il mondo piatto sostituisce il generatore");
        assert!(server.requested.is_empty());
        assert_eq!(server.chunks.len(), wanted((0, 0), 2).len(), "i chunk attesi sono diventati piatti");
        assert_eq!(server.spawn, Some(crate::world::FLAT_SPAWN), "senza generatore lo spawn è quello piatto");
        let flat = Chunk::flat(Dim::Overworld, 0, 0).network_bytes();
        assert_eq!(server.chunks[&(0, 1, 1)].network_bytes(), flat);
        server.players.get_mut(&1).unwrap().last_chunk = (30, 30);
        server.stream_tick();
        assert!(server.requested.is_empty() && server.chunks.contains_key(&(0, 30, 30)), "i nuovi chunk sono piatti, senza richieste");
    }

    #[test]
    fn start_game_waits_for_the_spawn() {
        let dir = TempDir::new("spawn");
        let mut rig = Rig::with_world(&dir);
        rig.server.spawn = None;
        rig.server.players.insert(1, Player::default());
        rig.server.start_game(1);
        assert!(rig.sent.try_recv().is_err(), "StartGame non parte senza spawn");
        assert_eq!(rig.server.waiting_start, vec![1]);
        rig.server.on_gen_event(GenEvent::Spawn(Dim::Overworld, 10, 70, -5));
        assert_eq!(rig.server.spawn, Some((10, 70, -5)));
        assert!(rig.server.waiting_start.is_empty());
        let Ok(Command::Send(1, data)) = rig.sent.try_recv() else { panic!("StartGame atteso") };
        let packets = Session::default().decode(&data).unwrap();
        assert!(packets.iter().any(|p| p.id == bedrock_protocol::generated::StartGame::ID));
        let p = &rig.server.players[&1];
        assert_eq!((p.pos[0], p.pos[1], p.pos[2]), (10.5, 71.62, -4.5), "occhi = piedi + 1.62");
        assert_eq!(p.last_chunk, (0, -1));
        drop(rig);
        assert_eq!(World::open(&dir.0).unwrap().spawn(), Some((10, 70, -5)), "spawn salvato in level.dat");
    }
}
