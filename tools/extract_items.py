#!/usr/bin/env python3
"""Estrae nome e ID numerico degli oggetti dai dati ufficiali di Mojang.

Uso: python tools/extract_items.py <cartella di Mojang/bedrock-samples> crates/server/data/items.txt
I dati sono soggetti all'EULA di Minecraft (github.com/Mojang/bedrock-samples): controllare cosa si può distribuire.
"""
import json, sys
src, out = sys.argv[1], sys.argv[2]
items = json.load(open(f"{src}/metadata/vanilladata_modules/mojang-items.json"))["data_items"]
open(out, "w").write("".join(f"{i['name']}\t{i['raw_id']}\n" for i in sorted(items, key=lambda i: i["raw_id"])))
print(len(items), "oggetti ->", out)
