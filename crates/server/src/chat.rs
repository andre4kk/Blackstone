//! Chat e comandi.
use crate::{entity::EntityKind, survival::DamageCause, GameMode, Server};
use blackstone_net::{debug, log, warn};
use blackstone_protocol::{
    commands::{AvailableCommands, CommandSpec, Param, ParamKind},
    generated::UpdatePlayerGameType,
    id,
    packets::{text_type, CommandRequest, Text},
    reader::Reader,
    wire::ServerPacket,
    Packet,
};

/// Comandi disponibili: nome e descrizione (usati anche da /help).
const COMMANDS: &[(&str, &str)] = &[
    ("help", "elenco dei comandi"),
    ("list", "giocatori online"),
    ("tps", "prestazioni del server"),
    ("bot [nome|rimuovi]", "crea un giocatore finto che si muove, o li toglie tutti"),
    ("say <testo>", "messaggio a tutti"),
    ("gamemode <modalità>", "cambia la tua modalità di gioco"),
    ("kill", "ti uccide"),
];

/// Valori di `/gamemode` che il client suggerisce: tutti e soli quelli che `GameMode::parse` accetta.
pub(crate) const GAMEMODE_VALUES: [&str; 12] = ["survival", "creative", "adventure", "spectator", "s", "c", "a", "sp", "0", "1", "2", "6"];

/// Descrizione di un comando, presa dall'elenco di /help.
fn describe(name: &str) -> String {
    COMMANDS.iter().find(|(usage, _)| usage.split(' ').next() == Some(name)).map_or(String::new(), |(_, d)| (*d).to_owned())
}

/// `AvailableCommands`: i comandi che il server esegue davvero, con i loro argomenti, perché il client li suggerisca mentre si scrive
/// (comportamento di Dragonfly `sendAvailableCommands`). Va tenuto d'accordo con `Server::command`.
pub(crate) fn available_commands() -> AvailableCommands {
    let spec = |name: &str, aliases: &[&str], overloads: Vec<Vec<Param>>| CommandSpec {
        name: name.into(),
        description: describe(name),
        aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
        overloads,
    };
    let sub = |name: &str| Param::new(name, ParamKind::Enum { name: format!("SubCommand{name}"), options: vec![name.into()] }, false);
    let mode = ParamKind::Enum { name: "GameMode".into(), options: GAMEMODE_VALUES.iter().map(|v| (*v).to_owned()).collect() };
    AvailableCommands {
        commands: vec![
            spec("help", &[], vec![vec![]]),
            spec("list", &[], vec![vec![]]),
            spec("tps", &[], vec![vec![]]),
            spec("bot", &[], vec![vec![], vec![sub("rimuovi")], vec![Param::new("nome", ParamKind::Text, false)]]),
            spec("say", &[], vec![vec![Param::new("testo", ParamKind::RawText, false)]]),
            spec("gamemode", &["gm"], vec![vec![Param::new("modalità", mode, false)]]),
            spec("kill", &[], vec![vec![]]),
        ],
    }
}

/// Lunghezza massima di un messaggio di chat (come in vanilla).
pub(crate) const MAX_CHAT_LEN: usize = 512;

fn packet(text: Text) -> Packet {
    Packet::new(id::TEXT, text.encode())
}

impl Server {
    /// Nome del giocatore preso dal login, non dal pacchetto di chat. Il login però non è ancora verificato (vedi login.rs).
    pub(crate) fn name(&self, pid: u64) -> String {
        self.players.get(&pid).and_then(|p| p.info.as_ref()).map_or_else(|| "giocatore".into(), |i| i.name.clone())
    }

    /// Un messaggio solo per questo giocatore.
    pub(crate) fn tell(&self, pid: u64, message: &str) {
        self.send(pid, &[packet(Text::new(text_type::RAW, message))]);
    }

    /// Un messaggio per tutti i giocatori che sono nel mondo.
    pub(crate) fn broadcast(&self, text: Text) {
        let packet = packet(text);
        self.players.iter().filter(|(_, p)| p.spawned).for_each(|(&pid, _)| self.send(pid, std::slice::from_ref(&packet)));
    }

    /// Un messaggio scritto in chat: si ritrasmette a tutti con il nome del mittente.
    pub(crate) fn chat(&self, pid: u64, body: &[u8]) {
        let Some(text) = Text::decode(body) else {
            return warn!("[#{pid}] messaggio di chat illeggibile: {body:02x?}");
        };
        let message = text.message.trim();
        if text.kind != text_type::CHAT || message.is_empty() {
            return;
        }
        if message.chars().count() > MAX_CHAT_LEN {
            return self.tell(pid, &format!("§cMessaggio troppo lungo (massimo {MAX_CHAT_LEN} caratteri)."));
        }
        let name = self.name(pid);
        log!("[chat] <{name}> {message}");
        let xuid = self.players.get(&pid).and_then(|p| p.info.as_ref()).and_then(|i| i.xuid.clone()).unwrap_or_default();
        self.broadcast(Text { kind: text_type::CHAT, source: name, message: message.into(), xuid, platform_id: String::new() });
    }

