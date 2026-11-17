//! Legge le mappature Geyser (`data/geyser`) e genera le tabelle usate da `src/translate.rs`.
//!
//! `blocks.nbt` è NBT big-endian compresso con gzip: `{bedrock_mappings: [{bedrock_identifier?, state?}, ...]}`,
//! una voce per ogni stato di blocco Java nell'ordine degli ID di stato. Se manca `bedrock_identifier`
//! il nome Bedrock è quello Java; se manca `state` lo stato Bedrock non ha proprietà.
//! I nomi Java non sono nel file: si leggono dal registro di Steel all'avvio.

use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

enum Nbt {
    Byte(i8),
    Int(i32),
    Str(String),
    Compound(Vec<(String, Nbt)>),
    List(Vec<Nbt>),
    Other,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        s
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn i16(&mut self) -> i16 {
        i16::from_be_bytes(self.take(2).try_into().unwrap())
    }
    fn i32(&mut self) -> i32 {
        i32::from_be_bytes(self.take(4).try_into().unwrap())
    }
    fn string(&mut self) -> String {
        let len = self.i16() as u16 as usize;
        String::from_utf8(self.take(len).to_vec()).expect("stringa NBT non UTF-8")
    }
    fn payload(&mut self, id: u8) -> Nbt {
        match id {
            1 => Nbt::Byte(self.u8() as i8),
            2 => {
                self.take(2);
                Nbt::Other
            }
            3 => Nbt::Int(self.i32()),
            4 | 6 => {
                self.take(8);
                Nbt::Other
            }
            5 => {
                self.take(4);
                Nbt::Other
            }
            7 => {
                let n = self.i32() as usize;
                self.take(n);
                Nbt::Other
            }
            8 => Nbt::Str(self.string()),
            9 => {
                let item = self.u8();
                let n = self.i32().max(0);
                Nbt::List((0..n).map(|_| self.payload(item)).collect())
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let id = self.u8();
                    if id == 0 {
                        break;
                    }
                    let key = self.string();
                    entries.push((key, self.payload(id)));
                }
                Nbt::Compound(entries)
            }
            11 => {
                let n = self.i32() as usize;
                self.take(4 * n);
                Nbt::Other
            }
            12 => {
                let n = self.i32() as usize;
                self.take(8 * n);
                Nbt::Other
            }
            _ => panic!("tipo NBT sconosciuto {id}"),
        }
    }
}

fn get<'a>(entries: &'a [(String, Nbt)], key: &str) -> Option<&'a Nbt> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn blocks(out_dir: &Path) {
    let file = std::fs::File::open("data/geyser/blocks.nbt").expect("data/geyser/blocks.nbt");
    let mut data = Vec::new();
    flate2::read::GzDecoder::new(file).read_to_end(&mut data).expect("blocks.nbt non è gzip valido");
    let mut r = Reader { data: &data, pos: 0 };
    assert_eq!(r.u8(), 10, "la radice di blocks.nbt deve essere un compound");
    r.string();
    let Nbt::Compound(root) = r.payload(10) else { unreachable!() };
    let Some(Nbt::List(mappings)) = get(&root, "bedrock_mappings") else {
        panic!("blocks.nbt senza bedrock_mappings");
    };

    let mut code = String::new();
    writeln!(code, "/// Una voce per stato di blocco Java, nell'ordine degli ID di stato (generato da build.rs).").unwrap();
    writeln!(code, "pub static GEYSER_BLOCKS: [GeyserBlock; {}] = [", mappings.len()).unwrap();
    for entry in mappings {
        let Nbt::Compound(entry) = entry else { panic!("voce di bedrock_mappings non compound") };
        let name = match get(entry, "bedrock_identifier") {
            // Il file omette lo spazio dei nomi `minecraft:`.
            Some(Nbt::Str(s)) if s.contains(':') => format!("Some({s:?})"),
            Some(Nbt::Str(s)) => format!("Some({:?})", format!("minecraft:{s}")),
            None => "None".to_string(),
            _ => panic!("bedrock_identifier non è una stringa"),
        };
        let mut states = String::new();
        if let Some(Nbt::Compound(props)) = get(entry, "state") {
            for (key, value) in props {
                let v = match value {
                    Nbt::Byte(b) => format!("State::Byte({})", *b as u8),
                    Nbt::Int(i) => format!("State::Int({i})"),
                    Nbt::Str(s) => format!("State::Str({s:?})"),
                    _ => panic!("tipo di stato Bedrock inatteso per {key}"),
                };
                write!(states, "({key:?}, {v}),").unwrap();
            }
        }
        writeln!(code, "GeyserBlock {{ name: {name}, states: &[{states}] }},").unwrap();
    }
    writeln!(code, "];").unwrap();
    std::fs::write(out_dir.join("geyser_blocks.rs"), code).unwrap();
}

/// `biomes.json` ha la forma `{"minecraft:x": {"bedrock_id": N}, ...}`: basta una scansione testuale.
fn biomes(out_dir: &Path) {
    let json = std::fs::read_to_string("data/geyser/biomes.json").expect("data/geyser/biomes.json");
    let mut code = String::from("/// (bioma Java, ID Bedrock) da biomes.json (generato da build.rs).\npub static GEYSER_BIOMES: &[(&str, u32)] = &[\n");
    let mut rest = json.as_str();
    let mut count = 0;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let end = rest.find('"').unwrap();
        let key = &rest[..end];
        rest = &rest[end + 1..];
        if key == "bedrock_id" {
            continue;
        }
        let id_at = rest.find("\"bedrock_id\"").expect("bioma senza bedrock_id");
        let after = &rest[id_at + "\"bedrock_id\"".len()..];
        let digits: String = after.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
        writeln!(code, "({key:?}, {digits}),").unwrap();
        rest = &after[after.find(&digits).unwrap() + digits.len()..];
        count += 1;
    }
    assert!(count > 50, "biomes.json: troppi pochi biomi ({count})");
    code.push_str("];\n");
    std::fs::write(out_dir.join("geyser_biomes.rs"), code).unwrap();
}

fn main() {
    println!("cargo::rerun-if-changed=data/geyser/blocks.nbt");
    println!("cargo::rerun-if-changed=data/geyser/biomes.json");
    println!("cargo::rerun-if-changed=build.rs");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir);
    blocks(out_dir);
    biomes(out_dir);
}
