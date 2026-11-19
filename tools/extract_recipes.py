#!/usr/bin/env python3
"""Costruisce crates/server/data/recipes.json: le ricette di crafting (banco da lavoro e griglia 2x2), con ID di rete stabili.

Uso: python -I tools/extract_recipes.py <vendor/dragonfly> <items.txt> <out.json>

Fonte: Dragonfly (MIT, commit 0f9a3a57, 1.26.60), `server/item/recipe/crafting_data.nbt` (NBT di rete; campi come in `vanilla.go`:
liste `shaped`, `shapeless`, `shulker_box`, `multi`) e `item_tags.json` (membri dei tag). Ingredienti e risultati come in `item.go`:
`name`+`meta` (meta assente = 0; 32767 = qualunque variante), `tag` oppure `minecraft:air` con quantita' 0 (casella vuota); i risultati
possono portare lo stato esatto del blocco (`block`).

Si tengono solo le ricette `shaped` e `shapeless` con blocco `crafting_table` (le ricette di fornace, tagliapietre, ecc. e quelle
`shulker_box` con dati utente e `multi` per ora non servono). Si scarta (e si stampa) una ricetta se un oggetto non e' in items.txt (nomi
vecchi come `minecraft:log`, che Dragonfly scarta pure), se lo stato di un blocco risultato non e' in `block_states.nbt`, o se un
risultato porta dati NBT (stendardi: la loro codifica non e' confermata).

ID di rete: come Dragonfly (`session/player.go` sendRecipes: indice + 1 nell'ordine di registrazione, mai 0), qui nell'ordine
`shapeless` poi `shaped` del file, contando solo le ricette tenute: sono stabili finche' non si rigenera il file con altri filtri. Il
client non li conosce se non dal pacchetto CraftingData, quindi non vanno salvati su disco.

Uscita: {"_license", "tags": {tag: [nomi]}, "recipes": [{id, kind, priority, block, width, height, input, output}]}. `input` ha una
voce per casella (shaped: larghezza x altezza, per righe; shapeless: una per ingrediente): null = vuota, {name, meta, count} oppure
{tag, count}. `output`: [{name, meta, count, block_states|null}] con block_states come in creative.json.
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # con `python -I` la cartella dello script non e' nel path
import nbtnet  # noqa: E402
from extract_blocks import load_states, type_of  # noqa: E402

LICENSE = (
    "Generato da tools/extract_recipes.py da server/item/recipe/crafting_data.nbt e item_tags.json di Dragonfly "
    "(df-mc/dragonfly, commit 0f9a3a57, 1.26.60). MIT License, Copyright (c) 2019 Dragonfly Tech."
)
ANY_META = 32767
AIR = "minecraft:air"


def read_items(path):
    with open(path, encoding="utf-8") as f:
        return {line.split("\t")[0] for line in f if line.strip()}


class Drop(Exception):
    """La ricetta si scarta; il messaggio e' il motivo."""


def convert_input(e, ids):
    if "tag" in e:
        return {"tag": e["tag"], "count": int(e["count"])}
    name = e["name"]
    if name == AIR:
        if int(e["count"]) != 0:
            raise Drop("aria con quantita'")
        return None
    if name not in ids:
        raise Drop(f"oggetto {name} assente da items.txt")
    return {"name": name, "meta": int(e.get("meta", 0)), "count": int(e["count"])}


def convert_output(e, ids, block_hashes):
    name = e["name"]
    if name not in ids:
        raise Drop(f"oggetto {name} assente da items.txt")
    if "data" in e:
        raise Drop("risultato con dati NBT")
    states = None
    if "block" in e:
        b = e["block"]
        props = b["states"]
        if nbtnet.block_hash(b["name"], props) not in block_hashes:
            raise Drop(f"stato di {b['name']} assente da block_states.nbt")
        states = [{"key": k, "type": type_of(props[k]), "value": props[k] if isinstance(props[k], str) else int(props[k])} for k in sorted(props)]
    count = int(e["count"])
    if count < 1:
        raise Drop("risultato vuoto")
    return {"name": name, "meta": int(e.get("meta", 0)), "count": count, "block_states": states}


def main(dragonfly, items_txt, out):
    ids = read_items(items_txt)
    block_hashes = {nbtnet.block_hash(n, s) for n, s in load_states(os.path.join(dragonfly, "server", "world", "block_states.nbt"))}
    recipe_dir = os.path.join(dragonfly, "server", "item", "recipe")
    with open(os.path.join(recipe_dir, "crafting_data.nbt"), "rb") as f:
        r = nbtnet.Reader(f.read())
        _, _, root = r.root()
        if not r.done():
            sys.exit("crafting_data.nbt: byte in eccesso dopo la radice")
    with open(os.path.join(recipe_dir, "item_tags.json"), encoding="utf-8") as f:
        all_tags = json.load(f)

    recipes = []
    dropped = {}
    used_tags = set()
    for kind in ("shapeless", "shaped"):
        for e in root[kind]:
            if e["block"] != "crafting_table":
                continue
            try:
                inputs = [convert_input(i, ids) for i in e["input"]]
                outputs = [convert_output(o, ids, block_hashes) for o in e["output"]]
                if not outputs:
                    raise Drop("senza risultato")
                rec = {"id": len(recipes) + 1, "kind": kind, "priority": int(e.get("priority", 0)), "block": "crafting_table"}
                if kind == "shaped":
                    w, h = int(e["width"]), int(e["height"])
                    if w < 1 or h < 1 or w * h != len(inputs):
                        raise Drop("forma e ingredienti non coincidono")
                    rec.update(width=w, height=h)
                elif not any(inputs):
                    raise Drop("senza ingredienti")
                rec.update(input=inputs, output=outputs)
            except Drop as d:
                dropped[str(d)] = dropped.get(str(d), 0) + 1
                continue
            used_tags.update(i["tag"] for i in inputs if i and "tag" in i)
            recipes.append(rec)

    tags = {}
    for t in sorted(used_tags):
        if t not in all_tags:
            sys.exit(f"tag {t} assente da item_tags.json")
        tags[t] = [n for n in all_tags[t] if n in ids]

    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump({"_license": LICENSE, "tags": tags, "recipes": recipes}, f, separators=(",", ":"), ensure_ascii=False)
        f.write("\n")
    shaped = sum(1 for x in recipes if x["kind"] == "shaped")
    print(f"{len(recipes)} ricette ({shaped} shaped, {len(recipes) - shaped} shapeless), {len(tags)} tag -> {out}")
    for why, n in sorted(dropped.items()):
        print(f"  scartate {n}: {why}")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
