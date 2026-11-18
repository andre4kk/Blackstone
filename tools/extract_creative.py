#!/usr/bin/env python3
"""Costruisce crates/server/data/creative.json: il menu creativo (gruppi e oggetti nell'ordine del gioco).

Uso: python -I tools/extract_creative.py <vendor/dragonfly> <items.txt> <out.json>

Fonte: Dragonfly (MIT, commit 0f9a3a57, 1.26.60), file `server/item/creative/creative_items.nbt` (NBT di rete: `groups` e `items`; formato in
`server/item/creative/creative.go`). Le categorie sono quelle di `category.go`: 1 costruzione, 2 natura, 3 equipaggiamento, 4 oggetti.
`group_index` assente vale 0 (come in Go). Si tiene l'ordine del file. Le voci il cui nome non e' in items.txt (oggetti dell'Education Edition)
si scartano e si stampano; i gruppi rimasti senza voci si scartano e gli indici dei gruppi si rinumerano. Ogni `block_properties` deve essere
uno stato esatto di `block_states.nbt`. Controllo incrociato, solo informativo: si stampa quanti nomi/ID di `server/world/vanilla_items.nbt`
differiscono da items.txt. Dragonfly e' indietro di qualche oggetto (items.txt ha ice_ball, nuovi ID per ice_crystal/icicle e gli altri
oggetti dopo l'ID 853 scalati di 3): gli ID restano quelli di items.txt, mai quelli di Dragonfly. Vanilla ha inoltre i blocchi senza oggetto
(aria, doppie lastre, cartelli...).

Uscita: {"_license", "groups": [{category, name, icon}], "items": [{name, meta, block_states, nbt_b64, group}]}; `block_states` e' una
lista [{key, type, value}] (il tipo serve a ricostruire l'hash) o null, `nbt_b64` e' l'NBT su disco dell'oggetto in base64 o null.
"""
import base64
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # con `python -I` la cartella dello script non e' nel path
import nbtnet  # noqa: E402
from extract_blocks import load_states, type_of  # noqa: E402

LICENSE = (
    "Generato da tools/extract_creative.py da server/item/creative/creative_items.nbt di Dragonfly (df-mc/dragonfly, commit 0f9a3a57, 1.26.60). "
    "MIT License, Copyright (c) 2019 Dragonfly Tech."
)


def read_items(path):
    ids = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            name, raw = line.rstrip("\n").split("\t")
            if name in ids:
                sys.exit(f"{name}: ripetuto in items.txt")
            ids[name] = int(raw)
    return ids


def check_vanilla(dragonfly, ids):
    """Stampa quanti oggetti di vanilla_items.nbt differiscono da items.txt (nome assente da una delle due parti o ID diverso)."""
    with open(os.path.join(dragonfly, "server", "world", "vanilla_items.nbt"), "rb") as f:
        _, _, root = nbtnet.Reader(f.read()).root()
    vanilla = {name: entry["runtime_id"] for name, entry in root.items()}
    only_vanilla = sorted(set(vanilla) - set(ids))
    only_items = sorted(set(ids) - set(vanilla))
    different = sorted(n for n in set(vanilla) & set(ids) if vanilla[n] != ids[n])
    print(f"controllo incrociato vanilla_items.nbt ({len(vanilla)}) / items.txt ({len(ids)}): solo in vanilla {len(only_vanilla)}, solo in items.txt {len(only_items)}, ID diversi {len(different)}")
    for label, names in (("solo in vanilla", only_vanilla), ("solo in items.txt", only_items), ("ID diversi", different)):
        if names:
            print(f"  {label}: {', '.join(names[:10])}{' ...' if len(names) > 10 else ''}")


def convert_entry(e, where, block_hashes):
    """Voce di creative_items.nbt -> (name, meta, block_states|None, nbt_b64|None); verifica lo stato del blocco."""
    name = e["name"]
    meta = int(e.get("meta", 0))
    if meta < 0:
        sys.exit(f"{where} {name}: meta negativo")
    states = None
    if "block_properties" in e:
        props = e["block_properties"]
        if nbtnet.block_hash(name, props) not in block_hashes:
            sys.exit(f"{where} {name}: stato {props} assente da block_states.nbt")
        states = [{"key": k, "type": type_of(props[k]), "value": props[k] if isinstance(props[k], str) else int(props[k])} for k in sorted(props)]
    nbt = None
    if e.get("nbt"):
        nbt = base64.b64encode(nbtnet.disk_root(e["nbt"])).decode("ascii")
    return name, meta, states, nbt


def main(dragonfly, items_txt, out):
    ids = read_items(items_txt)
    check_vanilla(dragonfly, ids)
    block_hashes = {nbtnet.block_hash(n, s) for n, s in load_states(os.path.join(dragonfly, "server", "world", "block_states.nbt"))}

    with open(os.path.join(dragonfly, "server", "item", "creative", "creative_items.nbt"), "rb") as f:
        r = nbtnet.Reader(f.read())
        _, _, root = r.root()
        if not r.done():
            sys.exit("creative_items.nbt: byte in eccesso dopo la radice")
    raw_groups, raw_items = root["groups"], root["items"]

    dropped = {}
    kept = []  # (indice di gruppo originale, voce)
    for i, e in enumerate(raw_items):
        name, meta, states, nbt = convert_entry(e, f"voce {i}", block_hashes)
        group = int(e.get("group_index", 0))
        if not 0 <= group < len(raw_groups):
            sys.exit(f"voce {i} {name}: gruppo {group} inesistente")
        if name not in ids:
            dropped[name] = dropped.get(name, 0) + 1
            continue
        kept.append((group, {"name": name, "meta": meta, "block_states": states, "nbt_b64": nbt}))

    used = sorted({g for g, _ in kept})
    remap = {old: new for new, old in enumerate(used)}
    groups = []
    for old in used:
        g = raw_groups[old]
        if g["category"] not in (1, 2, 3, 4):
            sys.exit(f"gruppo {old}: categoria {g['category']} sconosciuta")
        icon = None
        if g["icon"].get("name") in ids:
            name, meta, states, nbt = convert_entry(g["icon"], f"icona del gruppo {old}", block_hashes)
            icon = {"name": name, "meta": meta, "block_states": states, "nbt_b64": nbt}
        groups.append({"category": int(g["category"]), "name": g["name"], "icon": icon})
    items = [dict(e, group=remap[g]) for g, e in kept]

    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump({"_license": LICENSE, "groups": groups, "items": items}, f, indent=1, ensure_ascii=False)
        f.write("\n")
    gone = len(raw_groups) - len(groups)
    print(f"{len(items)} voci su {len(raw_items)} ({sum(dropped.values())} scartate, {len(dropped)} nomi assenti da items.txt), {len(groups)} gruppi su {len(raw_groups)} ({gone} senza voci scartati) -> {out}")
    if dropped:
        print("  scartati:", ", ".join(sorted(dropped)[:12]), "..." if len(dropped) > 12 else "")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
