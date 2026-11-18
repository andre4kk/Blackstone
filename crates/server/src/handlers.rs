//! Un handler per ogni pacchetto che il server sa gestire.
use crate::{inventory::HOTBAR_SLOTS, playerdata, Player, Server, HEX_LOGS};
use bedrock_net::log;
use bedrock_protocol::{
    generated::MobEquipment,
    id,
    input::{flag, use_item, InventoryTransaction, ItemStackRequestPacket, PlayerAction, PlayerAuthInput, StackRequest},
    login,
    packets::{
        disconnect_reason, pack_response, play_status, text_type, Disconnect, Login, NetworkSettings, PacketViolationWarning, PlayStatus,
        RequestNetworkSettings, ResourcePackClientResponse, ResourcePackStack, ResourcePacksInfo, Text,
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
        self.waiting_start.retain(|&w| w != pid);
        if let Some(mut p) = self.players.remove(&pid) {
            playerdata::persist(self.disk.as_ref(), &mut p);
            if p.spawned {
                self.broadcast(Text::new(text_type::RAW, format!("§e{name} ha lasciato il gioco")));
            }
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
            id::TEXT
            | id::COMMAND_REQUEST
            | id::SET_PLAYER_GAME_TYPE
            | id::ITEM_STACK_REQUEST
            | id::MOB_EQUIPMENT
            | id::PLAYER_ACTION
            | id::INVENTORY_TRANSACTION => spawned,
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
            id::SET_PLAYER_GAME_TYPE => self.client_game_type(pid, &packet.body),
            // Pacchetti di gioco che il client manda di continuo e che arriveranno con movimento, blocchi e inventario:
            // per ora si ignorano in silenzio, altrimenti il log si riempie.
            id::PLAYER_AUTH_INPUT => self.player_auth_input(pid, &packet.body),
            id::ITEM_STACK_REQUEST => self.item_stack_request(pid, &packet.body),
            id::MOB_EQUIPMENT => self.mob_equipment(pid, &packet.body),
            id::PLAYER_ACTION => self.player_action(pid, &packet.body),
            id::INVENTORY_TRANSACTION => self.inventory_transaction(pid, &packet.body),
            id::ANIMATE | id::INTERACT | id::EMOTE_LIST => {}
            id::SERVERBOUND_LOADING_SCREEN | id::CLIENT_CACHE_STATUS => {} // il client dichiara se usa la cache dei chunk: per ora non serve
            id::PACKET_VIOLATION_WARNING => self.packet_violation(pid, &packet.body),
            other => {
                let body = &packet.body;
                let hex = if body.len() <= 80 { format!(": {body:02x?}") } else { String::new() };
                log!("[#{pid}] pacchetto {other} non gestito ({} byte){hex}", body.len());
            }
        }
    }

    /// Input del giocatore: la posizione e l'eventuale richiesta di pila (flag 36). I valori non finiti o fuori dal mondo si scartano.
    /// Se il corpo non si legge per intero (il layout dopo la posizione non è verificato sul client reale) si logga
    /// la prima volta e si usa comunque la posizione: il movimento non deve dipendere dal resto del pacchetto.
    fn player_auth_input(&mut self, pid: u64, body: &[u8]) {
        let Some(player) = self.players.get_mut(&pid).filter(|p| p.spawned) else { return };
        let (position, request, input) = match PlayerAuthInput::decode(body) {
            Some(input) => {
                let has_actions = [flag::PERFORM_ITEM_INTERACTION, flag::PERFORM_BLOCK_ACTIONS, flag::PERFORM_ITEM_STACK_REQUEST].iter().any(|&f| input.has(f));
                if has_actions && player.action_logs < HEX_LOGS {
                    player.action_logs += 1;
                    log!("[#{pid}] PlayerAuthInput con azioni (flag {:#x}, {} byte): {:02x?}", input.flags, body.len(), &body[..body.len().min(400)]);
                }
                // la rotazione e l'accovacciamento servono a orientare i blocchi che il giocatore piazza
                if input.pitch.is_finite() && input.yaw.is_finite() {
                    (player.pitch, player.yaw) = (input.pitch, input.yaw);
                }
                player.sneaking = input.has(flag::SNEAKING);
                (input.position, input.stack_request.clone(), Some(input))
            }
            None => {
                if !std::mem::replace(&mut player.pai_logged, true) {
                    let head = &body[..body.len().min(64)];
                    log!("[#{pid}] PlayerAuthInput illeggibile ({} byte): {head:02x?}", body.len());
                }
                let Some((pitch, yaw, position)) = PlayerAuthInput::decode_rotation_position(body) else { return };
                if pitch.is_finite() && yaw.is_finite() {
                    (player.pitch, player.yaw) = (pitch, yaw);
                }
                (position, None, None)
            }
        };
        // Limite del mondo di Minecraft: 30 milioni di blocchi per lato.
        if position.iter().all(|v| v.is_finite() && v.abs() <= 30_000_000.0) {
            self.on_move(pid, position);
        }
        if let Some(request) = request {
            self.stack_request(pid, &request);
        }
        let Some(input) = input else { return };
        // Prima le azioni di rottura nell'ordine in cui il client le ha scritte, poi l'uso di un oggetto su un blocco.
        if input.has(flag::PERFORM_BLOCK_ACTIONS) {
            for a in &input.block_actions {
                self.block_action(pid, a.action, a.pos);
            }
        }
        if let Some(u) = input.item_use.as_ref().filter(|u| input.has(flag::PERFORM_ITEM_INTERACTION) && u.action == use_item::DESTROY) {
            self.break_block(pid, u.pos);
        }
    }

    /// Pacchetto `PlayerAction`: inizio, fine e abbandono della rottura di un blocco, e rottura creativa.
    fn player_action(&mut self, pid: u64, body: &[u8]) {
        match PlayerAction::decode(body) {
            Some(a) => self.block_action(pid, a.action, a.pos),
            None => log!("[#{pid}] PlayerAction illeggibile ({} byte): ignorato", body.len()),
        }
    }

    /// Pacchetto `InventoryTransaction`: l'uso di un oggetto con azione Destroy rompe un blocco, con azione Place attiva il blocco
    /// cliccato (porta, leva...) o ne piazza uno. Gli altri usi e gli altri tipi di transazione non servono ancora.
    fn inventory_transaction(&mut self, pid: u64, body: &[u8]) {
        let Some(transaction) = InventoryTransaction::decode(body) else {
            return log!("[#{pid}] InventoryTransaction illeggibile ({} byte): ignorata", body.len());
        };
        match transaction.use_item {
            Some(u) if u.action == use_item::DESTROY => self.break_block(pid, u.pos),
            Some(u) if u.action == use_item::PLACE => self.place_item(pid, &u),
            _ => {}
        }
    }

    /// Pacchetto `ItemStackRequest`: ogni richiesta ha la sua risposta.
    fn item_stack_request(&mut self, pid: u64, body: &[u8]) {
        if let Some(p) = self.players.get_mut(&pid).filter(|p| p.stack_logs < HEX_LOGS) {
            p.stack_logs += 1;
            log!("[#{pid}] ItemStackRequest ({} byte): {:02x?}", body.len(), &body[..body.len().min(400)]);
        }
        let Some(packet) = ItemStackRequestPacket::decode(body) else {
            return log!("[#{pid}] ItemStackRequest illeggibile ({} byte): ignorato", body.len());
        };
        for request in &packet.requests {
            self.stack_request(pid, request);
        }
    }

    /// Una richiesta di pila: si applica all'inventario e si risponde (con le caselle cambiate, o con l'errore e lo stato vero).
    fn stack_request(&mut self, pid: u64, request: &StackRequest) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        let result = p.inventory.handle(request, p.mode.creative_inventory());
        if let Some(error) = &result.error {
            log!("[#{pid}] richiesta di pila {} rifiutata: {error}", request.request_id);
        }
        let packets = p.inventory.response(request.request_id, &result);
        self.send(pid, &packets);
    }

    /// Il giocatore cambia la casella della barra in mano. Caselle fuori dalla barra si ignorano.
    fn mob_equipment(&mut self, pid: u64, body: &[u8]) {
        let Some(equipment) = MobEquipment::decode(body) else {
            return log!("[#{pid}] MobEquipment illeggibile ({} byte): ignorato", body.len());
        };
        let Some(p) = self.players.get_mut(&pid) else { return };
        if equipment.selected_slot < HOTBAR_SLOTS {
            p.inventory.selected = equipment.selected_slot;
        } else {
            log!("[#{pid}] MobEquipment con la casella {} fuori dalla barra: ignorato", equipment.selected_slot);
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
                log!(
                    "[#{pid}] giocatore {} (XUID {:?}, autenticazione {:?}, dispositivo {:?}, NON verificato)",
                    info.name,
                    info.xuid,
                    info.authentication_type,
                    info.device_os
                );
                if info.xuid.is_none() {
                    log!("[#{pid}] XUID non trovato: chiavi del Token = {:?}", info.token_claim_keys);
                }
                self.kick_same_identity(pid, &info);
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

    /// Come in Minecraft vince l'ultimo ad entrare: chi è già connesso con la stessa identità viene disconnesso prima, passando
    /// dalla chiusura normale, così i suoi dati sono sul disco (e davanti nella coda del disco) prima che il nuovo li chieda.
    /// Senza questo due sessioni leggerebbero lo stesso salvataggio e l'ultima a uscire cancellerebbe le modifiche dell'altra.
    fn kick_same_identity(&mut self, pid: u64, info: &login::PlayerInfo) {
        let identity = playerdata::identity(info);
        let old: Vec<u64> = self
            .players
            .iter()
            .filter(|(&other, p)| other != pid && p.info.as_ref().is_some_and(|i| playerdata::identity(i) == identity))
            .map(|(&other, _)| other)
            .collect();
        for other in old {
            log!("[#{other}] stessa identità di #{pid} ({identity}): disconnesso, vale il nuovo ingresso");
            self.disconnect(other, "Sei entrato da un'altra posizione");
            self.close(other); // salva i dati e toglie il giocatore: il Close di rete che arriva dopo non trova più nulla
            self.sender.kick(other);
        }
    }

    /// Il client dice che un pacchetto che gli abbiamo mandato è malformato: lo registriamo per intero.
    fn packet_violation(&self, pid: u64, body: &[u8]) {
        match PacketViolationWarning::decode(body) {
            Some(w) => {
                log!(
                    "[#{pid}] !! IL CLIENT SEGNALA UN PACCHETTO MALFORMATO: id {} (tipo {}, gravità {}), contesto: {:?}",
                    w.packet_id,
                    w.violation_type,
                    w.severity,
                    w.context
                )
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
