//! Test end-to-end: un client WebRTC che si comporta come Minecraft contro il server vero
//! (signaling HTTP, identità firmata, ICE/DTLS, data channel, frammentazione, compressione, login, resource pack,
//! StartGame, registro oggetti, chunk e spawn).
use base64::{
    engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64U},
    Engine,
};
use bedrock_protocol::{
    generated::{ChunkRadiusUpdated, CreativeContent, ItemRegistry, JigsawStructureData, LevelChunk, NetworkChunkPublisherUpdate, StartGame, VoxelShapes},
    id,
    packets::{pack_response, text_type, CommandRequest, Login, RequestChunkRadius, RequestNetworkSettings, ResourcePackClientResponse, Text},
    wire::ServerPacket,
    Packet, Session, PROTOCOL,
};
use p384::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use std::{
    io::{Read, Write},
    net::{TcpStream, UdpSocket},
    time::{Duration, Instant},
};
use str0m::{
    change::SdpAnswer,
    channel::{ChannelConfig, Reliability},
    net::{Protocol, Receive},
    Candidate, Event, Input, Output, Rtc,
};

const PORT: u16 = 29132;

/// Richiesta HTTP minimale. Ritorna (status line, corpo).
fn http(method: &str, path: &str, body: &str) -> (String, String) {
    let mut s = (0..50)
        .find_map(|_| {
            TcpStream::connect(("127.0.0.1", PORT)).ok().or_else(|| {
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

fn client() {
    // 1. Info del server.
    let (status, body) = http("GET", "/v1/join", "");
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

    let (status, answer) = http("POST", "/v1/join/123456", &offer.to_sdp_string());
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
    while start.elapsed() < Duration::from_secs(10) {
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
                    (4, ids) if ids == [ChunkRadiusUpdated::ID, id::PLAY_STATUS, CreativeContent::ID] => {
                        assert_eq!(packets[0].body, [(RADIUS * 2) as u8], "raggio concesso (ZigZag)");
                        assert_eq!(packets[1].body, [0, 0, 0, 3], "PlayerSpawn");
                        // Come Minecraft: solo ora, a mondo caricato, dichiara di essere inizializzato (ID di runtime 1).
                        Packet::new(id::SET_LOCAL_PLAYER_AS_INITIALIZED, vec![1])
                    }
                    (5, _) => {
                        // Dopo l'inizializzazione: benvenuto e annuncio di ingresso (testo), pubblicazione e chunk.
                        for p in &packets {
                            match p.id {
                                x if x == NetworkChunkPublisherUpdate::ID => {}
                                id::TEXT => {}
                                x if x == LevelChunk::ID => chunks += 1,
                                other => panic!("pacchetto inatteso durante lo streaming: {other}"),
                            }
                        }
                        if chunks != EXPECTED_CHUNKS {
                            continue;
                        }
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
    panic!("flusso non completato entro 10 secondi (fermo al passo {stage})");
}

#[tokio::test(flavor = "multi_thread")]
async fn client_joins_the_world_and_receives_chunks() {
    tokio::spawn(bedrock_server::run(bedrock_server::config(PORT)));
    tokio::task::spawn_blocking(client).await.unwrap();
}
