//! Test end-to-end: un client WebRTC che si comporta come Minecraft contro il server vero
//! (signaling HTTP, identità firmata, ICE/DTLS, data channel, frammentazione, compressione, login, resource pack,
//! StartGame, registro oggetti, chunk e spawn).
use base64::{
    engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64U},
    Engine,
};
use bedrock_protocol::{
    generated::{
        BiomeDefinitionList, ChunkRadiusUpdated, CreativeContent, InventoryContent, ItemRegistry, JigsawStructureData, LevelChunk, NetworkChunkPublisherUpdate,
        PlayerHotbar, SetPlayerGameType, StartGame, UpdateAbilities, UpdateAttributes, VoxelShapes,
    },
    id,
    packets::{pack_response, text_type, CommandRequest, Login, RequestChunkRadius, RequestNetworkSettings, ResourcePackClientResponse, Text},
    varint,
    wire::ServerPacket,
    Packet, Session, PROTOCOL,
};
use bedrock_server::config::ServerConfig;
use p384::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use std::{
    collections::HashSet,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    path::PathBuf,
    time::{Duration, Instant},
};
use str0m::{
    change::SdpAnswer,
    channel::{ChannelConfig, Reliability},
    net::{Protocol, Receive},
    Candidate, Event, Input, Output, Rtc,
};

const PORT: u16 = 29132;
/// Porta del test con il generatore finto.
const GEN_PORT: u16 = 29133;

/// Che mondo ci si aspetta: quello piatto di ripiego o quello del generatore finto, con un movimento di 5 chunk.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Flat,
    Generated,
}

