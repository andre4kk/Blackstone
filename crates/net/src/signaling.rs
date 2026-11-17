//! Signaling HTTP di NetherNet: GET /v1/join (info del server) e POST /v1/join/{id} (offerta SDP -> risposta SDP).
use crate::{media::Client, Shared};
use serde_json::json;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{atomic::Ordering::Relaxed, Arc},
    time::{Duration, Instant},
};
use str0m::{change::SdpOffer, Candidate, Rtc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc::UnboundedSender,
};

const MAX_HEADERS: usize = 64 * 1024;
const MAX_BODY: usize = 1 << 20;
/// Tempo massimo per ricevere una richiesta completa: chi si connette e tace non tiene occupato il server.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn serve(tcp: TcpListener, shared: Arc<Shared>, new_clients: UnboundedSender<Client>) {
    while let Ok((stream, _)) = tcp.accept().await {
        let (shared, tx) = (shared.clone(), new_clients.clone());
        tokio::spawn(async move { handle(stream, &shared, &tx).await });
    }
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

/// Mini-parser HTTP/1.1: basta per il signaling. Ritorna `None` per qualunque cosa non sia HTTP valido
/// (in particolare i tentativi TLS: il client ripiega su HTTP).
async fn read_request_within(s: &mut TcpStream, limit: Duration) -> Option<Request> {
    tokio::time::timeout(limit, read_request(s)).await.ok()?
}

async fn read_request(s: &mut TcpStream) -> Option<Request> {
    let (mut buf, mut tmp) = (Vec::new(), [0u8; 4096]);
    let end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 || buf.len() > MAX_HEADERS {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = std::str::from_utf8(&buf[..end]).ok()?;
    let mut lines = head.lines();
    let mut first = lines.next()?.split(' ');
    let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
    let len = lines.find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))?.1.trim().parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return None;
    }
    while buf.len() < end + len {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    Some(Request { method, path, body: buf[end..end + len].to_vec() })
}

async fn respond(s: &mut TcpStream, status: &str, content_type: &str, body: &str) -> Option<()> {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    s.write_all(head.as_bytes()).await.ok()?;
    s.write_all(body.as_bytes()).await.ok()
}

async fn handle(mut s: TcpStream, shared: &Shared, new_clients: &UnboundedSender<Client>) -> Option<()> {
    let local = s.local_addr().ok()?;
    let req = read_request_within(&mut s, REQUEST_TIMEOUT).await?;
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/join") => {
            let c = &shared.cfg;
            let info = json!({
                "name": c.name, "protocol": c.protocol, "version": c.version, "level": c.level,
                "players": shared.players.load(Relaxed), "maxPlayers": c.max_players, "gameType": 0
            });
            respond(&mut s, "200 OK", "application/json", &info.to_string()).await
        }
        ("POST", p) if p.starts_with("/v1/join/") => {
            let offer = std::str::from_utf8(&req.body).ok()?;
            let answer = join(shared, new_clients, local, offer)?;
            respond(&mut s, "200 OK", "application/sdp", &answer).await
        }
        _ => respond(&mut s, "404 Not Found", "text/plain", "").await,
    }
}

/// Crea la connessione WebRTC per un'offerta SDP, la passa al ciclo di rete e ritorna la risposta firmata.
fn join(shared: &Shared, new_clients: &UnboundedSender<Client>, local: SocketAddr, offer: &str) -> Option<String> {
    // Il client firma l'offerta con a=identity: non la verifichiamo ancora, e str0m non la capisce.
    let offer: String = offer.lines().filter(|l| !l.starts_with("a=identity:")).flat_map(|l| [l, "\r\n"]).collect();
    let mut rtc = Rtc::new(Instant::now());

    // Candidati: l'IP su cui il client ci ha raggiunti + la porta UDP condivisa. Minecraft ignora i candidati
    // loopback, quindi in quel caso pubblichiamo l'IP della LAN. `public_ip` serve per giocare da Internet.
    let ip = if local.ip().is_loopback() { lan_ip().unwrap_or(local.ip()) } else { local.ip() };
    let addr = SocketAddr::new(ip, shared.cfg.port);
    rtc.add_local_candidate(Candidate::host(addr, "udp").ok()?);
    if let Some(public) = shared.cfg.public_ip {
        rtc.add_local_candidate(Candidate::host(SocketAddr::new(public, shared.cfg.port), "udp").ok()?);
    }

    let answer = match rtc.sdp_api().accept_offer(SdpOffer::from_sdp_string(&offer).ok()?) {
        Ok(a) => a.to_sdp_string(),
        Err(e) => {
            crate::log!("signaling: offerta rifiutata: {e}");
            return None;
        }
    };
    let id = shared.next_id.fetch_add(1, Relaxed);
    new_clients.send(Client::new(id, rtc, addr)).ok()?;
    crate::log!("[#{id}] signaling OK, candidato {addr}");
    Some(shared.identity.sign_answer(&answer))
}

/// IP della LAN di questa macchina (nessun pacchetto viene inviato: connect() su UDP sceglie solo la route).
pub fn lan_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn silent_connections_are_dropped_after_the_timeout() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let _client = TcpStream::connect(tcp.local_addr().unwrap()).await.unwrap(); // si connette e tace
        let (mut server_side, _) = tcp.accept().await.unwrap();
        let started = std::time::Instant::now();
        assert!(read_request_within(&mut server_side, Duration::from_millis(100)).await.is_none());
        assert!(started.elapsed() < Duration::from_secs(2), "deve arrendersi al timeout, non aspettare per sempre");
    }

    #[tokio::test]
    async fn reads_a_complete_request() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(tcp.local_addr().unwrap()).await.unwrap();
        client.write_all(b"POST /v1/join/1 HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc").await.unwrap();
        let (mut server_side, _) = tcp.accept().await.unwrap();
        let req = read_request_within(&mut server_side, Duration::from_secs(1)).await.unwrap();
        assert_eq!((req.method.as_str(), req.path.as_str(), &req.body[..]), ("POST", "/v1/join/1", &b"abc"[..]));
    }
}
