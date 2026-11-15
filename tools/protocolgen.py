#!/usr/bin/env python3
"""Genera gli encoder dei pacchetti server -> client dagli schemi JSON di Mojang.

Uso:  python tools/protocolgen.py <cartella json di bedrock-protocol-docs> <file .rs di output> <Pacchetto>...
Esempio: python tools/protocolgen.py ../bedrock-protocol-docs/json crates/protocol/src/generated.rs StartGame ItemRegistry

Gli schemi sono soggetti all'EULA di Minecraft (github.com/Mojang/bedrock-protocol-docs): non vanno copiati nel repository.
Si genera solo la codifica. Regole di codifica:
  - interi: fissi little-endian; con l'opzione "Compression" varint (con segno = ZigZag); "Big Endian" big-endian
  - proprietà non elencate in "required": opzionali, scritte come un byte di presenza + valore
  - array: contatore varint, oppure u32 con l'opzione "No size compression"
  - campi in BYTES: lunghezza varint + byte arbitrari
  - mappe: contatore varint, poi chiave e valore di ogni voce (`Vec<(K, V)>`)
  - campi senza tipo (NBT) e oneOf: non generabili, il valore è `Vec<u8>` già codificato (raw)
"""
import json, os, re, sys

docs, out_path, roots = sys.argv[1], sys.argv[2], sys.argv[3:]
RESERVED = set("as break const continue crate else enum extern false fn for if impl in let loop match mod move mut pub ref return self static struct super trait true type unsafe use where while async await dyn abstract become box do final macro override priv typeof unsized virtual yield try".split())

load = lambda n: json.load(open(f"{docs}/{n}.json"))
norm = lambda r: re.sub(r"^\./|\.json$", "", r)
pascal = lambda n: "".join(p[:1].upper() + p[1:] for p in re.split(r"[^A-Za-z0-9]+", n) if p)

def snake(n):
    n = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", n)
    n = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", n)
    n = re.sub(r"[^A-Za-z0-9]+", "_", n).strip("_").lower()
    return n + "_" if n in RESERVED else ("f_" + n if n[:1].isdigit() else n)

def int_kind(under, opts):
    """(tipo Rust, metodo del Writer) per un intero."""
    bits, signed = int(re.sub(r"\D", "", under)), under.startswith("int")
    rust = ("i" if signed else "u") + str(bits)
    if "Big Endian" in opts: return rust, f"{rust}_be"
    if "Compression" in opts and bits >= 16:
        return rust, ("var_i64" if signed else "var_u64") if bits == 64 else ("var_i32" if signed else "var_u32")
    return rust, rust

# Campi che lo schema chiama "string" ma contengono byte arbitrari (non UTF-8): lunghezza varint + byte.
BYTES = {"LevelChunkPacketPayload.Serialized Chunk Data"}

structs, enums, order, warnings = {}, {}, [], []

def resolve(v, ctx):
    """Kind di una proprietà: ('int', rust, metodo) | ('f32'|'f64'|'bool'|'str'|'raw',) | ('struct', nome) | ('vec', kind, u32?)."""
    opts = set(v.get("x-serialization-options", []))
    if ctx in BYTES: return ("bytes",)
    if "oneOf" in v:
        warnings.append(f"{ctx}: oneOf -> raw"); return ("raw",)
    if "$ref" in v:
        name = norm(v["$ref"]); t = load(name)
        opts |= set(t.get("x-serialization-options", []))
        under = v.get("x-underlying-type") or t.get("x-underlying-type")
        if "enum" in t:
            register_enum(name, t)
            r, m = int_kind(under, opts); return ("int", r, m)
        props = t.get("properties")
        if props is not None:
            if len(props) == 1:  # wrapper di un solo intero (ActorUniqueID...): si usa direttamente l'intero
                (p,) = props.values()
                if p.get("type") == "integer":
                    r, m = int_kind(p["x-underlying-type"], set(p.get("x-serialization-options", []))); return ("int", r, m)
            register_struct(name); return ("struct", pascal(name))
        merged = {**t, **{k: x for k, x in v.items() if k != "$ref"}}
        return resolve(merged, ctx)
    t = v.get("type")
    if t == "object" and "additionalProperties" in v:  # mappa: contatore varint, poi coppie chiave + valore
        entry = v["additionalProperties"].get("properties")
        if not entry or "key" not in entry or "value" not in entry:
            warnings.append(f"{ctx}: mappa non gestita -> raw"); return ("raw",)
        return ("map", resolve(entry["key"], ctx + ".key"), resolve(entry["value"], ctx + ".value"))
    if t == "integer":
        r, m = int_kind(v["x-underlying-type"], opts); return ("int", r, m)
    if t == "number": return ("f64",) if v.get("x-underlying-type") == "double" else ("f32",)
    if t == "boolean": return ("bool",)
    if t == "string": return ("str",)
    if t == "array": return ("vec", resolve(v["items"], ctx), "No size compression" in opts)
    warnings.append(f"{ctx}: tipo non gestito ({t}) -> raw"); return ("raw",)

