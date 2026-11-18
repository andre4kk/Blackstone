"""Estrae da Dragonfly (MIT) la versione, il flag `component_based` e i dati dei componenti di ogni oggetto vanilla.

Uso: python -I tools/extract_item_meta.py <vendor/dragonfly> <out.json>

`vanilla_items.nbt` e' NBT di rete: i dati di ogni oggetto si copiano byte per byte (radice compound senza nome), cosi'
arrivano al client identici a come li manda Dragonfly. Gli ID restano quelli di items.txt: qui si prende solo il resto.
Il client collega un oggetto al suo blocco solo se la versione e' giusta (i blocchi hanno versione 2 = None).
"""
import base64
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nbtnet  # noqa: E402


def main(dragonfly, out):
    with open(os.path.join(dragonfly, "server", "world", "vanilla_items.nbt"), "rb") as f:
        r = nbtnet.Reader(f.read())
    assert r.take(1)[0] == 10, "radice non compound"
    r.string()
    items = {}
    while True:
        t = r.take(1)[0]
        if t == 0:
            break
        name = r.string()
        assert t == 10, name
        entry = {"version": None, "component_based": False, "data": None}
        while True:
            tt = r.take(1)[0]
            if tt == 0:
                break
            key = r.string()
            start = r.p
            value = r.payload(tt)
            if key == "version":
                entry["version"] = int(value)
            elif key == "component_based":
                entry["component_based"] = bool(value)
            elif key == "data" and tt == 10 and value:
                # radice di rete senza nome: tipo compound, nome vuoto (varint 0), poi il contenuto letto
                entry["data"] = base64.b64encode(bytes([10, 0]) + r.b[start : r.p]).decode()
        assert entry["version"] is not None, name
        items[name] = [entry["version"], entry["component_based"], entry["data"]]
    doc = {
        "_license": "Da Dragonfly (MIT, https://github.com/df-mc/dragonfly) server/world/vanilla_items.nbt: [versione, component_based, dati NBT di rete in base64].",
        "items": dict(sorted(items.items())),
    }
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(doc, f, indent=0, ensure_ascii=False)
        f.write("\n")
    print(f"{len(items)} oggetti -> {out}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
