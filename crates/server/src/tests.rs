//! Test della logica di gioco senza rete: i pacchetti inviati si leggono dal `Sender` di prova.
use crate::{registry, GameMode, Player, Server};
use bedrock_net::{Command, Sender};
use bedrock_protocol::{
    generated::{
        ContainerClose, ContainerOpen, CreativeContent, InventoryContent, MobEquipment, PlayerHotbar, SetPlayerGameType, StartGame, UpdateAbilities,
        UpdateAttributes,
    },
    id,
    item::ItemStack,
    packets::{text_type, CommandRequest, Login, RequestChunkRadius, Text},
    reader::Reader,
    wire::{ServerPacket, Writer},
    Packet, Session, PROTOCOL,
};
use tokio::sync::mpsc::UnboundedReceiver;

struct Harness {
    server: Server,
    sent: UnboundedReceiver<Command>,
}

impl Harness {
    fn new() -> Self {
        let (sender, sent) = Sender::for_tests();
        Self { server: Server::new(sender), sent }
    }

    /// Un client manda questi pacchetti (senza compressione: nei test non si negozia).
    fn receive(&mut self, pid: u64, packets: &[Packet]) {
        self.server.message(pid, &Session::default().encode(packets));
    }

    /// Pacchetti di testo arrivati a `pid` da quando si è letto l'ultima volta.
    fn texts_to(&mut self, pid: u64) -> Vec<Text> {
        let mut out = Vec::new();
        while let Ok(cmd) = self.sent.try_recv() {
            if let Command::Send(to, data) = cmd {
                let packets = Session::default().decode(&data).unwrap();
                out.extend(packets.into_iter().filter(|p| to == pid && p.id == id::TEXT).filter_map(|p| Text::decode(&p.body)));
            }
        }
        out
    }

    /// Tutti i pacchetti arrivati a `pid` da quando si è letto l'ultima volta.
    fn packets_to(&mut self, pid: u64) -> Vec<Packet> {
        let mut out = Vec::new();
        while let Ok(cmd) = self.sent.try_recv() {
            if let Command::Send(to, data) = cmd {
                let packets = Session::default().decode(&data).unwrap();
                out.extend(packets.into_iter().filter(|_| to == pid));
            }
        }
        out
    }

    /// Tutti i pacchetti inviati da quando si è letto l'ultima volta, per destinatario (`packets_to` scarta quelli degli altri).
    fn packets_by_player(&mut self) -> std::collections::HashMap<u64, Vec<Packet>> {
        let mut out: std::collections::HashMap<u64, Vec<Packet>> = Default::default();
        while let Ok(cmd) = self.sent.try_recv() {
            if let Command::Send(to, data) = cmd {
                out.entry(to).or_default().extend(Session::default().decode(&data).unwrap());
            }
        }
        out
    }

    /// Porta un giocatore fino a "nel mondo": login, raggio, inizializzato.
    fn join(&mut self, pid: u64, name: &str) {
        self.server.open(pid);
        self.receive(pid, &[login(name)]);
        self.receive(pid, &[radius()]);
        self.server.initialized(pid);
    }
}

fn jwt(payload: serde_json::Value) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64U, Engine};
    format!("{}.{}.x", B64U.encode(r#"{"alg":"ES384"}"#), B64U.encode(payload.to_string()))
}

fn login(name: &str) -> Packet {
    let chain = serde_json::json!({"chain": [jwt(serde_json::json!({"extraData": {"displayName": name}}))]});
    let identity = serde_json::json!({"AuthenticationType": 0, "Certificate": chain.to_string(), "Token": ""}).to_string();
    let client_data = jwt(serde_json::json!({}));
    Packet::new(id::LOGIN, Login { protocol: PROTOCOL as i32, identity: &identity, client_data: &client_data }.encode())
}

fn radius() -> Packet {
    Packet::new(id::REQUEST_CHUNK_RADIUS, RequestChunkRadius { chunk_radius: 2, max_radius: 2 }.encode())
}

fn chat(message: &str) -> Packet {
    Packet::new(id::TEXT, Text::chat("finto", message).encode())
}

fn command(line: &str) -> Packet {
    Packet::new(id::COMMAND_REQUEST, CommandRequest { command: line.into(), ..Default::default() }.encode())
}

#[test]
fn chat_and_commands_before_joining_are_ignored() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.texts_to(1);
    h.server.open(2); // connesso ma senza login
    h.receive(2, &[chat("ciao a tutti")]);
    h.receive(2, &[command("/say spam")]);
    assert!(h.texts_to(1).is_empty(), "chi non è nel mondo non deve poter scrivere agli altri");
}

#[test]
fn chunk_radius_before_login_is_ignored() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.texts_to(1);
    h.server.open(2);
    h.receive(2, &[radius()]);
    h.server.initialized(2);
    assert!(h.texts_to(1).is_empty(), "senza login non si entra nel mondo (né si annuncia l'ingresso)");
}

#[test]
fn join_is_announced_once_even_if_the_client_repeats_initialized() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.texts_to(1);
    h.join(2, "Bob");
    h.server.initialized(2);
    let joins = h.texts_to(1).into_iter().filter(|t| t.message.contains("Bob")).count();
    assert_eq!(joins, 1);
}

#[test]
fn a_second_login_on_the_same_connection_is_ignored() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.receive(1, &[login("Mallory")]);
    assert_eq!(h.server.name(1), "Alice");
}

#[test]
fn overlong_chat_messages_are_rejected() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.join(2, "Bob");
    h.texts_to(1);
    h.receive(2, &[chat(&"x".repeat(crate::chat::MAX_CHAT_LEN + 1))]);
    assert!(h.texts_to(1).iter().all(|t| t.kind != text_type::CHAT), "un messaggio troppo lungo non va ritrasmesso");
    h.receive(2, &[chat("ok")]);
    assert_eq!(h.texts_to(1).iter().filter(|t| t.kind == text_type::CHAT && t.message == "ok").count(), 1);
}

#[test]
fn flush_on_shutdown_writes_dirty_chunks() {
    use crate::{storage::World, world::Chunk};
    use vgen::Dim;

    let dir = std::env::temp_dir().join(format!("versmc-test-shutdown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (sender, _sent) = Sender::for_tests();
    let (events, _events) = tokio::sync::mpsc::unbounded_channel();
    let mut server = Server::with_world(sender, World::open_with(&dir, "Prova", Some(7)).unwrap(), events);
    let mut dirty = Chunk::flat(Dim::Overworld, 2, 3);
    dirty.dirty = true;
    let clean = Chunk::flat(Dim::Overworld, 4, 4); // non modificato: non va riscritto
    let expected = dirty.network_bytes();
    server.chunks.insert((0, 2, 3), dirty);
    server.chunks.insert((0, 4, 4), clean);
    server.shutdown();
    assert!(!server.chunks[&(0, 2, 3)].dirty, "salvato e ripulito");
    drop(server);

    let mut world = World::open(&dir).unwrap();
    assert_eq!(world.load(Dim::Overworld, 2, 3).expect("chunk modificato salvato").network_bytes(), expected);
    assert!(world.load(Dim::Overworld, 4, 4).is_none(), "il chunk pulito non viene scritto");
    drop(world);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Corpo valido di `PlayerAuthInput` senza flag né opzionali, con la sola posizione (occhi) data.
fn auth_input_body(x: f32, y: f32, z: f32) -> Vec<u8> {
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let mut b = floats(&[0.0, 0.0, x, y, z, 0.0, 0.0, 0.0]); // rotazione, posizione, movimento, head yaw
    b.extend([0, 0, 0, 0]); // nessun flag, modalità di input e di gioco, modello di interazione
    b.extend(floats(&[0.0, 0.0])); // rotazione di interazione
    b.push(0); // tick
    b.extend(floats(&[0.0; 3])); // delta
    b.extend([0, 0, 0, 0, 0]); // transazione, richiesta, azioni di blocco, veicolo (rotazione e id) assenti
    b.extend(floats(&[0.0; 7])); // vettore analogico, camera, vettore grezzo
    b
}

#[test]
fn malformed_pai_still_moves_player_and_logs_once() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    // Corpo valido solo fino alla posizione e ai primi campi: il decoder completo lo rifiuta.
    let truncated = |x: f32| Packet::new(id::PLAYER_AUTH_INPUT, auth_input_body(x, 70.0, -3.0)[..30].to_vec());
    assert!(!h.server.players[&1].pai_logged);
    h.receive(1, &[truncated(40.5)]);
    assert_eq!((h.server.players[&1].pos, h.server.players[&1].last_chunk), ([40.5, 70.0, -3.0], (2, -1)));
    assert!(h.server.players[&1].pai_logged, "primo errore registrato");
    h.receive(1, &[truncated(100.5)]);
    assert_eq!(h.server.players[&1].pos[0], 100.5, "il movimento continua anche dopo il primo errore");
    // Meno di 20 byte: nemmeno la posizione si legge, nessun effetto.
    h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, auth_input_body(1.0, 70.0, 0.0)[..19].to_vec())]);
    assert_eq!(h.server.players[&1].pos[0], 100.5);
}

#[test]
fn player_auth_input_moves_the_player_and_ignores_bad_positions() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    let input = |x: f32, y: f32, z: f32| Packet::new(id::PLAYER_AUTH_INPUT, auth_input_body(x, y, z));
    h.receive(1, &[input(40.5, 70.0, -3.0)]);
    assert_eq!((h.server.players[&1].pos, h.server.players[&1].last_chunk), ([40.5, 70.0, -3.0], (2, -1)));
    h.receive(1, &[input(f32::NAN, 70.0, 0.0), input(1.0e9, 70.0, 0.0)]);
    assert_eq!(h.server.players[&1].last_chunk, (2, -1), "posizioni non valide scartate");
    h.server.open(2); // non ancora nel mondo: l'input non conta
    h.receive(2, &[input(100.0, 70.0, 0.0)]);
    assert_eq!(h.server.players[&2].pos, [0.0; 3]);
}

// ---------- inventario, menu creativo e modalità ----------

/// Legge un `NetworkItemInstanceDescriptor` di `CreativeContent`: (id, lunghezza dei dati utente).
fn creative_descriptor(r: &mut Reader) -> (i32, usize) {
    let id = r.var_i32().unwrap();
    r.u16().unwrap();
    r.var_u32().unwrap();
    r.var_i32().unwrap();
    (id, r.bytes().unwrap().len())
}

#[test]
fn join_sends_full_creative_menu() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    let packets = h.packets_to(1);
    let creative = packets.iter().find(|p| p.id == CreativeContent::ID).expect("CreativeContent mandato al login");
    let r = &mut Reader(&creative.body);
    let (groups, items) = registry::creative();
    assert_eq!(r.var_u32().unwrap() as usize, groups.len());
    for g in groups {
        assert_eq!((r.u8().unwrap(), r.string().unwrap()), (g.category, g.name.as_str()));
        let (icon, user_data) = creative_descriptor(r);
        assert!(icon != 0 && user_data >= 10, "l'icona di un gruppo non è aria e ha i dati utente");
    }
    assert_eq!(r.var_u32().unwrap() as usize, items.len(), "tante voci quante quelle del registro");
    for (index, item) in items.iter().enumerate() {
        assert_eq!(r.var_u32().unwrap(), index as u32 + 1, "net id = indice + 1");
        let (id, user_data) = creative_descriptor(r);
        assert_eq!(id, registry::items().id(&item.name).unwrap() as i32);
        assert!(user_data >= 10, "ogni oggetto ha i dati utente");
        assert_eq!(r.var_u32().unwrap(), item.group);
    }
    assert_eq!(r.remaining(), 0);
}

#[test]
fn join_sends_inventory_game_type_and_abilities() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    let packets = h.packets_to(1);
    let of = |id: u32| packets.iter().filter(|p| p.id == id).collect::<Vec<_>>();
    assert_eq!(of(SetPlayerGameType::ID)[0].body, [2], "creativo = 1, ZigZag 2");
    // UpdateAbilities: id giocatore, permessi, un solo livello (base) con tutte le abilità decise e quelle del creativo accese
    let r = &mut Reader(&of(UpdateAbilities::ID)[0].body);
    assert_eq!((r.i64().unwrap(), r.i8().unwrap(), r.u8().unwrap(), r.var_u32().unwrap(), r.u16().unwrap()), (1, 1, 0, 1, 1));
    let (set, values) = (r.u32().unwrap(), r.u32().unwrap());
    assert_eq!(set, 0xF_FFFF);
    let creative = (1 << 10) | (1 << 8) | (1 << 11) | 0b11 | (0b11 << 2) | (0b11 << 4); // vola, invulnerabile, costruisce subito, build/mine, interazioni
    assert_eq!(values, creative);
    // inventario (36 caselle), mano secondaria (1), armatura (4), tutte vuote; poi la barra
    let contents: Vec<_> = of(InventoryContent::ID).iter().map(|p| Reader(&p.body).var_u32().unwrap()).collect();
    assert_eq!(contents, [0, 119, 120]);
    let main = &of(InventoryContent::ID)[0].body;
    assert_eq!(main[1], 36);
    assert!(main[2..2 + 36 * 8].iter().all(|&b| b == 0), "caselle vuote = aria");
    assert_eq!(of(PlayerHotbar::ID)[0].body, [0, 0, 1], "casella 0, finestra 0, selezionala");
}

#[test]
fn start_game_uses_the_player_mode() {
    let mut h = Harness::new();
    h.server.open(1);
    h.receive(1, &[login("Alice")]);
    h.server.players.get_mut(&1).unwrap().mode = GameMode::Survival;
    h.packets_to(1);
    h.server.start_game(1);
    let packets = h.packets_to(1);
    let start = packets.iter().find(|p| p.id == StartGame::ID).unwrap();
    let r = &mut Reader(&start.body);
    r.var_i64().unwrap();
    r.var_u64().unwrap();
    assert_eq!(r.var_i32().unwrap(), 0, "sopravvivenza");
    assert_eq!(GameMode::default(), GameMode::Creative);
}

fn slot_bytes(w: &mut Writer, container: u8, slot: u8, stack_id: i32) {
    w.u8(container);
    w.u8(0); // niente dynamic id
    w.u8(slot);
    w.i32(stack_id);
}

/// Corpo di una richiesta: CraftCreative + Place dalla casella creata alla casella `dst` della barra.
fn craft_and_place(w: &mut Writer, request_id: i32, net_id: u32, dst: u8) {
    craft_and_place_in(w, request_id, net_id, 28, dst);
}

/// Come `craft_and_place`, ma verso la casella `dst` del contenitore `container` (28 = barra, 6 = armatura).
fn craft_and_place_in(w: &mut Writer, request_id: i32, net_id: u32, container: u8, dst: u8) {
    craft_and_place_count(w, request_id, net_id, container, dst, 64);
}

/// Come `craft_and_place_in`, ma sposta `count` oggetti (una pila di spade o di elmi ne ha uno solo).
fn craft_and_place_count(w: &mut Writer, request_id: i32, net_id: u32, container: u8, dst: u8, count: u8) {
    w.var_i32(request_id);
    w.var_u32(2);
    w.var_u32(12); // variante CraftCreative
    w.u8(14);
    w.var_u32(net_id);
    w.u8(1);
    w.var_u32(1); // variante Place
    w.u8(1);
    w.u8(count);
    slot_bytes(w, 60, 50, request_id);
    slot_bytes(w, container, dst, 0);
    w.var_u32(0); // niente stringhe da filtrare
    w.i32(-1);
}

/// Una richiesta che non può riuscire: prende 5 oggetti da una casella vuota.
fn take_from_empty(w: &mut Writer, request_id: i32) {
    w.var_i32(request_id);
    w.var_u32(1);
    w.var_u32(0);
    w.u8(0);
    w.u8(5);
    slot_bytes(w, 28, 0, 0);
    slot_bytes(w, 28, 1, 0);
    w.var_u32(0);
    w.i32(-1);
}

fn stone_net_id() -> u32 {
    registry::creative().1.iter().position(|c| c.name == "minecraft:stone" && c.meta == 0).unwrap() as u32 + 1
}

fn stack_request_packet(build: impl FnOnce(&mut Writer)) -> Packet {
    let mut w = Writer::default();
    w.var_u32(1);
    build(&mut w);
    Packet::new(id::ITEM_STACK_REQUEST, w.0)
}

#[test]
fn item_stack_request_gives_a_creative_item() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.packets_to(1);
    h.receive(1, &[stack_request_packet(|w| craft_and_place(w, -1, stone_net_id(), 2))]);
    let packets = h.packets_to(1);
    assert_eq!(packets.iter().map(|p| p.id).collect::<Vec<_>>(), [id::ITEM_STACK_RESPONSE]);
    assert_eq!(&packets[0].body[..4], [1, 0, 1, 1], "una risposta, riuscita, richiesta -1, contenitori presenti");
    let stack = h.server.players[&1].inventory.stack_at(28, 2).unwrap();
    assert_eq!((stack.id, stack.count), (registry::items().id("minecraft:stone").unwrap(), 64));
    assert!(stack.net_id.is_some_and(|n| n > 0));
    assert_eq!(h.server.players[&1].inventory.held().id, 0, "la barra è su una casella vuota");
}

#[test]
fn rejected_item_stack_request_resends_the_slots() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.packets_to(1);
    h.receive(1, &[stack_request_packet(|w| take_from_empty(w, -1))]);
    let packets = h.packets_to(1);
    assert_eq!(packets.iter().map(|p| p.id).collect::<Vec<_>>(), [id::ITEM_STACK_RESPONSE, id::INVENTORY_SLOT, id::INVENTORY_SLOT]);
    assert_eq!(packets[0].body, [1, 1, 1, 0], "errore");
    // Un corpo troncato non risponde e non fa nulla.
    let mut w = Writer::default();
    w.var_u32(1);
    take_from_empty(&mut w, -3);
    h.receive(1, &[Packet::new(id::ITEM_STACK_REQUEST, w.0[..w.0.len() - 3].to_vec())]);
    assert!(h.packets_to(1).is_empty());
}

#[test]
fn survival_player_cannot_use_the_creative_menu() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.server.players.get_mut(&1).unwrap().mode = GameMode::Survival;
    h.packets_to(1);
    h.receive(1, &[stack_request_packet(|w| craft_and_place(w, -1, stone_net_id(), 0))]);
    assert_eq!(h.packets_to(1)[0].body, [1, 1, 1, 0]);
    assert_eq!(h.server.players[&1].inventory.stack_at(28, 0), Some(ItemStack::air()));
}

#[test]
fn item_stack_requests_before_spawning_are_ignored() {
    let mut h = Harness::new();
    h.server.open(1);
    h.receive(1, &[login("Alice"), radius()]); // dentro il mondo ma senza SetLocalPlayerAsInitialized
    h.packets_to(1);
    h.receive(1, &[stack_request_packet(|w| craft_and_place(w, -1, stone_net_id(), 0))]);
    assert!(h.packets_to(1).is_empty());
    assert_eq!(h.server.players[&1].inventory.stack_at(28, 0), Some(ItemStack::air()));
}

/// Presa reale dal menu creativo (client 1.26.6x): CraftCreative della voce 4, CraftResults, Take di 64 sul cursore.
#[test]
fn real_creative_take_puts_the_item_on_the_cursor() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.packets_to(1);
    let body: [u8; 78] = [
        0x01, 0x05, 0x03, 0x0c, 0x0e, 0x04, 0x01, 0x12, 0x14, 0x01, 0x01, 0x01, 0x17, 0x6d, 0x69, 0x6e, 0x65, 0x63, 0x72, 0x61, 0x66, 0x74, 0x3a, 0x6a, 0x75,
        0x6e, 0x67, 0x6c, 0x65, 0x5f, 0x70, 0x6c, 0x61, 0x6e, 0x6b, 0x73, 0x00, 0x01, 0x00, 0x97, 0xa5, 0x81, 0x93, 0x04, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x40, 0x3c, 0x00, 0x32, 0xfd, 0xff, 0xff, 0xff, 0x3b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
        0xff, 0xff, 0xff,
    ];
    h.receive(1, &[Packet::new(id::ITEM_STACK_REQUEST, body.to_vec())]);
    let cursor = h.server.players[&1].inventory.stack_at(59, 0).unwrap();
    assert_eq!(cursor.count, 64, "l'oggetto preso dal menu creativo è sul cursore");
    let responses: Vec<Packet> = h.packets_to(1).into_iter().filter(|p| p.id == id::ITEM_STACK_RESPONSE).collect();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].body[..3], [1, 0, 5], "una risposta, OK, alla richiesta -3");
}

/// `PlayerAuthInput` con il flag 36 e una richiesta di pila (CraftCreative + Place), posizione (x, 70, 0).
fn pai_with_request(x: f32) -> Vec<u8> {
    let mut w = Writer::default();
    for v in [0.0, 0.0, x, 70.0, 0.0, 0.0, 0.0, 0.0] {
        w.f32(v);
    }
    w.var_u32(1);
    w.var_i32(36);
    w.var_u32(1); // modalità di input
    w.var_u32(0);
    w.var_i32(2);
    w.f32(0.0);
    w.f32(0.0);
    w.var_u64(10);
    (0..3).for_each(|_| w.f32(0.0));
    w.u8(0); // niente transazione
    w.u8(1);
    craft_and_place(&mut w, -1, stone_net_id(), 5);
    w.u8(0); // niente azioni di blocco
    w.u8(0);
    w.u8(0);
    (0..7).for_each(|_| w.f32(0.0));
    w.0
}

#[test]
fn player_auth_input_carries_an_item_stack_request() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.packets_to(1);
    h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_request(40.5))]);
    assert_eq!(h.server.players[&1].pos[0], 40.5, "il movimento passa anche con la richiesta");
    let stack = h.server.players[&1].inventory.stack_at(28, 5).unwrap();
    assert_eq!((stack.id, stack.count), (registry::items().id("minecraft:stone").unwrap(), 64));
    assert!(h.packets_to(1).iter().any(|p| p.id == id::ITEM_STACK_RESPONSE));
}

#[test]
fn only_the_first_item_stack_requests_are_logged_in_hex() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    for n in 0..5 {
        h.receive(1, &[stack_request_packet(|w| take_from_empty(w, -(2 * n + 1)))]);
        h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_request(1.0))]);
    }
    assert_eq!((h.server.players[&1].stack_logs, h.server.players[&1].action_logs), (3, 3));
}

