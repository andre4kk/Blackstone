#!/usr/bin/env python3
"""Costruisce crates/server/data/smelting.json: ricette di fusione e combustibili della fornace.

Uso: python -I tools/extract_smelting.py <items.txt> <out.json>

Fonte: Dragonfly (MIT, commit 0f9a3a57, 1.26.60). Dragonfly non ha un file dati: la fusione sta nei metodi `SmeltInfo()` e il combustibile nei
metodi `FuelInfo()` dei singoli oggetti e blocchi (`server/item/*.go`, `server/block/*.go`). Qui le stesse regole sono trascritte a mano
(vedi i riferimenti accanto a ogni gruppo) e tradotte nei nomi degli oggetti di Bedrock; gli oggetti assenti da items.txt si scartano e
si stampano. La fornace accetta ogni `SmeltInfo` (`Furnace.Tick`: `supported` sempre vero) e cuoce in 10 s = 200 tick (`tickSmelting(10s)`).

Uscita: {"_license", "smelting": [{input, output, xp}], "fuel": [{item, ticks, residue}]}. `ticks` = secondi di Dragonfly x 20; `residue` e' il
nome dell'oggetto che resta al posto del combustibile (secchio di lava -> secchio vuoto) o null (si toglie un pezzo). Un oggetto compare una volta sola.
"""
import json
import sys

LICENSE = (
    "Generato da tools/extract_smelting.py dalle regole SmeltInfo/FuelInfo di Dragonfly (df-mc/dragonfly, commit 0f9a3a57, 1.26.60). "
    "MIT License, Copyright (c) 2019 Dragonfly Tech."
)

# Legni infiammabili (`wood.Flammable`: tutti tranne cremisi e distorto). Il bambu' e' a parte.
WOODS = ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak"]

smelting = {}  # input -> (output, xp)
fuel = {}  # item -> (secondi, residuo)


def smelt(inp, out, xp):
    smelting.setdefault(inp, (out, xp))


def burn(item, seconds, residue=None):
    fuel.setdefault(item, (seconds, residue))


def m(name):
    return "minecraft:" + name


# Minerali e cibi grezzi (`item/raw_*.go`, `block/*_ore.go`, `item/beef.go`...) e blocchi (`block/clay.go`, `sand.go`, `cobblestone.go`...).
for src, out, xp in [
    ("raw_iron", "iron_ingot", 0.7), ("raw_gold", "gold_ingot", 1), ("raw_copper", "copper_ingot", 0.7),
    ("iron_ore", "iron_ingot", 0.7), ("deepslate_iron_ore", "iron_ingot", 0.7),
    ("gold_ore", "gold_ingot", 1), ("deepslate_gold_ore", "gold_ingot", 1), ("nether_gold_ore", "gold_ingot", 1),
    ("copper_ore", "copper_ingot", 0.7), ("deepslate_copper_ore", "copper_ingot", 0.7),
    ("coal_ore", "coal", 0.1), ("deepslate_coal_ore", "coal", 0.1),
    ("diamond_ore", "diamond", 1), ("deepslate_diamond_ore", "diamond", 1),
    ("emerald_ore", "emerald", 1), ("deepslate_emerald_ore", "emerald", 1),
    ("lapis_ore", "lapis_lazuli", 0.2), ("deepslate_lapis_ore", "lapis_lazuli", 0.2),
    ("redstone_ore", "redstone", 0.7), ("deepslate_redstone_ore", "redstone", 0.7),
    ("quartz_ore", "quartz", 0.2), ("ancient_debris", "netherite_scrap", 2),
    ("beef", "cooked_beef", 0.35), ("porkchop", "cooked_porkchop", 0.35), ("chicken", "cooked_chicken", 0.35),
    ("mutton", "cooked_mutton", 0.35), ("rabbit", "cooked_rabbit", 0.35), ("cod", "cooked_cod", 0.35),
    ("salmon", "cooked_salmon", 0.35), ("potato", "baked_potato", 0.35), ("kelp", "dried_kelp", 0.1),
    ("clay_ball", "brick", 0.3), ("clay", "hardened_clay", 0.35), ("cobblestone", "stone", 0.1), ("sand", "glass", 0.1),
    ("netherrack", "netherbrick", 0.1), ("sea_pickle", "lime_dye", 0.1),
    ("stone", "smooth_stone", 0.1), ("stone_bricks", "cracked_stone_bricks", 0.1),
    ("cobbled_deepslate", "deepslate", 0.1), ("deepslate_bricks", "cracked_deepslate_bricks", 0.1),
    ("deepslate_tiles", "cracked_deepslate_tiles", 0.1), ("nether_brick", "cracked_nether_bricks", 0.1),
    ("polished_blackstone_bricks", "cracked_polished_blackstone_bricks", 0.1),
    ("quartz_block", "smooth_quartz", 0.1), ("sandstone", "smooth_sandstone", 0.1), ("red_sandstone", "smooth_red_sandstone", 0.1),
    ("wet_sponge", "sponge", 0.15),
]:
    smelt(m(src), m(out), xp)

