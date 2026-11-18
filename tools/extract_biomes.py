#!/usr/bin/env python3
"""Costruisce crates/server/data/biomes.json: i biomi che il server manda nella BiomeDefinitionList.

Uso: python -I tools/extract_biomes.py <pmmp/BedrockData> <Mojang/bedrock-samples> <out.json> [biomes.json di Geyser]

Fonti:
- pmmp/BedrockData (licenza CC0, release 1.26.30): `biome_definitions.json` (clima, profondita', colore dell'acqua, tag) e
  `biome_id_map.json` (nome -> ID numerico). I nomi sono quelli di Bedrock ("ice_plains", "swampland", ...).
- Mojang/bedrock-samples (v1.26.60.5, soggetto all'EULA di Minecraft): `behavior_pack/biomes/*.biome.json`, solo per i biomi
  che BedrockData non ha ancora (oggi `minecraft:dappled_forest`). L'ID numerico di questi viene dalla tabella di Geyser
  (`generator/data/geyser/biomes.json`, secondo argomento opzionale).

Per un bioma preso da bedrock-samples: temperatura, precipitazioni, tag e `snow_accumulation` stanno nel JSON; profondita' e
scala si ricavano dal `noise_type` di `minecraft:overworld_height` con la tabella appresa dai biomi di BedrockData (che hanno
entrambe le fonti); `rain` e' vero se le precipitazioni sono > 0 (vale per tutti i biomi di BedrockData tranne `the_end`).
Il colore dell'acqua non e' nei JSON di Mojang: si usa quello della pianura (ARGB 0xA544AFF5), un'ipotesi dichiarata qui.
"""
import glob
import json
import os
import sys

LICENSE = (
    "Generato da tools/extract_biomes.py. Biomi da pmmp/BedrockData (CC0, 1.26.30); i biomi mancanti da Mojang/bedrock-samples "
    "v1.26.60.5 (soggetti all'EULA di Minecraft, come items.txt)."
)
DEFAULT_WATER = {"a": 165, "r": 68, "g": 175, "b": 245}  # pianura
FOLIAGE_SNOW = 0.0  # tutti i biomi di BedrockData hanno 0


def argb(c):
    return (c["a"] << 24) | (c["r"] << 16) | (c["g"] << 8) | c["b"]


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def main(bedrock_data, samples, out, geyser):
    definitions = load(os.path.join(bedrock_data, "biome_definitions.json"))
    ids = load(os.path.join(bedrock_data, "biome_id_map.json"))
    biome_dir = os.path.join(samples, "behavior_pack", "biomes")
    sample = {}
    for path in sorted(glob.glob(os.path.join(biome_dir, "*.biome.json"))):
        doc = load(path)["minecraft:biome"]
        sample[doc["description"]["identifier"]] = doc["components"]

    biomes = []
    noise = {}  # noise_type -> (depth, scale), appreso dai biomi che hanno entrambe le fonti
    for name, d in definitions.items():
        short = name.split(":", 1)[1]
        if short not in ids:
            sys.exit(f"{name}: manca in biome_id_map.json")
        biomes.append(
            {
                "name": name,
                "id": ids[short],
                "temperature": d["temperature"],
                "downfall": d["downfall"],
                "foliage_snow": d["foliageSnow"],
                "depth": d["depth"],
                "scale": d["scale"],
                "water_argb": argb(d["mapWaterColour"]),
                "rain": d["rain"],
                "tags": d["tags"],
                "source": "BedrockData",
            }
        )
        kind = sample.get(name, {}).get("minecraft:overworld_height", {}).get("noise_type")
        if kind is not None:
            if noise.setdefault(kind, (d["depth"], d["scale"])) != (d["depth"], d["scale"]):
                sys.exit(f"noise_type {kind}: profondita'/scala incoerenti tra i biomi")

    have = {b["name"] for b in biomes}
    geyser_ids = {k: v["bedrock_id"] for k, v in load(geyser).items()}
    for name in sorted(set(sample) - have):
        comp = sample[name]
        climate = comp["minecraft:climate"]
        kind = comp.get("minecraft:overworld_height", {}).get("noise_type")
        if kind not in noise:
            sys.exit(f"{name}: noise_type {kind!r} sconosciuto (noti: {sorted(noise)})")
        if name not in geyser_ids:
            sys.exit(f"{name}: manca l'ID numerico nella tabella di Geyser")
        depth, scale = noise[kind]
        biomes.append(
            {
                "name": name,
                "id": geyser_ids[name],
                "temperature": climate["temperature"],
                "downfall": climate["downfall"],
                "foliage_snow": FOLIAGE_SNOW,
                "depth": depth,
                "scale": scale,
                "water_argb": argb(DEFAULT_WATER),
                "rain": climate["downfall"] > 0,
                "tags": sorted(comp["minecraft:tags"]["tags"]),
                "source": "bedrock-samples",
            }
        )

    biomes.sort(key=lambda b: (b["id"], b["name"]))
    seen = {}
    for b in biomes:
        if b["id"] in seen:
            sys.exit(f"ID {b['id']} duplicato: {seen[b['id']]} e {b['name']}")
        seen[b["id"]] = b["name"]
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump({"_license": LICENSE, "biomes": biomes}, f, indent=1, ensure_ascii=False)
        f.write("\n")
    extra = [b["name"] for b in biomes if b["source"] != "BedrockData"]
    print(f"{len(biomes)} biomi -> {out} (da bedrock-samples: {', '.join(extra) or 'nessuno'})")


if __name__ == "__main__":
    if len(sys.argv) not in (4, 5):
        sys.exit(__doc__)
    default_geyser = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "generator", "data", "geyser", "biomes.json")
    main(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] if len(sys.argv) == 5 else default_geyser)