fn mob_equipment(slot: u8, selected: u8) -> Packet {
    MobEquipment { target_runtime_id: 1, item: ItemStack::air(), slot, selected_slot: selected, container_id: 0 }.packet()
}

#[test]
fn mob_equipment_selects_a_hotbar_slot() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.receive(1, &[mob_equipment(3, 3)]);
    assert_eq!(h.server.players[&1].inventory.selected, 3);
    h.receive(1, &[mob_equipment(9, 9)]);
    h.receive(1, &[mob_equipment(200, 200)]);
    assert_eq!(h.server.players[&1].inventory.selected, 3, "fuori dalla barra: ignorato");
    h.receive(1, &[Packet::new(id::MOB_EQUIPMENT, vec![1, 2])]);
    assert_eq!(h.server.players[&1].inventory.selected, 3, "corpo malformato: ignorato");
}

#[test]
fn the_player_gets_speed_health_and_hunger_on_spawn() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    let attributes: Vec<Packet> = h.packets_to(1).into_iter().filter(|p| p.id == UpdateAttributes::ID).collect();
    assert_eq!(attributes.len(), 1, "un UpdateAttributes allo spawn");
    let expected = crate::join::spawn_attributes(1, &Player::default()).packet();
    assert_eq!(attributes[0].body, expected.body);
    let attributes = crate::join::spawn_attributes(1, &Player::default()).attribute_list;
    for name in ["minecraft:movement", "minecraft:health", "minecraft:player.hunger", "minecraft:player.saturation", "minecraft:player.exhaustion"] {
        assert!(attributes.iter().any(|a| a.name == name), "manca {name}");
    }
    let movement = attributes.iter().find(|a| a.name == "minecraft:movement").unwrap();
    assert_eq!((movement.current_value, movement.default_value), (0.1, 0.1), "velocità di camminata vanilla");
}

#[test]
fn the_player_gets_its_actor_data_with_gravity_on_spawn() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    let data: Vec<Packet> = h.packets_to(1).into_iter().filter(|p| p.id == bedrock_protocol::actor::SET_ACTOR_DATA).collect();
    assert_eq!(data.len(), 1, "senza i dati dell'entità il client non applica la gravità al giocatore");
    assert_eq!(data[0].body, bedrock_protocol::actor::set_actor_data(1, &h.server.entity_data(1)).body);
    let flags = h.server.entity_data(1).into_iter().find_map(|(k, v)| match v {
        bedrock_protocol::actor::DataValue::Int64(f) if k == bedrock_protocol::actor::key::FLAGS => Some(f),
        _ => None,
    });
    let gravity = 1i64 << bedrock_protocol::actor::flag::HAS_GRAVITY;
    assert_eq!(flags.unwrap() & gravity, gravity, "gravità accesa");
}

/// `PlayerAuthInput` minimo con un solo flag di input, posizione (0, 70, 0).
fn pai_with_flag(input_flag: i32) -> Vec<u8> {
    let mut w = Writer::default();
    for v in [0.0, 0.0, 0.0, 70.0, 0.0, 0.0, 0.0, 0.0] {
        w.f32(v);
    }
    w.var_u32(1);
    w.var_i32(input_flag);
    w.var_u32(1); // modalità di input
    w.var_u32(0);
    w.var_i32(2);
    w.f32(0.0);
    w.f32(0.0);
    w.var_u64(10);
    (0..3).for_each(|_| w.f32(0.0));
    w.u8(0); // niente transazione
    w.u8(0); // niente richiesta di pila
    w.u8(0); // niente azioni di blocco
    w.u8(0);
    w.u8(0);
    (0..7).for_each(|_| w.f32(0.0));
    w.0
}

/// Il corpo di `UpdateAbilities` che il server manda a `pid` per la modalità e lo stato di volo.
fn abilities_body(pid: u64, mode: GameMode, flying: bool) -> Vec<u8> {
    let data = bedrock_protocol::generated::SerializedAbilitiesData {
        target_player_raw_id: pid as i64,
        player_permissions: bedrock_protocol::generated::enums::player_permission_level::MEMBER,
        command_permissions: 0,
        layers: crate::join::ability_layers(mode, flying),
    };
    UpdateAbilities { data }.packet().body
}

#[test]
fn start_and_stop_flying_are_answered_with_update_abilities() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.packets_to(1);
    h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(42))]);
    let abilities: Vec<Packet> = h.packets_to(1).into_iter().filter(|p| p.id == UpdateAbilities::ID).collect();
    assert_eq!(abilities.len(), 1, "StartFlying va accettato o rifiutato con UpdateAbilities");
    assert_eq!(abilities[0].body, abilities_body(1, GameMode::Creative, true), "in creativa si vola");
    h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(43))]);
    let abilities: Vec<Packet> = h.packets_to(1).into_iter().filter(|p| p.id == UpdateAbilities::ID).collect();
    assert_eq!(abilities.len(), 1, "StopFlying va confermato, altrimenti il client resta sospeso");
    assert_eq!(abilities[0].body, abilities_body(1, GameMode::Creative, false));
}

#[test]
fn start_flying_in_survival_is_rejected() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.server.set_mode(1, GameMode::Survival);
    h.packets_to(1);
    h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(42))]);
    let abilities: Vec<Packet> = h.packets_to(1).into_iter().filter(|p| p.id == UpdateAbilities::ID).collect();
    assert_eq!(abilities.len(), 1);
    assert_eq!(abilities[0].body, abilities_body(1, GameMode::Survival, false), "in sopravvivenza il volo si rifiuta");
}

#[test]
fn item_registry_keeps_vanilla_versions_and_components() {
    let entries = crate::join::item_entries();
    let get = |name: &str| entries.iter().find(|e| e.item_name == name).unwrap_or_else(|| panic!("manca {name}"));
    for block in ["minecraft:warped_planks", "minecraft:oak_planks", "minecraft:stone"] {
        assert_eq!(get(block).item_version, 2, "{block}: i blocchi hanno versione None, altrimenti il client non trova il blocco");
    }
    let apple = get("minecraft:apple");
    assert!(apple.is_component_based && apple.item_version == 1, "la mela è un oggetto data-driven");
    assert!(apple.item_component_data.len() > 100 && apple.item_component_data[..2] == [0x0a, 0], "componenti come radice NBT di rete");
    assert_eq!(get("minecraft:warped_planks").item_id, -243, "gli ID restano quelli di items.txt");
}

/// `Interact` del client: azione e runtime id del bersaglio (la posizione opzionale manca).
fn interact(action: u8, target: u32) -> Packet {
    let mut body = vec![action];
    bedrock_protocol::varint::write(&mut body, target);
    body.push(0); // posizione assente
    Packet::new(id::INTERACT, body)
}

#[test]
fn open_inventory_answers_with_container_open_once_until_closed() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.packets_to(1);
    h.receive(1, &[interact(6, 1)]);
    let opens: Vec<Packet> = h.packets_to(1).into_iter().filter(|p| p.id == ContainerOpen::ID).collect();
    assert_eq!(opens.len(), 1, "l'inventario si apre con un ContainerOpen");
    assert_eq!(opens[0].body[..2], [0, 0xff], "finestra 0, tipo inventario (0xff)");
    assert_eq!(opens[0].body[opens[0].body.len() - 1], 1, "bersaglio -1 (zigzag): come Dragonfly");
    h.receive(1, &[interact(6, 1)]);
    assert!(h.packets_to(1).iter().all(|p| p.id != ContainerOpen::ID), "un secondo OpenInventory senza chiusura farebbe crashare il client");
    h.receive(1, &[ContainerClose { container_id: 0, container_type: 0xff, server_initiated_close: false }.packet()]);
    let closes: Vec<Packet> = h.packets_to(1).into_iter().filter(|p| p.id == ContainerClose::ID).collect();
    assert_eq!(closes.len(), 1, "la chiusura va confermata, altrimenti il client non lo riapre");
    assert_eq!(closes[0].body, vec![0, 0, 0], "finestra 0, chiusa dal client");
    h.receive(1, &[interact(6, 1)]);
    assert_eq!(h.packets_to(1).iter().filter(|p| p.id == ContainerOpen::ID).count(), 1, "dopo la chiusura si riapre");
}

// ---------- modifica dei blocchi ----------

mod blocks {
    use super::*;
    use crate::{
        events::{BlockChange, WorldAction},
        world::{block_hash, Chunk, State},
    };
    use bedrock_protocol::generated::UpdateBlock;
    use vgen::Dim;

    pub(super) fn stone() -> i32 {
        block_hash("minecraft:stone", &[])
    }

    pub(super) fn air() -> i32 {
        block_hash("minecraft:air", &[])
    }

    pub(super) fn water() -> i32 {
        block_hash("minecraft:water", &[("liquid_depth", State::Int(0))])
    }

    pub(super) fn grass() -> i32 {
        block_hash("minecraft:grass_block", &[])
    }

    /// Tutti i pacchetti `UpdateBlock` inviati da quando si è letto l'ultima volta: (destinatario, posizione, hash, flag, strato).
    pub(super) fn updates(sent: &mut UnboundedReceiver<Command>) -> Vec<(u64, [i32; 3], u32, u32, u32)> {
        let mut out = Vec::new();
        while let Ok(cmd) = sent.try_recv() {
            if let Command::Send(to, data) = cmd {
                for p in Session::default().decode(&data).unwrap().into_iter().filter(|p| p.id == UpdateBlock::ID) {
                    let mut r = Reader(&p.body);
                    let pos = r.block_pos().unwrap();
                    out.push((to, pos, r.var_u32().unwrap(), r.var_u32().unwrap(), r.var_u32().unwrap()));
                }
            }
        }
        out
    }

    pub(super) fn harness_with_chunk() -> Harness {
        let mut h = Harness::new();
        h.server.chunks.insert((0, 0, 0), Chunk::flat(Dim::Overworld, 0, 0));
        h
    }

    pub(super) fn change(pos: [i32; 3], hash: i32, water: bool) -> BlockChange {
        BlockChange { pos, hash, water }
    }

    #[test]
    fn set_blocks_sends_update_only_to_viewers() {
        let mut h = harness_with_chunk();
        h.join(1, "Alice");
        h.join(2, "Bob");
        h.server.players.get_mut(&1).unwrap().sent.insert((0, 0));
        h.packets_to(1);
        h.server.set_blocks(Dim::Overworld, &[change([3, 10, 4], stone(), false), change([1, 11, 2], stone(), true)]);
        let got = updates(&mut h.sent);
        assert_eq!(
            got,
            vec![
                (1, [3, 10, 4], stone() as u32, 3, 0),
                (1, [3, 10, 4], air() as u32, 3, 1),
                (1, [1, 11, 2], stone() as u32, 3, 0),
                (1, [1, 11, 2], water() as u32, 3, 1),
            ],
            "solo Alice ha il chunk in `sent`"
        );
        assert_eq!(h.server.block_at(Dim::Overworld, [3, 10, 4]), Some((stone(), false)));
        assert_eq!(h.server.block_at(Dim::Overworld, [1, 11, 2]), Some((stone(), true)));
    }

    #[test]
    fn block_at_out_of_world_or_unloaded_is_none() {
        let h = harness_with_chunk();
        let bedrock = block_hash("minecraft:bedrock", &[("infiniburn_bit", State::Byte(0))]);
        assert_eq!(h.server.block_at(Dim::Overworld, [0, -64, 0]), Some((bedrock, false)));
        assert_eq!(h.server.block_at(Dim::Overworld, [0, -65, 0]), None, "sotto il mondo");
        assert_eq!(h.server.block_at(Dim::Overworld, [0, 320, 0]), None, "sopra il mondo");
        assert_eq!(h.server.block_at(Dim::Overworld, [0, 319, 0]), Some((air(), false)));
        assert_eq!(h.server.block_at(Dim::Overworld, [16, 0, 0]), None, "chunk non in memoria");
        assert_eq!(h.server.block_at(Dim::Overworld, [-1, 0, 0]), None, "chunk (-1, 0) non in memoria");
        assert_eq!(h.server.block_at(Dim::Nether, [0, 0, 0]), None, "altra dimensione");
        assert_eq!(h.server.block_at(Dim::Overworld, [i32::MIN, 0, i32::MAX]), None, "coordinate estreme senza overflow");
    }

    #[test]
    fn set_blocks_outside_loaded_chunks_or_world_changes_nothing() {
        let mut h = harness_with_chunk();
        h.join(1, "Alice");
        h.server.players.get_mut(&1).unwrap().sent.insert((0, 0));
        h.packets_to(1);
        h.server.set_blocks(Dim::Overworld, &[change([100, 10, 4], stone(), false), change([0, 400, 0], stone(), false)]);
        assert!(updates(&mut h.sent).is_empty());
        assert!(!h.server.chunks[&(0, 0, 0)].dirty);
    }

    #[test]
    fn negative_coordinates_map_to_the_right_chunk_cell() {
        let mut h = harness_with_chunk();
        h.server.chunks.insert((0, -1, -1), Chunk::flat(Dim::Overworld, -1, -1));
        h.server.set_blocks(Dim::Overworld, &[change([-1, 5, -16], stone(), false)]);
        assert_eq!(h.server.chunks[&(0, -1, -1)].block(15, 5, 0), stone());
        assert_eq!(h.server.block_at(Dim::Overworld, [-1, 5, -16]), Some((stone(), false)));
    }

    #[test]
    fn resend_blocks_sends_the_true_state_to_one_player() {
        let mut h = harness_with_chunk();
        h.join(1, "Alice");
        h.join(2, "Bob");
        h.packets_to(1);
        h.server.resend_blocks(2, Dim::Overworld, &[[0, -61, 0], [100, 0, 0]]);
        let got = updates(&mut h.sent);
        assert_eq!(got, vec![(2, [0, -61, 0], grass() as u32, 3, 0), (2, [0, -61, 0], air() as u32, 3, 1)], "niente per il chunk non caricato");
    }

    #[test]
    fn cancelled_action_changes_nothing() {
        let mut h = harness_with_chunk();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        h.server.hooks.push(Box::new(move |a| {
            log.lock().unwrap().push(a.clone());
            !matches!(a, WorldAction::Break { .. })
        }));
        let break_it = WorldAction::Break { pid: 1, pos: [0, -61, 0], old: grass() };
        let place = WorldAction::Place { pid: 1, changes: vec![change([0, -60, 0], stone(), false)] };
        assert!(!h.server.allow(&break_it), "il gancio annulla la rottura");
        assert!(h.server.allow(&place));
        assert_eq!(seen.lock().unwrap().len(), 2);
        // chi chiama non applica nulla se `allow` dice no: il mondo non cambia
        if h.server.allow(&break_it) {
            h.server.set_blocks(Dim::Overworld, &[change([0, -61, 0], air(), false)]);
        }
        assert_eq!(h.server.block_at(Dim::Overworld, [0, -61, 0]), Some((grass(), false)));
        assert!(!h.server.chunks[&(0, 0, 0)].dirty);
        h.server.hooks.clear();
        assert!(h.server.allow(&break_it), "senza ganci passa tutto");
    }

    #[test]
    fn one_cancelling_hook_among_many_cancels() {
        let mut h = Harness::new();
        h.server.hooks.push(Box::new(|_| true));
        h.server.hooks.push(Box::new(|_| false));
        assert!(!h.server.allow(&WorldAction::Place { pid: 1, changes: vec![] }));
    }

