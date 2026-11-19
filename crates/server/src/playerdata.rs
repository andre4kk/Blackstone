//! Dati del giocatore nel formato dei mondi Bedrock: NBT little-endian con posizione, modalità e inventario.
//!
//! I nomi dei campi sono quelli dei mondi dei server dedicati (DT§5): `Pos`, `Rotation`, `DimensionId`, `PlayerGameType`,
//! `Inventory`, `Offhand`, `Armor`, `SelectedInventorySlot`. Il file lo scrive e lo legge il thread del mondo; qui c'è solo la
//! conversione da e verso l'NBT e la regola su quando salvare.
use crate::{
    disk::Disk,
    inventory::{Inventory, Item, ARMOR_SLOTS, HOTBAR_SLOTS, MAIN_SLOTS, MAX_STACK},
    nbt::{write_root, Enc, Reader, Tag},
    registry,
    storage::{block_from_tag, BLOCK_STATE_VERSION},
    survival::{MAX_FOOD, MAX_HEALTH, MAX_SATURATION, START_SATURATION},
    DataState, GameMode, Player,
};
use bedrock_net::log;
use bedrock_protocol::login::PlayerInfo;

/// Quanto gli occhi stanno sopra i piedi.
pub(crate) const EYE_HEIGHT: f32 = 1.62;

/// Ciò che si ricorda di un giocatore tra una sessione e l'altra.
pub(crate) struct PlayerData {
    /// Posizione dei piedi.
    pub feet: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub dim: i32,
    pub mode: GameMode,
    pub inventory: Inventory,
    /// Vita e fame: se mancano nei dati salvati valgono i valori pieni (`survival`).
    pub health: f32,
    pub food: u8,
    pub saturation: f32,
    pub exhaustion: f32,
}

/// Intero di un tag numerico qualunque (i mondi veri usano Byte, Short o Int a seconda del campo).
pub(crate) fn int(tag: &Tag) -> Option<i64> {
    match tag {
        Tag::Byte(v) => Some(*v as i64),
        Tag::Short(v) => Some(*v as i64),
        Tag::Int(v) => Some(*v as i64),
        Tag::Long(v) => Some(*v),
        _ => None,
    }
}

/// Elementi di una lista; vuoti se il tag manca o è di un altro tipo.
fn list_items(tag: Option<&Tag>) -> &[Tag] {
    match tag {
        Some(Tag::List(_, items)) => items,
        _ => &[],
    }
}

/// Lista di `N` float, tutti finiti.
fn floats<const N: usize>(tag: Option<&Tag>) -> Option<[f32; N]> {
    let Some(Tag::List(_, items)) = tag else { return None };
    if items.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        match item {
            Tag::Float(v) if v.is_finite() => *slot = *v,
            _ => return None,
        }
    }
    Some(out)
}

/// Il campo `Block` di un oggetto: `{name, states, version}` dello stato che piazza.
fn block_tag(hash: i32) -> Option<Tag> {
    let state = registry::blocks().state(hash)?;
    let (_, mut tag) = Reader::new(&state.nbt(), Enc::Disk).root()?;
    tag.set("version", Tag::Int(BLOCK_STATE_VERSION));
    Some(tag)
}

/// Un oggetto come pila NBT; `None` se è vuoto o il suo ID non ha un nome. `slot` è la casella, per le liste che la dichiarano.
pub(crate) fn stack_to_tag(item: &Item, slot: Option<u8>) -> Option<Tag> {
    if item.is_empty() {
        return None;
    }
    let Some(name) = registry::items().name(item.id) else {
        log!("[mondo] oggetto con ID {} senza nome: non salvato", item.id);
        return None;
    };
    let mut fields = Vec::new();
    if let Some(slot) = slot {
        fields.push(("Slot".to_string(), Tag::Byte(slot as i8)));
    }
    fields.push(("Name".into(), Tag::String(name.into())));
    fields.push(("Count".into(), Tag::Byte(item.count.min(MAX_STACK) as i8)));
    fields.push(("Damage".into(), Tag::Short(item.aux as i16)));
    fields.push(("WasPickedUp".into(), Tag::Byte(0)));
    if let Some(block) = item.block.and_then(block_tag) {
        fields.push(("Block".into(), block));
    }
    if let Some(nbt) = &item.nbt {
        match Reader::new(nbt, Enc::Disk).root() {
            Some((_, tag @ Tag::Compound(_))) => fields.push(("tag".into(), tag)),
            _ => log!("[mondo] NBT di un oggetto illeggibile: salvato senza"),
        }
    }
    Some(Tag::Compound(fields))
}