def register_enum(name, t):
    if name in enums: return
    under = t.get("x-underlying-type", "int32"); rust = ("i" if under.startswith("int") else "u") + re.sub(r"\D", "", under)
    items, seen = [], {}
    for label, val in zip(t["enum"], t["x-enum-binary-value"]):
        c = re.sub(r"[^A-Za-z0-9]+", "_", re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", str(label))).strip("_").upper() or "VALUE"
        c = "V_" + c if c[0].isdigit() else c
        seen[c] = seen.get(c, 0) + 1
        items.append((c if seen[c] == 1 else f"{c}_{seen[c]}", val))
    enums[name] = (rust, items)

def register_struct(name, root=False, struct_name=None):
    if name in structs: return
    structs[name] = None  # segnaposto contro la ricorsione
    d = load(name); required = set(d.get("required", []))
    fields = []
    for k, v in sorted(d["properties"].items(), key=lambda kv: kv[1].get("x-ordinal-index", 0)):
        fields.append((snake(k), resolve(v, f"{name}.{k}"), k not in required, k))
    structs[name] = (struct_name or pascal(name), fields); order.append(name)

def rust_type(k):
    return {"f32": "f32", "f64": "f64", "bool": "bool", "str": "String", "raw": "Vec<u8>", "bytes": "Vec<u8>"}.get(k[0]) or (
        k[1] if k[0] in ("int", "struct") else f"Vec<({rust_type(k[1])}, {rust_type(k[2])})>" if k[0] == "map" else f"Vec<{rust_type(k[1])}>")

def emit(k, e, ref, ind):
    """Righe Rust che scrivono il valore `e` (se ref, `e` è un riferimento)."""
    p = " " * ind; d = "*" + e if ref else e
    if k[0] == "int": return [f"{p}w.{k[2]}({d});"]
    if k[0] in ("f32", "f64", "bool"): return [f"{p}w.{k[0]}({d});"]
    if k[0] == "str": return [f"{p}w.string({e if ref else '&' + e});"]
    if k[0] == "raw": return [f"{p}w.raw({e if ref else '&' + e});"]
    if k[0] == "bytes": return [f"{p}w.bytes({e if ref else '&' + e});"]
    if k[0] == "struct": return [f"{p}{e}.encode(w);"]
    if k[0] == "map":
        return [f"{p}w.var_u32({e}.len() as u32);", f"{p}for (k, v) in &{e} {{", *emit(k[1], "k", True, ind + 4), *emit(k[2], "v", True, ind + 4), f"{p}}}"]
    inner = k[1]; cnt = f"w.u32({e}.len() as u32);" if k[2] else f"w.var_u32({e}.len() as u32);"
    return [f"{p}{cnt}", f"{p}for v in &{e} {{", *emit(inner, "v", True, ind + 4), f"{p}}}"]

# --- radici: un pacchetto = <Nome>PacketPayload + metadati in <Nome>Packet
packets = []
for r in roots:
    pid = load(f"{r}Packet")["$metaProperties"]["[cereal:packet]"]
    register_struct(f"{r}PacketPayload", struct_name=r)
    packets.append((r, pid))

version = load(f"{roots[0]}PacketPayload").get("x-protocol-version")
o = [f"// Generato da tools/protocolgen.py dagli schemi Mojang (protocollo {version}). NON MODIFICARE A MANO: rigenerare.",
     "// Contiene solo la codifica dei pacchetti server -> client. Campi `Vec<u8>` = dati già codificati (NBT, oneOf).",
     "#![allow(clippy::all, dead_code)]", "use crate::wire::{Encode, ServerPacket, Writer};", ""]
for name in order:
    sname, fields = structs[name]
    o += ["#[derive(Clone, Debug, Default, PartialEq)]", f"pub struct {sname} {{"]
    for f, k, opt, orig in fields:
        t = rust_type(k); o.append(f"    pub {f}: {'Option<' + t + '>' if opt else t},")
    o += ["}", "", f"impl Encode for {sname} {{", "    fn encode(&self, w: &mut Writer) {"]
    for f, k, opt, orig in fields:
        if opt: o += [f"        match &self.{f} {{", "            Some(v) => {", "                w.u8(1);", *emit(k, "v", True, 16), "            }", "            None => w.u8(0),", "        }"]
        else: o += emit(k, f"self.{f}", False, 8)
    o += ["    }", "}", ""]
for r, pid in packets: o += [f"impl ServerPacket for {r} {{", f"    const ID: u32 = {pid};", "}", ""]
if enums:
    o += ["/// Valori degli enum degli schemi (i campi sono interi: si usano queste costanti).", "pub mod enums {"]
    for name, (rust, items) in sorted(enums.items()):
        o += [f"    pub mod {snake(name)} {{"] + [f"        pub const {c}: {rust} = {v};" for c, v in items] + ["    }"]
    o.append("}")
open(out_path, "w").write("\n".join(o) + "\n")
print(f"{len(order)} struct, {len(enums)} enum, {len(packets)} pacchetti -> {out_path}")
for w in dict.fromkeys(warnings): print("  avviso:", w)