    #[test]
    fn autosave_writes_dirty_chunks_every_1200_ticks() {
        use crate::storage::World;
        let dir = std::env::temp_dir().join(format!("versmc-test-autosave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (sender, _sent) = Sender::for_tests();
        let (events, _events) = tokio::sync::mpsc::unbounded_channel();
        let mut server = Server::with_world(sender, World::open_with(&dir, "Prova", Some(7)).unwrap(), events);
        server.flat = false; // chunk "veri": si salvano
                             // un giocatore lì vicino, altrimenti `stream_tick` scarica il chunk al primo tick
        server.players.insert(1, Player { spawned: true, radius: 2, last_chunk: (2, 3), ..Player::default() });
        let mut chunk = Chunk::flat(Dim::Overworld, 2, 3);
        chunk.fallback = false;
        server.chunks.insert((0, 2, 3), chunk);
        server.set_blocks(Dim::Overworld, &[change([32, 0, 48], stone(), false)]);
        assert!(server.chunks[&(0, 2, 3)].dirty);
        for _ in 0..1199 {
            server.tick();
        }
        assert!(server.chunks[&(0, 2, 3)].dirty, "prima del 1200° tick non si salva");
        server.tick();
        assert!(!server.chunks[&(0, 2, 3)].dirty, "salvato e pulito");
        server.disk.as_ref().unwrap().flush_blocking();
        drop(server);
        let mut world = World::open(&dir).unwrap();
        assert_eq!(world.load(Dim::Overworld, 2, 3).expect("salvato").block(0, 0, 0), stone());
        drop(world);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fallback_chunks_are_never_saved_even_when_the_server_is_not_flat() {
        // generazione fallita: il chunk di ripiego sta in memoria mentre il server ha il generatore (`flat` è falso)
        let mut h = harness_with_chunk();
        h.server.flat = false;
        assert!(h.server.chunks[&(0, 0, 0)].fallback && !h.server.flat_warned);
        h.server.set_blocks(Dim::Overworld, &[change([0, 10, 0], stone(), false)]);
        assert_eq!(h.server.block_at(Dim::Overworld, [0, 10, 0]), Some((stone(), false)), "la modifica c'è in memoria");
        assert!(!h.server.chunks[&(0, 0, 0)].dirty, "ma non si salva");
        assert!(h.server.flat_warned, "avviso nel log dato una volta");
        h.server.put_block_entity(Dim::Overworld, vgen::BlockEntity { x: 1, y: 10, z: 1, nbt: Vec::new() });
        assert!(!h.server.chunks[&(0, 0, 0)].dirty, "nemmeno con un block entity");
    }

    #[test]
    fn fallback_flat_chunks_are_never_marked_for_saving() {
        let mut h = harness_with_chunk();
        assert!(h.server.flat && !h.server.flat_warned);
        h.server.set_blocks(Dim::Overworld, &[change([0, 10, 0], stone(), false)]);
        assert_eq!(h.server.block_at(Dim::Overworld, [0, 10, 0]), Some((stone(), false)), "la modifica c'è in memoria");
        assert!(!h.server.chunks[&(0, 0, 0)].dirty, "ma non si salva");
        assert!(h.server.flat_warned, "avviso nel log dato");
    }
}

// ---------- rottura dei blocchi ----------

mod breaking {
    use super::blocks::{air, change, harness_with_chunk, stone, updates, water};
    use super::*;
    use crate::{
        breaking::drops_for,
        entity::EntityKind,
        events::WorldAction,
        inventory::Item,
        world::{block_hash, State},
    };
    use bedrock_protocol::{
        generated::{LevelEvent, UpdateBlock},
        input::{player_action, use_item},
    };
    use std::sync::{Arc, Mutex};
    use vgen::Dim;

    const ROCK: [i32; 3] = [3, 10, 4];
    const NEAR: [f32; 3] = [3.5, 11.6, 4.5];

    /// Un mondo con una pietra in `ROCK`, Alice (1, che vede il chunk) in modalità `mode` vicino alla pietra, Bob (2, lo vede) e Carol (3, no).
    fn setup(mode: GameMode) -> Harness {
        let mut h = harness_with_chunk();
        for (pid, name) in [(1, "Alice"), (2, "Bob"), (3, "Carol")] {
            h.join(pid, name);
        }
        for pid in [1, 2] {
            h.server.players.get_mut(&pid).unwrap().sent.insert((0, 0));
        }
        h.server.players.get_mut(&3).unwrap().sent.clear();
        let alice = h.server.players.get_mut(&1).unwrap();
        (alice.mode, alice.pos) = (mode, NEAR);
        h.server.set_blocks(Dim::Overworld, &[change(ROCK, stone(), false)]);
        let chunk = h.server.chunks.get_mut(&(0, 0, 0)).unwrap();
        (chunk.fallback, chunk.dirty) = (false, false); // come un chunk vero: le modifiche si salvano
        (1..=3).for_each(|pid| drop(h.packets_to(pid)));
        h
    }

    /// Passa al tick successivo senza far girare il resto del gioco: un blocco già tentato si può riprovare.
    fn next_tick(h: &mut Harness) {
        h.server.tick_count += 1;
    }

    /// Tutti i pacchetti inviati da quando si è letto l'ultima volta, con il destinatario.
    fn outbox(sent: &mut UnboundedReceiver<Command>) -> Vec<(u64, Packet)> {
        let mut out = Vec::new();
        while let Ok(cmd) = sent.try_recv() {
            if let Command::Send(to, data) = cmd {
                out.extend(Session::default().decode(&data).unwrap().into_iter().map(|p| (to, p)));
            }
        }
        out
    }

    /// I `LevelEvent` di `outbox`: (destinatario, evento, posizione, dati).
    fn events(outbox: &[(u64, Packet)]) -> Vec<(u64, i32, [f32; 3], i32)> {
        outbox
            .iter()
            .filter(|(_, p)| p.id == LevelEvent::ID)
            .map(|(to, p)| {
                let r = &mut Reader(&p.body);
                (*to, r.var_i32().unwrap(), r.vec3().unwrap(), r.var_i32().unwrap())
            })
            .collect()
    }

    /// Un `PlayerAction` (36) con `runtime_id` 1.
    fn action(kind: i32, pos: [i32; 3]) -> Packet {
        let mut w = Writer::default();
        w.var_u64(1);
        w.var_i32(kind);
        pos.iter().chain(&[0, 0, 0]).for_each(|&c| w.var_i32(c));
        w.var_i32(1); // faccia
        Packet::new(id::PLAYER_ACTION, w.0)
    }

    /// `UseItem` (aria in mano) con quell'azione sul blocco `pos`.
    fn use_item_bytes(w: &mut Writer, kind: i32, pos: [i32; 3]) {
        w.var_i32(kind);
        w.u8(1); // trigger
        pos.iter().for_each(|&c| w.var_i32(c));
        w.u8(1); // faccia
        w.var_i32(0); // casella
        w.u8(0); // mano
        ItemStack::air().encode(w);
        (0..6).for_each(|_| w.f32(0.5)); // posizione del giocatore e punto cliccato
        w.var_u32(0); // blocco secondo il client
        w.u8(1);
        w.u8(0);
    }

    /// `PlayerAuthInput` con gli occhi in `eyes`, l'eventuale `UseItem` (flag 34) e le azioni di blocco (flag 35).
    fn pai(eyes: [f32; 3], item: Option<(i32, [i32; 3])>, blocks: &[(i32, [i32; 3])]) -> Packet {
        let mut w = Writer::default();
        for v in [0.0, 0.0, eyes[0], eyes[1], eyes[2], 0.0, 0.0, 0.0] {
            w.f32(v);
        }
        let flags: Vec<i32> = [item.map(|_| 34), (!blocks.is_empty()).then_some(35)].into_iter().flatten().collect();
        w.var_u32(flags.len() as u32);
        flags.iter().for_each(|&f| w.var_i32(f));
        w.var_u32(1); // modalità di input
        w.var_u32(0);
        w.var_i32(2);
        (0..2).for_each(|_| w.f32(0.0));
        w.var_u64(10);
        (0..3).for_each(|_| w.f32(0.0));
        match item {
            Some((kind, pos)) => {
                w.u8(1);
                w.var_i32(0); // id legacy
                w.u8(0); // niente slot legacy
                w.var_u32(0); // niente azioni
                use_item_bytes(&mut w, kind, pos);
            }
            None => w.u8(0),
        }
        w.u8(0); // niente richiesta di pila
        if blocks.is_empty() {
            w.u8(0);
        } else {
            w.u8(1);
            w.var_u32(blocks.len() as u32);
            for (kind, pos) in blocks {
                w.var_i32(*kind);
                pos.iter().for_each(|&c| w.var_i32(c));
                w.var_i32(1);
            }
        }
        w.u8(0); // niente veicolo
        w.u8(0);
        (0..7).for_each(|_| w.f32(0.0));
        Packet::new(id::PLAYER_AUTH_INPUT, w.0)
    }

    /// `InventoryTransaction` di tipo ItemUse.
    fn transaction(kind: i32, pos: [i32; 3]) -> Packet {
        let mut w = Writer::default();
        w.var_i32(0); // id legacy
        w.u8(0); // niente slot legacy
        w.var_u32(2); // tipo ItemUse
        w.var_u32(0); // niente azioni
        use_item_bytes(&mut w, kind, pos);
        Packet::new(id::INVENTORY_TRANSACTION, w.0)
    }

    /// Un gancio che registra le azioni che vede e risponde `allow`.
    fn counting_hook(h: &mut Harness, allow: bool) -> Arc<Mutex<Vec<WorldAction>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        h.server.hooks.push(Box::new(move |a| {
            log.lock().unwrap().push(a.clone());
            allow
        }));
        seen
    }

    fn assert_broken(h: &Harness) {
        assert_eq!(h.server.block_at(Dim::Overworld, ROCK), Some((air(), false)));
        assert!(h.server.chunks[&(0, 0, 0)].dirty, "il chunk si salva");
    }

    fn assert_untouched(h: &Harness) {
        assert_eq!(h.server.block_at(Dim::Overworld, ROCK), Some((stone(), false)));
        assert!(!h.server.chunks[&(0, 0, 0)].dirty);
    }

    fn held(name: &str) -> Item {
        Item::from_name(name, 1).unwrap()
    }

    fn drop_names(block: &str, tool: &str, rolls: &[f32]) -> Vec<(String, u16)> {
        let mut rolls = rolls.iter().copied().cycle();
        let held = if tool.is_empty() { Item::default() } else { held(tool) };
        drops_for(block, &held, &mut || rolls.next().unwrap())
            .into_iter()
            .map(|i| (crate::registry::items().name(i.id).unwrap().to_string(), i.count))
            .collect()
    }

    fn dropped_items(h: &Harness) -> Vec<(Item, [f32; 3], [f32; 3], u64)> {
        h.server
            .entities
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Item { item, velocity, pickup_at, .. } => Some((item.clone(), e.pos, *velocity, *pickup_at)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn drops_depend_on_tool_and_tier() {
        assert!(drop_names("minecraft:stone", "", &[0.5]).is_empty(), "la pietra a mano non lascia niente");
        assert_eq!(drop_names("minecraft:stone", "minecraft:wooden_pickaxe", &[0.5]), [("minecraft:cobblestone".to_string(), 1)]);
        assert!(drop_names("minecraft:stone", "minecraft:wooden_axe", &[0.5]).is_empty(), "serve il piccone");
        assert!(drop_names("minecraft:iron_ore", "minecraft:wooden_pickaxe", &[0.5]).is_empty());
        assert_eq!(drop_names("minecraft:iron_ore", "minecraft:stone_pickaxe", &[0.5]), [("minecraft:raw_iron".to_string(), 1)]);
        assert!(drop_names("minecraft:diamond_ore", "minecraft:stone_pickaxe", &[0.5]).is_empty());
        assert_eq!(drop_names("minecraft:diamond_ore", "minecraft:iron_pickaxe", &[0.5]).len(), 1);
        assert_eq!(drop_names("minecraft:grass_block", "", &[0.5]), [("minecraft:dirt".to_string(), 1)], "senza strumento richiesto");
    }

    #[test]
    fn zero_count_roll_drops_nothing() {
        // arbusto secco: bastoni 0-2 (probabilità 1, quindi tiro esclusivo: il primo valore sceglie la voce, il secondo la quantità)
        assert!(drop_names("minecraft:deadbush", "", &[0.0, 0.0]).is_empty());
        assert_eq!(drop_names("minecraft:deadbush", "", &[0.0, 0.99]), [("minecraft:stick".to_string(), 2)]);
    }

    #[test]
    fn gravel_is_one_exclusive_roll() {
        assert_eq!(drop_names("minecraft:gravel", "", &[0.05]), [("minecraft:flint".to_string(), 1)]);
        assert_eq!(drop_names("minecraft:gravel", "", &[0.5]), [("minecraft:gravel".to_string(), 1)]);
    }

    #[test]
    fn independent_chances_and_counts() {
        // l'erba alta: semi con probabilità 0,125 (tiro indipendente)
        assert_eq!(drop_names("minecraft:short_grass", "", &[0.05, 0.0]).len(), 1);
        assert!(drop_names("minecraft:short_grass", "", &[0.5]).is_empty());
        // il vetro ha una voce senza oggetti: niente
        assert!(drop_names("minecraft:glass", "", &[0.0]).is_empty());
    }

    #[test]
    fn block_without_entry_drops_itself_only_if_it_is_an_item() {
        assert_eq!(drop_names("minecraft:oak_planks", "", &[0.5]), [("minecraft:oak_planks".to_string(), 1)]);
        assert!(drop_names("minecraft:redstone_wire", "", &[0.5]).is_empty());
        let planks = drops_for("minecraft:oak_planks", &Item::default(), &mut || 0.5);
        assert_eq!(planks[0].block, crate::registry::items().block_of("minecraft:oak_planks"), "porta lo stato del blocco");
    }

    #[test]
    fn survival_break_with_pickaxe_spawns_cobblestone() {
        let mut h = setup(GameMode::Survival);
        h.server.players.get_mut(&1).unwrap().inventory.set_main(0, held("minecraft:wooden_pickaxe"));
        h.receive(1, &[transaction(use_item::DESTROY, ROCK)]);
        assert_broken(&h);
        let items = dropped_items(&h);
        assert_eq!(items.len(), 1);
        let (item, pos, velocity, pickup_at) = &items[0];
        assert_eq!((item.id, item.count), (crate::registry::items().id("minecraft:cobblestone").unwrap(), 1));
        assert_eq!(*pos, [3.5, 10.5, 4.5], "al centro del blocco");
        assert!(velocity[0].abs() <= 0.1 && velocity[2].abs() <= 0.1 && velocity[1] == 0.2, "{velocity:?}");
        assert_eq!(*pickup_at, h.server.tick_count + 10);
    }

    #[test]
    fn survival_break_by_hand_leaves_nothing_for_stone() {
        let mut h = setup(GameMode::Survival);
        h.receive(1, &[transaction(use_item::DESTROY, ROCK)]);
        assert_broken(&h);
        assert!(dropped_items(&h).is_empty());
    }

    #[test]
    fn creative_break_spawns_nothing() {
        let mut h = setup(GameMode::Creative);
        h.server.players.get_mut(&1).unwrap().inventory.set_main(0, held("minecraft:wooden_pickaxe"));
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_broken(&h);
        assert!(dropped_items(&h).is_empty());
    }

    #[test]
    fn refused_break_spawns_nothing() {
        let mut h = setup(GameMode::Survival);
        h.server.players.get_mut(&1).unwrap().inventory.set_main(0, held("minecraft:wooden_pickaxe"));
        counting_hook(&mut h, false);
        h.receive(1, &[transaction(use_item::DESTROY, ROCK)]);
        assert_untouched(&h);
        assert!(dropped_items(&h).is_empty());
    }

    #[test]
    fn double_break_in_one_tick_spawns_one_item() {
        let mut h = setup(GameMode::Survival);
        h.server.players.get_mut(&1).unwrap().inventory.set_main(0, held("minecraft:wooden_pickaxe"));
        h.receive(1, &[transaction(use_item::DESTROY, ROCK), action(player_action::START_BREAK, ROCK), action(player_action::STOP_BREAK, ROCK)]);
        assert_eq!(dropped_items(&h).len(), 1);
    }

    /// Il blocco non è cambiato e lo stato vero è stato rimandato al solo autore, senza effetti di rottura.
    fn assert_reverted(h: &mut Harness) {
        assert_untouched(h);
        let out = outbox(&mut h.sent);
        assert!(events(&out).is_empty());
        let state: Vec<_> = out.iter().filter(|(_, p)| p.id == UpdateBlock::ID).map(|(to, _)| *to).collect();
        // la cella e i suoi sei vicini (il client prevede anche le altre metà di porte e letti), strato 0 e 1, solo ad Alice
        assert_eq!(state, [1; 14]);
    }

    #[test]
    fn legacy_slots_are_resent_with_the_real_stack_id() {
        // Il client chiede di riavere la casella 3 della barra (contenitore 28) dopo una sua richiesta legacy (-8):
        // senza risposta la nomina con -8 e il lancio con Q successivo veniva rifiutato.
        let mut h = setup(GameMode::Creative);
        drop(outbox(&mut h.sent));
        let mut w = Writer::default();
        w.var_i32(-8);
        w.u8(1);
        w.var_u32(1);
        w.u8(28);
        w.var_u32(1);
        w.u8(3);
        w.var_u32(0); // tipo normale
        w.var_u32(0); // niente azioni
        h.receive(1, &[Packet::new(id::INVENTORY_TRANSACTION, w.0)]);
        let out = outbox(&mut h.sent);
        let want = h.server.players[&1].inventory.slot_packet_at(28, 3).unwrap();
        assert!(out.iter().any(|(to, p)| *to == 1 && p.id == id::INVENTORY_SLOT && p.body == want.body), "la casella 3 rimandata ad Alice");
    }

    #[test]
    fn a_refused_break_resends_the_neighbours_too() {
        let mut h = setup(GameMode::Creative);
        let above = [ROCK[0], ROCK[1] + 1, ROCK[2]];
        h.server.set_blocks(Dim::Overworld, &[change(above, stone(), false)]);
        h.server.players.get_mut(&1).unwrap().pos = [3.5, 30.0, 4.5]; // fuori portata
        drop(outbox(&mut h.sent));
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        let got = updates(&mut h.sent);
        let cells: std::collections::HashSet<_> = got.iter().filter(|u| u.0 == 1 && u.4 == 0).map(|u| u.1).collect();
        assert!(cells.contains(&ROCK) && cells.contains(&above), "la cella e quella sopra (l'altra metà): {cells:?}");
        assert_eq!(cells.len(), 7, "la cella e i sei vicini");
        assert!(got.iter().all(|u| u.0 == 1), "solo ad Alice");
    }

    #[test]
    fn creative_break_sets_air_and_notifies() {
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, true);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_broken(&h);
        assert_eq!(seen.lock().unwrap().as_slice(), [WorldAction::Break { pid: 1, pos: ROCK, old: stone() }]);
        let out = outbox(&mut h.sent);
        // UpdateBlock (blocco e acqua) a chi vede il chunk; a Carol niente
        let mut blocks: Vec<_> = out.iter().filter(|(_, p)| p.id == UpdateBlock::ID).map(|(to, p)| (*to, Reader(&p.body).block_pos().unwrap())).collect();
        blocks.sort();
        assert_eq!(blocks, [(1, ROCK), (1, ROCK), (2, ROCK), (2, ROCK)]);
        // LevelEvent 2001 con l'hash del blocco rotto, al centro del blocco
        let mut effects = events(&out);
        effects.sort_by_key(|e| e.0); // l'ordine dei giocatori non è fissato
        assert_eq!(effects, [(1, 2001, [3.5, 10.5, 4.5], stone()), (2, 2001, [3.5, 10.5, 4.5], stone())]);
    }

    #[test]
    fn break_sends_air_to_the_viewers() {
        let mut h = setup(GameMode::Creative);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        let mut got = updates(&mut h.sent);
        got.sort(); // l'ordine dei giocatori non è fissato
        assert_eq!(got, [(1, ROCK, air() as u32, 3, 0), (1, ROCK, air() as u32, 3, 1), (2, ROCK, air() as u32, 3, 0), (2, ROCK, air() as u32, 3, 1)]);
    }

    #[test]
    fn break_via_use_item_destroy() {
        let mut h = setup(GameMode::Creative);
        h.receive(1, &[pai(NEAR, Some((use_item::DESTROY, ROCK)), &[])]);
        assert_broken(&h);
        assert_eq!(events(&outbox(&mut h.sent)).len(), 2, "evento di rottura a chi vede il chunk");
    }

    #[test]
    fn break_via_inventory_transaction_destroy() {
        let mut h = setup(GameMode::Survival);
        h.receive(1, &[transaction(use_item::DESTROY, ROCK)]);
        assert_broken(&h);
        // un uso diverso (piazzare, usare) non rompe nulla
        let mut h = setup(GameMode::Survival);
        h.receive(1, &[transaction(use_item::PLACE, ROCK), transaction(use_item::USE, ROCK)]);
        assert_untouched(&h);
    }

    #[test]
    fn survival_breaks_instantly_through_start_and_stop() {
        let mut h = setup(GameMode::Survival);
        h.receive(1, &[pai(NEAR, None, &[(player_action::START_BREAK, ROCK), (player_action::STOP_BREAK, ROCK)])]);
        assert_broken(&h);
        let mut h = setup(GameMode::Survival);
        h.receive(1, &[pai(NEAR, None, &[(player_action::CONTINUE_DESTROY, ROCK), (player_action::PREDICT_DESTROY, ROCK)])]);
        assert_broken(&h);
        let mut h = setup(GameMode::Survival); // le stesse azioni arrivano anche come PlayerAction
        h.receive(1, &[action(player_action::START_BREAK, ROCK), action(player_action::STOP_BREAK, ROCK)]);
        assert_broken(&h);
    }

    #[test]
    fn stop_break_without_start_or_after_abort_or_elsewhere_breaks_nothing() {
        let mut h = setup(GameMode::Survival);
        let elsewhere = [ROCK[0] + 1, ROCK[1], ROCK[2]];
        h.receive(1, &[action(player_action::STOP_BREAK, ROCK)]);
        h.receive(1, &[action(player_action::START_BREAK, ROCK), action(player_action::ABORT_BREAK, ROCK), action(player_action::STOP_BREAK, ROCK)]);
        h.receive(1, &[action(player_action::START_BREAK, ROCK), action(player_action::STOP_BREAK, elsewhere)]);
        h.receive(1, &[action(player_action::STOP_BREAK, ROCK)]);
        h.receive(1, &[action(player_action::START_BREAK, ROCK), action(player_action::CRACK, ROCK)]);
        assert_untouched(&h);
    }

    #[test]
    fn break_out_of_reach_is_reverted() {
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, true);
        h.server.players.get_mut(&1).unwrap().pos = [3.5, 30.0, 4.5]; // 19.5 dal centro del blocco
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_reverted(&mut h);
        assert!(seen.lock().unwrap().is_empty(), "i ganci non vedono un'azione fuori portata");
        // in sopravvivenza il limite è 8 blocchi
        let mut h = setup(GameMode::Survival);
        h.server.players.get_mut(&1).unwrap().pos = [3.5, 19.0, 4.5]; // 8.5
        h.receive(1, &[transaction(use_item::DESTROY, ROCK)]);
        assert_reverted(&mut h);
        next_tick(&mut h);
        h.server.players.get_mut(&1).unwrap().pos = [3.5, 18.0, 4.5]; // 7.5
        h.receive(1, &[transaction(use_item::DESTROY, ROCK)]);
        assert_broken(&h);
    }

    #[test]
    fn reach_uses_the_position_of_the_same_input() {
        let mut h = setup(GameMode::Creative);
        h.server.players.get_mut(&1).unwrap().pos = [3.5, 300.0, 4.5];
        h.receive(1, &[pai(NEAR, Some((use_item::DESTROY, ROCK)), &[])]);
        assert_broken(&h);
    }

    #[test]
    fn spectator_and_adventure_cannot_break() {
        for mode in [GameMode::Spectator, GameMode::Adventure] {
            let mut h = setup(mode);
            let seen = counting_hook(&mut h, true);
            h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
            next_tick(&mut h);
            h.receive(1, &[transaction(use_item::DESTROY, ROCK)]);
            next_tick(&mut h);
            h.receive(1, &[pai(NEAR, Some((use_item::DESTROY, ROCK)), &[(player_action::START_BREAK, ROCK), (player_action::STOP_BREAK, ROCK)])]);
            assert!(seen.lock().unwrap().is_empty(), "{mode:?}");
            assert_untouched(&h);
            let sent = updates(&mut h.sent);
            assert_eq!(sent.len(), 3 * 14, "{mode:?}: tre rifiuti, sette celle e due strati ciascuno");
            assert!(sent.iter().all(|u| u.0 == 1), "{mode:?}: lo stato vero va solo ad Alice");
            assert!(sent.iter().filter(|u| u.1 == ROCK).count() >= 6, "{mode:?}: tre volte la cella rifiutata");
            assert!(sent.iter().any(|u| u.2 == stone() as u32 && u.4 == 0), "{mode:?}");
        }
    }

    #[test]
    fn creative_destroy_is_only_for_creative_players() {
        let mut h = setup(GameMode::Survival);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_reverted(&mut h);
    }

    #[test]
    fn a_hook_can_cancel_the_break() {
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, false);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_reverted(&mut h);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn breaking_waterlogged_kelp_leaves_water() {
        let mut h = setup(GameMode::Creative);
        let kelp = block_hash("minecraft:kelp", &[("kelp_age", State::Int(3))]);
        h.server.set_blocks(Dim::Overworld, &[change(ROCK, kelp, true)]);
        drop(h.packets_to(1));
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_eq!(h.server.block_at(Dim::Overworld, ROCK), Some((air(), true)), "il blocco sparisce, l'acqua resta");
        let alice: Vec<_> = updates(&mut h.sent).into_iter().filter(|u| u.0 == 1).collect(); // l'ordine dei giocatori non è fissato
        assert_eq!(alice, [(1, ROCK, air() as u32, 3, 0), (1, ROCK, water() as u32, 3, 1)]);
        assert!(h.server.chunks[&(0, 0, 0)].water(3, 10, 4));
    }

    #[test]
    fn double_break_same_tick_happens_once() {
        for allow in [true, false] {
            let mut h = setup(GameMode::Creative);
            let seen = counting_hook(&mut h, allow);
            // PlayerAction 13 e UseItem Destroy sulla stessa posizione, nello stesso tick (anche in due pacchetti diversi)
            let both = [action(player_action::CREATIVE_DESTROY, ROCK), pai(NEAR, Some((use_item::DESTROY, ROCK)), &[]), transaction(use_item::DESTROY, ROCK)];
            h.receive(1, &both);
            assert_eq!(seen.lock().unwrap().len(), 1, "un solo Break (il gancio lascia passare: {allow})");
            assert_eq!(events(&outbox(&mut h.sent)).len(), if allow { 2 } else { 0 });
        }
        // nel tick dopo lo stesso punto si rompe di nuovo
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, true);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        h.server.set_blocks(Dim::Overworld, &[change(ROCK, stone(), false)]);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_eq!(seen.lock().unwrap().len(), 1, "stesso tick");
        next_tick(&mut h);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_eq!(seen.lock().unwrap().len(), 2, "tick successivo");
    }

    #[test]
    fn break_air_does_nothing() {
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, true);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, [3, 12, 4]), transaction(use_item::DESTROY, [3, 13, 4])]);
        assert!(seen.lock().unwrap().is_empty());
        assert!(outbox(&mut h.sent).is_empty(), "nessun pacchetto a nessuno");
        assert!(!h.server.chunks[&(0, 0, 0)].dirty);
    }

    #[test]
    fn break_in_unloaded_chunk_or_outside_the_world_does_nothing() {
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, true);
        for pos in [[40, 10, 4], [3, 400, 4], [3, -100, 4], [i32::MAX, i32::MIN, i32::MAX]] {
            h.receive(1, &[action(player_action::CREATIVE_DESTROY, pos)]);
        }
        assert!(seen.lock().unwrap().is_empty());
        assert!(outbox(&mut h.sent).is_empty());
    }

    #[test]
    fn bedrock_and_liquids_resist_survival_but_bedrock_breaks_in_creative() {
        let bedrock = block_hash("minecraft:bedrock", &[("infiniburn_bit", State::Byte(0))]);
        let lava = block_hash("minecraft:lava", &[("liquid_depth", State::Int(0))]);
        for hash in [bedrock, lava, water_block()] {
            let mut h = setup(GameMode::Survival);
            h.server.set_blocks(Dim::Overworld, &[change(ROCK, hash, false)]);
            h.server.chunks.get_mut(&(0, 0, 0)).unwrap().dirty = false;
            drop(h.packets_to(1));
            h.receive(1, &[transaction(use_item::DESTROY, ROCK)]);
            assert_eq!(h.server.block_at(Dim::Overworld, ROCK), Some((hash, false)));
            assert!(!updates(&mut h.sent).is_empty(), "stato vero rimandato");
        }
        let mut h = setup(GameMode::Creative);
        h.server.set_blocks(Dim::Overworld, &[change(ROCK, bedrock, false)]);
        h.receive(1, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert_eq!(h.server.block_at(Dim::Overworld, ROCK), Some((air(), false)));
    }

    fn water_block() -> i32 {
        block_hash("minecraft:water", &[("liquid_depth", State::Int(0))])
    }

    #[test]
    fn malformed_or_early_break_packets_are_ignored() {
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, true);
        let good = action(player_action::CREATIVE_DESTROY, ROCK).body;
        for cut in 0..good.len() {
            h.receive(1, &[Packet::new(id::PLAYER_ACTION, good[..cut].to_vec())]);
        }
        h.receive(1, &[Packet::new(id::INVENTORY_TRANSACTION, vec![0, 0, 2, 0, 2, 1])]);
        assert!(seen.lock().unwrap().is_empty());
        assert_untouched(&h);
        // un giocatore non ancora nel mondo non rompe nulla
        h.server.open(9);
        h.receive(9, &[action(player_action::CREATIVE_DESTROY, ROCK)]);
        assert!(seen.lock().unwrap().is_empty());
        assert_untouched(&h);
    }

    #[test]
    fn in_reach_limits() {
        use crate::breaking::in_reach;
        let eyes = [0.5, 0.5, 0.5];
        assert!(in_reach(eyes, [0, 0, 0], false));
        assert!(in_reach(eyes, [8, 0, 0], false), "8.0 esatti");
        assert!(!in_reach(eyes, [9, 0, 0], false));
        assert!(in_reach(eyes, [14, 0, 0], true), "14.0 esatti");
        assert!(!in_reach(eyes, [15, 0, 0], true));
        assert!(!in_reach([f32::NAN, 0.0, 0.0], [0, 0, 0], true));
        assert!(!in_reach([f32::INFINITY, 0.0, 0.0], [0, 0, 0], true));
        assert!(!in_reach(eyes, [i32::MAX, i32::MAX, i32::MAX], true), "senza overflow");
        assert!(!in_reach(eyes, [8, 8, 0], false), "la distanza è in 3D");
    }
}

