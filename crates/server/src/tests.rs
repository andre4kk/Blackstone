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
    w.var_i32(request_id);
    w.var_u32(2);
    w.var_u32(12); // variante CraftCreative
    w.u8(14);
    w.var_u32(net_id);
    w.u8(1);
    w.var_u32(1); // variante Place
    w.u8(1);
    w.u8(64);
    slot_bytes(w, 60, 50, request_id);
    slot_bytes(w, 28, dst, 0);
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
    let expected = crate::join::spawn_attributes(1).packet();
    assert_eq!(attributes[0].body, expected.body);
    let attributes = crate::join::spawn_attributes(1).attribute_list;
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
    assert_eq!(data[0].body, bedrock_protocol::actor::player_data(1).body);
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
        events::WorldAction,
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