/// La casella di una lista fissa (mano secondaria, armatura): la pila o, se vuota, la pila vuota come la scrive il gioco.
fn slot_tag(item: &Item, slot: u8) -> Tag {
    stack_to_tag(item, Some(slot)).unwrap_or_else(|| {
        Tag::Compound(vec![
            ("Slot".into(), Tag::Byte(slot as i8)),
            ("Name".into(), Tag::String(String::new())),
            ("Count".into(), Tag::Byte(0)),
            ("Damage".into(), Tag::Short(0)),
            ("WasPickedUp".into(), Tag::Byte(0)),
        ])
    })
}

/// Una pila NBT come oggetto. `None` se è vuota o se l'oggetto non esiste in questa versione (si salta, con una riga di log).
pub(crate) fn stack_from_tag(tag: &Tag) -> Option<Item> {
    let name = tag.get("Name")?.as_str()?;
    let count = tag.get("Count").and_then(int).unwrap_or(0);
    if name.is_empty() || count <= 0 {
        return None;
    }
    let Some(id) = registry::items().id(name) else {
        log!("[mondo] oggetto {name} sconosciuto nei dati salvati: saltato");
        return None;
    };
    let aux = tag.get("Damage").and_then(int).map_or(0, |d| d as u16 as u32);
    let block = tag.get("Block").and_then(block_from_tag).map(|(_, hash)| hash);
    let nbt = match tag.get("tag") {
        Some(t @ Tag::Compound(_)) => Some(write_root("", t, Enc::Disk)),
        _ => None,
    };
    Some(Item { id, count: count.min(MAX_STACK as i64) as u16, aux, block, nbt })
}

impl PlayerData {
    pub fn to_nbt(&self) -> Vec<u8> {
        let inv = &self.inventory;
        let main = (0..MAIN_SLOTS).filter_map(|i| stack_to_tag(inv.main(i), Some(i as u8))).collect();
        let armor = (0..ARMOR_SLOTS).map(|i| slot_tag(inv.armor(i), i as u8)).collect();
        let floats = |values: &[f32]| Tag::List(5, values.iter().map(|&v| Tag::Float(v)).collect());
        let root = Tag::Compound(vec![
            ("Pos".into(), floats(&self.feet)),
            ("Rotation".into(), floats(&[self.yaw, self.pitch])),
            ("DimensionId".into(), Tag::Int(self.dim)),
            ("PlayerGameType".into(), Tag::Int(self.mode as i32)),
            ("Inventory".into(), Tag::List(10, main)),
            ("Offhand".into(), Tag::List(10, vec![slot_tag(inv.offhand(), 0)])),
            ("Armor".into(), Tag::List(10, armor)),
            ("SelectedInventorySlot".into(), Tag::Int(inv.selected as i32)),
            // nomi di PocketMine (i mondi vanilla tengono questi valori tra gli `Attributes`, che il server non scrive)
            ("Health".into(), Tag::Float(self.health)),
            ("foodLevel".into(), Tag::Int(self.food as i32)),
            ("foodSaturationLevel".into(), Tag::Float(self.saturation)),
            ("foodExhaustionLevel".into(), Tag::Float(self.exhaustion)),
        ]);
        write_root("", &root, Enc::Disk)
    }