// ---------- piazzamento ----------

mod placing {
    use super::blocks::{air, change, harness_with_chunk, stone, updates};
    use super::*;
    use crate::{events::WorldAction, world::block_hash};
    use bedrock_protocol::{
        generated::{InventorySlot, LevelSoundEvent, UpdateBlock},
        input::{player_action, use_item},
    };
    use std::sync::{Arc, Mutex};
    use vgen::Dim;

    /// La pietra su cui si clicca e la cella sopra, dove va il blocco piazzato.
    pub(super) const ROCK: [i32; 3] = [3, 10, 4];
    pub(super) const TOP: [i32; 3] = [3, 11, 4];
    /// Occhi di Alice: tre blocchi a est della pietra, i piedi a y 10.
    pub(super) const EYES: [f32; 3] = [6.5, 11.62, 4.5];

    /// Un mondo con una pietra in `ROCK`; Alice (1) in modalità `mode` con in mano 64 pietre (casella 0), Bob (2) vede il chunk, Carol (3) no.
    pub(super) fn setup(mode: GameMode) -> Harness {
        let mut h = harness_with_chunk();
        for (pid, name) in [(1, "Alice"), (2, "Bob"), (3, "Carol")] {
            h.join(pid, name);
        }
        for pid in [1, 2] {
            h.server.players.get_mut(&pid).unwrap().sent.insert((0, 0));
        }
        h.server.players.get_mut(&3).unwrap().sent.clear();
        h.server.set_blocks(Dim::Overworld, &[change(ROCK, stone(), false)]);
        // le pietre si prendono dal menu creativo (in sopravvivenza non si può): la modalità si cambia dopo
        h.receive(1, &[stack_request_packet(|w| craft_and_place(w, -1, stone_net_id(), 0))]);
        let alice = h.server.players.get_mut(&1).unwrap();
        (alice.mode, alice.pos) = (mode, EYES);
        let chunk = h.server.chunks.get_mut(&(0, 0, 0)).unwrap();
        (chunk.fallback, chunk.dirty) = (false, false);
        (1..=3).for_each(|pid| drop(h.packets_to(pid)));
        h
    }

    /// Tutti i pacchetti inviati da quando si è letto l'ultima volta, con il destinatario.
    pub(super) fn packets(sent: &mut UnboundedReceiver<Command>) -> Vec<(u64, Packet)> {
        let mut out = Vec::new();
        while let Ok(cmd) = sent.try_recv() {
            if let Command::Send(to, data) = cmd {
                out.extend(Session::default().decode(&data).unwrap().into_iter().map(|p| (to, p)));
            }
        }
        out
    }

    /// `InventoryTransaction` di tipo ItemUse con azione Place sul blocco `pos`, faccia `face`, con `held` in mano secondo il client.
    pub(super) fn place_packet(pos: [i32; 3], face: u8, held: &ItemStack) -> Packet {
        place_packet_slot(pos, face, held, 0)
    }

    /// Come `place_packet`, con la casella della barra che il client dichiara in mano.
    pub(super) fn place_packet_slot(pos: [i32; 3], face: u8, held: &ItemStack, slot: i32) -> Packet {
        let mut w = Writer::default();
        w.var_i32(0); // id legacy
        w.u8(0); // niente slot legacy
        w.var_u32(2); // tipo ItemUse
        w.var_u32(0); // niente azioni
        w.var_i32(use_item::PLACE);
        w.u8(1); // trigger
        pos.iter().for_each(|&c| w.var_i32(c));
        w.u8(face);
        w.var_i32(slot); // casella
        w.u8(0); // mano
        held.encode(&mut w);
        EYES.iter().for_each(|&v| w.f32(v));
        [0.5f32, 1.0, 0.5].iter().for_each(|&v| w.f32(v)); // punto cliccato
        w.var_u32(0); // blocco secondo il client
        w.u8(1);
        w.u8(0);
        Packet::new(id::INVENTORY_TRANSACTION, w.0)
    }

    /// `PlayerAction` con `runtime_id` 1.
    pub(super) fn destroy(pos: [i32; 3]) -> Packet {
        let mut w = Writer::default();
        w.var_u64(1);
        w.var_i32(player_action::CREATIVE_DESTROY);
        pos.iter().chain(&[0, 0, 0]).for_each(|&c| w.var_i32(c));
        w.var_i32(1);
        Packet::new(id::PLAYER_ACTION, w.0)
    }

    /// L'oggetto in mano ad Alice come lo vede il suo client.
    pub(super) fn held(h: &Harness) -> ItemStack {
        h.server.players[&1].inventory.held().stack(1)
    }

    fn assert_placed(h: &Harness) {
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((stone(), false)));
        assert!(h.server.chunks[&(0, 0, 0)].dirty, "il chunk si salva");
    }

    /// Niente è cambiato e lo stato vero (la pietra cliccata e l'aria sopra) è stato rimandato al solo autore.
    fn assert_reverted(h: &mut Harness) {
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)));
        assert!(!h.server.chunks[&(0, 0, 0)].dirty);
        let mut cells: Vec<_> = updates(&mut h.sent).iter().map(|(to, pos, hash, _, layer)| (*to, *pos, *hash, *layer)).collect();
        cells.sort();
        // la cella cliccata, quella accanto e i loro vicini: il client prevede anche le seconde metà (porte, letti, piante alte)
        let mut want = Vec::new();
        for pos in crate::placement::with_neighbors(&[ROCK, TOP]) {
            want.push((1, pos, if pos == ROCK { stone() } else { air() } as u32, 0));
            want.push((1, pos, air() as u32, 1));
        }
        want.sort();
        assert_eq!(cells, want);
    }

    pub(super) fn counting_hook(h: &mut Harness, allow: bool) -> Arc<Mutex<Vec<WorldAction>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        h.server.hooks.push(Box::new(move |a| {
            log.lock().unwrap().push(a.clone());
            allow
        }));
        seen
    }

    #[test]
    fn creative_place_sets_the_block_and_notifies_viewers() {
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, true);
        let before = h.server.players[&1].inventory.held().clone();
        assert_eq!((before.id, before.count), (registry::items().id("minecraft:stone").unwrap(), 64));
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_placed(&h);
        assert_eq!(seen.lock().unwrap().as_slice(), [WorldAction::Place { pid: 1, changes: vec![change(TOP, stone(), false)] }]);
        assert_eq!(h.server.players[&1].inventory.held().count, 64, "in creativo non si consuma");
        let out = packets(&mut h.sent);
        let mut got: Vec<_> = out.iter().filter(|(_, p)| p.id == UpdateBlock::ID).map(|(to, p)| (*to, Reader(&p.body).block_pos().unwrap())).collect();
        got.sort();
        assert_eq!(
            got,
            [(1, ROCK), (1, ROCK), (1, TOP), (1, TOP), (1, TOP), (1, TOP), (2, TOP), (2, TOP)],
            "strato 0 e 1 ad Alice e Bob, non a Carol; ad Alice anche la cella cliccata e quella prevista"
        );
        let sounds: Vec<_> = out
            .iter()
            .filter(|(_, p)| p.id == LevelSoundEvent::ID)
            .map(|(to, p)| {
                let r = &mut Reader(&p.body);
                (*to, r.string().unwrap().to_string(), r.vec3().unwrap(), r.var_i32().unwrap())
            })
            .collect();
        assert_eq!(sounds.len(), 2);
        assert!(sounds.iter().all(|(_, name, pos, data)| name == "place" && *pos == [3.5, 11.5, 4.5] && *data == stone()));
        assert!(out.iter().all(|(_, p)| p.id != InventorySlot::ID), "nessun cambio all'inventario");
    }

    #[test]
    fn survival_place_does_not_consume_and_resyncs_the_slot() {
        // la spec di questa tappa: la sopravvivenza piazza senza consumare; il client però ha previsto il consumo
        let mut h = setup(GameMode::Survival);
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_placed(&h);
        assert_eq!(h.server.players[&1].inventory.held().count, 64, "nessun consumo");
        let out = packets(&mut h.sent);
        let slots: Vec<_> = out.iter().filter(|(_, p)| p.id == InventorySlot::ID).collect();
        assert_eq!(slots.len(), 1, "il client sa com'è davvero la casella");
        assert_eq!(slots[0].0, 1);
        assert_eq!(slots[0].1.body, h.server.players[&1].inventory.held_slot_packet().body, "con il conteggio vero (64)");
        // ripetuto fino a svuotare la scorta che non c'è: la mano resta piena
        for _ in 0..70 {
            h.server.set_blocks(Dim::Overworld, &[change(TOP, air(), false)]);
            h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        }
        assert_eq!(h.server.players[&1].inventory.held().count, 64);
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((stone(), false)));
    }

    #[test]
    fn held_item_different_from_the_clients_is_refused() {
        // Review Focus 4: il server non ha in mano ciò che il client dice
        let mut h = setup(GameMode::Creative);
        let mut unknown = held(&h);
        unknown.id = 9999; // un oggetto che nessuno conosce
        h.receive(1, &[place_packet(ROCK, 1, &unknown)]);
        assert_reverted(&mut h);
        let mut other_block = held(&h);
        other_block.block_runtime_id = block_hash("minecraft:dirt", &[]) as u32;
        h.receive(1, &[place_packet(ROCK, 1, &other_block)]);
        assert_reverted(&mut h);
        // il server ha la mano vuota e il client dice di avere pietra
        let stone_stack = held(&h);
        h.receive(1, &[mob_equipment(5, 5)]);
        h.receive(1, &[place_packet_slot(ROCK, 1, &stone_stack, 5)]);
        assert_reverted(&mut h);
    }

    #[test]
    fn empty_hands_do_nothing_and_send_nothing() {
        let mut h = setup(GameMode::Creative);
        h.receive(1, &[mob_equipment(5, 5)]); // casella vuota
        h.receive(1, &[place_packet_slot(ROCK, 1, &ItemStack::air(), 5)]);
        assert!(packets(&mut h.sent).is_empty());
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)));
    }

    #[test]
    fn out_of_reach_wrong_mode_and_hooks_are_refused() {
        let mut h = setup(GameMode::Creative);
        h.server.players.get_mut(&1).unwrap().pos = [40.5, 11.62, 4.5];
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_reverted(&mut h);
        for mode in [GameMode::Adventure, GameMode::Spectator] {
            let mut h = setup(mode);
            h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
            assert_reverted(&mut h);
        }
        // sopravvivenza: la portata è 8, non 14
        let mut h = setup(GameMode::Survival);
        h.server.players.get_mut(&1).unwrap().pos = [12.5, 11.62, 4.5];
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_reverted(&mut h);
        assert_eq!(h.server.players[&1].inventory.held().count, 64, "nulla si consuma");
        // un gancio che annulla
        let mut h = setup(GameMode::Creative);
        let seen = counting_hook(&mut h, false);
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_reverted(&mut h);
    }

    #[test]
    fn nobody_can_be_inside_the_new_block() {
        // Alice sta sulla pietra: i suoi piedi sono nella cella da riempire
        let mut h = setup(GameMode::Creative);
        h.server.players.get_mut(&1).unwrap().pos = [3.5, 12.62, 4.5];
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_reverted(&mut h);
        // Bob sta nella cella
        let mut h = setup(GameMode::Creative);
        h.server.players.get_mut(&2).unwrap().pos = [3.5, 12.62, 4.5];
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_reverted(&mut h);
    }

    #[test]
    fn spectators_are_not_in_the_way() {
        // Bob, spettatore, sta nella cella da riempire: non ostruisce
        let mut h = setup(GameMode::Creative);
        let bob = h.server.players.get_mut(&2).unwrap();
        (bob.pos, bob.mode) = ([3.5, 12.62, 4.5], GameMode::Spectator);
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_placed(&h);
    }

    #[test]
    fn place_then_break_in_the_same_tick_both_work() {
        let mut h = setup(GameMode::Creative);
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        h.receive(1, &[destroy(TOP)]);
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)), "rotto");
        h.receive(1, &[place_packet(ROCK, 1, &held(&h))]);
        assert_placed(&h);
        h.receive(1, &[destroy(TOP)]);
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)), "la rottura dopo il piazzamento non è scartata come doppione");
    }

    #[test]
    fn malformed_and_early_place_packets_are_ignored() {
        let mut h = setup(GameMode::Creative);
        let good = place_packet(ROCK, 1, &held(&h)).body;
        for cut in 0..good.len() {
            h.receive(1, &[Packet::new(id::INVENTORY_TRANSACTION, good[..cut].to_vec())]);
        }
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)));
        h.receive(1, &[place_packet(ROCK, 9, &held(&h))]); // faccia inesistente
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)));
        // clic in un chunk non caricato o fuori dal mondo: rifiutato, nessun chunk creato
        let chunks = h.server.chunks.len();
        for pos in [[1000, 10, 1000], [3, 400, 4], [3, -500, 4], [i32::MAX, 0, i32::MIN]] {
            h.receive(1, &[place_packet(pos, 1, &held(&h))]);
        }
        assert_eq!(h.server.chunks.len(), chunks);
        // un giocatore non ancora nel mondo non piazza nulla
        h.server.open(9);
        h.receive(9, &[place_packet(ROCK, 1, &held(&h))]);
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)));
    }

    #[test]
    fn player_auth_input_keeps_rotation_and_sneaking() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        let body = |yaw: f32, pitch: f32, sneaking: bool| {
            let mut b = auth_input_body(1.0, 70.0, 1.0);
            b[..4].copy_from_slice(&pitch.to_le_bytes());
            b[4..8].copy_from_slice(&yaw.to_le_bytes());
            if sneaking {
                b.splice(32..33, [1, 16]); // un flag attivo (var_i32 con zigzag: 8 diventa 16): l'accovacciamento
            }
            Packet::new(id::PLAYER_AUTH_INPUT, b)
        };
        h.receive(1, &[body(90.0, 15.0, true)]);
        let p = &h.server.players[&1];
        assert_eq!((p.yaw, p.pitch, p.sneaking), (90.0, 15.0, true));
        h.receive(1, &[body(-45.0, 0.0, false)]);
        let p = &h.server.players[&1];
        assert_eq!((p.yaw, p.pitch, p.sneaking), (-45.0, 0.0, false));
        h.receive(1, &[body(f32::NAN, 0.0, false)]);
        assert_eq!(h.server.players[&1].yaw, -45.0, "valori non finiti scartati");
        // anche quando il resto del pacchetto non si legge, la rotazione arriva dalla via di ripiego
        let mut cut = auth_input_body(1.0, 70.0, 1.0)[..30].to_vec();
        cut[4..8].copy_from_slice(&120.0f32.to_le_bytes());
        h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, cut)]);
        assert_eq!(h.server.players[&1].yaw, 120.0);
    }

    /// Mette in mano ad Alice (casella 1) l'oggetto del menu creativo con quel nome e quell'aux.
    pub(super) fn hold_creative(h: &mut Harness, name: &str, aux: u32) {
        let net_id = registry::creative().1.iter().position(|c| c.name == name && c.meta == aux).unwrap() as u32 + 1;
        h.receive(1, &[stack_request_packet(|w| craft_and_place(w, -2, net_id, 1))]);
        h.receive(1, &[mob_equipment(1, 1)]);
        assert_eq!(h.server.players[&1].inventory.held().aux, aux, "{name}");
    }

    /// Il NBT di rete di un `BlockActorData`: la posizione e il compound letto.
    fn block_actor(p: &Packet) -> ([i32; 3], crate::nbt::Tag) {
        let mut r = Reader(&p.body);
        let pos = r.block_pos().unwrap();
        (pos, crate::nbt::Reader::new(r.0, crate::nbt::Enc::Network).root().expect("NBT di rete").1)
    }

    #[test]
    fn bed_sends_block_actor_data_with_the_items_color() {
        use bedrock_protocol::generated::BlockActorData;
        let mut h = setup(GameMode::Creative);
        // la testa, davanti ad Alice (guarda a sud), ha bisogno del suo appoggio
        let support = [3, 10, 5];
        h.server.set_blocks(Dim::Overworld, &[change(support, stone(), false)]);
        hold_creative(&mut h, "minecraft:bed", 8);
        drop(packets(&mut h.sent));
        h.receive(1, &[place_packet_slot(ROCK, 1, &held(&h), 1)]);
        let name = |pos| registry::blocks().state(h.server.block_at(Dim::Overworld, pos).unwrap().0).unwrap().name.clone();
        assert_eq!((name(TOP), name([3, 11, 5])), ("minecraft:bed".to_string(), "minecraft:bed".to_string()));
        // le due metà hanno il block entity con il colore dell'aux (8: grigio chiaro), nel chunk e mandate a chi lo vede
        let chunk = &h.server.chunks[&(0, 0, 0)];
        let mut entities: Vec<_> = chunk.block_entities.iter().map(|b| [b.x, b.y, b.z]).collect();
        entities.sort();
        assert_eq!(entities, [TOP, [3, 11, 5]]);
        let out = packets(&mut h.sent);
        let mut seen: Vec<_> = out.iter().filter(|(_, p)| p.id == BlockActorData::ID).map(|(to, p)| (*to, block_actor(p))).collect();
        seen.sort_by_key(|(to, (pos, _))| (*to, *pos));
        assert_eq!(
            seen.iter().map(|(to, (pos, _))| (*to, *pos)).collect::<Vec<_>>(),
            [(1, TOP), (1, [3, 11, 5]), (2, TOP), (2, [3, 11, 5])],
            "Alice e Bob, non Carol"
        );
        for (_, (_, tag)) in &seen {
            assert_eq!(tag.get("id").and_then(crate::nbt::Tag::as_str), Some("Bed"));
            assert_eq!(tag.get("color"), Some(&crate::nbt::Tag::Byte(8)));
        }
        assert!(h.server.chunks[&(0, 0, 0)].dirty, "il chunk si salva");
        // il chunk che si manda dopo porta i block entity nel suo payload
        let payload = h.server.chunks[&(0, 0, 0)].network_bytes();
        assert!(payload.windows(3).any(|w| w == b"Bed"), "il letto sta nei dati del chunk");
        // rotto un pezzo, l'altro sparisce con il suo block entity
        h.receive(1, &[destroy(TOP)]);
        assert_eq!((h.server.block_at(Dim::Overworld, TOP), h.server.block_at(Dim::Overworld, [3, 11, 5])), (Some((air(), false)), Some((air(), false))));
        assert!(h.server.chunks[&(0, 0, 0)].block_entities.is_empty());
        let removed: Vec<_> = updates(&mut h.sent).into_iter().filter(|(to, pos, _, _, layer)| *to == 2 && *layer == 0 && *pos == [3, 11, 5]).collect();
        assert_eq!(removed.len(), 1, "anche Bob vede sparire la testa");
    }

    #[test]
    fn breaking_the_support_removes_what_it_held() {
        let mut h = setup(GameMode::Creative);
        let standing = registry::blocks().state(registry::blocks().default_hash("minecraft:torch").unwrap()).unwrap();
        let torch = standing.with("torch_facing_direction", registry::StateValue::Str("top".into())).hash();
        h.server.set_blocks(Dim::Overworld, &[change(TOP, torch, false)]);
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((torch, false)), "la torcia regge sulla pietra");
        drop(packets(&mut h.sent));
        h.receive(1, &[destroy(ROCK)]);
        assert_eq!(h.server.block_at(Dim::Overworld, ROCK), Some((air(), false)));
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)), "senza appoggio la torcia sparisce");
        let seen: Vec<_> =
            updates(&mut h.sent).into_iter().filter(|(to, _, _, _, layer)| *to == 2 && *layer == 0).map(|(_, pos, hash, _, _)| (pos, hash)).collect();
        assert_eq!(seen, [(ROCK, air() as u32), (TOP, air() as u32)], "Bob vede sparire la pietra e la torcia");
    }

    #[test]
    fn placing_a_block_next_to_a_wall_reconnects_it() {
        let mut h = setup(GameMode::Creative);
        let wall = registry::blocks().default_hash("minecraft:cobblestone_wall").unwrap();
        h.server.set_blocks(Dim::Overworld, &[change(TOP, wall, false)]);
        drop(packets(&mut h.sent));
        // un blocco pieno accanto al muro: il muro si ricalcola e il client lo sa
        h.server.set_blocks(Dim::Overworld, &[change([4, 11, 4], stone(), false)]);
        let state = registry::blocks().state(h.server.block_at(Dim::Overworld, TOP).unwrap().0).unwrap();
        assert_eq!(state.get("wall_connection_type_east"), Some(&registry::StateValue::Str("short".into())));
        let wall_update = updates(&mut h.sent).into_iter().filter(|(to, pos, ..)| *to == 1 && *pos == TOP).count();
        assert_eq!(wall_update, 2, "Alice ha ricevuto il muro ricalcolato (strato 0 e 1)");
    }

    #[test]
    fn placed_blocks_follow_the_players_yaw() {
        let mut h = setup(GameMode::Creative);
        // una scala in mano al posto della pietra, giocatore che guarda a nord
        let stairs = registry::creative().1.iter().position(|c| c.name == "minecraft:oak_stairs").unwrap() as u32 + 1;
        h.receive(1, &[stack_request_packet(|w| craft_and_place(w, -2, stairs, 1))]);
        h.receive(1, &[mob_equipment(1, 1)]);
        h.server.players.get_mut(&1).unwrap().yaw = 180.0;
        h.receive(1, &[place_packet_slot(ROCK, 1, &held(&h), 1)]);
        let (hash, _) = h.server.block_at(Dim::Overworld, TOP).unwrap();
        let state = registry::blocks().state(hash).unwrap();
        assert_eq!(state.name, "minecraft:oak_stairs");
        assert_eq!(state.get("weirdo_direction"), Some(&registry::StateValue::Int(3)));
    }

    /// Le celle (strato 0) che `pid` ha ricevuto in `UpdateBlock`, con l'hash.
    fn cells_for(h: &mut Harness, pid: u64) -> Vec<([i32; 3], u32)> {
        updates(&mut h.sent).into_iter().filter(|u| u.0 == pid && u.4 == 0).map(|u| (u.1, u.2)).collect()
    }

    #[test]
    fn a_refused_door_resends_the_cell_above_to_the_author() {
        let mut h = setup(GameMode::Creative);
        hold_creative(&mut h, "minecraft:wooden_door", 0);
        h.server.players.get_mut(&1).unwrap().pos = [40.5, 11.62, 4.5]; // fuori portata: rifiutato
        drop(packets(&mut h.sent));
        h.receive(1, &[place_packet_slot(ROCK, 1, &held(&h), 1)]);
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)));
        let all = updates(&mut h.sent);
        assert!(all.iter().all(|u| u.0 == 1), "solo all'autore");
        let got: Vec<_> = all.iter().filter(|u| u.4 == 0).map(|u| (u.1, u.2)).collect();
        // il client ha previsto la porta in TOP e nella cella sopra: tutte e due tornano come sono davvero
        assert!(got.contains(&(TOP, air() as u32)), "{got:?}");
        assert!(got.contains(&([3, 12, 4], air() as u32)), "la metà alta prevista dal client: {got:?}");
    }

    #[test]
    fn merged_half_slabs_resend_the_cell_the_client_predicted() {
        let mut h = setup(GameMode::Creative);
        let slab = registry::blocks().default_hash("minecraft:oak_slab").unwrap();
        h.server.set_blocks(Dim::Overworld, &[change(ROCK, slab, false)]);
        hold_creative(&mut h, "minecraft:oak_slab", 0);
        drop(packets(&mut h.sent));
        // il client prevede una mezza lastra in TOP; il server unisce le due in una lastra doppia in ROCK
        h.receive(1, &[place_packet_slot(ROCK, 1, &held(&h), 1)]);
        let name = |pos| registry::blocks().state(h.server.block_at(Dim::Overworld, pos).unwrap().0).unwrap().name.clone();
        assert_eq!((name(ROCK).as_str(), name(TOP).as_str()), ("minecraft:oak_double_slab", "minecraft:air"));
        let got = cells_for(&mut h, 1);
        assert!(got.contains(&(TOP, air() as u32)), "la cella prevista dal client torna aria: {got:?}");
    }

    /// Un `MobEquipment` con quella finestra.
    fn equipment_in_window(slot: u8, container_id: u8) -> Packet {
        MobEquipment { target_runtime_id: 1, item: ItemStack::air(), slot, selected_slot: slot, container_id }.packet()
    }

    #[test]
    fn use_item_adopts_the_hotbar_slot_the_client_declares() {
        let mut h = setup(GameMode::Creative);
        hold_creative(&mut h, "minecraft:oak_stairs", 0); // casella 1
        h.receive(1, &[mob_equipment(0, 0)]);
        assert_eq!(h.server.players[&1].inventory.selected, 0);
        // il client ha già in mano la casella 1 (il MobEquipment è in ritardo o perso): vale ciò che dichiara l'uso
        let stairs = h.server.players[&1].inventory.main(1).stack(1);
        h.receive(1, &[place_packet_slot(ROCK, 1, &stairs, 1)]);
        assert_eq!(h.server.players[&1].inventory.selected, 1);
        let (hash, _) = h.server.block_at(Dim::Overworld, TOP).unwrap();
        assert_eq!(registry::blocks().state(hash).unwrap().name, "minecraft:oak_stairs");
        // una casella fuori dalla barra non cambia nulla
        h.receive(1, &[place_packet_slot(ROCK, 1, &stairs, 9)]);
        h.receive(1, &[place_packet_slot(ROCK, 1, &stairs, -1)]);
        assert_eq!(h.server.players[&1].inventory.selected, 1);
    }

    #[test]
    fn equipment_of_the_offhand_window_does_not_move_the_selection() {
        let mut h = setup(GameMode::Creative);
        h.receive(1, &[equipment_in_window(3, crate::inventory::window::OFFHAND as u8)]);
        assert_eq!(h.server.players[&1].inventory.selected, 0, "la finestra 119 è la mano secondaria");
        h.receive(1, &[equipment_in_window(3, crate::inventory::window::ARMOR as u8)]);
        assert_eq!(h.server.players[&1].inventory.selected, 0);
        h.receive(1, &[equipment_in_window(3, crate::inventory::window::INVENTORY as u8)]);
        assert_eq!(h.server.players[&1].inventory.selected, 3, "la finestra dell'inventario sì");
    }

    #[test]
    fn a_mismatched_hand_resends_the_slot_and_the_selection() {
        use bedrock_protocol::generated::PlayerHotbar;
        let mut h = setup(GameMode::Creative);
        // il client crede di avere la mano vuota, il server ha 64 pietre
        h.receive(1, &[place_packet(ROCK, 1, &ItemStack::air())]);
        assert_eq!(h.server.block_at(Dim::Overworld, TOP), Some((air(), false)));
        let out = packets(&mut h.sent);
        let slots: Vec<_> = out.iter().filter(|(_, p)| p.id == InventorySlot::ID).collect();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].0, 1, "solo all'autore");
        assert_eq!(slots[0].1.body, h.server.players[&1].inventory.held_slot_packet().body, "la casella com'è per il server");
        let hotbar: Vec<_> = out.iter().filter(|(_, p)| p.id == PlayerHotbar::ID).collect();
        assert_eq!(hotbar.len(), 1);
        assert_eq!((hotbar[0].0, hotbar[0].1.body.as_slice()), (1, [0u8, 0, 1].as_slice()), "casella 0, finestra 0, selezionala");
    }
}

