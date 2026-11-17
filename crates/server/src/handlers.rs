//! Un handler per ogni pacchetto che il server sa gestire.
use crate::{Player, Server};
use bedrock_net::log;
use bedrock_protocol::{
    id, login,
    packets::{
        disconnect_reason, pack_response, play_status, text_type, Disconnect, Login, NetworkSettings, PacketViolationWarning, PlayStatus, RequestNetworkSettings,
        ResourcePackClientResponse, ResourcePackStack, ResourcePacksInfo, Text,
    },
    Packet, PROTOCOL, VERSION,
};

impl Server {
    pub(crate) fn open(&mut self, pid: u64) {
        log!("[#{pid}] connesso");
        self.players.insert(pid, Player::default());
    }

    pub(crate) fn close(&mut self, pid: u64) {
        log!("[#{pid}] disconnesso");
        let name = self.name(pid);
        if self.players.remove(&pid).is_some_and(|p| p.spawned) {
            self.broadcast(Text::new(text_type::RAW, format!("§e{name} ha lasciato il gioco")));
        }
    }

    pub(crate) fn message(&mut self, pid: u64, data: &[u8]) {
        let Some(player) = self.players.get(&pid) else { return };
        match player.session.decode(data) {
            Some(packets) => packets.into_iter().for_each(|p| self.handle(pid, p)),
            None => {
                log!("[#{pid}] batch non valido, lo disconnetto");
                self.sender.kick(pid);
            }
        }
    }

    fn handle(&mut self, pid: u64, packet: Packet) {
        // Ogni pacchetto è ammesso solo nella fase giusta: prima del login non si chiedono chunk, prima di entrare nel
        // mondo non si scrive in chat. Così un client modificato non può saltare passaggi.
        let (logged_in, spawned) = self.players.get(&pid).map_or((false, false), |p| (p.info.is_some(), p.spawned));
        let allowed = match packet.id {
            id::LOGIN => !logged_in,
            id::RESOURCE_PACK_CLIENT_RESPONSE | id::REQUEST_CHUNK_RADIUS | id::SET_LOCAL_PLAYER_AS_INITIALIZED => logged_in,
            id::TEXT | id::COMMAND_REQUEST => spawned,
            _ => true,
        };
        if !allowed {
            return log!("[#{pid}] pacchetto {} fuori sequenza (login: {logged_in}, nel mondo: {spawned}): ignorato", packet.id);
        }
        match packet.id {
            id::REQUEST_NETWORK_SETTINGS => self.request_network_settings(pid, &packet.body),
            id::LOGIN => self.login(pid, &packet.body),
            id::RESOURCE_PACK_CLIENT_RESPONSE => self.resource_pack_response(pid, &packet.body),
            id::REQUEST_CHUNK_RADIUS => self.request_chunk_radius(pid, &packet.body),
            id::SET_LOCAL_PLAYER_AS_INITIALIZED => self.initialized(pid),
            id::TEXT => self.chat(pid, &packet.body),
            id::COMMAND_REQUEST => self.command(pid, &packet.body),
            // Pacchetti di gioco che il client manda di continuo e che arriveranno con movimento, blocchi e inventario:
            // per ora si ignorano in silenzio, altrimenti il log si riempie.
            id::PLAYER_AUTH_INPUT | id::ANIMATE | id::PLAYER_ACTION | id::MOB_EQUIPMENT | id::INTERACT | id::EMOTE_LIST => {}
            id::SERVERBOUND_LOADING_SCREEN | id::CLIENT_CACHE_STATUS => {} // il client dichiara se usa la cache dei chunk: per ora non serve
            id::PACKET_VIOLATION_WARNING => self.packet_violation(pid, &packet.body),
            other => {
                let body = &packet.body;
                let hex = if body.len() <= 80 { format!(": {body:02x?}") } else { String::new() };
                log!("[#{pid}] pacchetto {other} non gestito ({} byte){hex}", body.len());
            }
        }
    }