    /// Cambia la modalità del giocatore e aggiorna il client (modalità e abilità); si salva con i suoi dati.
    pub(crate) fn set_mode(&mut self, pid: u64, mode: GameMode) {
        let Some(p) = self.players.get_mut(&pid) else { return };
        p.mode = mode;
        p.flying &= mode == GameMode::Creative; // fuori dalla creativa non si vola (lo spettatore vola col suo livello)
        debug!("[#{pid}] modalità di gioco: {mode:?}");
        self.send_abilities(pid);
        self.tell(pid, &format!("Modalità di gioco impostata a {}.", mode.name()));
    }

    /// Il client ha chiesto di cambiare modalità (dal menu di pausa): non si accetta, si rimanda quella vera.
    pub(crate) fn client_game_type(&self, pid: u64, body: &[u8]) {
        let mut r = Reader(body);
        let Some(requested) = r.var_i32() else { return warn!("[#{pid}] SetPlayerGameType illeggibile: {body:02x?}") };
        let Some(p) = self.players.get(&pid) else { return };
        debug!("[#{pid}] il client chiede la modalità {requested}: resta {:?}", p.mode);
        self.send(pid, &[UpdatePlayerGameType { player_game_type: p.mode as i32, target_player: pid as i64, tick: self.tick_count }.packet()]);
    }

    /// Un comando (`/help`...): si esegue e si risponde a chi l'ha scritto.
    pub(crate) fn command(&mut self, pid: u64, body: &[u8]) {
        let Some(request) = CommandRequest::decode(body) else {
            return warn!("[#{pid}] comando illeggibile: {body:02x?}");
        };
        let line = request.command.trim().trim_start_matches('/');
        let (name, args) = line.split_once(' ').unwrap_or((line, ""));
        log!("[comando] {}: /{line}", self.name(pid));
        match name.to_lowercase().as_str() {
            "help" => {
                let list: Vec<_> = COMMANDS.iter().map(|(c, d)| format!("§e/{c}§r - {d}")).collect();
                self.tell(pid, &format!("Comandi:\n{}", list.join("\n")));
            }
            "list" => {
                let mut names: Vec<_> = self.players.iter().filter(|(_, p)| p.spawned).map(|(&id, _)| self.name(id)).collect();
                names.extend(self.bots.iter().filter_map(|b| match &self.entities.get(b.id)?.kind {
                    EntityKind::Player { name, .. } => Some(name.clone()),
                    _ => None,
                }));
                self.tell(pid, &format!("Giocatori online ({}): {}", names.len(), names.join(", ")));
            }
            "tps" => self.tell(pid, &format!("TPS: §a{:.1}§r (tick medio {:.2} ms)", self.tps(), self.mspt)),
            "bot" => match args.trim() {
                "rimuovi" => {
                    self.remove_bots();
                    self.tell(pid, "Bot rimossi.");
                }
                raw => match self.bot_name(raw) {
                    Ok(name) => {
                        if self.spawn_bot(pid, &name) != 0 {
                            self.tell(pid, &format!("Bot §e{name}§r creato qui. Per toglierli: /bot rimuovi"));
                        }
                    }
                    Err(message) => self.tell(pid, &message),
                },
            },
            // come in vanilla colpisce anche in creativa; lo spettatore non può subire danno
            "kill" => {
                if !self.hurt(pid, f32::MAX, DamageCause::Kill) {
                    self.tell(pid, "§cNon puoi morire ora.");
                }
            }
            "say" if !args.trim().is_empty() => self.broadcast(Text::new(text_type::RAW, format!("§d[Server] {}", args.trim()))),
            "say" => self.tell(pid, "Uso: /say <testo>"),
            "gamemode" | "gm" => match GameMode::parse(args.trim()) {
                Some(mode) => self.set_mode(pid, mode),
                None => self.tell(pid, "Uso: /gamemode <survival|creative|adventure|spectator> (o s, c, a, sp, oppure 0, 1, 2, 6)"),
            },
            _ => self.tell(pid, &format!("§cComando sconosciuto: /{name}§r. Scrivi /help.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_command_is_offered_and_gamemode_values_all_parse() {
        let offered = available_commands();
        let names: Vec<&str> = offered.commands.iter().map(|c| c.name.as_str()).collect();
        for (usage, _) in COMMANDS {
            assert!(names.contains(&usage.split(' ').next().unwrap()), "{usage}");
        }
        assert!(offered.commands.iter().all(|c| !c.description.is_empty()));
        assert!(GAMEMODE_VALUES.iter().all(|v| GameMode::parse(v).is_some()));
        let gamemode = offered.commands.iter().find(|c| c.name == "gamemode").unwrap();
        assert_eq!(gamemode.aliases, ["gm"]);
    }
}