// ---------- interazioni col tasto destro ----------

mod interacting {
    use super::blocks::{air, change, stone, updates};
    use super::placing::{counting_hook, destroy, held, hold_creative, packets, place_packet, place_packet_slot, setup, TOP};
    use super::*;
    use crate::{
        events::WorldAction,
        registry::{blocks, StateValue},
    };
    use bedrock_protocol::generated::{LevelSoundEvent, UpdateBlock};
    use vgen::Dim;

    /// La metà alta della porta sta sopra `TOP`.
    const UPPER: [i32; 3] = [3, 12, 4];
    /// Il pulsante sta sulla faccia est della pietra (che lo regge).
    const BUTTON: [i32; 3] = [4, 10, 4];
    const EAST: u8 = 5;
    /// La cella accanto alla porta, dove va una pietra piazzata contro di lei.
    const BESIDE: [i32; 3] = [4, 11, 4];

    fn door(name: &str, upper: bool, open: bool) -> i32 {
        let base = blocks().state(blocks().default_hash(name).unwrap()).unwrap();
        base.with("upper_block_bit", StateValue::Byte(upper as u8)).with("open_bit", StateValue::Byte(open as u8)).hash()
    }

    fn put_door(h: &mut Harness, name: &str) {
        h.server.set_blocks(Dim::Overworld, &[change(TOP, door(name, false, false), false), change(UPPER, door(name, true, false), false)]);
        assert_eq!(h.server.block_at(Dim::Overworld, UPPER), Some((door(name, true, false), false)), "la porta regge sulla pietra");
    }

    /// Mette il pulsante e restituisce il suo stato su e quello premuto.
    fn put_button(h: &mut Harness, name: &str) -> (i32, i32) {
        let base = blocks().state(blocks().default_hash(name).unwrap()).unwrap().with("facing_direction", StateValue::Int(EAST as i32));
        let (up, down) = (base.with("button_pressed_bit", StateValue::Byte(0)).hash(), base.with("button_pressed_bit", StateValue::Byte(1)).hash());
        h.server.set_blocks(Dim::Overworld, &[change(BUTTON, up, false)]);
        assert_eq!(h.server.block_at(Dim::Overworld, BUTTON), Some((up, false)), "il pulsante regge sulla pietra");
        (up, down)
    }

    fn at(h: &Harness, pos: [i32; 3]) -> i32 {
        h.server.block_at(Dim::Overworld, pos).unwrap().0
    }

    /// Alice, con la mano vuota (la casella 5 non ha nulla), clicca la faccia est di `pos`.
    fn click_with_empty_hand(h: &mut Harness, pos: [i32; 3]) {
        h.receive(1, &[mob_equipment(5, 5)]);
        h.receive(1, &[place_packet_slot(pos, EAST, &ItemStack::air(), 5)]);
    }

    /// I suoni arrivati: (destinatario, nome, dato).
    fn sounds(out: &[(u64, Packet)]) -> Vec<(u64, String, i32)> {
        out.iter()
            .filter(|(_, p)| p.id == LevelSoundEvent::ID)
            .map(|(to, p)| {
                let r = &mut Reader(&p.body);
                let name = r.string().unwrap().to_string();
                r.vec3().unwrap();
                (*to, name, r.var_i32().unwrap())
            })
            .collect()
    }

    fn updates_in(out: &[(u64, Packet)]) -> Vec<(u64, [i32; 3])> {
        out.iter().filter(|(_, p)| p.id == UpdateBlock::ID).map(|(to, p)| (*to, Reader(&p.body).block_pos().unwrap())).collect()
    }

    #[test]
    fn right_click_opens_the_door_with_a_block_in_hand_and_tells_the_viewers() {
        let mut h = setup(GameMode::Creative);
        put_door(&mut h, "minecraft:wooden_door");
        let seen = counting_hook(&mut h, true);
        drop(packets(&mut h.sent));
        let in_hand = held(&h);
        h.receive(1, &[place_packet(TOP, EAST, &in_hand)]);
        let open = door("minecraft:wooden_door", false, true);
        assert_eq!((at(&h, TOP), at(&h, UPPER)), (open, door("minecraft:wooden_door", true, true)));
        assert_eq!(at(&h, BESIDE), air(), "nessuna pietra piazzata accanto");
        assert!(matches!(seen.lock().unwrap().as_slice(), [WorldAction::Interact { pid: 1, changes }] if changes.len() == 2));
        assert!(h.server.chunks[&(0, 0, 0)].dirty, "il chunk si salva");
        let out = packets(&mut h.sent);
        let mut cells = updates_in(&out);
        cells.sort();
        assert_eq!(cells, [(1, TOP), (1, TOP), (1, UPPER), (1, UPPER), (2, TOP), (2, TOP), (2, UPPER), (2, UPPER)], "Alice e Bob, non Carol");
        let mut heard = sounds(&out);
        heard.sort();
        assert_eq!(heard, [(1, "door.open".to_string(), open), (2, "door.open".to_string(), open)]);
        assert_eq!(h.server.players[&1].inventory.held().count, 64, "non consuma nulla");
        // di nuovo, dalla metà alta: si chiude
        h.receive(1, &[place_packet(UPPER, EAST, &in_hand)]);
        assert_eq!((at(&h, TOP), at(&h, UPPER)), (door("minecraft:wooden_door", false, false), door("minecraft:wooden_door", true, false)));
        assert!(sounds(&packets(&mut h.sent)).iter().all(|(_, name, _)| name == "door.close"));
    }

    #[test]
    fn interactions_work_whatever_the_client_thinks_is_in_hand() {
        let mut h = setup(GameMode::Creative);
        put_door(&mut h, "minecraft:wooden_door");
        // il server ha 64 pietre in mano, il client dice di avere la mano vuota (o un'altra cosa): la porta si apre lo stesso
        h.receive(1, &[place_packet(TOP, EAST, &ItemStack::air())]);
        assert_eq!(at(&h, TOP), door("minecraft:wooden_door", false, true));
        let mut other = held(&h);
        other.id = 9999;
        h.receive(1, &[place_packet(TOP, EAST, &other)]);
        assert_eq!(at(&h, TOP), door("minecraft:wooden_door", false, false));
        assert_eq!(at(&h, BESIDE), air(), "e non piazza nulla");
    }

    #[test]
    fn a_refused_break_of_half_a_door_resends_the_other_half() {
        let mut h = setup(GameMode::Survival);
        put_door(&mut h, "minecraft:wooden_door");
        drop(packets(&mut h.sent));
        h.receive(1, &[destroy(TOP)]); // la rottura creativa non è ammessa in sopravvivenza: rifiutata
        assert_eq!(at(&h, TOP), door("minecraft:wooden_door", false, false));
        let got = updates(&mut h.sent);
        assert!(got.iter().all(|u| u.0 == 1), "solo all'autore");
        let cells: Vec<_> = got.iter().filter(|u| u.4 == 0).map(|u| (u.1, u.2)).collect();
        assert!(cells.contains(&(UPPER, door("minecraft:wooden_door", true, false) as u32)), "anche l'altra metà: {cells:?}");
    }

    #[test]
    fn sneaking_with_block_places_instead_of_opening() {
        let mut h = setup(GameMode::Creative);
        put_door(&mut h, "minecraft:wooden_door");
        h.server.players.get_mut(&1).unwrap().sneaking = true;
        h.receive(1, &[place_packet(TOP, EAST, &held(&h))]);
        assert_eq!(at(&h, TOP), door("minecraft:wooden_door", false, false), "la porta resta chiusa");
        assert_eq!(at(&h, BESIDE), stone(), "la pietra va accanto alla porta");
        // accovacciato con la mano vuota si apre lo stesso
        click_with_empty_hand(&mut h, TOP);
        assert_eq!(at(&h, TOP), door("minecraft:wooden_door", false, true));
    }

    #[test]
    fn iron_door_is_placed_against_not_opened() {
        let mut h = setup(GameMode::Creative);
        put_door(&mut h, "minecraft:iron_door");
        h.receive(1, &[place_packet(TOP, EAST, &held(&h))]);
        assert_eq!(at(&h, TOP), door("minecraft:iron_door", false, false));
        assert_eq!(at(&h, BESIDE), stone());
        // con la mano vuota il clic non fa nulla e non manda nulla
        drop(packets(&mut h.sent));
        click_with_empty_hand(&mut h, TOP);
        assert_eq!(at(&h, TOP), door("minecraft:iron_door", false, false));
        assert!(packets(&mut h.sent).is_empty());
    }

    #[test]
    fn creative_survival_and_adventure_use_doors_but_spectators_do_not() {
        for (mode, works) in [(GameMode::Creative, true), (GameMode::Survival, true), (GameMode::Adventure, true), (GameMode::Spectator, false)] {
            let mut h = setup(mode);
            put_door(&mut h, "minecraft:wooden_door");
            drop(packets(&mut h.sent));
            h.receive(1, &[place_packet(TOP, EAST, &held(&h))]);
            let opened = at(&h, TOP) == door("minecraft:wooden_door", false, true);
            assert_eq!(opened, works, "{mode:?}");
            if !works {
                // lo stato vero va al solo autore, per tutte e due le metà, e la pietra non si piazza al posto della porta
                let mut cells: Vec<_> = updates(&mut h.sent).iter().filter(|u| u.4 == 0).map(|u| (u.0, u.1, u.2)).collect();
                cells.sort();
                let (low, high) = (door("minecraft:wooden_door", false, false) as u32, door("minecraft:wooden_door", true, false) as u32);
                assert_eq!(cells, [(1, TOP, low), (1, UPPER, high)]);
                assert_eq!(at(&h, BESIDE), air());
            }
            assert_eq!(h.server.players[&1].inventory.held().count, 64, "{mode:?}: una porta non consuma l'oggetto");
        }
    }

    #[test]
    fn out_of_reach_and_vetoed_interactions_are_reverted() {
        let mut h = setup(GameMode::Creative);
        put_door(&mut h, "minecraft:wooden_door");
        h.server.players.get_mut(&1).unwrap().pos = [40.5, 11.62, 4.5];
        drop(packets(&mut h.sent));
        h.receive(1, &[place_packet(TOP, EAST, &held(&h))]);
        assert_eq!(at(&h, TOP), door("minecraft:wooden_door", false, false));
        let got = updates(&mut h.sent);
        assert!(!got.is_empty() && got.iter().all(|u| u.0 == 1), "lo stato vero va ad Alice sola");
        // sopravvivenza: la portata è 8
        let mut h = setup(GameMode::Survival);
        put_door(&mut h, "minecraft:wooden_door");
        h.server.players.get_mut(&1).unwrap().pos = [12.5, 11.62, 4.5];
        h.receive(1, &[place_packet(TOP, EAST, &held(&h))]);
        assert_eq!(at(&h, TOP), door("minecraft:wooden_door", false, false));
        // un gancio che annulla
        let mut h = setup(GameMode::Creative);
        put_door(&mut h, "minecraft:wooden_door");
        let seen = counting_hook(&mut h, false);
        drop(packets(&mut h.sent));
        h.receive(1, &[place_packet(TOP, EAST, &held(&h))]);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(at(&h, TOP), door("minecraft:wooden_door", false, false));
        assert_eq!(at(&h, BESIDE), air(), "e non piazza al suo posto");
        let got = updates(&mut h.sent);
        assert!(!got.is_empty() && got.iter().all(|u| u.0 == 1), "lo stato vero va ad Alice sola");
    }

