//! Test della logica di gioco senza rete: i pacchetti inviati si leggono dal `Sender` di prova.
use crate::{registry, GameMode, Player, Server};
use bedrock_net::{Command, Sender};
use bedrock_protocol::{
    generated::{CreativeContent, InventoryContent, MobEquipment, PlayerHotbar, SetPlayerGameType, StartGame, UpdateAbilities},
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
        assert_eq!(state, [1, 1], "strato 0 e strato 1, solo ad Alice");
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
        assert_eq!(events(&out), [(1, 2001, [3.5, 10.5, 4.5], stone()), (2, 2001, [3.5, 10.5, 4.5], stone())]);
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
            assert_eq!(sent.len(), 6, "{mode:?}: tre rifiuti, due strati ciascuno");
            assert!(sent.iter().all(|u| u.0 == 1 && u.1 == ROCK), "{mode:?}: lo stato vero va solo ad Alice");
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
        assert_eq!(updates(&mut h.sent)[..2], [(1, ROCK, air() as u32, 3, 0), (1, ROCK, water() as u32, 3, 1)]);
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
