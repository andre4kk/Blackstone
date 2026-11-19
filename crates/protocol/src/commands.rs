//! Pacchetto `AvailableCommands` (suggerimenti dei comandi nella chat). Byte da gophertunnel v1.63.0: `packet/available_commands.go`
//! e `protocol/command.go`; la costruzione (elenchi di valori degli enum senza doppioni, indice dell'enum nel tipo del parametro,
//! alias come enum `<nome>Aliases`) segue Dragonfly `server/session/command.go` (`BuildAvailableCommands`).
use crate::wire::{Encode, ServerPacket, Writer};
use std::collections::HashMap;

/// `CommandArgValid`, `CommandArgEnum`: bit che il client vuole nel tipo del parametro.
const ARG_VALID: u32 = 0x10_0000;
const ARG_ENUM: u32 = 0x20_0000;
/// Tipi di argomento (`CommandArgType*`).
const ARG_INT: u32 = 1;
const ARG_FLOAT: u32 = 3;
const ARG_TARGET: u32 = 8;
const ARG_STRING: u32 = 56;
const ARG_RAW_TEXT: u32 = 70;
/// Nessun alias (`math.MaxUint32` in Dragonfly).
const NO_ALIASES: u32 = u32::MAX;

/// Che cosa accetta un parametro.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    Int,
    Float,
    /// Una parola.
    Text,
    /// Il resto della riga.
    RawText,
    /// Un giocatore (selettore).
    Target,
    /// Uno tra valori fissi: il nome del tipo e i valori.
    Enum {
        name: String,
        options: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub kind: ParamKind,
    pub optional: bool,
}

impl Param {
    pub fn new(name: &str, kind: ParamKind, optional: bool) -> Self {
        Self { name: name.into(), kind, optional }
    }
}

/// Un comando: nome, descrizione, alias e le sue forme (ogni forma è un elenco di parametri; una forma vuota = nessun parametro).
#[derive(Clone, Debug, PartialEq)]
pub struct CommandSpec {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub overloads: Vec<Vec<Param>>,
}

/// L'elenco dei comandi da mandare al client.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AvailableCommands {
    pub commands: Vec<CommandSpec>,
}

/// Una forma già calcolata: (nome, tipo con i bit, opzionale) per parametro.
type BuiltOverload = Vec<(String, u32, bool)>;

impl Encode for AvailableCommands {
    fn encode(&self, w: &mut Writer) {
        // Come Dragonfly: gli enum si raccolgono man mano (uno per tipo), i valori si numerano una volta sola.
        let mut enums: Vec<(String, Vec<String>)> = Vec::new();
        let mut enum_index: HashMap<String, u32> = HashMap::new();
        let mut add_enum = |name: &str, options: &[String]| -> u32 {
            *enum_index.entry(name.to_owned()).or_insert_with(|| {
                enums.push((name.to_owned(), options.to_vec()));
                (enums.len() - 1) as u32
            })
        };
        let mut built: Vec<(&CommandSpec, u32, Vec<BuiltOverload>)> = Vec::new();
        for c in &self.commands {
            let aliases = if c.aliases.is_empty() { NO_ALIASES } else { add_enum(&format!("{}Aliases", c.name), &c.aliases) };
            let overloads = c
                .overloads
                .iter()
                .map(|params| {
                    params
                        .iter()
                        .map(|p| {
                            let t = match &p.kind {
                                ParamKind::Int => ARG_INT | ARG_VALID,
                                ParamKind::Float => ARG_FLOAT | ARG_VALID,
                                ParamKind::Text => ARG_STRING | ARG_VALID,
                                ParamKind::RawText => ARG_RAW_TEXT | ARG_VALID,
                                ParamKind::Target => ARG_TARGET | ARG_VALID,
                                ParamKind::Enum { name, options } => ARG_VALID | ARG_ENUM | add_enum(name, options),
                            };
                            (p.name.clone(), t, p.optional)
                        })
                        .collect()
                })
                .collect();
            built.push((c, aliases, overloads));
        }
        let mut values: Vec<&str> = Vec::new();
        let mut value_index: HashMap<&str, u32> = HashMap::new();
        let mut wire_enums: Vec<(&str, Vec<u32>)> = Vec::new();
        for (name, options) in &enums {
            let idx = options
                .iter()
                .map(|o| {
                    *value_index.entry(o.as_str()).or_insert_with(|| {
                        values.push(o);
                        (values.len() - 1) as u32
                    })
                })
                .collect();
            wire_enums.push((name, idx));
        }
        w.var_u32(values.len() as u32);
        for v in &values {
            w.string(v);
        }
        w.var_u32(0); // valori dei sottocomandi concatenati
        w.var_u32(0); // suffissi
        w.var_u32(wire_enums.len() as u32);
        for (name, idx) in &wire_enums {
            w.string(name);
            w.var_u32(idx.len() as u32);
            for i in idx {
                w.u32(*i);
            }
        }
        w.var_u32(0); // sottocomandi concatenati
        w.var_u32(built.len() as u32);
        for (c, aliases, overloads) in &built {
            w.string(&c.name);
            w.string(&c.description);
            w.u16(0); // flag
            w.string("any"); // livello di permesso
            w.u32(*aliases);
            w.var_u32(0); // offset dei sottocomandi concatenati
            w.var_u32(overloads.len() as u32);
            for params in overloads {
                w.bool(false); // non concatenata
                w.var_u32(params.len() as u32);
                for (pname, t, optional) in params {
                    w.string(pname);
                    w.u32(*t);
                    w.bool(*optional);
                    w.u8(0); // opzioni
                }
            }
        }
        w.var_u32(0); // enum dinamici
        w.var_u32(0); // vincoli
    }
}