    #[test]
    fn lever_works_through_the_packet_even_with_a_door_in_hand() {
        let mut h = setup(GameMode::Creative);
        let lever = blocks().state(blocks().default_hash("minecraft:lever").unwrap()).unwrap().with("lever_direction", StateValue::Str("east".into()));
        h.server.set_blocks(Dim::Overworld, &[change(BUTTON, lever.hash(), false)]);
        assert_eq!(at(&h, BUTTON), lever.hash(), "la leva regge sulla pietra");
        drop(packets(&mut h.sent));
        h.receive(1, &[place_packet(BUTTON, EAST, &held(&h))]);
        let on = lever.with("open_bit", StateValue::Byte(1)).hash();
        assert_eq!(at(&h, BUTTON), on);
        assert_eq!(sounds(&packets(&mut h.sent)).iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), ["power.on", "power.on"]);
        h.receive(1, &[place_packet(BUTTON, EAST, &held(&h))]);
        assert_eq!(at(&h, BUTTON), lever.hash());
        // una porta in mano (che non è un blocco già pronto): la leva si usa comunque
        hold_creative(&mut h, "minecraft:wooden_door", 0);
        h.receive(1, &[place_packet_slot(BUTTON, EAST, &held(&h), 1)]);
        assert_eq!(at(&h, BUTTON), on);
    }

    /// Alice preme il pulsante con la mano vuota; restituisce lo stato su e quello premuto.
    fn press(h: &mut Harness, name: &str) -> (i32, i32) {
        let (up, down) = put_button(h, name);
        click_with_empty_hand(h, BUTTON);
        assert_eq!(at(h, BUTTON), down, "{name} premuto");
        (up, down)
    }

    fn assert_resets_after(name: &str, ticks: u64) {
        let mut h = setup(GameMode::Creative);
        let (up, down) = press(&mut h, name);
        drop(packets(&mut h.sent));
        for n in 1..ticks {
            h.server.tick();
            assert_eq!(at(&h, BUTTON), down, "{name}: ancora premuto dopo {n} tick");
        }
        h.server.tick();
        assert_eq!(at(&h, BUTTON), up, "{name}: su dopo {ticks} tick");
        let out = packets(&mut h.sent);
        assert!(updates_in(&out).contains(&(2, BUTTON)), "Bob vede il pulsante risalire");
        assert_eq!(sounds(&out).first().map(|s| s.1.as_str()), Some("button.click_off"));
        assert!(h.server.scheduled.is_empty(), "appuntamento consumato");
        assert!(h.server.chunks[&(0, 0, 0)].dirty);
    }

    #[test]
    fn wooden_button_resets_after_30_ticks() {
        assert_resets_after("minecraft:wooden_button", 30);
        assert_resets_after("minecraft:birch_button", 30);
    }

    #[test]
    fn stone_button_resets_after_20_ticks() {
        assert_resets_after("minecraft:stone_button", 20);
        assert_resets_after("minecraft:polished_blackstone_button", 20);
    }

    #[test]
    fn pressing_a_pressed_button_does_nothing_and_keeps_one_reset() {
        let mut h = setup(GameMode::Creative);
        let (up, down) = press(&mut h, "minecraft:stone_button");
        for _ in 0..10 {
            h.server.tick();
        }
        drop(packets(&mut h.sent));
        click_with_empty_hand(&mut h, BUTTON);
        assert!(packets(&mut h.sent).is_empty(), "nessun cambio, nessun suono");
        assert_eq!(h.server.scheduled.len(), 1);
        for _ in 0..9 {
            h.server.tick();
        }
        assert_eq!(at(&h, BUTTON), down);
        h.server.tick();
        assert_eq!(at(&h, BUTTON), up, "risale 20 tick dopo la prima pressione, non dopo il secondo clic");
    }

    #[test]
    fn a_new_press_on_a_replaced_button_replaces_the_old_appointment() {
        let mut h = setup(GameMode::Creative);
        press(&mut h, "minecraft:stone_button"); // appuntamento a 20 tick
        for _ in 0..5 {
            h.server.tick();
        }
        // il pulsante di pietra si rompe e al suo posto ne va uno di legno, che si preme: vale il nuovo appuntamento
        let (up, down) = press(&mut h, "minecraft:wooden_button");
        assert_eq!(h.server.scheduled.len(), 1, "una sola voce per cella");
        for n in 1..30 {
            h.server.tick();
            assert_eq!(at(&h, BUTTON), down, "ancora premuto dopo {n} tick");
        }
        h.server.tick();
        assert_eq!(at(&h, BUTTON), up, "risale dopo 30 tick dalla nuova pressione");
    }

    #[test]
    fn button_reset_skipped_if_block_changed() {
        let mut h = setup(GameMode::Creative);
        press(&mut h, "minecraft:wooden_button");
        h.server.set_blocks(Dim::Overworld, &[change(BUTTON, stone(), false)]);
        drop(packets(&mut h.sent));
        for _ in 0..40 {
            h.server.tick();
        }
        assert_eq!(at(&h, BUTTON), stone(), "la pietra non è toccata");
        assert!(h.server.scheduled.is_empty());
        assert!(updates_in(&packets(&mut h.sent)).is_empty(), "nessun aggiornamento");
    }

    #[test]
    fn button_reset_waits_for_an_unloaded_chunk() {
        let mut h = setup(GameMode::Creative);
        let (up, _) = press(&mut h, "minecraft:stone_button");
        let chunk = h.server.chunks.remove(&(0, 0, 0)).unwrap();
        // senza `tick()`, che rigenererebbe il chunk mancante: il tempo passa e si guardano gli appuntamenti
        h.server.tick_count += 30;
        h.server.run_scheduled();
        assert_eq!(h.server.scheduled.len(), 1, "scaduto ma il chunk non c'è: aspetta");
        h.server.chunks.insert((0, 0, 0), chunk);
        h.server.run_scheduled();
        assert_eq!((at(&h, BUTTON), h.server.scheduled.len()), (up, 0));
    }

    #[test]
    fn a_pressed_button_without_an_appointment_gets_one_when_clicked() {
        // un pulsante salvato premuto: al riavvio nessuno lo farebbe risalire
        let mut h = setup(GameMode::Creative);
        let (up, down) = put_button(&mut h, "minecraft:wooden_button");
        h.server.set_blocks(Dim::Overworld, &[change(BUTTON, down, false)]);
        assert!(h.server.scheduled.is_empty());
        click_with_empty_hand(&mut h, BUTTON);
        assert_eq!(h.server.scheduled.len(), 1);
        for _ in 0..30 {
            h.server.tick();
        }
        assert_eq!(at(&h, BUTTON), up);
    }
}

// ---------- /gamemode ----------

/// Modalità di gioco e valori delle abilità dell'ultimo `SetPlayerGameType`/`UpdateAbilities` ricevuti da `pid`.
fn mode_and_abilities(h: &mut Harness, pid: u64) -> (Vec<u8>, u32) {
    let packets = h.packets_to(pid);
    let game_type = packets.iter().find(|p| p.id == SetPlayerGameType::ID).expect("SetPlayerGameType").body.clone();
    let r = &mut Reader(&packets.iter().find(|p| p.id == UpdateAbilities::ID).expect("UpdateAbilities").body);
    assert_eq!((r.i64().unwrap(), r.i8().unwrap(), r.u8().unwrap(), r.var_u32().unwrap(), r.u16().unwrap()), (pid as i64, 1, 0, 1, 1));
    let _set = r.u32().unwrap();
    (game_type, r.u32().unwrap())
}

#[test]
fn gamemode_survival_updates_client() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.packets_to(1);
    h.receive(1, &[command("/gamemode survival")]);
    assert_eq!(h.server.players[&1].mode, GameMode::Survival);
    let (game_type, values) = mode_and_abilities(&mut h, 1);
    assert_eq!(game_type, [0], "sopravvivenza = 0");
    assert_eq!(values & ((1 << 10) | (1 << 11)), 0, "niente volo né costruzione istantanea");
}

#[test]
fn gamemode_accepts_names_numbers_and_short_forms() {
    let cases = [
        ("creative", GameMode::Creative),
        ("c", GameMode::Creative),
        ("1", GameMode::Creative),
        ("survival", GameMode::Survival),
        ("s", GameMode::Survival),
        ("0", GameMode::Survival),
        ("adventure", GameMode::Adventure),
        ("a", GameMode::Adventure),
        ("2", GameMode::Adventure),
        ("spectator", GameMode::Spectator),
        ("sp", GameMode::Spectator),
        ("6", GameMode::Spectator),
        ("CREATIVE", GameMode::Creative),
    ];
    for (arg, expected) in cases {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.server.players.get_mut(&1).unwrap().mode = if expected == GameMode::Survival { GameMode::Creative } else { GameMode::Survival };
        h.receive(1, &[command(&format!("/gamemode {arg}"))]);
        assert_eq!(h.server.players[&1].mode, expected, "/gamemode {arg}");
    }
}

#[test]
fn gamemode_unknown_shows_usage() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.texts_to(1);
    for line in ["/gamemode", "/gamemode pizza", "/gamemode 3"] {
        h.receive(1, &[command(line)]);
        let texts = h.texts_to(1);
        assert!(texts.iter().any(|t| t.message.contains("Uso: /gamemode")), "{line}");
        assert_eq!(h.server.players[&1].mode, GameMode::Creative, "{line}");
    }
}

#[test]
fn gamemode_is_listed_in_help() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.texts_to(1);
    h.receive(1, &[command("/help")]);
    assert!(h.texts_to(1).iter().any(|t| t.message.contains("/gamemode")));
}

#[test]
fn client_set_player_game_type_gets_true_mode_back() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    h.packets_to(1);
    let mut w = Writer::default();
    w.var_i32(0); // il client vorrebbe passare a sopravvivenza dal menu
    h.receive(1, &[Packet::new(id::SET_PLAYER_GAME_TYPE, w.0)]);
    assert_eq!(h.server.players[&1].mode, GameMode::Creative, "il server non cambia modalità su richiesta del client");
    let packets = h.packets_to(1);
    let reply = packets.iter().find(|p| p.id == id::UPDATE_PLAYER_GAME_TYPE).expect("UpdatePlayerGameType");
    let r = &mut Reader(&reply.body);
    assert_eq!((r.var_i32().unwrap(), r.var_i64().unwrap()), (1, 1));
    // pacchetto illeggibile: ignorato
    h.receive(1, &[Packet::new(id::SET_PLAYER_GAME_TYPE, vec![])]);
    assert!(h.packets_to(1).is_empty());
}

#[test]
fn spectator_has_no_creative_inventory() {
    assert!(GameMode::Creative.creative_inventory());
    assert!(!GameMode::Spectator.creative_inventory());
}

mod entities {
    use super::*;
    use crate::entity::{Entity, EntityKind, FIRST_ENTITY_ID};

    fn item_at(h: &mut Harness, pos: [f32; 3]) -> u64 {
        let id = h.server.entities.alloc();
        let item = crate::inventory::Item { id: registry::items().id("minecraft:stone").unwrap(), count: 1, ..Default::default() };
        h.server.entities.insert(Entity {
            id,
            kind: EntityKind::Item { item, pickup_at: 0, despawn_at: u64::MAX, velocity: [0.0; 3] },
            pos,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            on_ground: true,
            moved: false,
        });
        id
    }

    fn count(packets: &[Packet], packet_id: u32) -> usize {
        packets.iter().filter(|p| p.id == packet_id).count()
    }

    fn joined() -> Harness {
        let mut h = Harness::new();
        h.join(1, "Alex");
        h.server.players.get_mut(&1).unwrap().sent.insert((0, 0));
        h.packets_to(1);
        h
    }

    #[test]
    fn entity_in_a_sent_chunk_appears_once() {
        let mut h = joined();
        item_at(&mut h, [3.0, 10.0, 4.0]);
        h.server.visibility_tick();
        assert_eq!(count(&h.packets_to(1), id::ADD_ITEM_ACTOR), 1);
        h.server.visibility_tick();
        assert_eq!(count(&h.packets_to(1), id::ADD_ITEM_ACTOR), 0);
    }

    #[test]
    fn entity_leaving_view_is_removed() {
        let mut h = joined();
        let eid = item_at(&mut h, [3.0, 10.0, 4.0]);
        h.server.visibility_tick();
        h.packets_to(1);
        h.server.players.get_mut(&1).unwrap().sent.clear();
        h.server.visibility_tick();
        let out = h.packets_to(1);
        assert_eq!(count(&out, id::REMOVE_ACTOR), 1);
        assert!(h.server.players[&1].visible.is_empty());
        // Un oggetto in un chunk non inviato non si mostra.
        item_at(&mut h, [100.0, 10.0, 100.0]);
        h.server.visibility_tick();
        assert!(h.packets_to(1).is_empty());
        // despawn manda RemoveActor a chi lo vede.
        h.server.players.get_mut(&1).unwrap().sent.insert((0, 0));
        h.server.visibility_tick();
        h.packets_to(1);
        h.server.despawn(eid);
        assert_eq!(count(&h.packets_to(1), id::REMOVE_ACTOR), 1);
        assert!(h.server.entities.get(eid).is_none());
    }

    #[test]
    fn players_never_see_themselves() {
        let mut h = joined();
        h.server.entities.insert(Entity {
            id: 1,
            kind: EntityKind::Player { pid: Some(1), name: "Alex".into(), uuid: [1; 16] },
            pos: [3.0, 11.62, 4.0],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            on_ground: true,
            moved: false,
        });
        h.server.visibility_tick();
        assert!(h.packets_to(1).is_empty());
        assert!(h.server.players[&1].visible.is_empty());
    }

    #[test]
    fn other_players_are_spawned_with_add_player() {
        let mut h = joined();
        h.server.entities.insert(Entity {
            id: 7,
            kind: EntityKind::Player { pid: None, name: "Bot".into(), uuid: [2; 16] },
            pos: [3.0, 11.62, 4.0],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            on_ground: true,
            moved: false,
        });
        h.server.visibility_tick();
        assert_eq!(count(&h.packets_to(1), id::ADD_PLAYER), 1);
    }

    #[test]
    fn ids_of_non_players_start_at_2_pow_32() {
        let mut h = Harness::new();
        let a = h.server.entities.alloc();
        let b = h.server.entities.alloc();
        assert_eq!(a, FIRST_ENTITY_ID);
        assert_eq!(b, FIRST_ENTITY_ID + 1);
    }
}

mod player_list {
    use super::*;
    use crate::{entity::EntityKind, playerdata};
    use bedrock_protocol::{
        entity::{player_list_add, player_list_remove, PlayerListEntry},
        skin::uuid_bytes,
    };

    fn count(packets: &[Packet], packet_id: u32) -> usize {
        packets.iter().filter(|p| p.id == packet_id).count()
    }

    fn uuid_of(h: &Harness, pid: u64) -> [u8; 16] {
        let info = h.server.players[&pid].info.as_ref().unwrap();
        uuid_bytes(info, &playerdata::identity(info))
    }

    /// La voce di `pid` come la deve scrivere il server.
    fn expected_add(h: &Harness, pids: &[u64]) -> Packet {
        let entries: Vec<PlayerListEntry> = pids
            .iter()
            .map(|&pid| {
                let info = h.server.players[&pid].info.as_ref().unwrap();
                PlayerListEntry {
                    uuid: uuid_of(h, pid),
                    unique_id: pid as i64,
                    name: info.name.clone(),
                    xuid: info.xuid.clone().unwrap_or_default(),
                    platform_chat_id: String::new(),
                    build_platform: info.device_os.map_or(-1, |d| d as i32),
                    skin: &info.skin,
                    teacher: false,
                    host: false,
                    sub_client: false,
                }
            })
            .collect();
        player_list_add(&entries)
    }

    fn only(packets: Vec<Packet>, packet_id: u32) -> Packet {
        let mut v: Vec<Packet> = packets.into_iter().filter(|p| p.id == packet_id).collect();
        assert_eq!(v.len(), 1, "un solo pacchetto {packet_id}");
        v.remove(0)
    }

    #[test]
    fn joining_player_gets_the_full_list_and_others_get_him() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.packets_to(1);
        h.join(2, "Bob");
        let mut sent = h.packets_by_player();
        let to_bob = only(sent.remove(&2).unwrap(), id::PLAYER_LIST);
        assert_eq!(to_bob.body, expected_add(&h, &[1, 2]).body, "Bob riceve Alice e Bob");
        let to_alice = only(sent.remove(&1).unwrap(), id::PLAYER_LIST);
        assert_eq!(to_alice.body, expected_add(&h, &[2]).body, "Alice riceve solo Bob");
        let e = h.server.entities.get(2).expect("Bob è un'entità");
        assert!(matches!(&e.kind, EntityKind::Player { pid: Some(2), name, uuid } if name == "Bob" && *uuid == uuid_of(&h, 2)));
    }

    #[test]
    fn quitting_player_disappears_for_everyone() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.join(2, "Bob");
        for pid in [1, 2] {
            h.server.players.get_mut(&pid).unwrap().sent.insert((0, 0));
        }
        h.server.visibility_tick();
        h.packets_by_player();
        let bob = uuid_of(&h, 2);
        h.server.close(2);
        let to_alice = h.packets_to(1);
        assert_eq!(count(&to_alice, id::REMOVE_ACTOR), 1, "Alice vedeva Bob");
        assert_eq!(only(to_alice, id::PLAYER_LIST).body, player_list_remove(&[bob]).body);
        assert!(h.server.entities.get(2).is_none(), "Bob non resta nel registro");
        assert!(h.server.players[&1].visible.is_empty());
    }

    #[test]
    fn quitting_player_nobody_saw_only_leaves_the_list() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.join(2, "Bob");
        h.packets_to(1);
        h.server.close(2);
        let to_alice = h.packets_to(1);
        assert_eq!(count(&to_alice, id::REMOVE_ACTOR), 0);
        assert_eq!(count(&to_alice, id::PLAYER_LIST), 1);
    }

    #[test]
    fn two_players_in_the_same_chunks_see_each_other() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.join(2, "Bob");
        for pid in [1, 2] {
            h.server.players.get_mut(&pid).unwrap().sent.insert((0, 0));
        }
        h.packets_by_player();
        h.server.visibility_tick();
        let sent = h.packets_by_player();
        for pid in [1, 2] {
            assert_eq!(count(&sent[&pid], id::ADD_PLAYER), 1, "#{pid} vede l'altro e non se stesso");
        }
        h.server.visibility_tick();
        assert_eq!(count(&h.packets_to(1), id::ADD_PLAYER), 0);
    }

    #[test]
    fn entity_position_follows_the_player() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.server.on_move(1, [40.5, 12.62, -3.5]);
        assert_eq!(h.server.entities.get(1).unwrap().pos, [40.5, 12.62, -3.5]);
    }

    /// Alice, Bob e Carol: Alice e Bob nel chunk (0, 0), Carol senza chunk inviati. Dopo la visibilità si svuotano i pacchetti.
    fn three_players() -> Harness {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.join(2, "Bob");
        h.join(3, "Carol");
        for pid in [1, 2] {
            h.server.players.get_mut(&pid).unwrap().sent.insert((0, 0));
        }
        h.server.visibility_tick();
        h.server.movement_tick();
        h.packets_by_player();
        h
    }

    fn pai_at(x: f32) -> Packet {
        Packet::new(id::PLAYER_AUTH_INPUT, auth_input_body(x, 70.0, 3.0))
    }

    #[test]
    fn movement_is_sent_once_per_tick_to_viewers_only() {
        let mut h = three_players();
        h.receive(1, &[pai_at(1.0), pai_at(2.0), pai_at(3.0)]);
        h.server.movement_tick();
        let sent = h.packets_by_player();
        let moves: Vec<&Packet> = sent[&2].iter().filter(|p| p.id == id::MOVE_ACTOR_ABSOLUTE).collect();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].body, bedrock_protocol::entity::move_actor_absolute(1, [3.0, 70.0, 3.0], 0.0, 0.0, 0.0, false, false).body);
        assert_eq!(count(sent.get(&1).map_or(&[][..], |v| v), id::MOVE_ACTOR_ABSOLUTE), 0, "Alice non vede se stessa");
        assert_eq!(count(sent.get(&3).map_or(&[][..], |v| v), id::MOVE_ACTOR_ABSOLUTE), 0, "Carol è fuori vista");
        h.server.movement_tick();
        assert!(h.packets_by_player().get(&2).is_none_or(|v| count(v, id::MOVE_ACTOR_ABSOLUTE) == 0), "il giro dopo non c'è più nulla");
    }

    #[test]
    fn unchanged_position_sends_nothing() {
        let mut h = three_players();
        h.receive(1, &[pai_at(1.0)]);
        h.server.movement_tick();
        h.packets_by_player();
        h.receive(1, &[pai_at(1.0)]);
        h.server.movement_tick();
        assert!(h.packets_by_player().get(&2).is_none_or(|v| count(v, id::MOVE_ACTOR_ABSOLUTE) == 0));
    }

    fn of_kind(packets: &[Packet], packet_id: u32) -> Vec<&Packet> {
        packets.iter().filter(|p| p.id == packet_id).collect()
    }

    #[test]
    fn swing_is_relayed_to_viewers() {
        let mut h = three_players();
        let mut w = Writer::default();
        w.u8(1);
        w.var_u64(1);
        w.f32(0.0);
        w.u8(0);
        w.u8(0);
        h.receive(1, &[Packet::new(id::ANIMATE, w.0)]);
        let sent = h.packets_by_player();
        let swings = of_kind(&sent[&2], id::ANIMATE);
        assert_eq!(swings.len(), 1);
        assert_eq!(swings[0].body, bedrock_protocol::entity::animate_swing(1).body);
        assert!(sent.get(&1).is_none_or(|v| count(v, id::ANIMATE) == 0), "Alice non rivede il proprio colpo");
        assert!(sent.get(&3).is_none_or(|v| count(v, id::ANIMATE) == 0), "Carol è fuori vista");
    }

    #[test]
    fn start_sneaking_sends_actor_data_with_the_flag() {
        use bedrock_protocol::actor::{flag, key, set_actor_data, DataValue};
        let mut h = three_players();
        h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(27))]);
        let sent = h.packets_by_player();
        let flags = (1i64 << flag::HAS_GRAVITY)
            | 1 << flag::HAS_COLLISION
            | 1 << flag::CAN_CLIMB
            | 1 << flag::BREATHING
            | 1 << flag::SNEAKING
            | 1 << flag::SHOW_NAME
            | 1 << flag::ALWAYS_SHOW_NAME;
        let expected = set_actor_data(
            1,
            &[
                (key::FLAGS, DataValue::Int64(flags)),
                (key::NAME, DataValue::Str("Alice".into())),
                (key::BOUNDING_BOX_WIDTH, DataValue::Float(0.6)),
                (key::BOUNDING_BOX_HEIGHT, DataValue::Float(1.5)),
                (key::ALWAYS_SHOW_NAMETAG, DataValue::Byte(1)),
            ],
        );
        for pid in [1, 2] {
            let data = of_kind(&sent[&pid], bedrock_protocol::actor::SET_ACTOR_DATA);
            assert_eq!(data.len(), 1, "#{pid}");
            assert_eq!(data[0].body, expected.body, "#{pid}");
        }
        assert!(sent.get(&3).is_none_or(|v| count(v, bedrock_protocol::actor::SET_ACTOR_DATA) == 0));
        // Lo stesso stato ripetuto non rimanda nulla; lo stop lo toglie.
        h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(27))]);
        assert!(h.packets_by_player().values().all(|v| count(v, bedrock_protocol::actor::SET_ACTOR_DATA) == 0));
        h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(28))]);
        assert_eq!(count(&h.packets_by_player()[&2], bedrock_protocol::actor::SET_ACTOR_DATA), 1);
        assert!(!h.server.players[&1].sneaking);
    }

    #[test]
    fn sprint_swim_and_glide_states_are_tracked() {
        use bedrock_protocol::actor::{flag, DataValue};
        let mut h = three_players();
        for (start, stop, bit) in [(25, 26, flag::SPRINTING), (29, 30, flag::SWIMMING), (32, 33, flag::GLIDING)] {
            h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(start))]);
            let DataValue::Int64(on) = h.server.entity_data(1)[0].1 else { panic!("flag") };
            assert_ne!(on & 1 << bit, 0, "bit {bit} acceso");
            h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(stop))]);
            let DataValue::Int64(off) = h.server.entity_data(1)[0].1 else { panic!("flag") };
            assert_eq!(off & 1 << bit, 0, "bit {bit} spento");
        }
        assert_eq!(count(&h.packets_by_player()[&2], bedrock_protocol::actor::SET_ACTOR_DATA), 6);
    }

    #[test]
    fn players_and_bots_carry_their_name_over_the_head() {
        use bedrock_protocol::actor::{flag, key, DataValue};
        let mut h = three_players();
        let data = h.server.entity_data(1);
        let get = |k: u32| data.iter().find(|(id, _)| *id == k).map(|(_, v)| v.clone());
        assert_eq!(get(key::NAME), Some(DataValue::Str("Alice".into())));
        assert_eq!(get(key::ALWAYS_SHOW_NAMETAG), Some(DataValue::Byte(1)));
        let Some(DataValue::Int64(flags)) = get(key::FLAGS) else { panic!("flag") };
        assert_ne!(flags & 1 << flag::SHOW_NAME, 0);
        assert_ne!(flags & 1 << flag::ALWAYS_SHOW_NAME, 0);
        // Anche AddPlayer dice il nome ai nuovi spettatori, e un bot lo ha come i giocatori.
        let bot = h.server.spawn_bot(1, "Robo");
        let data = h.server.entity_data(bot);
        assert!(data.iter().any(|(k, v)| *k == key::NAME && *v == DataValue::Str("Robo".into())));
    }

    #[test]
    fn add_player_of_a_bot_has_a_valid_build_platform() {
        // 0 non è una piattaforma: il client rispondeva "invalid enum value" su AddPlayer (12) e chiudeva.
        let mut h = three_players();
        let bot = h.server.spawn_bot(1, "Robo");
        let add = h.server.spawn_packets(h.server.entities.get(bot).unwrap()).remove(0);
        assert_eq!(add.id, id::ADD_PLAYER);
        assert_eq!(add.body[add.body.len() - 4..], (-1i32).to_le_bytes());
    }

    #[test]
    fn body_box_follows_the_state() {
        use bedrock_protocol::actor::{key, DataValue};
        let mut h = three_players();
        let height = |h: &Harness| match h.server.entity_data(1).iter().find(|(k, _)| *k == key::BOUNDING_BOX_HEIGHT) {
            Some((_, DataValue::Float(v))) => *v,
            _ => panic!("altezza"),
        };
        assert_eq!(height(&h), 1.8);
        for (start, stop, expected) in [(27, 28, 1.5), (29, 30, 0.6), (32, 33, 0.6)] {
            h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(start))]);
            assert_eq!(height(&h), expected, "stato {start}");
            // sia a sé sia agli spettatori arriva il riquadro dello stato
            let sent = h.packets_by_player();
            for pid in [1, 2] {
                let data = of_kind(&sent[&pid], bedrock_protocol::actor::SET_ACTOR_DATA);
                let want = bedrock_protocol::actor::set_actor_data(1, &h.server.entity_data(1));
                assert_eq!(data.last().map(|p| &p.body), Some(&want.body), "#{pid} stato {start}");
            }
            h.receive(1, &[Packet::new(id::PLAYER_AUTH_INPUT, pai_with_flag(stop))]);
            assert_eq!(height(&h), 1.8, "stop {stop}");
            h.packets_by_player();
        }
    }

    #[test]
    fn changing_held_slot_shows_the_item_to_viewers() {
        let mut h = three_players();
        h.receive(1, &[stack_request_packet(|w| craft_and_place(w, -1, stone_net_id(), 2))]);
        let sent = h.packets_by_player();
        assert_eq!(sent.get(&2).map_or(0, |v| count(v, id::MOB_EQUIPMENT)), 0, "la casella in mano non è cambiata");
        h.receive(1, &[mob_equipment(2, 2)]);
        let sent = h.packets_by_player();
        let equipment = of_kind(&sent[&2], id::MOB_EQUIPMENT);
        assert_eq!(equipment.len(), 1);
        let m = MobEquipment::decode(&equipment[0].body).unwrap();
        assert_eq!((m.target_runtime_id, m.item.id, m.slot, m.selected_slot), (1, registry::items().id("minecraft:stone").unwrap(), 2, 2));
        assert!(sent.get(&1).is_none_or(|v| count(v, id::MOB_EQUIPMENT) == 0));
        assert!(sent.get(&3).is_none_or(|v| count(v, id::MOB_EQUIPMENT) == 0));
    }

    #[test]
    fn stack_request_on_the_held_slot_updates_viewers() {
        let mut h = three_players();
        h.receive(1, &[stack_request_packet(|w| craft_and_place(w, -1, stone_net_id(), 0))]);
        assert_eq!(count(&h.packets_by_player()[&2], id::MOB_EQUIPMENT), 1);
    }

    #[test]
    fn armor_change_is_relayed() {
        let mut h = three_players();
        let helmet = registry::creative().1.iter().position(|c| c.name == "minecraft:iron_helmet").unwrap() as u32 + 1;
        h.receive(1, &[stack_request_packet(|w| craft_and_place_count(w, -1, helmet, 6, 0, 1))]);
        let sent = h.packets_by_player();
        let armor = of_kind(&sent[&2], id::MOB_ARMOR_EQUIPMENT);
        assert_eq!(armor.len(), 1);
        let expected = crate::entity::armor_of(&h.server.players[&1]);
        assert_eq!(expected[0].id, registry::items().id("minecraft:iron_helmet").unwrap());
        let packet = bedrock_protocol::entity::mob_armor_equipment(1, &expected);
        assert_eq!(armor[0].body, packet.body);
        assert!(sent.get(&3).is_none_or(|v| count(v, id::MOB_ARMOR_EQUIPMENT) == 0));
    }

    #[test]
    fn a_new_viewer_sees_the_worn_armor_and_the_state() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.join(2, "Bob");
        h.server.players.get_mut(&1).unwrap().sent.insert((0, 0));
        let iron_helmet = registry::items().id("minecraft:iron_helmet").unwrap();
        h.server.players.get_mut(&1).unwrap().inventory.set_armor(0, crate::inventory::Item { id: iron_helmet, count: 1, ..Default::default() });
        h.server.players.get_mut(&1).unwrap().sprinting = true;
        h.server.players.get_mut(&2).unwrap().sent.insert((0, 0));
        h.packets_by_player();
        h.server.visibility_tick();
        let sent = h.packets_by_player();
        assert_eq!(count(&sent[&2], id::ADD_PLAYER), 1);
        assert_eq!(count(&sent[&2], id::MOB_ARMOR_EQUIPMENT), 1);
    }
}

