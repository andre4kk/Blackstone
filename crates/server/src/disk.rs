//! Il mondo su disco fuori dal tick: un thread possiede il `World` e riceve comandi in ordine (letture, scritture, flush).
//! Le letture dei chunk tornano al ciclo di gioco come `DiskEvent`, così il tick non aspetta mai LevelDB.
use crate::{storage::World, world::Chunk};
use blackstone_net::warn;
use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
};
use tokio::sync::mpsc::UnboundedSender;
use vgen::Dim;

/// Risultato di una richiesta al disco.
pub(crate) enum DiskEvent {
    /// Chunk letto (`None` se non è mai stato salvato o è illeggibile).
    Loaded(Dim, i32, i32, Option<Chunk>),
    /// Dati salvati di un giocatore (`None` se non ne ha o sono illeggibili); il numero è l'ID della connessione.
    PlayerLoaded(u64, Option<Vec<u8>>),
}

enum Cmd {
    Load(Dim, i32, i32),
    Save(Box<Chunk>),
    LoadPlayer(u64, String),
    SavePlayer(String, Vec<u8>),
    Palette(Vec<(i32, Vec<u8>)>),
    Spawn(i32, i32, i32),
    Flush(mpsc::Sender<()>),
}

pub(crate) struct Disk {
    tx: Option<mpsc::Sender<Cmd>>,
    thread: Option<JoinHandle<()>>,
}

impl Disk {
    /// Sposta `world` nel suo thread. Gli eventi di lettura arrivano su `events`.
    pub(crate) fn start(mut world: World, events: UnboundedSender<DiskEvent>) -> Disk {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let thread = thread::Builder::new()
            .name("mondo".into())
            .spawn(move || {
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Cmd::Load(dim, x, z) => {
                            let chunk = world.load(dim, x, z);
                            let _ = events.send(DiskEvent::Loaded(dim, x, z, chunk));
                        }
                        Cmd::Save(chunk) => {
                            if let Err(e) = world.save(&chunk) {
                                warn!("[mondo] salvataggio del chunk {},{} fallito: {e}", chunk.x, chunk.z);
                            }
                        }
                        Cmd::LoadPlayer(pid, identity) => {
                            let data = world.load_player(&identity);
                            let _ = events.send(DiskEvent::PlayerLoaded(pid, data));
                        }
                        Cmd::SavePlayer(identity, nbt) => {
                            if let Err(e) = world.save_player(&identity, &nbt) {
                                warn!("[mondo] salvataggio dei dati di {identity} fallito: {e}");
                            }
                        }
                        Cmd::Palette(entries) => world.set_palette(entries),
                        Cmd::Spawn(x, y, z) => world.set_spawn(x, y, z),
                        Cmd::Flush(reply) => {
                            if let Err(e) = world.flush() {
                                warn!("[mondo] flush fallito: {e}");
                            }
                            let _ = reply.send(());
                        }
                    }
                }
            })
            .expect("thread del mondo non avviabile");
        Disk { tx: Some(tx), thread: Some(thread) }
    }

    fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    pub(crate) fn load(&self, dim: Dim, x: i32, z: i32) {
        self.send(Cmd::Load(dim, x, z));
    }

    pub(crate) fn save(&self, chunk: Chunk) {
        self.send(Cmd::Save(Box::new(chunk)));
    }

    /// Chiede i dati salvati del giocatore `identity`: la risposta arriva come `DiskEvent::PlayerLoaded(pid, ..)`.
    pub(crate) fn load_player(&self, pid: u64, identity: String) {
        self.send(Cmd::LoadPlayer(pid, identity));
    }

    pub(crate) fn save_player(&self, identity: String, nbt: Vec<u8>) {
        self.send(Cmd::SavePlayer(identity, nbt));
    }

    pub(crate) fn set_palette(&self, entries: Vec<(i32, Vec<u8>)>) {
        self.send(Cmd::Palette(entries));
    }

    pub(crate) fn set_spawn(&self, x: i32, y: i32, z: i32) {
        self.send(Cmd::Spawn(x, y, z));
    }

    /// Aspetta che tutto quanto inviato finora sia sul disco. Solo alla chiusura: blocca il chiamante.
    pub(crate) fn flush_blocking(&self) {
        let (tx, rx) = mpsc::channel();
        self.send(Cmd::Flush(tx));
        let _ = rx.recv();
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        self.tx = None; // il thread finisce i comandi in coda, esce e chiude il database
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