# Argilla indurita colorata -> terracotta smaltata (`block/stained_terracotta.go`); `silver` e' il grigio chiaro dei nomi vecchi.
for colour in ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue", "brown",
               "green", "red", "black"]:
    glazed = "silver" if colour == "light_gray" else colour
    smelt(m(colour + "_terracotta"), m(glazed + "_glazed_terracotta"), 0.1)

# Tronchi e legno -> carbone di legna (`block/log.go`, `block/wood.go`: 0.15 XP); solo legni infiammabili.
for w in WOODS:
    for name in [f"{w}_log", f"stripped_{w}_log", f"{w}_wood", f"stripped_{w}_wood"]:
        smelt(m(name), m("charcoal"), 0.15)

# Attrezzi e armature di ferro, oro, rame (e maglia) -> pepita (`item/axe.go`, `boots.go`...: 0.1 XP).
for part in ["helmet", "chestplate", "leggings", "boots"]:
    smelt(m("iron_" + part), m("iron_nugget"), 0.1)
    smelt(m("chainmail_" + part), m("iron_nugget"), 0.1)
    smelt(m("golden_" + part), m("gold_nugget"), 0.1)
    smelt(m("copper_" + part), m("copper_nugget"), 0.1)
for tool in ["sword", "axe", "pickaxe", "shovel", "hoe"]:
    smelt(m("iron_" + tool), m("iron_nugget"), 0.1)
    smelt(m("golden_" + tool), m("gold_nugget"), 0.1)
    smelt(m("copper_" + tool), m("copper_nugget"), 0.1)

# Combustibili. Oggetti (`item/*.go`).
for name, s in [("coal", 80), ("charcoal", 80), ("blaze_rod", 120), ("stick", 5), ("bow", 10), ("bowl", 10), ("crossbow", 15),
                ("wooden_sword", 10), ("wooden_axe", 10), ("wooden_pickaxe", 10), ("wooden_shovel", 10), ("wooden_hoe", 10)]:
    burn(m(name), s)
burn(m("lava_bucket"), 1000, m("bucket"))
# Blocchi (`block/coal.go`, `bamboo*.go`, `dried_kelp.go`, ...); le forme "oak" senza prefisso sono i nomi vecchi di porta, botola, cancelletto.
for name, s in [("coal_block", 800), ("dried_kelp", 200), ("bamboo", 2.5), ("bamboo_block", 15), ("stripped_bamboo_block", 15),
                ("bamboo_mosaic", 15), ("barrel", 15), ("bookshelf", 15), ("chest", 15), ("composter", 15), ("crafting_table", 15),
                ("fletching_table", 15), ("jukebox", 15), ("ladder", 15), ("lectern", 15), ("loom", 15), ("noteblock", 15),
                ("banner", 15), ("bamboo_planks", 15), ("bamboo_fence", 15), ("bamboo_fence_gate", 15), ("bamboo_trapdoor", 15),
                ("bamboo_door", 10), ("bamboo_sign", 10), ("bamboo_slab", 15), ("bamboo_stairs", 15), ("bamboo_mosaic_slab", 15),
                ("bamboo_mosaic_stairs", 15), ("fence_gate", 15), ("trapdoor", 15), ("wooden_door", 10), ("wooden_slab", 15)]:
    burn(m(name), s)
for w in WOODS:
    burn(m(w + "_sapling"), 5)
    for name, s in [("planks", 15), ("log", 15), ("wood", 15), ("fence", 15), ("fence_gate", 15), ("trapdoor", 15), ("door", 10),
                    ("sign", 10), ("slab", 15), ("stairs", 15)]:
        burn(m(f"{w}_{name}"), s)
    for name in ["log", "wood"]:
        burn(m(f"stripped_{w}_{name}"), 15)


def main(items_txt, out):
    with open(items_txt, encoding="utf-8") as f:
        ids = {line.split("\t")[0] for line in f if line.strip()}
    missing = set()
    rec = []
    for inp in sorted(smelting):
        o, xp = smelting[inp]
        if inp in ids and o in ids:
            rec.append({"input": inp, "output": o, "xp": xp})
        else:
            missing.update(n for n in (inp, o) if n not in ids)
    fl = []
    for item in sorted(fuel):
        s, residue = fuel[item]
        if item in ids and (residue is None or residue in ids):
            fl.append({"item": item, "ticks": int(round(s * 20)), "residue": residue})
        elif item not in ids:
            missing.add(item)
    for n in sorted(missing):
        print("assente da items.txt (scartato):", n)
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump({"_license": LICENSE, "smelting": rec, "fuel": fl}, f, indent=1)
        f.write("\n")
    print(f"{len(rec)} fusioni, {len(fl)} combustibili")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
