//! Chat e comandi.
use crate::Server;
use bedrock_net::log;
use bedrock_protocol::{
    id,
    packets::{text_type, CommandRequest, Text},
    Packet,
};

/// Comandi disponibili: nome e descrizione (usati anche da /help).
const COMMANDS: &[(&str, &str)] = &[("help", "elenco dei comandi"), ("list", "giocatori online"), ("tps", "prestazioni del server"), ("say <testo>", "messaggio a tutti")];

fn packet(text: Text) -> Packet {
    Packet::new(id::TEXT, text.encode())
}

impl Server {
    /// Nome (verificato dal login, non dichiarato dal client) del giocatore.
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
            return log!("[#{pid}] messaggio di chat illeggibile: {body:02x?}");
        };
        let message = text.message.trim();
        if text.kind != text_type::CHAT || message.is_empty() {
            return;
        }
        let name = self.name(pid);
        log!("[chat] {name}: {message}");
        let xuid = self.players.get(&pid).and_then(|p| p.info.as_ref()).and_then(|i| i.xuid.clone()).unwrap_or_default();
        self.broadcast(Text { kind: text_type::CHAT, source: name, message: message.into(), xuid, platform_id: String::new() });
    }

    /// Un comando (`/help`...): si esegue e si risponde a chi l'ha scritto.
    pub(crate) fn command(&self, pid: u64, body: &[u8]) {
        let Some(request) = CommandRequest::decode(body) else {
            return log!("[#{pid}] comando illeggibile: {body:02x?}");
        };
        let line = request.command.trim().trim_start_matches('/');
        let (name, args) = line.split_once(' ').unwrap_or((line, ""));
        log!("[#{pid}] comando: /{line}");
        match name.to_lowercase().as_str() {
            "help" => {
                let list: Vec<_> = COMMANDS.iter().map(|(c, d)| format!("§e/{c}§r - {d}")).collect();
                self.tell(pid, &format!("Comandi:\n{}", list.join("\n")));
            }
            "list" => {
                let names: Vec<_> = self.players.iter().filter(|(_, p)| p.spawned).map(|(&id, _)| self.name(id)).collect();
                self.tell(pid, &format!("Giocatori online ({}): {}", names.len(), names.join(", ")));
            }
            "tps" => self.tell(pid, &format!("TPS: §a{:.1}§r (tick medio {:.2} ms)", self.tps(), self.mspt)),
            "say" if !args.trim().is_empty() => self.broadcast(Text::new(text_type::RAW, format!("§d[Server] {}", args.trim()))),
            "say" => self.tell(pid, "Uso: /say <testo>"),
            _ => self.tell(pid, &format!("§cComando sconosciuto: /{name}§r. Scrivi /help.")),
        }
    }
}