mod skin_change {
    use super::*;
    use bedrock_protocol::skin::{player_skin_packet, PlayerSkinChange, Skin};

    fn uuid_of(h: &Harness, pid: u64) -> [u8; 16] {
        match h.server.entities.get(pid).unwrap().kind {
            crate::entity::EntityKind::Player { uuid, .. } => uuid,
            _ => unreachable!(),
        }
    }

    fn new_skin() -> Skin {
        let mut s = Skin::fallback();
        s.id = "nuova".into();
        s.full_id = "nuova".into();
        s.image.data = [1, 2, 3, 255].repeat(64 * 64);
        s
    }

    fn change(uuid: [u8; 16]) -> Packet {
        player_skin_packet(&PlayerSkinChange { uuid, skin: new_skin(), new_name: "n".into(), old_name: "o".into() })
    }

    #[test]
    fn skin_change_reaches_everyone_and_is_stored() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.join(2, "Bob");
        h.packets_by_player();
        let alice = uuid_of(&h, 1);
        h.receive(1, &[change(alice)]);
        let sent = h.packets_by_player();
        for pid in [1, 2] {
            let skins: Vec<_> = sent[&pid].iter().filter(|p| p.id == id::PLAYER_SKIN).collect();
            assert_eq!(skins.len(), 1, "#{pid} riceve il cambio (anche chi non vede Alice)");
            let got = PlayerSkinChange::decode(&skins[0].body).unwrap();
            assert_eq!((got.uuid, got.skin), (alice, new_skin()));
        }
        assert_eq!(h.server.players[&1].info.as_ref().unwrap().skin, new_skin());
        h.join(3, "Carol");
        let to_carol = h.packets_to(3);
        let list = to_carol.iter().find(|p| p.id == id::PLAYER_LIST).unwrap();
        let mut w = Writer::default();
        new_skin().encode(&mut w);
        assert!(list.body.windows(w.0.len()).any(|s| s == w.0.as_slice()), "la lista dei nuovi contiene la nuova skin");
    }

    #[test]
    fn skin_change_with_wrong_uuid_is_ignored() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.join(2, "Bob");
        h.packets_by_player();
        let before = h.server.players[&1].info.as_ref().unwrap().skin.clone();
        let bob = uuid_of(&h, 2);
        h.receive(1, &[change(bob)]);
        let sent = h.packets_by_player();
        assert!(sent.values().flatten().all(|p| p.id != id::PLAYER_SKIN));
        assert_eq!(h.server.players[&1].info.as_ref().unwrap().skin, before);
        assert_eq!(h.server.players[&2].info.as_ref().unwrap().skin, before);
    }

    #[test]
    fn skin_change_before_spawn_or_malformed_is_ignored() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.server.open(2);
        h.receive(2, &[login("Bob")]);
        h.packets_by_player();
        h.receive(2, &[change([0; 16])]);
        h.receive(1, &[Packet::new(id::PLAYER_SKIN, vec![1, 2, 3])]);
        assert!(h.packets_by_player().values().flatten().all(|p| p.id != id::PLAYER_SKIN));
    }

    #[test]
    fn skin_change_hex_is_logged_only_a_few_times() {
        let mut h = Harness::new();
        h.join(1, "Alice");
        let alice = uuid_of(&h, 1);
        for _ in 0..5 {
            h.receive(1, &[change(alice)]);
        }
        assert_eq!(h.server.players[&1].skin_logs, 3);
    }
}

mod ground_items {
    use super::blocks::{change, harness_with_chunk, stone};
    use super::placing::{setup, EYES};
    use super::*;
    use crate::entity::EntityKind;
    use crate::events::WorldAction;
    use vgen::Dim;

    /// Richiesta con una sola azione Drop di `count` oggetti dalla casella 0 della barra (ID di pila `stack_id`).
    fn drop_request(request_id: i32, count: u8, stack_id: i32) -> Packet {
        stack_request_packet(|w| {
            w.var_i32(request_id);
            w.var_u32(1);
            w.var_u32(3); // variante Drop
            w.u8(3);
            w.u8(count);
            slot_bytes(w, 28, 0, stack_id);
            w.bool(false);
            w.var_u32(0);
            w.i32(-1);
        })
    }

    fn held_net_id(h: &Harness) -> i32 {
        h.server.players[&1].inventory.stack_at(28, 0).unwrap().net_id.unwrap()
    }

    fn items(h: &Harness) -> Vec<(u64, u16, [f32; 3], [f32; 3])> {
        let mut v: Vec<_> = h
            .server
            .entities
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Item { item, velocity, .. } => Some((e.id, item.count, e.pos, *velocity)),
                _ => None,
            })
            .collect();
        v.sort_by_key(|i| i.0);
        v
    }

    /// Un mondo con una pietra piena a y = 70 nel chunk (0, 0) e un oggetto a y = 72 sopra.
    fn item_over_stone() -> (Harness, u64) {
        let mut h = harness_with_chunk();
        h.server.set_blocks(Dim::Overworld, &[change([3, 70, 4], stone(), false)]);
        let item = crate::inventory::Item { id: registry::items().id("minecraft:stone").unwrap(), count: 1, ..Default::default() };
        let id = h.server.spawn_item(item, [3.5, 72.0, 4.5], [0.0; 3], 10);
        (h, id)
    }

    #[test]
    fn drop_action_spawns_an_item_entity_and_empties_the_slot() {
        let mut h = setup(GameMode::Survival);
        let id = held_net_id(&h);
        h.server.players.get_mut(&1).unwrap().yaw = 90.0; // verso ovest
        h.receive(1, &[drop_request(-3, 1, id)]);
        assert_eq!(h.server.players[&1].inventory.held().count, 63);
        let dropped = items(&h);
        assert_eq!(dropped.len(), 1);
        let (_, count, pos, velocity) = dropped[0];
        assert_eq!(count, 1);
        assert!((pos[1] - (EYES[1] - 0.3)).abs() < 1e-4 && pos[0] == EYES[0]);
        assert!(velocity[0] < -0.29 && velocity[1].abs() < 1e-4 && velocity[2].abs() < 1e-4, "{velocity:?}");
        let EntityKind::Item { pickup_at, despawn_at, .. } = &h.server.entities.get(dropped[0].0).unwrap().kind else { panic!("oggetto") };
        assert_eq!((*pickup_at, *despawn_at), (h.server.tick_count + 40, h.server.tick_count + 6000));
        // Bob (che vede il chunk) lo vede apparire con la velocità e la posizione 0.125 più in alto
        h.packets_by_player();
        h.server.visibility_tick();
        let sent = h.packets_by_player();
        let add = sent[&2].iter().find(|p| p.id == id::ADD_ITEM_ACTOR).expect("AddItemActor a Bob");
        assert!(sent.get(&3).is_none_or(|v| v.iter().all(|p| p.id != id::ADD_ITEM_ACTOR)));
        assert!(add.body.len() > 8);
    }

    #[test]
    fn cancelled_drop_keeps_the_item() {
        let mut h = setup(GameMode::Survival);
        h.server.hooks.push(Box::new(|a| !matches!(a, WorldAction::DropItem { .. })));
        let id = held_net_id(&h);
        h.receive(1, &[drop_request(-3, 5, id)]);
        assert_eq!(h.server.players[&1].inventory.held().count, 64, "l'oggetto è tornato nella casella");
        assert!(items(&h).is_empty());
    }

    #[test]
    fn item_falls_and_rests_on_stone() {
        let (mut h, id) = item_over_stone();
        for _ in 0..40 {
            h.server.items_tick();
        }
        let e = h.server.entities.get(id).unwrap();
        assert!(e.on_ground);
        assert!((71.0..=71.3).contains(&e.pos[1]), "{}", e.pos[1]);
    }

    #[test]
    fn item_below_the_world_is_despawned() {
        let mut h = harness_with_chunk();
        let item = crate::inventory::Item { id: registry::items().id("minecraft:stone").unwrap(), count: 1, ..Default::default() };
        let id = h.server.spawn_item(item, [3.5, -65.0, 4.5], [0.0; 3], 0);
        h.server.items_tick();
        assert!(h.server.entities.get(id).is_none());
    }

    #[test]
    fn item_in_an_unloaded_chunk_is_frozen_and_not_lost() {
        let (mut h, id) = item_over_stone();
        for _ in 0..40 {
            h.server.items_tick();
        }
        let resting = h.server.entities.get(id).unwrap().pos;
        let chunk = h.server.chunks.remove(&(0, 0, 0)).unwrap();
        for _ in 0..200 {
            h.server.items_tick();
        }
        assert_eq!(h.server.entities.get(id).expect("l'oggetto non si perde").pos, resting);
        h.server.chunks.insert((0, 0, 0), chunk);
        h.server.items_tick();
        let e = h.server.entities.get(id).unwrap();
        assert!(e.on_ground && e.pos == resting, "di nuovo simulato, sempre appoggiato");
        // il tempo di vita scorre anche da fermo
        h.server.chunks.remove(&(0, 0, 0));
        h.server.tick_count += 6000;
        h.server.items_tick();
        assert!(h.server.entities.get(id).is_none());
    }

    #[test]
    fn item_despawns_after_6000_ticks() {
        let (mut h, id) = item_over_stone();
        for _ in 0..5999 {
            h.server.items_tick();
            h.server.tick_count += 1;
        }
        assert!(h.server.entities.get(id).is_some());
        h.server.tick_count += 1;
        h.server.items_tick();
        assert!(h.server.entities.get(id).is_none());
    }

    #[test]
    fn item_in_a_chunk_nobody_sees_is_not_announced() {
        let (mut h, _) = item_over_stone();
        h.join(1, "Alex");
        h.packets_by_player();
        h.server.players.get_mut(&1).unwrap().sent.clear();
        h.server.items_tick();
        h.server.visibility_tick();
        h.server.movement_tick();
        assert!(h.packets_by_player().is_empty());
    }

    #[test]
    fn landed_item_is_moved_for_viewers_with_the_network_offset() {
        let (mut h, id) = item_over_stone();
        h.join(1, "Alex");
        h.server.players.get_mut(&1).unwrap().sent.insert((0, 0));
        h.server.visibility_tick();
        h.packets_by_player();
        for _ in 0..40 {
            h.server.items_tick();
            h.server.movement_tick();
        }
        let sent = h.packets_by_player();
        let moves: Vec<_> = sent[&1].iter().filter(|p| p.id == id::MOVE_ACTOR_ABSOLUTE).collect();
        assert!(!moves.is_empty());
        let last = bedrock_protocol::entity::move_actor_absolute(id, [3.5, 71.125, 4.5], 0.0, 0.0, 0.0, true, false);
        assert_eq!(moves.last().unwrap().body, last.body);
        assert!(sent[&1].iter().any(|p| p.id == id::SET_ACTOR_MOTION), "la caduta cambia la velocità");
    }

    /// Alice (1, i piedi a y = 10) con l'inventario vuoto e un oggetto `count` pietre ai suoi piedi, pronto da raccogliere.
    fn pickup_setup(mode: GameMode, count: u16) -> (Harness, u64) {
        let mut h = setup(mode);
        let alice = h.server.players.get_mut(&1).unwrap();
        (0..crate::inventory::MAIN_SLOTS).for_each(|i| alice.inventory.set_main(i, crate::inventory::Item::default()));
        let item = crate::inventory::Item { id: stone_id(), count, ..Default::default() };
        let id = h.server.spawn_item(item, [EYES[0], EYES[1] - 1.62, EYES[2]], [0.0; 3], 0);
        h.server.visibility_tick();
        h.packets_by_player();
        (h, id)
    }

    fn stone_id() -> i16 {
        registry::items().id("minecraft:stone").unwrap()
    }

    fn carried(h: &Harness, pid: u64) -> u32 {
        (0..crate::inventory::MAIN_SLOTS).map(|i| h.server.players[&pid].inventory.main(i)).filter(|i| i.id == stone_id()).map(|i| i.count as u32).sum()
    }

    #[test]
    fn pickup_adds_to_inventory_and_removes_the_entity() {
        for mode in [GameMode::Survival, GameMode::Creative, GameMode::Adventure] {
            let (mut h, id) = pickup_setup(mode, 5);
            h.server.pickup_tick();
            assert!(h.server.entities.get(id).is_none(), "{mode:?}");
            assert_eq!(carried(&h, 1), 5, "{mode:?}");
            let sent = h.packets_by_player();
            let take = bedrock_protocol::entity::take_item_actor(id, 1);
            assert!(sent[&1].iter().any(|p| p.id == take.id && p.body == take.body), "{mode:?}: il raccoglitore vede l'animazione");
            assert!(sent[&2].iter().any(|p| p.id == take.id && p.body == take.body), "{mode:?}: Bob pure");
            assert!(sent[&1].iter().any(|p| p.id == id::INVENTORY_SLOT), "{mode:?}: la casella cambiata");
            assert!(sent[&2].iter().any(|p| p.id == id::REMOVE_ACTOR));
            assert!(sent.get(&3).is_none_or(|v| v.iter().all(|p| p.id != take.id)), "Carol non vede nulla");
        }
    }

    #[test]
    fn pickup_in_the_held_slot_shows_the_item_to_viewers() {
        let (mut h, _) = pickup_setup(GameMode::Survival, 3);
        h.server.pickup_tick();
        let sent = h.packets_by_player();
        assert!(sent[&2].iter().any(|p| p.id == id::MOB_EQUIPMENT));
    }

    #[test]
    fn full_inventory_leaves_the_item() {
        let (mut h, id) = pickup_setup(GameMode::Survival, 5);
        let dirt = registry::items().id("minecraft:dirt").unwrap();
        let alice = h.server.players.get_mut(&1).unwrap();
        (0..crate::inventory::MAIN_SLOTS).for_each(|i| alice.inventory.set_main(i, crate::inventory::Item { id: dirt, count: 64, ..Default::default() }));
        h.server.pickup_tick();
        assert!(h.server.entities.get(id).is_some());
        assert!(h.packets_by_player().is_empty());
    }

    #[test]
    fn partial_pickup_leaves_the_rest() {
        let (mut h, id) = pickup_setup(GameMode::Survival, 10);
        let dirt = registry::items().id("minecraft:dirt").unwrap();
        let alice = h.server.players.get_mut(&1).unwrap();
        (0..crate::inventory::MAIN_SLOTS).for_each(|i| alice.inventory.set_main(i, crate::inventory::Item { id: dirt, count: 64, ..Default::default() }));
        alice.inventory.set_main(9, crate::inventory::Item { id: stone_id(), count: 60, ..Default::default() });
        h.server.pickup_tick();
        assert_eq!(carried(&h, 1), 64);
        assert_eq!(items(&h), vec![(id, 6, h.server.entities.get(id).unwrap().pos, [0.0; 3])]);
    }

    #[test]
    fn two_players_on_the_same_item_only_one_gets_it() {
        let (mut h, id) = pickup_setup(GameMode::Survival, 5);
        h.server.players.get_mut(&2).unwrap().pos = EYES;
        h.server.pickup_tick();
        assert_eq!(carried(&h, 1) + carried(&h, 2), 5);
        assert_eq!((carried(&h, 1), carried(&h, 2)), (5, 0), "vince il pid più basso");
        assert!(h.server.entities.get(id).is_none());
    }

    #[test]
    fn two_players_split_a_partially_taken_item_without_duplicating() {
        let (mut h, _) = pickup_setup(GameMode::Survival, 10);
        h.server.players.get_mut(&2).unwrap().pos = EYES;
        let dirt = registry::items().id("minecraft:dirt").unwrap();
        let alice = h.server.players.get_mut(&1).unwrap();
        (0..crate::inventory::MAIN_SLOTS).for_each(|i| alice.inventory.set_main(i, crate::inventory::Item { id: dirt, count: 64, ..Default::default() }));
        alice.inventory.set_main(9, crate::inventory::Item { id: stone_id(), count: 60, ..Default::default() });
        h.server.pickup_tick();
        assert_eq!(carried(&h, 1) + carried(&h, 2) + items(&h).iter().map(|i| i.1 as u32).sum::<u32>(), 70);
        assert_eq!((carried(&h, 1), carried(&h, 2)), (64, 6));
    }

    #[test]
    fn pickup_waits_for_the_delay_and_the_reach() {
        let (mut h, id) = pickup_setup(GameMode::Survival, 5);
        let EntityKind::Item { pickup_at, .. } = &mut h.server.entities.get_mut(id).unwrap().kind else { panic!("oggetto") };
        *pickup_at = h.server.tick_count + 2;
        h.server.pickup_tick();
        h.server.tick_count += 1;
        h.server.pickup_tick();
        assert!(h.server.entities.get(id).is_some(), "ritardo non ancora passato");
        h.server.tick_count += 1;
        h.server.players.get_mut(&1).unwrap().pos[0] += 2.0; // oltre 1 + 0.3 + 0.125 di distanza
        h.server.pickup_tick();
        assert!(h.server.entities.get(id).is_some(), "troppo lontano");
        h.server.players.get_mut(&1).unwrap().pos[0] -= 1.0; // dentro il riquadro allargato
        h.server.pickup_tick();
        assert!(h.server.entities.get(id).is_none());
    }

    #[test]
    fn spectators_do_not_pick_up() {
        let (mut h, id) = pickup_setup(GameMode::Spectator, 5);
        h.server.pickup_tick();
        assert!(h.server.entities.get(id).is_some());
        assert_eq!(carried(&h, 1), 0);
    }

    #[test]
    fn cancelled_pickup_leaves_everything() {
        let (mut h, id) = pickup_setup(GameMode::Survival, 5);
        h.server.hooks.push(Box::new(|a| !matches!(a, WorldAction::PickupItem { .. })));
        h.server.pickup_tick();
        assert!(h.server.entities.get(id).is_some());
        assert_eq!(carried(&h, 1), 0);
        assert!(h.packets_by_player().is_empty());
    }
}

