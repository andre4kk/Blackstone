//! Un handler per ogni pacchetto che il server sa gestire.
use crate::{
    chest::CONTAINER_TYPE_CONTAINER,
    entity::{armor_of, EntityKind},
    events::WorldAction,
    inventory::{window, CONTAINER_TYPE_WORKBENCH, HOTBAR_SLOTS},
    playerdata::{self, EYE_HEIGHT},
    survival, GameMode, Player, Server, HEX_LOGS,
};
use bedrock_net::log;
use bedrock_protocol::{
    entity::animate_swing,
    generated::{BlockPos, ContainerClose, ContainerOpen, MobEquipment},
    id,
    input::{
        action_source, flag, interact_action, player_action, respawn_state, use_entity, use_item, Animate, Interact, InventoryAction, InventoryTransaction,
        ItemStackRequestPacket, LegacySlots, PlayerAction, PlayerAuthInput, Respawn, StackAction, StackRequest,
    },
    login,
    packets::{
        disconnect_reason, pack_response, play_status, text_type, Disconnect, Login, NetworkSettings, PacketViolationWarning, PlayStatus,
        RequestNetworkSettings, ResourcePackClientResponse, ResourcePackStack, ResourcePacksInfo, Text,
    },
    skin::{player_skin_packet, PlayerSkinChange},
    wire::ServerPacket,
    Packet, PROTOCOL, VERSION,
};

/// Tipo di contenitore dell'inventario del giocatore in `ContainerOpen` (-1 nel protocollo).
const CONTAINER_TYPE_INVENTORY: u8 = 0xff;

/// Dopo quanti tick (5 s) un teletrasporto non raggiunto smette di bloccare il movimento.
const TELEPORT_TIMEOUT: u64 = 100;

impl Server {
    pub(crate) fn open(&mut self, pid: u64) {
        log!("[#{pid}] connesso");
        self.players.insert(pid, Player::default());
    }