/// Richiesta HTTP minimale. Ritorna (status line, corpo).
fn http(port: u16, method: &str, path: &str, body: &str) -> (String, String) {
    let mut s = (0..50)
        .find_map(|_| {
            TcpStream::connect(("127.0.0.1", port)).ok().or_else(|| {
                std::thread::sleep(Duration::from_millis(100));
                None
            })
        })
        .expect("il server non si è avviato");
    write!(s, "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut r = String::new();
    s.read_to_string(&mut r).unwrap();
    let (head, body) = r.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

/// Verifica come farebbe il client: JWT autofirmato valido, JWS detached sulle fingerprint, a=identity a livello di sessione.
fn verify_identity(sdp: &str) {
    let line = sdp.lines().find(|l| l.starts_with("a=identity:")).expect("manca a=identity");
    let envelope: serde_json::Value = serde_json::from_slice(&B64.decode(&line["a=identity:".len()..]).unwrap()).unwrap();
    let assertion: serde_json::Value = serde_json::from_str(envelope["assertion"].as_str().unwrap()).unwrap();

    let jwt: Vec<_> = assertion["token"].as_str().unwrap().split('.').collect();
    let claims: serde_json::Value = serde_json::from_slice(&B64U.decode(jwt[1]).unwrap()).unwrap();
    let mut point = vec![4u8];
    for c in ["x", "y"] {
        point.extend(B64U.decode(claims["cpk"][c].as_str().unwrap()).unwrap());
    }
    let key = VerifyingKey::from_sec1_bytes(&point).unwrap();
    let sig = Signature::from_slice(&B64U.decode(jwt[2]).unwrap()).unwrap();
    key.verify(format!("{}.{}", jwt[0], jwt[1]).as_bytes(), &sig).expect("firma del JWT non valida");

    let fingerprints: Vec<_> = sdp
        .lines()
        .filter_map(|l| l.strip_prefix("a=fingerprint:")?.split_once(' '))
        .map(|(a, d)| serde_json::json!({"algorithm": a, "digest": d.trim()}))
        .collect();
    assert!(!fingerprints.is_empty());
    let payload = serde_json::json!({"fingerprint": fingerprints}).to_string();
    let (header, sig) = assertion["fingerprints"].as_str().unwrap().split_once("..").expect("JWS non detached");
    let sig = Signature::from_slice(&B64U.decode(sig).unwrap()).unwrap();
    key.verify(format!("{header}.{}", B64U.encode(payload)).as_bytes(), &sig).expect("firma delle fingerprint non valida");

    assert!(sdp.find("a=identity:").unwrap() < sdp.find("\nm=").unwrap(), "a=identity deve stare prima di m=");
}

/// JWT finto (la firma non viene verificata dal server).
fn jwt(payload: serde_json::Value) -> String {
    let b = |s: &str| B64U.encode(s);
    format!("{}.{}.{}", b(r#"{"alg":"ES384"}"#), b(&payload.to_string()), b("firma"))
}

/// Login come lo manderebbe Minecraft: protocollo, catena di identità (envelope "Certificate") e client data.
fn login_body() -> Vec<u8> {
    let chain = serde_json::json!({"chain": [jwt(serde_json::json!({"extraData": {"displayName": "Tester", "XUID": "42"}}))]});
    let identity = serde_json::json!({"AuthenticationType": 0, "Certificate": chain.to_string(), "Token": ""}).to_string();
    let client_data = jwt(serde_json::json!({"ThirdPartyName": "Tester", "DeviceOS": 7, "GameVersion": "1.26.63"}));
    Login { protocol: PROTOCOL as i32, identity: &identity, client_data: &client_data }.encode()
}

fn command(line: &str) -> Packet {
    let request = CommandRequest { command: line.into(), origin_type: "player".into(), version: "latest".into(), ..Default::default() };
    Packet::new(id::COMMAND_REQUEST, request.encode())
}

fn response(tag: u32) -> Packet {
    Packet::new(id::RESOURCE_PACK_CLIENT_RESPONSE, ResourcePackClientResponse::new(tag).encode())
}

/// Pacchetti di modalità e inventario che arrivano appena il client è inizializzato: li conta in `sync`
/// (tipo di gioco, abilità, contenuti dell'inventario, barra). `false` se il pacchetto è un altro.
fn count_sync(packet: u32, sync: &mut [usize; 4]) -> bool {
    match packet {
        SetPlayerGameType::ID => sync[0] += 1,
        UpdateAbilities::ID => sync[1] += 1,
        InventoryContent::ID => sync[2] += 1,
        PlayerHotbar::ID => sync[3] += 1,
        _ => return false,
    }
    true
}

fn client(port: u16, mode: Mode) {
    // 1. Info del server.
    let (status, body) = http(port, "GET", "/v1/join", "");
    assert!(status.contains("200"), "{status}");
    let info: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(info["protocol"], PROTOCOL);

    // 2. Offerta con i due data channel di Minecraft. Il candidato è l'IP della LAN, come per il vero client.
    let ip = bedrock_net::lan_ip().unwrap_or_else(|| "127.0.0.1".parse().unwrap());
    let sock = UdpSocket::bind((ip, 0)).unwrap();
    let local = sock.local_addr().unwrap();
    let mut rtc = Rtc::new(Instant::now());
    rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
    let mut api = rtc.sdp_api();
    let reliable = api.add_channel("ReliableDataChannel".into());
    api.add_channel_with_config(ChannelConfig {
        label: "UnreliableDataChannel".into(),
        ordered: false,
        reliability: Reliability::MaxRetransmits { retransmits: 0 },
        ..Default::default()
    });
    let (offer, pending) = api.apply().unwrap();

    let (status, answer) = http(port, "POST", "/v1/join/123456", &offer.to_sdp_string());
    assert!(status.contains("200"), "{status}");
    verify_identity(&answer);
    let answer: String = answer.lines().filter(|l| !l.starts_with("a=identity:")).flat_map(|l| [l, "\r\n"]).collect();
    rtc.sdp_api().accept_answer(pending, SdpAnswer::from_sdp_string(&answer).unwrap()).unwrap();

    // 3. Ciclo di rete del client: RequestNetworkSettings (in 2 frammenti) -> NetworkSettings -> Login ->
    //    PlayStatus + ResourcePacksInfo -> "ho tutto" -> ResourcePackStack -> "completato" -> StartGame + oggetti ->
    //    richiesta chunk -> raggio + PlayerSpawn + contenuto creativo -> "inizializzato" -> chunk -> chat -> /list -> /tps.
    const RADIUS: i32 = 4;
    const EXPECTED_CHUNKS: usize = 69; // punti interi con x² + z² <= r² + r in [-4, 4]²
    let (start, mut buf, mut session, mut stage, mut chunks) = (Instant::now(), vec![0u8; 2048], Session::default(), 0, 0);
    let mut pending: Vec<u8> = Vec::new();
    let mut sync = [0usize; 4];
    let mut player_lists = 0usize; // PlayerList con il giocatore stesso, una sola volta
                                   // Solo `Mode::Generated`: chunk e biomi ricevuti e centro (in chunk) dopo il movimento.
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    let (mut biomes, mut moved_to) = (false, None::<(i32, i32)>);
    let limit = Duration::from_secs(if mode == Mode::Generated { 30 } else { 10 });
    while start.elapsed() < limit {
        let deadline = match rtc.poll_output().unwrap() {
            Output::Timeout(t) => t,
            Output::Transmit(t) => {
                sock.send_to(&t.contents, t.destination).unwrap();
                continue;
            }
            Output::Event(Event::ChannelOpen(id, _)) if id == reliable => {
                let request = Packet::new(id::REQUEST_NETWORK_SETTINGS, RequestNetworkSettings { protocol: PROTOCOL as i32 }.encode());
                let msg = Session::default().encode(&[request]);
                rtc.channel(reliable).unwrap().write(true, &[&[1u8][..], &msg[..3]].concat()).unwrap();
                rtc.channel(reliable).unwrap().write(true, &[&[0u8][..], &msg[3..]].concat()).unwrap();
                continue;
            }
            Output::Event(Event::ChannelData(d)) => {
                // I messaggi grandi arrivano in più frammenti: il client li riassembla come fa Minecraft.
                pending.extend_from_slice(&d.data[1..]);
                if d.data[0] != 0 {
                    continue;
                }
                let packets = session.decode(&std::mem::take(&mut pending)).expect("batch non valido");
                let ids: Vec<u32> = packets.iter().map(|p| p.id).collect();
                let reply = match (stage, ids.as_slice()) {
                    (0, [id::NETWORK_SETTINGS]) => {
                        assert_eq!(packets[0].body.len(), 10);
                        session.enable_compression();
                        Packet::new(id::LOGIN, login_body()) // grande: passa anche dalla compressione
                    }
                    (1, [id::PLAY_STATUS, id::RESOURCE_PACKS_INFO]) => {
                        assert_eq!(packets[0].body, [0, 0, 0, 0], "LoginSuccess");
                        response(pack_response::DOWNLOADING_FINISHED)
                    }
                    (2, [id::RESOURCE_PACK_STACK]) => response(pack_response::STACK_FINISHED),
                    (3, ids) if ids == [JigsawStructureData::ID, VoxelShapes::ID, StartGame::ID, ItemRegistry::ID] => {
                        assert!(packets[3].body.len() > 10_000, "il registro oggetti è grande: ha attraversato la frammentazione");
                        Packet::new(id::REQUEST_CHUNK_RADIUS, RequestChunkRadius { chunk_radius: RADIUS, max_radius: 8 }.encode())
                    }
                    (4, ids)
                        if ids
                            == [
                                ChunkRadiusUpdated::ID,
                                id::PLAY_STATUS,
                                CreativeContent::ID,
                                bedrock_protocol::crafting::CraftingData::ID,
                                bedrock_protocol::commands::AvailableCommands::ID,
                                UpdateAttributes::ID,
                                bedrock_protocol::actor::SET_ACTOR_DATA,
                            ] =>
                    {
                        assert_eq!(packets[0].body, [(RADIUS * 2) as u8], "raggio concesso (ZigZag)");
                        assert_eq!(packets[1].body, [0, 0, 0, 3], "PlayerSpawn");
                        assert!(packets[2].body.len() > 50_000, "il menu creativo è pieno: ha attraversato la frammentazione");
                        // Come Minecraft: solo ora, a mondo caricato, dichiara di essere inizializzato (ID di runtime 1).
                        Packet::new(id::SET_LOCAL_PLAYER_AS_INITIALIZED, vec![1])
                    }
                    (5, _) if mode == Mode::Generated => {
                        for p in &packets {
                            match p.id {
                                x if x == BiomeDefinitionList::ID => biomes = true,
                                x if x == NetworkChunkPublisherUpdate::ID || x == id::TEXT || count_sync(x, &mut sync) => {}
                                x if x == id::PLAYER_LIST => player_lists += 1,
                                x if x == LevelChunk::ID => {
                                    let mut b = &p.body[..];
                                    let pos = (varint::read_signed(&mut b).unwrap(), varint::read_signed(&mut b).unwrap());
                                    assert!(seen.insert(pos), "chunk {pos:?} inviato due volte");
                                }
                                other => panic!("pacchetto inatteso durante lo streaming: {other}"),
                            }
                        }
                        if seen.len() < EXPECTED_CHUNKS {
                            continue;
                        }
                        // Spenta di default: il client 1.26.6x si disconnette con la nostra lista (prova del 18/11).
                        assert!(!biomes, "la BiomeDefinitionList non va mandata senza SEND=biomes");
                        assert_eq!(seen.len(), EXPECTED_CHUNKS, "chunk attorno allo spawn");
                        assert_eq!(sync, [1, 1, 3, 1], "modalità, abilità, inventario/mano secondaria/armatura e barra, una volta sola");
                        assert_eq!(player_lists, 1, "la PlayerList con il giocatore, una volta sola");
                        assert!(seen.iter().all(|&(x, z)| x.abs() <= RADIUS && z.abs() <= RADIUS), "solo chunk vicini allo spawn: {seen:?}");
                        // Sposta il giocatore di 5 chunk in X (lo spawn finto è in 8, 61, 8).
                        moved_to = Some((5, 0));
                        // PlayerAuthInput completo: rotazione, posizione, movimento, head yaw, nessun flag, modalità, rotazione di
                        // interazione, tick, delta, opzionali assenti, vettori finali.
                        let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
                        let mut body = floats(&[0.0, 0.0, 88.0, 63.0, 8.0, 0.0, 0.0, 0.0]);
                        body.extend([0, 0, 0, 0]);
                        body.extend(floats(&[0.0; 2]));
                        body.push(0);
                        body.extend(floats(&[0.0; 3]));
                        body.extend([0, 0, 0, 0, 0]);
                        body.extend(floats(&[0.0; 7]));
                        Packet::new(id::PLAYER_AUTH_INPUT, body)
                    }
                    (6, _) if mode == Mode::Generated => {
                        let before = seen.clone();
                        for p in &packets {
                            match p.id {
                                x if x == NetworkChunkPublisherUpdate::ID => {}
                                x if x == LevelChunk::ID => {
                                    let mut b = &p.body[..];
                                    let pos = (varint::read_signed(&mut b).unwrap(), varint::read_signed(&mut b).unwrap());
                                    assert!(seen.insert(pos), "chunk {pos:?} rimandato dopo il movimento");
                                }
                                other => panic!("pacchetto inatteso dopo il movimento: {other}"),
                            }
                        }
                        // Il bordo nuovo (x = 5 + RADIUS) deve arrivare, e ogni chunk nuovo sta nel raggio del nuovo centro.
                        let (cx, cz) = moved_to.unwrap();
                        assert!(seen.iter().filter(|c| !before.contains(c)).all(|&(x, z)| (x - cx).pow(2) + (z - cz).pow(2) <= RADIUS * RADIUS + RADIUS));
                        if !seen.contains(&(cx + RADIUS, cz)) {
                            continue;
                        }
                        assert!(seen.len() > EXPECTED_CHUNKS, "dopo il movimento servono chunk nuovi");
                        return;
                    }
                    (5, _) => {
                        // Dopo l'inizializzazione: benvenuto e annuncio di ingresso (testo), biomi, pubblicazione e chunk.
                        for p in &packets {
                            match p.id {
                                x if x == NetworkChunkPublisherUpdate::ID || x == BiomeDefinitionList::ID || count_sync(x, &mut sync) => {}
                                x if x == id::PLAYER_LIST => player_lists += 1,
                                id::TEXT => {}
                                x if x == LevelChunk::ID => chunks += 1,
                                other => panic!("pacchetto inatteso durante lo streaming: {other}"),
                            }
                        }
                        if chunks != EXPECTED_CHUNKS {
                            continue;
                        }
                        assert_eq!(sync, [1, 1, 3, 1], "modalità, abilità, inventario/mano secondaria/armatura e barra, una volta sola");
                        assert_eq!(player_lists, 1, "la PlayerList con il giocatore, una volta sola");
                        // Chat: il client si spaccia per "Hacker", ma il server usa il nome verificato dal login.
                        Packet::new(id::TEXT, Text::chat("Hacker", "ciao").encode())
                    }
                    (6, ids) if ids == [id::TEXT] => {
                        let text = Text::decode(&packets[0].body).expect("testo illeggibile");
                        assert_eq!((text.kind, text.source.as_str(), text.message.as_str()), (text_type::CHAT, "Tester", "ciao"));
                        command("/list")
                    }
                    (7, ids) if ids == [id::TEXT] => {
                        let text = Text::decode(&packets[0].body).expect("testo illeggibile");
                        assert!(text.message.contains("Giocatori online (1): Tester"), "{}", text.message);
                        command("/tps")
                    }
                    (8, ids) if ids == [id::TEXT] => {
                        let text = Text::decode(&packets[0].body).expect("testo illeggibile");
                        assert!(text.message.starts_with("TPS: "), "{}", text.message);
                        return;
                    }
                    _ => panic!("al passo {stage} sono arrivati pacchetti inattesi: {ids:?}"),
                };
                let msg = session.encode(&[reply]);
                rtc.channel(reliable).unwrap().write(true, &[&[0u8][..], &msg].concat()).unwrap();
                stage += 1;
                continue;
            }
            Output::Event(_) => continue,
        };
        sock.set_read_timeout(Some(deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)))).unwrap();
        let now = Instant::now();
        match sock.recv_from(&mut buf) {
            Ok((n, source)) => {
                let receive = Receive { proto: Protocol::Udp, source, destination: local, contents: buf[..n].try_into().unwrap() };
                rtc.handle_input(Input::Receive(now, receive)).unwrap();
            }
            Err(_) => rtc.handle_input(Input::Timeout(now)).unwrap(),
        }
    }
    panic!("flusso non completato entro {limit:?} (fermo al passo {stage})");
}

// ---------- generatore finto ----------

/// NBT su disco little-endian `{name, states: {}}` di un blocco senza stati e il suo hash FNV-1a (l'ID di rete).
fn fake_block(name: &str) -> (i32, Vec<u8>) {
    let mut nbt = vec![0x0a, 0, 0, 0x08, 4, 0];
    nbt.extend_from_slice(b"name");
    nbt.extend_from_slice(&(name.len() as u16).to_le_bytes());
    nbt.extend_from_slice(name.as_bytes());
    nbt.extend_from_slice(&[0x0a, 6, 0]);
    nbt.extend_from_slice(b"states");
    nbt.extend_from_slice(&[0, 0]);
    let hash = nbt.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193)) as i32;
    (hash, nbt)
}