mod bot {
    use super::*;
    use crate::entity::EntityKind;

    fn count(packets: &[Packet], packet_id: u32) -> usize {
        packets.iter().filter(|p| p.id == packet_id).count()
    }

    fn has_name(packets: &[Packet], packet_id: u32, name: &str) -> bool {
        packets.iter().filter(|p| p.id == packet_id).any(|p| p.body.windows(name.len()).any(|w| w == name.as_bytes()))
    }

    /// Alice (che crea i bot) e Bob (lo spettatore) vedono entrambi il chunk (0, 0), dove gira il bot.
    fn two_players() -> Harness {
        let mut h = Harness::new();
        h.join(1, "Alice");
        h.join(2, "Bob");
        for pid in [1, 2] {
            let p = h.server.players.get_mut(&pid).unwrap();
            p.pos = [8.5, 70.0, 8.5];
            p.sent.insert((0, 0));
        }
        h.packets_by_player();
        h
    }

    #[test]
    fn bot_appears_in_list_and_moves() {
        let mut h = two_players();
        h.receive(1, &[command("/bot Prova")]);
        let to_bob = h.packets_to(2);
        assert!(has_name(&to_bob, id::PLAYER_LIST, "Prova"), "la voce del bot va a tutti");
        for _ in 0..20 {
            h.server.tick();
        }
        let to_bob = h.packets_to(2);
        assert!(has_name(&to_bob, id::ADD_PLAYER, "Prova"), "Bob vede il corpo del bot");
        assert!(count(&to_bob, id::MOVE_ACTOR_ABSOLUTE) > 0, "il bot si muove");
    }

    #[test]
    fn bot_throws_items() {
        let mut h = two_players();
        h.receive(1, &[command("/bot Prova")]);
        for _ in 0..200 {
            h.server.tick();
        }
        assert!(h.server.entities.iter().any(|e| matches!(e.kind, EntityKind::Item { .. })), "dopo 200 tick il bot ha lanciato una pietra");
    }

    #[test]
    fn bot_remove_cleans_everything() {
        let mut h = two_players();
        h.receive(1, &[command("/bot Prova")]);
        h.server.tick();
        h.packets_by_player();
        h.receive(1, &[command("/bot rimuovi")]);
        let mut sent = h.packets_by_player();
        let to_bob = sent.remove(&2).unwrap();
        assert_eq!(count(&to_bob, id::REMOVE_ACTOR), 1);
        assert_eq!(count(&to_bob, id::PLAYER_LIST), 1);
        assert!(sent.remove(&1).unwrap().iter().any(|p| p.id == id::PLAYER_LIST), "anche chi l'ha creato lo toglie dalla lista");
        assert!(!h.server.entities.iter().any(|e| matches!(e.kind, EntityKind::Player { pid: None, .. })));
        assert!(h.server.bots.is_empty());
        h.server.tick();
        assert_eq!(count(&h.packets_to(2), id::MOVE_ACTOR_ABSOLUTE), 0, "nessun movimento di un bot che non c'è più");
    }

    #[test]
    fn newcomer_gets_bots_in_the_list() {
        let mut h = two_players();
        h.receive(1, &[command("/bot Prova")]);
        h.packets_by_player();
        h.join(3, "Carol");
        let to_carol = h.packets_to(3);
        assert!(has_name(&to_carol, id::PLAYER_LIST, "Prova"));
    }

    fn bot_names(h: &Harness) -> Vec<String> {
        h.server
            .entities
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Player { pid: None, name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bot_name_is_cleaned_and_limited() {
        let mut h = two_players();
        h.receive(1, &[command("/bot   Un nome lunghissimo davvero  ")]);
        assert_eq!(bot_names(&h), vec!["Un nome lunghiss".to_string()], "16 caratteri");
        h.server.remove_bots();
        h.receive(1, &[command(&format!("/bot §cRo{}sso§r", '\u{7}'))]);
        assert_eq!(bot_names(&h), vec!["cRossor".to_string()], "senza § e controlli");
        h.server.remove_bots();
        h.receive(1, &[command("/bot àèìòùàèìòùàèìòùàèìòù")]);
        assert_eq!(bot_names(&h)[0].chars().count(), 16, "si contano i caratteri, non i byte");
        h.server.remove_bots();
        h.receive(1, &[command("/bot §")]);
        assert_eq!(bot_names(&h), vec!["Bot".to_string()]);
    }

    #[test]
    fn bot_with_a_taken_name_is_refused() {
        let mut h = two_players();
        h.texts_to(1);
        h.receive(1, &[command("/bot bob")]);
        assert!(bot_names(&h).is_empty());
        assert!(h.texts_to(1).iter().any(|t| t.message.contains("già in uso")));
        h.receive(1, &[command("/bot Prova")]);
        h.receive(1, &[command("/bot PROVA")]);
        assert_eq!(bot_names(&h).len(), 1);
    }

    #[test]
    fn ninth_bot_is_refused() {
        let mut h = two_players();
        for n in 0..9 {
            h.receive(1, &[command(&format!("/bot B{n}"))]);
        }
        assert_eq!(h.server.bots.len(), 8);
        assert!(h.texts_to(1).iter().any(|t| t.message.contains("già 8 bot")));
    }

    #[test]
    fn list_shows_bots() {
        let mut h = two_players();
        h.receive(1, &[command("/bot Prova")]);
        h.texts_to(1);
        h.receive(1, &[command("/list")]);
        let texts = h.texts_to(1);
        assert!(texts.iter().any(|t| t.message.contains("Prova") && t.message.contains("(3)")), "{:?}", texts.iter().map(|t| &t.message).collect::<Vec<_>>());
    }
}

mod survival {
    use super::*;
    use crate::{events::WorldAction, join::vitals_attributes, survival::DamageCause, world::Chunk};
    use bedrock_protocol::entity::{actor_event, actor_event_type};
    use vgen::Dim;

    /// Alice (1) in `mode`, Bob (2) che la vede, Carol (3) che non la vede.
    fn setup(mode: GameMode) -> Harness {
        let mut h = Harness::new();
        for (pid, name) in [(1, "Alice"), (2, "Bob"), (3, "Carol")] {
            h.join(pid, name);
        }
        h.server.players.get_mut(&1).unwrap().mode = mode;
        h.server.players.get_mut(&2).unwrap().visible.insert(1);
        h.packets_by_player();
        h
    }

    fn health(h: &Harness) -> f32 {
        h.server.players[&1].health
    }

    fn vitals_packet(h: &Harness) -> Packet {
        UpdateAttributes { target_runtime_id: 1, attribute_list: vitals_attributes(&h.server.players[&1]), tick: 0 }.packet()
    }

    fn hurt_event() -> Packet {
        actor_event(1, actor_event_type::HURT, 0)
    }

    fn health_attribute(h: &Harness) -> f32 {
        vitals_attributes(&h.server.players[&1]).iter().find(|a| a.name == "minecraft:health").unwrap().current_value
    }

    #[test]
    fn hurt_in_survival_takes_health_and_tells_self_and_viewers() {
        let mut h = setup(GameMode::Survival);
        assert!(h.server.hurt(1, 3.0, DamageCause::Fall));
        assert_eq!(health(&h), 17.0);
        let sent = h.packets_by_player();
        assert!(sent[&1].contains(&vitals_packet(&h)), "UpdateAttributes a sé");
        assert!(sent[&1].contains(&hurt_event()), "ActorEvent HURT a sé");
        assert!(sent[&2].contains(&hurt_event()), "ActorEvent HURT agli spettatori");
        assert!(!sent[&2].contains(&vitals_packet(&h)), "la vita la vede solo lui");
        assert!(sent.get(&3).is_none_or(|v| !v.contains(&hurt_event())), "chi non lo vede non riceve nulla");
        assert_eq!(health_attribute(&h), 17.0);
    }

    #[test]
    fn second_hit_within_ten_ticks_only_counts_the_difference() {
        let mut h = setup(GameMode::Survival);
        assert!(h.server.hurt(1, 4.0, DamageCause::Attack { attacker: 2 }));
        // più debole o uguale: niente
        assert!(!h.server.hurt(1, 3.0, DamageCause::Attack { attacker: 2 }));
        assert!(!h.server.hurt(1, 4.0, DamageCause::Attack { attacker: 2 }));
        assert_eq!(health(&h), 16.0);
        h.packets_by_player();
        // più forte: solo la differenza
        assert!(h.server.hurt(1, 6.0, DamageCause::Attack { attacker: 2 }));
        assert_eq!(health(&h), 14.0);
        assert!(h.packets_by_player()[&2].contains(&hurt_event()));
        // passati i 10 tick dal colpo più recente si subisce di nuovo per intero
        h.server.tick_count += 10;
        assert!(h.server.hurt(1, 2.0, DamageCause::Fall));
        assert_eq!(health(&h), 12.0);
    }

    /// `PlayerAuthInput` minimo con gli occhi in (0.5, `eyes_y`, 0.5) e i flag di input dati.
    fn pai_fall(eyes_y: f32, flags: &[i32]) -> Packet {
        let mut w = Writer::default();
        for v in [0.0, 0.0, 0.5, eyes_y, 0.5, 0.0, 0.0, 0.0] {
            w.f32(v);
        }
        w.var_u32(flags.len() as u32);
        flags.iter().for_each(|&f| w.var_i32(f));
        w.var_u32(1); // modalità di input
        w.var_u32(0);
        w.var_i32(2);
        (0..2).for_each(|_| w.f32(0.0));
        w.var_u64(10);
        (0..3).for_each(|_| w.f32(0.0));
        (0..5).for_each(|_| w.u8(0)); // niente transazione, richiesta, azioni di blocco, veicolo
        (0..7).for_each(|_| w.f32(0.0));
        Packet::new(id::PLAYER_AUTH_INPUT, w.0)
    }

    const VERTICAL_COLLISION: i32 = 50;

    /// Il giocatore 1 manda un input con i piedi a `feet` (a terra se `ground`).
    fn feet_at(h: &mut Harness, feet: f32, ground: bool) {
        let flags: &[i32] = if ground { &[VERTICAL_COLLISION] } else { &[] };
        h.receive(1, &[pai_fall(feet + 1.62, flags)]);
    }

    /// Fa cadere il giocatore 1 da `from` a `to` (piedi) a un blocco per tick, atterrando in `to`.
    fn fall(h: &mut Harness, from: f32, to: f32) {
        feet_at(h, from, true);
        let mut y = from;
        while y > to {
            y = (y - 1.0).max(to);
            feet_at(h, y, y == to);
            h.server.tick_count += 1;
        }
    }

    #[test]
    fn falling_ten_blocks_in_survival_deals_seven() {
        let mut h = setup(GameMode::Survival);
        fall(&mut h, 80.0, 70.0);
        assert_eq!(health(&h), 13.0);
        assert_eq!(h.server.players[&1].fall_start_y, None, "l'atterraggio azzera la caduta");
    }

    #[test]
    fn falling_three_blocks_deals_nothing() {
        let mut h = setup(GameMode::Survival);
        fall(&mut h, 73.0, 70.0);
        assert_eq!(health(&h), 20.0);
        fall(&mut h, 74.0, 70.0);
        assert_eq!(health(&h), 19.0, "oltre 3 blocchi si arrotonda per eccesso: 4 blocchi = 1");
    }

    #[test]
    fn falling_in_creative_deals_nothing() {
        let mut h = setup(GameMode::Creative);
        fall(&mut h, 90.0, 70.0);
        assert_eq!(health(&h), 20.0);
    }

    #[test]
    fn landing_in_water_deals_nothing() {
        let mut h = setup(GameMode::Survival);
        let mut chunk = Chunk::flat(Dim::Overworld, 0, 0);
        chunk.set_block(0, 50, 0, crate::world::block_hash("minecraft:air", &[]), true);
        h.server.chunks.insert((0, 0, 0), chunk);
        feet_at(&mut h, 80.0, true);
        for y in (51..80).rev() {
            feet_at(&mut h, y as f32, false);
        }
        feet_at(&mut h, 50.0, true);
        assert_eq!(health(&h), 20.0);
    }

    #[test]
    fn climbing_down_a_ladder_deals_nothing() {
        let mut h = setup(GameMode::Survival);
        let mut chunk = Chunk::flat(Dim::Overworld, 0, 0);
        let ladder = crate::world::block_hash("minecraft:ladder", &[("facing_direction", crate::world::State::Int(2))]);
        for y in 70..=80 {
            chunk.set_block(0, y, 0, ladder, false);
        }
        h.server.chunks.insert((0, 0, 0), chunk);
        feet_at(&mut h, 80.0, false);
        for y in (70..80).rev() {
            feet_at(&mut h, y as f32 + 0.5, false);
        }
        feet_at(&mut h, 69.0, true); // scende dalla scala sul pavimento
        assert_eq!(health(&h), 20.0);
    }

    #[test]
    fn jumping_up_resets_the_fall() {
        let mut h = setup(GameMode::Survival);
        feet_at(&mut h, 80.0, true);
        feet_at(&mut h, 76.0, false);
        feet_at(&mut h, 77.0, false); // risale: la caduta riparte da qui
        feet_at(&mut h, 76.0, false);
        feet_at(&mut h, 75.0, true);
        assert_eq!(health(&h), 20.0, "solo 2 blocchi dall'ultimo punto più alto");
    }

    #[test]
    fn flight_then_switching_to_survival_in_air_falls_only_from_there() {
        let mut h = setup(GameMode::Creative);
        feet_at(&mut h, 100.0, false);
        h.server.players.get_mut(&1).unwrap().flying = true;
        feet_at(&mut h, 100.0, false);
        // cambio di modalità in aria: in survival non si vola più (chat.rs azzera `flying`)
        h.server.players.get_mut(&1).unwrap().mode = GameMode::Survival;
        h.server.players.get_mut(&1).unwrap().flying = false;
        feet_at(&mut h, 100.0, false);
        for y in (95..100).rev() {
            feet_at(&mut h, y as f32, y == 95);
        }
        assert_eq!(health(&h), 18.0, "5 blocchi - 3 = 2");
    }

    #[test]
    fn gliding_clears_the_fall() {
        let mut h = setup(GameMode::Survival);
        feet_at(&mut h, 100.0, false);
        feet_at(&mut h, 90.0, false);
        h.server.players.get_mut(&1).unwrap().gliding = true;
        feet_at(&mut h, 80.0, false);
        assert_eq!(h.server.players[&1].fall_start_y, None);
        h.server.players.get_mut(&1).unwrap().gliding = false;
        feet_at(&mut h, 79.0, true);
        assert_eq!(health(&h), 20.0, "niente danno da quota 100");
    }

    #[test]
    fn below_the_world_hurts_every_ten_ticks_even_in_creative_but_not_spectator() {
        for (mode, hurts) in [(GameMode::Survival, true), (GameMode::Creative, true), (GameMode::Spectator, false)] {
            let mut h = setup(mode);
            h.server.players.get_mut(&1).unwrap().pos = [0.5, -64.0 - 0.5 + 1.62, 0.5]; // piedi a -64.5
            for tick in 1..=20 {
                h.server.tick_count = tick;
                h.server.void_tick();
                let expected = if hurts { 20.0 - 4.0 * (tick / 10) as f32 } else { 20.0 };
                assert_eq!(health(&h), expected, "{mode:?} tick {tick}");
            }
        }
        let mut h = setup(GameMode::Survival);
        h.server.players.get_mut(&1).unwrap().pos = [0.5, -64.0 + 1.62, 0.5]; // piedi esattamente al fondo: dentro il mondo
        h.server.tick_count = 10;
        h.server.void_tick();
        assert_eq!(health(&h), 20.0);
    }

    #[test]
    fn void_ignores_invulnerability() {
        let mut h = setup(GameMode::Survival);
        assert!(h.server.hurt(1, 5.0, DamageCause::Fall));
        assert!(h.server.hurt(1, 1.0, DamageCause::Void));
        assert_eq!(health(&h), 14.0);
    }

    #[test]
    fn creative_and_spectator_take_no_damage_except_the_void() {
        for mode in [GameMode::Creative, GameMode::Spectator] {
            let mut h = setup(mode);
            assert!(!h.server.hurt(1, 5.0, DamageCause::Fall));
            assert!(!h.server.hurt(1, 5.0, DamageCause::Attack { attacker: 2 }));
            assert_eq!(health(&h), 20.0);
            assert!(h.packets_to(1).is_empty(), "nessun pacchetto se non c'è danno");
        }
        let mut h = setup(GameMode::Creative);
        assert!(h.server.hurt(1, 4.0, DamageCause::Void), "il vuoto colpisce anche in creativa");
        assert_eq!(health(&h), 16.0);
        let mut h = setup(GameMode::Spectator);
        assert!(!h.server.hurt(1, 4.0, DamageCause::Void));
        let mut h = setup(GameMode::Adventure);
        assert!(h.server.hurt(1, 4.0, DamageCause::Fall), "l'avventura subisce danno come la sopravvivenza");
    }

    #[test]
    fn nonsense_amounts_do_nothing() {
        let mut h = setup(GameMode::Survival);
        for amount in [0.0, -2.0, f32::NAN] {
            assert!(!h.server.hurt(1, amount, DamageCause::Fall));
        }
        assert!(!h.server.hurt(99, 1.0, DamageCause::Fall), "giocatore inesistente");
        assert_eq!(health(&h), 20.0);
    }

    #[test]
    fn a_hook_can_refuse_damage_and_sees_what_is_left() {
        use std::sync::{Arc, Mutex};
        let mut h = setup(GameMode::Survival);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let allow = Arc::new(Mutex::new(false));
        let gate = allow.clone();
        h.server.hooks.push(Box::new(move |a| {
            if matches!(a, WorldAction::Damage { .. }) {
                log.lock().unwrap().push(a.clone());
                return *gate.lock().unwrap();
            }
            true
        }));
        assert!(!h.server.hurt(1, 5.0, DamageCause::Fall));
        assert_eq!(health(&h), 20.0);
        assert!(h.packets_to(1).is_empty(), "un colpo annullato non manda nulla");
        // il rifiuto non dà invulnerabilità: il colpo successivo passa per intero
        *allow.lock().unwrap() = true;
        assert!(h.server.hurt(1, 5.0, DamageCause::Fall));
        assert!(h.server.hurt(1, 8.0, DamageCause::Fall));
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [
                WorldAction::Damage { pid: 1, amount: 5.0, cause: DamageCause::Fall },
                WorldAction::Damage { pid: 1, amount: 5.0, cause: DamageCause::Fall },
                WorldAction::Damage { pid: 1, amount: 3.0, cause: DamageCause::Fall },
            ]
        );
        assert_eq!(health(&h), 12.0);
    }

    #[test]
    fn health_at_zero_kills_once() {
        let mut h = setup(GameMode::Survival);
        assert!(h.server.hurt(1, 25.0, DamageCause::Void));
        assert_eq!(health(&h), 0.0);
        assert!(h.server.players[&1].dead);
        assert!(!h.server.hurt(1, 1.0, DamageCause::Void), "un morto non subisce altro danno");
        h.server.heal(1, 5.0);
        assert_eq!(health(&h), 0.0, "né si cura");
    }

    #[test]
    fn heal_caps_at_twenty_and_sends_vitals() {
        let mut h = setup(GameMode::Survival);
        h.server.players.get_mut(&1).unwrap().health = 10.0;
        h.server.heal(1, 4.5);
        assert_eq!(health(&h), 14.5);
        assert!(h.packets_to(1).contains(&vitals_packet(&h)));
        assert_eq!(health_attribute(&h), 15.0, "i cuori arrivano interi (come Dragonfly)");
        h.server.heal(1, 100.0);
        assert_eq!(health(&h), 20.0);
        h.packets_to(1);
        h.server.heal(1, 1.0);
        assert!(h.packets_to(1).is_empty(), "già a vita piena: nulla da dire");
        h.server.heal(1, -3.0);
        assert_eq!(health(&h), 20.0);
    }

    #[test]
    fn heal_does_nothing_in_creative() {
        let mut h = setup(GameMode::Creative);
        h.server.players.get_mut(&1).unwrap().health = 10.0;
        h.server.heal(1, 5.0);
        assert_eq!(health(&h), 10.0);
    }

    #[test]
    fn send_vitals_carries_health_hunger_saturation_and_exhaustion() {
        let mut h = setup(GameMode::Survival);
        let p = h.server.players.get_mut(&1).unwrap();
        (p.food, p.saturation, p.exhaustion) = (12, 3.5, 1.5);
        h.server.send_vitals(1);
        let sent = h.packets_to(1);
        assert_eq!(sent, vec![vitals_packet(&h)]);
        let attributes = vitals_attributes(&h.server.players[&1]);
        let names: Vec<&str> = attributes.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["minecraft:health", "minecraft:player.hunger", "minecraft:player.saturation", "minecraft:player.exhaustion"]);
        assert_eq!(attributes[1].current_value, 12.0);
        assert_eq!(attributes[2].current_value, 3.5);
        assert_eq!(attributes[3].current_value, 1.5);
    }
}