    pub(crate) fn close(&mut self, pid: u64) {
        log!("[#{pid}] disconnesso");
        let name = self.name(pid);
        self.waiting_start.retain(|&w| w != pid);
        self.close_chest_view(pid); // l'animazione del forziere si chiude se era l'ultimo a guardarlo
        self.return_ui_items(pid); // la griglia di crafting aperta non si perde
        if let Some(mut p) = self.players.remove(&pid) {
            survival::settle_dead(&mut p, self.spawn);
            playerdata::persist(self.disk.as_ref(), &mut p);
            if p.spawned {
                self.broadcast(Text::new(text_type::RAW, format!("§e{name} ha lasciato il gioco")));
            }
        }
        self.remove_player_entity(pid);
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
            | id::ANIMATE
            | id::PLAYER_SKIN
            | id::PLAYER_ACTION
            | id::RESPAWN
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
            id::ANIMATE => self.animate(pid, &packet.body),
            id::PLAYER_SKIN => self.player_skin(pid, &packet.body),
            id::INTERACT => self.interact_packet(pid, &packet.body),
            id::CONTAINER_CLOSE => self.container_close(pid, &packet.body),
            id::PLAYER_ACTION => self.player_action(pid, &packet.body),
            id::RESPAWN => self.respawn_packet(pid, &packet.body),
            id::INVENTORY_TRANSACTION => self.inventory_transaction(pid, &packet.body),
            // 307 SetPlayerInventoryOptions (schede del menu), 358 ServerboundCursorItemDrag: solo interfaccia del client.
            id::EMOTE_LIST | 307 | 358 => {}
            id::SERVERBOUND_LOADING_SCREEN | id::CLIENT_CACHE_STATUS => {} // il client dichiara se usa la cache dei chunk: per ora non serve
            id::PACKET_VIOLATION_WARNING => self.packet_violation(pid, &packet.body),
            other => {
                let body = &packet.body;
                let hex = if body.len() <= 80 { format!(": {body:02x?}") } else { String::new() };
                log!("[#{pid}] pacchetto {other} non gestito ({} byte){hex}", body.len());
            }
        }
    }

    /// Porta nell'entità del giocatore la rotazione (già letta in `Player`) e, se il pacchetto si è letto per intero, testa e contatto col suolo.
    /// Segna `moved` solo se qualcosa è cambiato davvero (per la posizione ci pensa `on_move`).
    fn update_entity_motion(&mut self, pid: u64, input: Option<&PlayerAuthInput>) {
        let Some(p) = self.players.get(&pid) else { return };
        let (pitch, yaw) = (p.pitch, p.yaw);
        let Some(e) = self.entities.get_mut(pid) else { return };
        let head_yaw = input.map(|i| i.head_yaw).filter(|v| v.is_finite()).unwrap_or(e.head_yaw);
        let on_ground = input.map_or(e.on_ground, |i| i.has(flag::VERTICAL_COLLISION));
        e.moved |= e.pitch != pitch || e.yaw != yaw || e.head_yaw != head_yaw || e.on_ground != on_ground;
        (e.pitch, e.yaw, e.head_yaw, e.on_ground) = (pitch, yaw, head_yaw, on_ground);
    }

    /// Input del giocatore: la posizione e l'eventuale richiesta di pila (flag 36). I valori non finiti o fuori dal mondo si scartano.
    /// Se il corpo non si legge per intero (il layout dopo la posizione non è verificato sul client reale) si logga
    /// la prima volta e si usa comunque la posizione: il movimento non deve dipendere dal resto del pacchetto.
    fn player_auth_input(&mut self, pid: u64, body: &[u8]) {
        // Chi è morto non si muove e non agisce finché non rinasce (il client mostra la schermata della morte).
        let Some(player) = self.players.get_mut(&pid).filter(|p| p.spawned && !p.dead) else { return };
        let mut state_changed = false;
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
                // Stato visibile agli altri: l'accovacciamento è anche lo stato tenuto dal flag; corsa, nuoto e planata cambiano solo agli inizi e alle fini.
                let before = (player.sneaking, player.sprinting, player.swimming, player.gliding);
                let toggle = |current: bool, start: u32, stop: u32| if input.has(stop) { false } else { current || input.has(start) };
                player.sneaking = if input.has(flag::STOP_SNEAKING) { false } else { input.has(flag::SNEAKING) || input.has(flag::START_SNEAKING) };
                player.sprinting = toggle(player.sprinting, flag::START_SPRINTING, flag::STOP_SPRINTING);
                player.swimming = toggle(player.swimming, flag::START_SWIMMING, flag::STOP_SWIMMING);
                player.gliding = toggle(player.gliding, flag::START_GLIDING, flag::STOP_GLIDING);
                state_changed = before != (player.sneaking, player.sprinting, player.swimming, player.gliding);
                // Il volo va sempre confermato o rifiutato con UpdateAbilities (changelog 776): senza risposta il client resta sospeso.
                let fly_request = if input.has(flag::START_FLYING) {
                    Some(true)
                } else if input.has(flag::STOP_FLYING) {
                    Some(false)
                } else {
                    None
                };
                if let Some(wants) = fly_request {
                    // Si vola solo dove la modalità lo permette; lo spettatore vola sempre (lo tiene in aria il suo livello).
                    player.flying = wants && player.mode == GameMode::Creative;
                }
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
        // Esaurimento da salto (Dragonfly `Jump`: 0,2 in corsa, 0,05 altrimenti) e da movimento (`Move`: 0,01 a nuoto, 0,1 in corsa per blocco).
        // Teletrasporto in attesa (Dragonfly `teleportPos`): il movimento partito da prima non si accetta, e non fa né caduta né esaurimento.
        // Appena il client riporta un punto a meno di un blocco dall'arrivo il movimento torna valido, con la caduta azzerata.
        let mut stale = false;
        if let Some(target) = player.teleport_to {
            let squared: f32 = (0..3).map(|i| (position[i] - target[i]).powi(2)).sum();
            // Uscita di sicurezza (Dragonfly non ce l'ha): se il client non arriva mai al punto, dopo 5 s il movimento torna valido,
            // così un teletrasporto perso non lascia il giocatore bloccato per sempre.
            let expired = self.tick_count.saturating_sub(player.teleport_at) >= TELEPORT_TIMEOUT;
            if squared <= 1.0 || expired {
                (player.teleport_to, player.fall_start_y) = (None, None);
            } else {
                stale = true; // con un NaN il confronto è falso: resta in attesa
            }
        }
        if !stale && input.as_ref().is_some_and(|i| i.has(flag::START_JUMPING)) {
            let sprinting = self.players.get(&pid).is_some_and(|p| p.sprinting);
            self.exhaust(pid, if sprinting { 0.2 } else { 0.05 });
        }
        if !stale && position.iter().all(|v| v.is_finite() && v.abs() <= 30_000_000.0) {
            self.exhaust_by_movement(pid, position);
            // la caduta si valuta prima di `on_move`: serve ancora la posizione del tick precedente
            let on_ground = input.as_ref().is_some_and(|i| i.has(flag::VERTICAL_COLLISION));
            self.fall_tick(pid, position[1] - EYE_HEIGHT, on_ground);
            self.on_move(pid, position);
            self.update_entity_motion(pid, input.as_ref());
        }
        if state_changed {
            self.send_entity_state(pid);
        }
        if let Some(request) = request {
            self.stack_request(pid, &request);
        }
        if input.as_ref().is_some_and(|i| i.has(flag::START_FLYING) || i.has(flag::STOP_FLYING)) {
            self.send_only_abilities(pid);
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
        self.resend_legacy_slots(pid, &input.legacy_slots);
    }

    /// Pacchetto `PlayerAction`: inizio, fine e abbandono della rottura di un blocco, e rottura creativa.
    fn player_action(&mut self, pid: u64, body: &[u8]) {
        match PlayerAction::decode(body) {
            // Come Dragonfly (`handlePlayerAction`: "Don't do anything for these actions"), `PlayerActionRespawn` non fa nulla: si rinasce solo
            // con `Respawn(ClientReadyToSpawn)` (vedi `respawn_packet`).
            Some(a) if a.action == player_action::RESPAWN => {}
            Some(_) if self.players.get(&pid).is_some_and(|p| p.dead) => {}
            Some(a) => self.block_action(pid, a.action, a.pos),
            None => log!("[#{pid}] PlayerAction illeggibile ({} byte): ignorato", body.len()),
        }
    }

    /// Pacchetto `InventoryTransaction`: l'uso di un oggetto con azione Destroy rompe un blocco, con azione Place attiva il blocco
    /// cliccato (porta, leva...) o ne piazza uno, con azione Use (ClickAir) comincia o finisce di mangiare; il rilascio (tipo ReleaseItem)
    /// interrompe o finisce il pasto; l'uso su un'entità (tipo UseItemOnEntity) con azione Attack è un colpo. Gli altri tipi di transazione non servono ancora.
    fn inventory_transaction(&mut self, pid: u64, body: &[u8]) {
        if self.players.get(&pid).is_some_and(|p| p.dead) {
            return;
        }
        let Some(transaction) = InventoryTransaction::decode(body) else {
            return log!("[#{pid}] InventoryTransaction illeggibile ({} byte): ignorata", body.len());
        };
        // gli usi nell'aria (ClickAir: si comincia o si finisce di mangiare) e i rilasci: i primi si loggano in esadecimale per vedere cosa manda il client vero
        let click_air = transaction.use_item.as_ref().is_some_and(|u| u.action == use_item::USE);
        if let Some(p) = self.players.get_mut(&pid).filter(|p| p.use_logs < HEX_LOGS && (click_air || transaction.release.is_some())) {
            p.use_logs += 1;
            let what = if click_air { "uso nell'aria" } else { "rilascio" };
            log!("[#{pid}] InventoryTransaction di {what} ({} byte): {:02x?}", body.len(), &body[..body.len().min(400)]);
        }
        match transaction.kind {
            // Come Dragonfly: con le transazioni Normal e Mismatch si rimanda sempre l'inventario, così client e server restano allineati
            0 => {
                if let Some(p) = self.players.get_mut(&pid).filter(|p| p.drop_logs < HEX_LOGS) {
                    p.drop_logs += 1;
                    log!("[#{pid}] InventoryTransaction Normal ({} byte): {:02x?}", body.len(), &body[..body.len().min(400)]);
                }
                if let Err(why) = self.drop_from_transaction(pid, &transaction.actions) {
                    if let Some(p) = self.players.get_mut(&pid).filter(|p| !p.drop_rejected_logged) {
                        p.drop_rejected_logged = true;
                        log!("[#{pid}] InventoryTransaction Normal rifiutata ({why}), {} byte: {:02x?}", body.len(), &body[..body.len().min(400)]);
                    }
                }
                self.send_inventory(pid);
            }
            1 => self.send_inventory(pid),
            _ => {}
        }
        if let Some(u) = &transaction.use_entity {
            if let Some(p) = self.players.get_mut(&pid).filter(|p| p.attack_logs < HEX_LOGS && u.action == use_entity::ATTACK) {
                p.attack_logs += 1;
                log!("[#{pid}] InventoryTransaction di attacco ({} byte): {:02x?}", body.len(), &body[..body.len().min(400)]);
            }
            self.use_item_on_entity(pid, u);
        }
        match &transaction.use_item {
            Some(u) if u.action == use_item::DESTROY => self.break_block(pid, u.pos),
            Some(u) if u.action == use_item::PLACE => self.place_item(pid, u),
            Some(u) if u.action == use_item::USE => self.use_item_in_air(pid, u),
            _ => {}
        }
        if let Some(r) = &transaction.release {
            self.release_item(pid, r);
        }
        self.resend_legacy_slots(pid, &transaction.legacy_slots);
    }

    /// Q (o Ctrl+Q) con l'inventario chiuso: il client manda una transazione Normal con due azioni, una sul mondo (con in `new_item` la quantità
    /// buttata) e una sull'inventario (casella e oggetto che c'era), come `handleNormalTransaction` di Dragonfly. L'oggetto si toglie dalla casella
    /// solo se i ganci permettono di buttarlo; l'errore dice perché la transazione non è valida (nulla è stato buttato).
    fn drop_from_transaction(&mut self, pid: u64, actions: &[InventoryAction]) -> Result<(), String> {
        let [a, b] = actions else {
            return Err(format!("servono due azioni per buttare un oggetto, ce ne sono {}", actions.len()));
        };
        let (mut slot, mut count, mut expected) = (0, 0, None);
        for action in [a, b] {
            if action.source_type == action_source::WORLD && action.slot == 0 {
                if action.old_item.id != 0 && action.old_item.count > 0 {
                    return Err("l'azione sul mondo ha un oggetto vecchio non vuoto".into());
                }
                count = action.new_item.count as u32;
            } else if action.source_type == action_source::CONTAINER && action.window_id == Some(window::INVENTORY as i8) {
                if action.old_item.id == 0 || action.old_item.count == 0 {
                    return Err("l'azione sull'inventario ha un oggetto vecchio vuoto".into());
                }
                (slot, expected) = (action.slot, Some(&action.old_item));
            } else {
                return Err("azione inattesa nella transazione per buttare un oggetto".into());
            }
        }
        let expected = expected.ok_or("manca l'azione sull'inventario")?;
        let player = self.players.get_mut(&pid).ok_or("giocatore sconosciuto")?;
        let item = player.inventory.take_dropped(slot, expected, count)?;
        if self.allow(&WorldAction::DropItem { pid, item: item.clone() }) {
            self.drop_item(pid, item);
        } else if let Some(p) = self.players.get_mut(&pid) {
            p.inventory.restore_main(slot, item);
        }
        Ok(())
    }

    /// Rimanda le caselle che il client ha chiesto con `LegacySetItemSlots` (al più 2 contenitori x 2 caselle, come Dragonfly), anche
    /// se non sono cambiate: fino ad allora il client le nomina con l'ID della sua richiesta legacy, che il server non conosce, e le
    /// richieste di pila successive su quelle caselle (per esempio buttare con Q) venivano rifiutate.
    pub(crate) fn resend_legacy_slots(&self, pid: u64, slots: &LegacySlots) {
        let Some(p) = self.players.get(&pid) else { return };
        let packets: Vec<Packet> =
            slots.iter().take(2).flat_map(|(container, list)| list.iter().take(2).filter_map(|&slot| p.inventory.slot_packet_at(*container, slot))).collect();
        if !packets.is_empty() {
            self.send(pid, &packets);
        }
    }

    /// Pacchetto `ItemStackRequest`: ogni richiesta ha la sua risposta.
    fn item_stack_request(&mut self, pid: u64, body: &[u8]) {
        if self.players.get(&pid).is_some_and(|p| p.dead) {
            return;
        }
        if let Some(p) = self.players.get_mut(&pid).filter(|p| p.stack_logs < HEX_LOGS) {
            p.stack_logs += 1;
            log!("[#{pid}] ItemStackRequest ({} byte): {:02x?}", body.len(), &body[..body.len().min(400)]);
        }
        let Some(packet) = ItemStackRequestPacket::decode(body) else {
            return log!("[#{pid}] ItemStackRequest illeggibile ({} byte): ignorato", body.len());
        };
        // Le richieste di crafting si loggano a parte (le prime `HEX_LOGS`): la loro codifica non è ancora confermata nel gioco.
        let crafts = packet
            .requests
            .iter()
            .any(|r| r.actions.iter().any(|a| matches!(a, StackAction::CraftRecipe { .. } | StackAction::CraftRecipeAuto { .. } | StackAction::Create { .. })));
        if let Some(p) = self.players.get_mut(&pid).filter(|p| crafts && p.craft_logs < HEX_LOGS) {
            p.craft_logs += 1;
            log!("[#{pid}] ItemStackRequest di crafting ({} byte): {:02x?} -> {:?}", body.len(), &body[..body.len().min(400)], packet.requests);
        }
        for request in &packet.requests {
            self.stack_request(pid, request);
        }
    }

    /// Una richiesta di pila: si applica all'inventario e si risponde (con le caselle cambiate, o con l'errore e lo stato vero).
    fn stack_request(&mut self, pid: u64, request: &StackRequest) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        let (held, armor) = (p.inventory.held().stack(0), armor_of(p));
        let result = p.inventory.handle(request, p.mode.creative_inventory());
        if p.inventory.touches_chest(request) {
            self.sync_chest(pid);
        }
        if let Some(error) = &result.error {
            log!("[#{pid}] richiesta di pila {} rifiutata: {error}", request.request_id);
        }
        // Gli oggetti buttati diventano oggetti a terra, a meno che un gancio non lo impedisca: allora tornano nella casella.
        for (item, (container, slot)) in result.dropped.iter().cloned().zip(result.dropped_from.iter().copied()) {
            if self.allow(&WorldAction::DropItem { pid, item: item.clone() }) {
                self.drop_item(pid, item);
            } else if let Some(p) = self.players.get_mut(&pid) {
                p.inventory.restore(container, slot, item);
            }
        }
        let Some(p) = self.players.get(&pid) else { return };
        let packets = p.inventory.response(request.request_id, &result);
        self.send(pid, &packets);
        self.send_equipment_changes(pid, &held, &armor);
    }

    /// `Respawn` dal client: con lo stato `ClientReadyToSpawn` e il proprio ID di runtime, chi è morto rinasce (come `RespawnHandler` di Dragonfly,
    /// che però rifiuta gli altri stati con un errore: qui si ignorano). I primi `HEX_LOGS` pacchetti si loggano in esadecimale per vedere cosa manda il client vero.
    fn respawn_packet(&mut self, pid: u64, body: &[u8]) {
        if let Some(p) = self.players.get_mut(&pid).filter(|p| p.respawn_logs < HEX_LOGS) {
            p.respawn_logs += 1;
            log!("[#{pid}] Respawn ({} byte): {:02x?}", body.len(), &body[..body.len().min(64)]);
        }
        let Some(r) = Respawn::decode(body) else {
            return log!("[#{pid}] Respawn illeggibile ({} byte): ignorato", body.len());
        };
        if r.runtime_id != pid || r.state != respawn_state::CLIENT_READY_TO_SPAWN {
            return log!("[#{pid}] Respawn con stato {} e ID {}: ignorato", r.state, r.runtime_id);
        }
        self.respawn(pid);
    }

    /// `Animate`: solo l'oscillazione del braccio (azione 1) si ripete agli spettatori; l'ID di runtime del pacchetto non si usa, vale quello del mittente.
    fn animate(&mut self, pid: u64, body: &[u8]) {
        if self.players.get(&pid).is_some_and(|p| p.dead) {
            return;
        }
        let Some(a) = Animate::decode(body) else {
            return log!("[#{pid}] Animate illeggibile ({} byte): ignorato", body.len());
        };
        if a.action == 1 {
            self.broadcast_to_viewers(pid, &[animate_swing(pid)]);
        }
    }

    /// Il giocatore cambia skin: si salva (i nuovi entrati la vedranno nella lista) e la ricevono tutti quelli nel mondo, lui compreso.
    /// Un cambio con l'UUID di un altro giocatore si ignora.
    fn player_skin(&mut self, pid: u64, body: &[u8]) {
        if let Some(p) = self.players.get_mut(&pid).filter(|p| p.skin_logs < HEX_LOGS) {
            p.skin_logs += 1;
            log!("[#{pid}] PlayerSkin ({} byte): {:02x?}", body.len(), &body[..body.len().min(400)]);
            // Corpo completo su file, per confrontare la struttura offline (`captures/` e' ignorata da git). Solo nel server vero: i test non scrivono file.
            #[cfg(not(test))]
            {
                let path = format!("captures/playerskin-{pid}-{}.bin", p.skin_logs);
                match std::fs::create_dir_all("captures").and_then(|()| std::fs::write(&path, body)) {
                    Ok(()) => log!("[#{pid}] PlayerSkin: corpo completo salvato in {path}"),
                    Err(e) => log!("[#{pid}] PlayerSkin: salvataggio di {path} non riuscito: {e}"),
                }
            }
        }
        let Some(change) = PlayerSkinChange::decode(body) else {
            return log!("[#{pid}] PlayerSkin illeggibile ({} byte): ignorato", body.len());
        };
        let Some(EntityKind::Player { uuid, .. }) = self.entities.get(pid).map(|e| &e.kind) else { return };
        if change.uuid != *uuid {
            return log!("[#{pid}] PlayerSkin con l'UUID di un altro giocatore: ignorato");
        }
        let Some(info) = self.players.get_mut(&pid).and_then(|p| p.info.as_mut()) else { return };
        info.skin = change.skin.clone();
        let packet = player_skin_packet(&change);
        let inside: Vec<u64> = self.players.iter().filter(|(_, p)| p.spawned).map(|(&id, _)| id).collect();
        for other in inside {
            self.send(other, std::slice::from_ref(&packet));
        }
    }

    /// Il giocatore cambia la casella della barra in mano. Vale solo per la finestra dell'inventario (0): quella della mano
    /// secondaria (119) non sposta la selezione (come Dragonfly). Caselle fuori dalla barra si ignorano.
    /// Il client chiede di aprire il proprio inventario: senza `ContainerOpen` (finestra 0, tipo inventario) non si apre. Come Dragonfly.
    fn interact_packet(&mut self, pid: u64, body: &[u8]) {
        let Some(i) = Interact::decode(body) else {
            return log!("[#{pid}] Interact illeggibile ({} byte): ignorato", body.len());
        };
        if i.action != interact_action::OPEN_INVENTORY {
            return;
        }
        let Some(p) = self.players.get_mut(&pid).filter(|p| !p.inventory_open) else { return };
        p.inventory_open = true;
        let position = BlockPos { x: p.pos[0].floor() as i32, y: p.pos[1].floor() as i32, z: p.pos[2].floor() as i32 };
        self.send(
            pid,
            &[ContainerOpen { container_id: window::INVENTORY as u8, container_type: CONTAINER_TYPE_INVENTORY, position, target_actor_id: -1 }.packet()],
        );
    }

    /// Il client ha chiuso una finestra: si conferma, altrimenti il client non la riapre più. 0xff arriva quando si apre la chat con l'inventario aperto.
    fn container_close(&mut self, pid: u64, body: &[u8]) {
        let Some(c) = ContainerClose::decode(body) else {
            return log!("[#{pid}] ContainerClose illeggibile ({} byte): ignorato", body.len());
        };
        let Some(p) = self.players.get_mut(&pid) else { return };
        p.inventory_open = false;
        // Griglia e cursore tornano all'inventario: alla chiusura del banco, e a quella dell'inventario se il banco non è aperto (griglia 2x2).
        let table_window = p.inventory.table_window();
        let closes_table = table_window == Some(c.container_id);
        let closes_chest = p.inventory.chest().is_some_and(|w| w.window == c.container_id);
        let chest_open = p.inventory.chest().is_some();
        if closes_table || closes_chest || c.container_id == 0xff || (c.container_id == window::INVENTORY as u8 && table_window.is_none() && !chest_open) {
            self.return_ui_items(pid);
        }
        // chiudere il forziere (o aprire la chat con una finestra aperta) toglie la vista, che spegne l'animazione se era l'ultimo
        if closes_chest || (c.container_id == 0xff && chest_open) {
            self.close_chest_view(pid);
        }
        if c.container_id == 0xff {
            return;
        }
        let container_type = if closes_table {
            CONTAINER_TYPE_WORKBENCH
        } else if closes_chest {
            CONTAINER_TYPE_CONTAINER
        } else if c.container_id == window::INVENTORY as u8 {
            0
        } else {
            c.container_type
        };
        self.send(pid, &[ContainerClose { container_id: c.container_id, container_type, server_initiated_close: false }.packet()]);
    }

    fn mob_equipment(&mut self, pid: u64, body: &[u8]) {
        if self.players.get(&pid).is_some_and(|p| p.dead) {
            return;
        }
        let Some(equipment) = MobEquipment::decode(body) else {
            return log!("[#{pid}] MobEquipment illeggibile ({} byte): ignorato", body.len());
        };
        if equipment.container_id != window::INVENTORY as u8 {
            return; // la mano secondaria (finestra 119) e le altre finestre non cambiano la casella in mano
        }
        let Some(p) = self.players.get_mut(&pid) else { return };
        if equipment.selected_slot < HOTBAR_SLOTS {
            let (held, armor) = (p.inventory.held().stack(0), armor_of(p));
            let changed = p.inventory.selected != equipment.selected_slot;
            p.inventory.selected = equipment.selected_slot;
            self.send_equipment_changes(pid, &held, &armor);
            if changed {
                self.stop_eating(pid); // cambiare casella interrompe il pasto (Dragonfly `SetHeldSlot`)
            }
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