    /// Legge i dati salvati. `None` se l'NBT è illeggibile o manca la posizione; per il resto, ciò che manca o è strano prende il valore di partenza.
    pub fn from_nbt(b: &[u8]) -> Option<Self> {
        let (_, root) = Reader::new(b, Enc::Disk).root()?;
        if !matches!(root, Tag::Compound(_)) {
            return None;
        }
        let feet = floats::<3>(root.get("Pos"))?;
        let [yaw, pitch] = floats::<2>(root.get("Rotation")).unwrap_or([0.0; 2]);
        let dim = root.get("DimensionId").and_then(int).unwrap_or(0) as i32;
        // il wiki chiama il campo `PlayerGameMode`, `level.dat` `PlayerGameType`: si leggono entrambi
        let mode =
            ["PlayerGameType", "PlayerGameMode"].iter().find_map(|k| root.get(k).and_then(int)).and_then(|m| GameMode::from_i32(m as i32)).unwrap_or_default();
        let mut inventory = Inventory::default();
        for entry in list_items(root.get("Inventory")) {
            if let (Some(slot), Some(item)) = (entry.get("Slot").and_then(int), stack_from_tag(entry)) {
                if (0..MAIN_SLOTS as i64).contains(&slot) {
                    inventory.set_main(slot as usize, item);
                }
            }
        }
        if let Some(item) = list_items(root.get("Offhand")).first().and_then(stack_from_tag) {
            inventory.set_offhand(item);
        }
        for (i, entry) in list_items(root.get("Armor")).iter().enumerate() {
            // `Slot` nell'armatura non è verificato su un mondo vero: se manca vale la posizione nella lista
            let slot = entry.get("Slot").and_then(int).unwrap_or(i as i64);
            if let (true, Some(item)) = ((0..ARMOR_SLOTS as i64).contains(&slot), stack_from_tag(entry)) {
                inventory.set_armor(slot as usize, item);
            }
        }
        inventory.selected = root.get("SelectedInventorySlot").and_then(int).filter(|s| (0..HOTBAR_SLOTS as i64).contains(s)).map_or(0, |s| s as u8);
        // una vita a 0 (morto all'uscita) o assurda riparte piena: un giocatore non rientra morto
        let float = |key: &str| match root.get(key) {
            Some(Tag::Float(v)) if v.is_finite() => Some(*v),
            _ => None,
        };
        let health = float("Health").filter(|&h| h > 0.0).map_or(MAX_HEALTH, |h| h.min(MAX_HEALTH));
        let food = root.get("foodLevel").and_then(int).map_or(MAX_FOOD, |f| f.clamp(0, MAX_FOOD as i64) as u8);
        let saturation = float("foodSaturationLevel").map_or(START_SATURATION, |s| s.clamp(0.0, MAX_SATURATION));
        let exhaustion = float("foodExhaustionLevel").map_or(0.0, |e| e.clamp(0.0, 4.0));
        Some(PlayerData { feet, yaw, pitch, dim, mode, inventory, health, food, saturation, exhaustion })
    }
}

/// Chiave con cui il mondo riconosce il giocatore: l'XUID se c'è, altrimenti `name:<nome>`. Un XUID che non è un numero non
/// vale: il login non è verificato e non deve poter puntare a chiavi del database che non sono sue.
pub(crate) fn identity(info: &PlayerInfo) -> String {
    match info.xuid.as_deref() {
        Some(x) if !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()) => x.to_string(),
        _ => format!("name:{}", info.name),
    }
}

