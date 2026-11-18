#!/usr/bin/env python3
"""Costruisce crates/server/data/blocks.json: gli stati dei blocchi (nome, chiavi, valori, stato di default).

Uso: python -I tools/extract_blocks.py <vendor/dragonfly> <out.json>

Fonte: Dragonfly (MIT, commit 0f9a3a57, 1.26.60), file `server/world/block_states.nbt`: NBT di rete (varint) con i compound
`{name, states, version}` concatenati, uno per stato. Di ogni stato si ricalcola l'hash di rete (FNV-1a dell'NBT su disco `{name, states}`):
`minecraft:air` deve valere -604749536 e gli hash devono essere tutti distinti. Ogni blocco deve essere il prodotto cartesiano completo
dei valori delle sue chiavi. Il file non dichiara lo stato di default: si prende il primo del gruppo (unico con meta 0 nei dati di
pmmp/BedrockData).
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # con `python -I` la cartella dello script non e' nel path
import nbtnet  # noqa: E402

LICENSE = (
    "Generato da tools/extract_blocks.py da server/world/block_states.nbt di Dragonfly (df-mc/dragonfly, commit 0f9a3a57, 1.26.60). "
    "MIT License, Copyright (c) 2019 Dragonfly Tech."
)
AIR_HASH = -604749536
TYPES = {nbtnet.Byte: "byte", int: "int", str: "string"}


def type_of(value):
    t = TYPES.get(type(value))
    if t is None:
        sys.exit(f"tipo di stato non gestito: {type(value).__name__}")
    return t


def load_states(path):
    with open(path, "rb") as f:
        r = nbtnet.Reader(f.read())
    states = []
    while not r.done():
        _, _, c = r.root()
        states.append((c["name"], c["states"]))
    return states


def main(dragonfly, out):
    states = load_states(os.path.join(dragonfly, "server", "world", "block_states.nbt"))
    groups = {}  # nome -> [(stati, hash) nell'ordine del file]
    hashes = {}
    for name, s in states:
        if list(s) != sorted(s):
            sys.exit(f"{name}: stati non in ordine alfabetico: {list(s)}")
        h = nbtnet.block_hash(name, s)
        if h in hashes:
            sys.exit(f"hash {h} duplicato: {hashes[h]} e {name}")
        hashes[h] = name
        groups.setdefault(name, []).append((s, h))
    if hashes.get(AIR_HASH) != "minecraft:air":
        sys.exit(f"hash di minecraft:air diverso da {AIR_HASH}")

    blocks = []
    for name, entries in groups.items():
        domain = {}  # chiave -> valori distinti nell'ordine di apparizione
        for s, _ in entries:
            for k, v in s.items():
                values = domain.setdefault(k, [])
                if type_of(v) != type_of(values[0] if values else v):
                    sys.exit(f"{name}.{k}: tipi misti")
                if v not in values:
                    values.append(v)
        total = 1
        for values in domain.values():
            total *= len(values)
        if total != len(entries):
            sys.exit(f"{name}: {len(entries)} stati ma il prodotto dei valori e' {total}")
        if len({tuple(s.items()) for s, _ in entries}) != len(entries):
            sys.exit(f"{name}: stati ripetuti")
        blocks.append(
            {
                "name": name,
                "states": [{"key": k, "type": type_of(v[0]), "values": [x if isinstance(x, str) else int(x) for x in v]} for k, v in domain.items()],
                "default": entries[0][1],
            }
        )

    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump({"_license": LICENSE, "blocks": blocks}, f, indent=1, ensure_ascii=False)
        f.write("\n")
    print(f"{len(blocks)} blocchi, {len(states)} stati, hash tutti distinti, air = {AIR_HASH} -> {out}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