impl ServerPacket for AvailableCommands {
    const ID: u32 = 76;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| i.to_string()).collect()
    }

    #[test]
    fn id_is_76_and_an_empty_list_has_eight_empty_fields() {
        assert_eq!(AvailableCommands::default().packet().id, 76);
        assert_eq!(AvailableCommands::default().to_bytes(), [0; 8]);
    }

    #[test]
    fn small_command_set_bytes() {
        let p = AvailableCommands {
            commands: vec![
                CommandSpec { name: "kill".into(), description: "x".into(), aliases: vec![], overloads: vec![vec![]] },
                CommandSpec {
                    name: "gm".into(),
                    description: "y".into(),
                    aliases: s(&["g"]),
                    overloads: vec![vec![Param::new("mode", ParamKind::Enum { name: "M".into(), options: s(&["s", "c"]) }, false)]],
                },
            ],
        };
        // valori: g, s, c
        let mut e: Vec<u8> = vec![3, 1, b'g', 1, b's', 1, b'c'];
        e.extend([0, 0]); // sottocomandi concatenati, suffissi
        e.push(2); // enum: gmAliases [0], M [1, 2]
        e.push(9);
        e.extend(b"gmAliases");
        e.extend([1, 0, 0, 0, 0]);
        e.extend([1, b'M', 2, 1, 0, 0, 0, 2, 0, 0, 0]);
        e.push(0); // sottocomandi concatenati
        e.push(2); // comandi
                   // kill: senza alias, una forma vuota
        e.push(4);
        e.extend(b"kill");
        e.extend([1, b'x', 0, 0, 3]);
        e.extend(b"any");
        e.extend([0xff, 0xff, 0xff, 0xff, 0, 1, 0, 0]);
        // gm: alias nell'enum 0, un parametro con l'enum 1
        e.push(2);
        e.extend(b"gm");
        e.extend([1, b'y', 0, 0, 3]);
        e.extend(b"any");
        e.extend([0, 0, 0, 0, 0, 1, 0, 1, 4]);
        e.extend(b"mode");
        e.extend([0x01, 0x00, 0x30, 0x00, 0, 0]); // valido | enum | indice 1
        e.extend([0, 0]); // enum dinamici, vincoli
        assert_eq!(p.to_bytes(), e);
    }

    #[test]
    fn plain_argument_types_and_shared_enums() {
        let opt = |n: &str, k| Param::new(n, k, true);
        let en = || Param::new("e", ParamKind::Enum { name: "E".into(), options: s(&["x"]) }, false);
        let spec = |name: &str, overloads| CommandSpec { name: name.into(), description: String::new(), aliases: vec![], overloads };
        let p = AvailableCommands {
            commands: vec![
                spec("a", vec![vec![opt("n", ParamKind::Int), opt("t", ParamKind::RawText)]]),
                spec("b", vec![vec![en()], vec![opt("who", ParamKind::Target)]]),
                spec("c", vec![vec![en()]]),
            ],
        };
        let bytes = p.to_bytes();
        // un solo valore "x" e un solo enum "E", anche se due comandi lo usano
        assert_eq!(bytes[..3], [1, 1, b'x']);
        assert_eq!(bytes.windows(3).filter(|w| *w == [1, b'E', 1]).count(), 1);
        let has = |t: u32| bytes.windows(4).any(|w| w == t.to_le_bytes());
        assert!(has(0x10_0001) && has(0x10_0046) && has(0x10_0008), "Int, RawText, Target");
    }
}