/// Manda al disco i dati del giocatore che esce (o del server che chiude). Non salva senza un mondo su disco né prima che i dati
/// veri siano arrivati: l'inventario vuoto di chi esce prima del caricamento cancellerebbe quello buono.
pub(crate) fn persist(disk: Option<&Disk>, p: &mut Player) {
    let (Some(disk), Some(info)) = (disk, p.info.as_ref()) else { return };
    if !p.started || p.data != DataState::Loaded {
        return;
    }
    p.started = false; // una sola volta: l'inventario passa ai dati e qui non resta più
    let data = PlayerData {
        feet: [p.pos[0], p.pos[1] - EYE_HEIGHT, p.pos[2]],
        yaw: p.yaw,
        pitch: p.pitch,
        dim: 0,
        mode: p.mode,
        inventory: std::mem::take(&mut p.inventory),
        health: p.health,
        food: p.food,
        saturation: p.saturation,
        exhaustion: p.exhaustion,
    };
    disk.save_player(identity(info), data.to_nbt());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        disk::DiskEvent,
        inventory::Item,
        nbt::{write_root, Enc, Tag},
        registry,
        storage::World,
        Player, Server,
    };
    use bedrock_net::{Command, Sender};
    use bedrock_protocol::{generated::StartGame, reader::Reader, wire::ServerPacket, Packet, Session};
    use std::path::{Path, PathBuf};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    /// Cartella temporanea di un test, rimossa alla fine.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("versmc-test-pd-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn item(name: &str, count: u16) -> Item {
        Item { id: registry::items().id(name).unwrap_or_else(|| panic!("oggetto {name} sconosciuto")), count, ..Default::default() }
    }

    fn info(name: &str, xuid: Option<&str>) -> PlayerInfo {
        PlayerInfo {
            name: name.into(),
            xuid: xuid.map(String::from),
            uuid: None,
            device_os: None,
            game_version: None,
            authentication_type: None,
            token_claim_keys: Vec::new(),
            skin: bedrock_protocol::skin::Skin::fallback(),
        }
    }

    fn sample() -> PlayerData {
        let mut inventory = Inventory::default();
        inventory.set_main(0, item("minecraft:stone", 64));
        inventory.set_main(5, Item { aux: 3, ..item("minecraft:diamond_sword", 1) });
        let planks = registry::blocks().default_hash("minecraft:oak_stairs").unwrap();
        let state = registry::blocks().state(planks).unwrap().with("weirdo_direction", registry::StateValue::Int(2));
        inventory.set_main(
            20,
            Item {
                block: Some(state.hash()),
                nbt: Some(write_root("", &Tag::Compound(vec![("RepairCost".into(), Tag::Int(7))]), Enc::Disk)),
                ..item("minecraft:oak_stairs", 12)
            },
        );
        inventory.set_offhand(item("minecraft:shield", 1));
        inventory.set_armor(0, item("minecraft:iron_helmet", 1));
        inventory.selected = 4;
        PlayerData {
            feet: [10.5, 64.0, -3.25],
            yaw: 90.0,
            pitch: -10.0,
            dim: 0,
            mode: GameMode::Survival,
            inventory,
            health: 7.0,
            food: 13,
            saturation: 2.5,
            exhaustion: 1.25,
        }
    }

    fn contents(inv: &Inventory) -> Vec<Item> {
        (0..36).map(|i| inv.main(i).clone()).chain([inv.offhand().clone()]).chain((0..4).map(|i| inv.armor(i).clone())).collect()
    }

    #[test]
    fn player_data_round_trip() {
        let data = sample();
        let back = PlayerData::from_nbt(&data.to_nbt()).expect("i dati scritti si rileggono");
        assert_eq!((back.feet, back.yaw, back.pitch, back.dim), (data.feet, 90.0, -10.0, 0));
        assert_eq!(back.mode, GameMode::Survival);
        assert_eq!(back.inventory.selected, 4);
        assert_eq!(contents(&back.inventory), contents(&data.inventory));
        assert_eq!(back.inventory.main(5).aux, 3);
        assert_eq!(back.inventory.main(20).count, 12);

        // forma su disco: campi con i nomi di Bedrock, solo le caselle occupate
        let (_, root) = crate::nbt::Reader::new(&data.to_nbt(), Enc::Disk).root().unwrap();
        let Some(Tag::List(_, slots)) = root.get("Inventory") else { panic!("manca Inventory") };
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0].get("Slot"), Some(&Tag::Byte(0)));
        assert_eq!(slots[0].get("Name").and_then(Tag::as_str), Some("minecraft:stone"));
        assert_eq!(slots[0].get("Count"), Some(&Tag::Byte(64)));
        assert_eq!(slots[1].get("Damage"), Some(&Tag::Short(3)));
        assert!(slots[2].get("Block").and_then(|b| b.get("name")).is_some(), "stato del blocco");
        assert!(slots[2].get("tag").is_some(), "NBT dell'oggetto");
        assert_eq!(root.get("PlayerGameType"), Some(&Tag::Int(0)));
        assert_eq!(root.get("SelectedInventorySlot"), Some(&Tag::Int(4)));
        assert!(matches!(root.get("Pos"), Some(Tag::List(5, p)) if p.len() == 3));
        assert!(matches!(root.get("Armor"), Some(Tag::List(10, a)) if a.len() == 4));
    }

    #[test]
    fn vitals_round_trip_and_default_to_full() {
        let back = PlayerData::from_nbt(&sample().to_nbt()).unwrap();
        assert_eq!((back.health, back.food, back.saturation, back.exhaustion), (7.0, 13, 2.5, 1.25));
        let (_, root) = crate::nbt::Reader::new(&sample().to_nbt(), Enc::Disk).root().unwrap();
        assert_eq!(root.get("Health"), Some(&Tag::Float(7.0)));
        assert_eq!(root.get("foodLevel"), Some(&Tag::Int(13)));

        // dati vecchi, senza vita né fame: tutto pieno
        let mut old = root.clone();
        let Tag::Compound(fields) = &mut old else { unreachable!() };
        fields.retain(|(k, _)| !["Health", "foodLevel", "foodSaturationLevel", "foodExhaustionLevel"].contains(&k.as_str()));
        let back = PlayerData::from_nbt(&write_root("", &old, Enc::Disk)).unwrap();
        assert_eq!((back.health, back.food, back.saturation, back.exhaustion), (20.0, 20, 5.0, 0.0));

        // morto all'uscita (vita 0) o valori fuori scala: si riparte pieni / nei limiti
        let mut odd = root.clone();
        odd.set("Health", Tag::Float(0.0));
        odd.set("foodLevel", Tag::Int(99));
        let back = PlayerData::from_nbt(&write_root("", &odd, Enc::Disk)).unwrap();
        assert_eq!((back.health, back.food), (20.0, 20));
    }

    #[test]
    fn from_nbt_accepts_the_wiki_game_mode_name_and_rejects_garbage() {
        let mut root = crate::nbt::Reader::new(&sample().to_nbt(), Enc::Disk).root().unwrap().1;
        let Tag::Compound(items) = &mut root else { unreachable!() };
        items.retain(|(k, _)| k != "PlayerGameType");
        root.set("PlayerGameMode", Tag::Int(1));
        let back = PlayerData::from_nbt(&write_root("", &root, Enc::Disk)).unwrap();
        assert_eq!(back.mode, GameMode::Creative);

        assert!(PlayerData::from_nbt(&[]).is_none());
        assert!(PlayerData::from_nbt(&[1, 2, 3, 4]).is_none());
        let good = sample().to_nbt();
        assert!(PlayerData::from_nbt(&good[..good.len() / 2]).is_none(), "troncato");
    }

    #[test]
    fn identity_prefers_the_xuid() {
        assert_eq!(identity(&info("Alice", Some("2535"))), "2535");
        assert_eq!(identity(&info("Alice", None)), "name:Alice");
        assert_eq!(identity(&info("Alice", Some(""))), "name:Alice");
        assert_eq!(identity(&info("Alice", Some("server_x"))), "name:Alice", "un XUID non numerico non può puntare a chiavi altrui");
    }

    struct Rig {
        server: Server,
        sent: UnboundedReceiver<Command>,
        events: UnboundedReceiver<DiskEvent>,
    }

    impl Rig {
        fn new(dir: &Path) -> Rig {
            let (sender, sent) = Sender::for_tests();
            let (tx, events) = unbounded_channel();
            Rig { server: Server::with_world(sender, World::open_with(dir, "Prova", Some(7)).unwrap(), tx), sent, events }
        }

        /// Un giocatore appena uscito dal login, che ha finito i resource pack: tocca a `start_game`.
        fn login(&mut self, pid: u64, name: &str, xuid: &str) {
            self.server.players.insert(pid, Player { info: Some(info(name, Some(xuid))), ..Default::default() });
            self.server.start_game(pid);
        }

        /// Il disco risponde a una richiesta (si aspetta il suo thread).
        fn pump(&mut self) {
            for _ in 0..500 {
                if let Ok(event) = self.events.try_recv() {
                    return self.server.on_disk_event(event);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("il disco non ha risposto entro 5 s");
        }

        /// (posizione degli occhi, modalità) dei `StartGame` mandati da quando si è guardato l'ultima volta.
        fn start_games(&mut self) -> Vec<([f32; 3], i32)> {
            let mut out = Vec::new();
            while let Ok(cmd) = self.sent.try_recv() {
                let Command::Send(_, data) = cmd else { continue };
                for p in Session::default().decode(&data).unwrap().into_iter().filter(|p: &Packet| p.id == StartGame::ID) {
                    let r = &mut Reader(&p.body);
                    r.var_i64().unwrap();
                    r.var_u64().unwrap();
                    let mode = r.var_i32().unwrap();
                    out.push((r.vec3().unwrap(), mode));
                }
            }
            out
        }
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    /// Un giocatore che entra, si sposta, cambia inventario ed esce: i suoi dati sono sul disco.
    fn play_and_quit(rig: &mut Rig, pid: u64, xuid: &str) {
        rig.login(pid, "Alice", xuid);
        rig.pump();
        let p = rig.server.players.get_mut(&pid).unwrap();
        p.pos = [100.5, 71.62, -20.5];
        p.yaw = 45.0;
        p.mode = GameMode::Survival;
        p.inventory.set_main(2, item("minecraft:stone", 10));
        rig.server.close(pid);
    }

    #[test]
    fn login_waits_for_player_data_before_start_game() {
        let dir = TempDir::new("wait");
        let mut rig = Rig::new(&dir.0);
        play_and_quit(&mut rig, 1, "2535");
        rig.start_games();

        rig.login(2, "Alice", "2535");
        assert!(rig.start_games().is_empty(), "StartGame aspetta i dati del giocatore (disco lento: nessun evento ancora gestito)");
        rig.pump();
        let starts = rig.start_games();
        assert_eq!(starts.len(), 1);
        assert!(near(starts[0].0, [100.5, 71.62, -20.5]), "vale la posizione salvata, non lo spawn: {:?}", starts[0].0);
        assert_eq!(starts[0].1, 0, "modalità sopravvivenza salvata");
        assert_eq!(rig.server.players[&2].inventory.main(2).count, 10);
        assert_eq!(rig.server.players[&2].yaw, 45.0);
    }

    #[test]
    fn health_and_food_are_saved_and_restored() {
        let dir = TempDir::new("vitals");
        let mut rig = Rig::new(&dir.0);
        rig.login(1, "Alice", "2535");
        rig.pump();
        let p = rig.server.players.get_mut(&1).unwrap();
        (p.health, p.food, p.saturation, p.exhaustion) = (7.0, 9, 1.5, 2.25);
        rig.server.close(1);

        rig.login(2, "Alice", "2535");
        rig.pump();
        let p = &rig.server.players[&2];
        assert_eq!((p.health, p.food, p.saturation, p.exhaustion), (7.0, 9, 1.5, 2.25));
    }

    /// Alice muore lontano dallo spawn, con una pietra in tasca, ed esce senza rinascere.
    fn die_and_quit(rig: &mut Rig) {
        rig.login(1, "Alice", "2535");
        rig.pump();
        let p = rig.server.players.get_mut(&1).unwrap();
        (p.mode, p.pos, p.food) = (GameMode::Survival, [100.5, 71.62, -20.5], 3);
        p.inventory.set_main(2, item("minecraft:stone", 10));
        assert!(rig.server.hurt(1, 100.0, crate::survival::DamageCause::Kill));
        rig.server.close(1);
    }

    #[test]
    fn quitting_dead_rejoins_alive_at_spawn_with_an_empty_inventory() {
        let dir = TempDir::new("dead-quit");
        let mut rig = Rig::new(&dir.0);
        die_and_quit(&mut rig);
        rig.login(2, "Alice", "2535");
        rig.pump();
        let p = &rig.server.players[&2];
        let (x, y, z) = crate::world::FLAT_SPAWN;
        assert!(near(p.pos, [x as f32 + 0.5, y as f32 + EYE_HEIGHT, z as f32 + 0.5]));
        assert_eq!((p.health, p.food, p.saturation, p.exhaustion, p.dead), (20.0, 20, 5.0, 0.0, false));
        assert!(p.inventory.main(2).is_empty(), "gli oggetti sono rimasti a terra dove è morto");
    }

    #[test]
    fn quitting_dead_with_keep_inventory_keeps_it() {
        let dir = TempDir::new("dead-quit-keep");
        let mut rig = Rig::new(&dir.0);
        rig.server.keep_inventory = true;
        die_and_quit(&mut rig);
        rig.login(2, "Alice", "2535");
        rig.pump();
        let p = &rig.server.players[&2];
        assert_eq!(p.inventory.main(2).count, 10);
        assert_eq!(p.health, 20.0);
    }

    #[test]
    fn first_login_without_data_starts_at_spawn() {
        let dir = TempDir::new("fresh");
        let mut rig = Rig::new(&dir.0);
        rig.login(1, "Alice", "2535");
        rig.pump();
        let starts = rig.start_games();
        let (x, y, z) = crate::world::FLAT_SPAWN;
        assert!(near(starts[0].0, [x as f32 + 0.5, y as f32 + EYE_HEIGHT, z as f32 + 0.5]));
        assert_eq!(starts[0].1, GameMode::default() as i32);
    }

    #[test]
    fn malformed_data_starts_fresh() {
        let dir = TempDir::new("bad");
        let mut rig = Rig::new(&dir.0);
        rig.login(1, "Alice", "2535");
        rig.server.on_disk_event(DiskEvent::PlayerLoaded(1, Some(vec![9, 9, 9])));
        let starts = rig.start_games();
        assert_eq!(starts.len(), 1, "il login non si blocca");
        assert!(rig.server.players[&1].inventory.main(2).is_empty());
    }

    #[test]
    fn quit_before_data_loaded_does_not_overwrite() {
        let dir = TempDir::new("early-quit");
        let mut rig = Rig::new(&dir.0);
        play_and_quit(&mut rig, 1, "2535");
        rig.start_games();

        // esce prima che il disco risponda: nessun salvataggio (sarebbe un inventario vuoto sopra quello buono)
        rig.login(2, "Alice", "2535");
        rig.server.close(2);
        rig.pump(); // la risposta arriva per un giocatore che non c'è più: si scarta
                    // anche la chiusura del server prima della risposta non salva
        rig.login(3, "Alice", "2535");
        rig.server.shutdown();
        rig.pump();

        rig.login(4, "Alice", "2535");
        rig.pump();
        let starts = rig.start_games();
        assert!(near(starts.last().unwrap().0, [100.5, 71.62, -20.5]), "la posizione buona è ancora lì");
        assert_eq!(rig.server.players[&4].inventory.main(2).count, 10, "l'inventario buono è ancora lì");
    }

    #[test]
    fn server_shutdown_saves_connected_players() {
        let dir = TempDir::new("shutdown");
        let mut rig = Rig::new(&dir.0);
        rig.login(1, "Alice", "2535");
        rig.pump();
        rig.server.players.get_mut(&1).unwrap().inventory.set_main(7, item("minecraft:stone", 5));
        rig.server.shutdown();
        drop(rig);

        let mut rig = Rig::new(&dir.0);
        rig.login(2, "Alice", "2535");
        rig.pump();
        assert_eq!(rig.server.players[&2].inventory.main(7).count, 5);
    }

    #[test]
    fn players_without_xuid_are_told_apart_by_name() {
        let dir = TempDir::new("names");
        let mut rig = Rig::new(&dir.0);
        rig.server.players.insert(1, Player { info: Some(info("Alice", None)), ..Default::default() });
        rig.server.start_game(1);
        rig.pump();
        rig.server.players.get_mut(&1).unwrap().inventory.set_main(1, item("minecraft:stone", 3));
        rig.server.close(1);
        rig.server.players.insert(2, Player { info: Some(info("Bob", None)), ..Default::default() });
        rig.server.start_game(2);
        rig.pump();
        assert!(rig.server.players[&2].inventory.main(1).is_empty(), "Bob non eredita le cose di Alice");
        rig.server.players.insert(3, Player { info: Some(info("Alice", None)), ..Default::default() });
        rig.server.start_game(3);
        rig.pump();
        assert_eq!(rig.server.players[&3].inventory.main(1).count, 3);
    }

    /// Pacchetto di login con questo nome e, se c'è, questo XUID (le firme non si verificano).
    fn login_packet(name: &str, xuid: Option<&str>) -> Packet {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64U, Engine};
        let jwt = |payload: serde_json::Value| format!("{}.{}.x", B64U.encode(r#"{"alg":"ES384"}"#), B64U.encode(payload.to_string()));
        let mut extra = serde_json::json!({"displayName": name});
        if let Some(x) = xuid {
            extra["XUID"] = x.into();
        }
        let chain = serde_json::json!({"chain": [jwt(serde_json::json!({"extraData": extra}))]});
        let identity = serde_json::json!({"AuthenticationType": 0, "Certificate": chain.to_string(), "Token": ""}).to_string();
        let client_data = jwt(serde_json::json!({}));
        let login = bedrock_protocol::packets::Login { protocol: bedrock_protocol::PROTOCOL as i32, identity: &identity, client_data: &client_data };
        Packet::new(bedrock_protocol::id::LOGIN, login.encode())
    }

    impl Rig {
        /// Un client si connette e manda il login.
        fn connect(&mut self, pid: u64, name: &str, xuid: Option<&str>) {
            self.server.open(pid);
            self.server.message(pid, &Session::default().encode(&[login_packet(name, xuid)]));
        }

        /// (a chi, id del pacchetto) di tutto ciò che il server ha mandato, e a chi ha tolto la connessione.
        fn traffic(&mut self) -> (Vec<(u64, u32)>, Vec<u64>) {
            let (mut packets, mut kicks) = (Vec::new(), Vec::new());
            while let Ok(cmd) = self.sent.try_recv() {
                match cmd {
                    Command::Send(to, data) => packets.extend(Session::default().decode(&data).unwrap().into_iter().map(|p| (to, p.id))),
                    Command::Kick(id) => kicks.push(id),
                }
            }
            (packets, kicks)
        }
    }

    #[test]
    fn second_login_with_same_identity_kicks_the_first_and_keeps_its_data() {
        let dir = TempDir::new("duplicate");
        let mut rig = Rig::new(&dir.0);
        rig.connect(1, "Alice", Some("2535"));
        rig.server.start_game(1);
        rig.pump();
        let p = rig.server.players.get_mut(&1).unwrap();
        p.pos = [7.5, 80.62, 9.5];
        p.inventory.set_main(3, item("minecraft:stone", 33));
        rig.traffic();

        // la stessa persona rientra mentre la vecchia connessione è ancora aperta
        rig.connect(2, "Alice", Some("2535"));
        let (packets, kicks) = rig.traffic();
        assert!(packets.contains(&(1, bedrock_protocol::id::DISCONNECT)), "la vecchia sessione riceve un Disconnect");
        assert_eq!(kicks, vec![1], "e la sua connessione si chiude");
        assert!(!rig.server.players.contains_key(&1), "tolta dalla chiusura normale");

        // il nuovo ingresso legge ciò che la vecchia sessione ha appena salvato
        rig.server.start_game(2);
        rig.pump();
        assert_eq!(rig.server.players[&2].inventory.main(3).count, 33);
        let starts = rig.start_games();
        assert!(near(starts.last().unwrap().0, [7.5, 80.62, 9.5]));

        // il Close di rete della vecchia sessione, arrivato dopo, non salva di nuovo (né tocca la nuova)
        rig.server.players.get_mut(&2).unwrap().inventory.set_main(3, item("minecraft:stone", 40));
        rig.server.close(1);
        assert!(rig.server.players.contains_key(&2));
        rig.server.close(2);
        rig.connect(3, "Alice", Some("2535"));
        rig.server.start_game(3);
        rig.pump();
        assert_eq!(rig.server.players[&3].inventory.main(3).count, 40, "vale l'ultimo salvataggio, quello della sessione 2");
    }

    #[test]
    fn different_identities_do_not_kick_each_other() {
        let dir = TempDir::new("no-duplicate");
        let mut rig = Rig::new(&dir.0);
        rig.connect(1, "Alice", Some("2535"));
        rig.connect(2, "Bob", Some("2536"));
        rig.connect(3, "Alice", None); // senza XUID: identità `name:Alice`, diversa da quella con XUID
        rig.connect(4, "Carol", None);
        let (packets, kicks) = rig.traffic();
        assert!(kicks.is_empty());
        assert!(packets.iter().all(|&(_, id)| id != bedrock_protocol::id::DISCONNECT));
        assert_eq!(rig.server.players.len(), 4);
        // due senza XUID e con lo stesso nome sono la stessa identità
        rig.connect(5, "Carol", None);
        assert!(!rig.server.players.contains_key(&4) && rig.server.players.contains_key(&5));
    }
}