/// Chunk deterministico: pietra fino a y=59, erba a y=60, aria sopra; bioma 1 (pianura).
fn fake_chunk(dim: vgen::Dim, x: i32, z: i32, palette: &[i32]) -> vgen::ChunkData {
    let sections = (-4..=3i8)
        .map(|index| {
            let mut blocks = Box::new([0u16; 4096]);
            for (n, b) in blocks.iter_mut().enumerate() {
                let y = index as i32 * 16 + (n % 16) as i32;
                *b = match y {
                    ..=59 => 1,
                    60 => 2,
                    _ => 0,
                };
            }
            vgen::Section { index, palette: palette.to_vec(), blocks, biomes: Box::new([1; 64]), water: None }
        })
        .collect();
    vgen::ChunkData { dim, x, z, sections, block_entities: Vec::new() }
}

/// Generatore finto in un thread: risponde a Hello (con la palette aria/pietra/erba), Generate e Spawn (8, 61, 8).
fn start_fake_generator() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let blocks = [fake_block("minecraft:air"), fake_block("minecraft:stone"), fake_block("minecraft:grass_block")];
                let palette: Vec<i32> = blocks.iter().map(|b| b.0).collect();
                while let Ok(frame) = vgen::read_frame(&mut stream) {
                    let replies = match vgen::ToGen::decode(&frame).expect("messaggio del server illeggibile") {
                        vgen::ToGen::Hello { .. } => vec![vgen::FromGen::Hello { version: vgen::VERSION }, vgen::FromGen::Palette { entries: blocks.to_vec() }],
                        vgen::ToGen::Generate { req, dim, x, z } => vec![vgen::FromGen::Chunk { req, data: fake_chunk(dim, x, z, &palette) }],
                        vgen::ToGen::Spawn { req, .. } => vec![vgen::FromGen::Spawn { req, x: 8, y: 61, z: 8 }],
                    };
                    for r in replies {
                        if vgen::write_frame(&mut stream, &r.encode()).is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    addr
}

/// Cartella temporanea del mondo del test, rimossa alla fine.
struct TempWorld(PathBuf);

impl Drop for TempWorld {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn client_joins_the_world_and_receives_chunks() {
    tokio::spawn(bedrock_server::run(bedrock_server::config(PORT)));
    tokio::task::spawn_blocking(|| client(PORT, Mode::Flat)).await.unwrap();
}

/// Con un generatore (finto): arrivano biomi e chunk attorno allo spawn dato dal generatore e,
/// dopo un `PlayerAuthInput` spostato di 5 chunk in X, i chunk del nuovo bordo (e nessuno già inviato).
#[tokio::test(flavor = "multi_thread")]
async fn generated_world_streams_chunks_while_the_player_moves() {
    let gen_addr = start_fake_generator();
    let world = TempWorld(std::env::temp_dir().join(format!("vgen-e2e-{}", std::process::id())));
    let server_cfg = ServerConfig { seed: Some(13579), view_distance: 4, port: GEN_PORT, ..ServerConfig::default() };
    let dir = world.0.clone();
    tokio::spawn(async move { bedrock_server::run_with_generator_at(bedrock_server::config(GEN_PORT), &server_cfg, &dir, gen_addr).await });
    tokio::task::spawn_blocking(|| client(GEN_PORT, Mode::Generated)).await.unwrap();
}