    fn request_network_settings(&mut self, pid: u64, body: &[u8]) {
        let Some(req) = RequestNetworkSettings::decode(body) else { return };
        log!("[#{pid}] RequestNetworkSettings: protocollo client = {}", req.protocol);
        if req.protocol != PROTOCOL as i32 {
            log!("[#{pid}] attenzione: il server supporta il protocollo {PROTOCOL}");
            // TODO: PlayStatus di versione errata
        }
        self.send(pid, &[Packet::new(id::NETWORK_SETTINGS, NetworkSettings::default().encode())]);
        // NetworkSettings viaggia senza compressione: da qui in poi ogni batch ha il byte di compressione.
        if let Some(p) = self.players.get_mut(&pid) {
            p.session.enable_compression();
        }
    }

    fn login(&mut self, pid: u64, body: &[u8]) {
        let Some(request) = Login::decode(body) else {
            log!("[#{pid}] login malformato, lo disconnetto");
            return self.sender.kick(pid);
        };
        log!("[#{pid}] LOGIN ricevuto: protocollo {}", request.protocol);
        if request.protocol != PROTOCOL as i32 {
            return self.disconnect(pid, "Versione del gioco non supportata da questo server");
        }
        match login::parse(&request) {
            Ok(info) => {
                log!("[#{pid}] giocatore {} (XUID {:?}, autenticazione {:?}, dispositivo {:?}, NON verificato)", info.name, info.xuid, info.authentication_type, info.device_os);
                if info.xuid.is_none() {
                    log!("[#{pid}] XUID non trovato: chiavi del Token = {:?}", info.token_claim_keys);
                }
                if let Some(p) = self.players.get_mut(&pid) {
                    p.info = Some(info);
                }
                // Login accettato: si passa ai resource pack (per ora nessuno).
                self.send(
                    pid,
                    &[
                        Packet::new(id::PLAY_STATUS, PlayStatus { status: play_status::LOGIN_SUCCESS }.encode()),
                        Packet::new(id::RESOURCE_PACKS_INFO, ResourcePacksInfo::default().encode()),
                    ],
                );
            }
            Err(e) => {
                log!("[#{pid}] login non leggibile: {e}");
                self.disconnect(pid, "Login non valido");
            }
        }
    }

    /// Il client dice che un pacchetto che gli abbiamo mandato è malformato: lo registriamo per intero.
    fn packet_violation(&self, pid: u64, body: &[u8]) {
        match PacketViolationWarning::decode(body) {
            Some(w) => {
                log!("[#{pid}] !! IL CLIENT SEGNALA UN PACCHETTO MALFORMATO: id {} (tipo {}, gravità {}), contesto: {:?}", w.packet_id, w.violation_type, w.severity, w.context)
            }
            None => log!("[#{pid}] !! violazione segnalata dal client, ma illeggibile: {body:02x?}"),
        }
    }

    /// Il client risponde ai resource pack: "ho tutto" -> si manda la pila; "completato" -> pronto per StartGame.
    fn resource_pack_response(&mut self, pid: u64, body: &[u8]) {
        let Some(response) = ResourcePackClientResponse::decode(body) else {
            return log!("[#{pid}] resource pack: risposta illeggibile ({body:02x?})");
        };
        log!("[#{pid}] resource pack: {} (tag {})", response.response_type, response.tag);
        match response.tag {
            pack_response::DOWNLOADING_FINISHED => {
                log!("[#{pid}] resource pack: il client ha tutto, mando la pila");
                self.send(pid, &[Packet::new(id::RESOURCE_PACK_STACK, ResourcePackStack { must_accept: false, base_game_version: VERSION }.encode())]);
            }
            pack_response::STACK_FINISHED => {
                log!("[#{pid}] resource pack completati: entro nel mondo");
                self.start_game(pid);
            }
            other => log!("[#{pid}] resource pack: tag {other} non gestito ({} id)", response.pack_ids.len()),
        }
    }

    /// Mostra `message` al giocatore e chiude: il client chiude la connessione appena lo riceve.
    fn disconnect(&self, pid: u64, message: &str) {
        let packet = Disconnect { reason: disconnect_reason::UNKNOWN, message: Some(message) };
        self.send(pid, &[Packet::new(id::DISCONNECT, packet.encode())]);
    }
}
