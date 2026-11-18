//! Test della logica di gioco senza rete: i pacchetti inviati si leggono dal `Sender` di prova.
use crate::Server;
use bedrock_net::{Command, Sender};
use bedrock_protocol::{
    id,
    packets::{text_type, CommandRequest, Login, RequestChunkRadius, Text},
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

#[test]
fn player_auth_input_moves_the_player_and_ignores_bad_positions() {
    let mut h = Harness::new();
    h.join(1, "Alice");
    let input = |x: f32, y: f32, z: f32| {
        let body: Vec<u8> = [0.0f32, 0.0, x, y, z].iter().flat_map(|v| v.to_le_bytes()).collect();
        Packet::new(id::PLAYER_AUTH_INPUT, body)
    };
    h.receive(1, &[input(40.5, 70.0, -3.0)]);
    assert_eq!((h.server.players[&1].pos, h.server.players[&1].last_chunk), ([40.5, 70.0, -3.0], (2, -1)));
    h.receive(1, &[input(f32::NAN, 70.0, 0.0), input(1.0e9, 70.0, 0.0)]);
    assert_eq!(h.server.players[&1].last_chunk, (2, -1), "posizioni non valide scartate");
    h.server.open(2); // non ancora nel mondo: l'input non conta
    h.receive(2, &[input(100.0, 70.0, 0.0)]);
    assert_eq!(h.server.players[&2].pos, [0.0; 3]);
}
