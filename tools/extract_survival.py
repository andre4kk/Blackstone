"""Estrae da Dragonfly (MIT) le tabelle di sopravvivenza: drop dei blocchi, strumenti/armi, cibi.

Uso: python -I tools/extract_survival.py <vendor/dragonfly> <crates/server/data>

Scrive in <data>: block_drops.json, tools.json, foods.json. Legge anche blocks.json e items.txt (gia' presenti) solo per
controllare che ogni nome emesso esista davvero nel registro: un nome sbagliato interrompe lo script.

Come funziona:
- blocchi: ogni `BreakInfo()` di `server/block/*.go` viene letto cosi' com'e' (argomenti di `newBreakInfo`). Si prendono lo
  strumento richiesto e il livello (`pickaxeHarvestable`, `alwaysHarvestable`, funzioni inline con `HarvestLevel() >=`) e i
  drop (`oneOf`, `simpleDrops`, `silkTouch*`, `oreDrops`, `multiOreDrops`, `discreteDrops`, `grassDrops`). Un blocco che
  lascia se stesso senza requisiti non ha voce (la tabella dice "lascia se stesso"). Quel che il parser non capisce e' in
  `EXCEPTIONS`/`UNSUPPORTED`; gli stati della Go-struct (colore, legno...) non si leggono: i nomi Bedrock dei tipi sono in `NAMES`.
- livelli di raccolta: i numeri di Dragonfly (`ToolTier.HarvestLevel`): legno/oro 1, pietra/rame 2, ferro 3, diamante 4, netherite 4.
- strumenti: `AttackDamage()` = BaseAttackDamage del livello + bonus del tipo (letto da ogni file).
- cibi: `Saturate(punti, saturazione)` di `Consume`, `AlwaysConsumable`.
"""

import glob
import json
import os
import re
import sys

LICENSE = "Da Dragonfly (MIT, https://github.com/df-mc/dragonfly, 1.26.60) server/block, server/item. Generato da tools/extract_survival.py."

COLOURS = ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black"]
ORE_PREFIX = ["", "deepslate_"]

# Tipo Go -> nomi Bedrock (solo dove il nome non e' lo snake_case del tipo). Ogni voce ha il motivo.
NAMES = {
    # Stone senza `Smooth` e' "stone" (il ramo letto per primo e' quello Smooth, poi sovrascritto da EXCEPTIONS)
    "Stone": ["minecraft:smooth_stone"],
    "Granite": ["minecraft:granite", "minecraft:polished_granite"],
    "Diorite": ["minecraft:diorite", "minecraft:polished_diorite"],
    "Andesite": ["minecraft:andesite", "minecraft:polished_andesite"],
    "Quartz": ["minecraft:smooth_quartz"],  # primo ramo = Smooth; quartz_block e' in EXCEPTIONS
    "Sandstone": ["minecraft:smooth_sandstone", "minecraft:smooth_red_sandstone"],  # primo ramo = Smooth (hardness 2)
    "Basalt": ["minecraft:basalt", "minecraft:polished_basalt"],
    "Cobblestone": ["minecraft:cobblestone", "minecraft:mossy_cobblestone"],
    "Deepslate": ["minecraft:cobbled_deepslate", "minecraft:polished_deepslate"],  # ramo non-Normal; "deepslate" e' in EXCEPTIONS
    "Blackstone": [],  # tre voci a mano in EXCEPTIONS
    "NetherBricks": ["minecraft:nether_brick", "minecraft:red_nether_brick", "minecraft:chiseled_nether_bricks", "minecraft:cracked_nether_bricks"],
    "Prismarine": ["minecraft:prismarine", "minecraft:dark_prismarine", "minecraft:prismarine_bricks"],
    "StoneBricks": ["minecraft:stone_bricks", "minecraft:mossy_stone_bricks", "minecraft:cracked_stone_bricks", "minecraft:chiseled_stone_bricks"],
    "Anvil": ["minecraft:anvil", "minecraft:chipped_anvil", "minecraft:damaged_anvil"],
    "Copper": [],  # famiglia di blocchi di rame: generata a parte (COPPER_FAMILY)
    "CopperOre": ["minecraft:copper_ore", "minecraft:deepslate_copper_ore"],
    "CoalOre": ["minecraft:coal_ore", "minecraft:deepslate_coal_ore"],
    "IronOre": ["minecraft:iron_ore", "minecraft:deepslate_iron_ore"],
    "GoldOre": ["minecraft:gold_ore", "minecraft:deepslate_gold_ore"],
    "DiamondOre": ["minecraft:diamond_ore", "minecraft:deepslate_diamond_ore"],
    "EmeraldOre": ["minecraft:emerald_ore", "minecraft:deepslate_emerald_ore"],
    "LapisOre": ["minecraft:lapis_ore", "minecraft:deepslate_lapis_ore"],
    "RedstoneOre": ["minecraft:redstone_ore", "minecraft:lit_redstone_ore", "minecraft:deepslate_redstone_ore", "minecraft:lit_deepslate_redstone_ore"],
    "NetherQuartzOre": ["minecraft:quartz_ore"],
    "NetherGoldOre": ["minecraft:nether_gold_ore"],
    "Concrete": [f"minecraft:{c}_concrete" for c in COLOURS],
    "StainedTerracotta": [f"minecraft:{c}_terracotta" for c in COLOURS],
    # Bedrock chiama "silver" il grigio chiaro nella terracotta smaltata (Colour.SilverString in Dragonfly)
    "GlazedTerracotta": [f"minecraft:{'silver' if c == 'light_gray' else c}_glazed_terracotta" for c in COLOURS],
    "Terracotta": ["minecraft:hardened_clay"],  # Bedrock chiama "hardened_clay" la terracotta semplice
    "Melon": ["minecraft:melon_block"],
    "Glowstone": ["minecraft:glowstone"],
    "Clay": ["minecraft:clay"],
    "Grass": ["minecraft:grass_block"],
    "DirtPath": ["minecraft:grass_path"],
    "ShortGrass": ["minecraft:short_grass"],
    "Fern": ["minecraft:fern"],
    "DoubleTallGrass": ["minecraft:tall_grass"],
    "Dirt": ["minecraft:dirt", "minecraft:coarse_dirt"],
    "Snow": ["minecraft:snow"],  # il blocco di neve, non snow_layer
    "SeaLantern": ["minecraft:sea_lantern"],
    "Bookshelf": ["minecraft:bookshelf"],
    "Gravel": ["minecraft:gravel"],
    "Ice": ["minecraft:ice"],
    "PackedIce": ["minecraft:packed_ice"],
    "BlueIce": ["minecraft:blue_ice"],
    "Glass": ["minecraft:glass"],
    "GlassPane": ["minecraft:glass_pane"],
    "StainedGlass": [f"minecraft:{c}_stained_glass" for c in COLOURS],
    "StainedGlassPane": [f"minecraft:{c}_stained_glass_pane" for c in COLOURS],
    "InfestedStoneBricks": ["minecraft:infested_stone_bricks", "minecraft:infested_mossy_stone_bricks", "minecraft:infested_cracked_stone_bricks", "minecraft:infested_chiseled_stone_bricks"],
    "EnderChest": ["minecraft:ender_chest"],
    "Coral": [f"minecraft:{t}_coral" for t in ["tube", "brain", "bubble", "fire", "horn"]] + [f"minecraft:dead_{t}_coral" for t in ["tube", "brain", "bubble", "fire", "horn"]],
    "CoralBlock": [f"minecraft:{t}_coral_block" for t in ["tube", "brain", "bubble", "fire", "horn"]],
    "Leaves": [],  # le foglie hanno una voce propria (LEAVES) sotto
    "Obsidian": ["minecraft:obsidian", "minecraft:crying_obsidian"],
    "Hopper": ["minecraft:hopper"],
    "Netherrack": ["minecraft:netherrack"],
    "EndStone": ["minecraft:end_stone"],
    "EndBricks": ["minecraft:end_bricks"],
    "Bricks": ["minecraft:brick_block"],  # Bedrock: brick_block
    "PolishedBlackstoneBrick": ["minecraft:polished_blackstone_bricks", "minecraft:cracked_polished_blackstone_bricks"],
    "DeepslateBricks": ["minecraft:deepslate_bricks", "minecraft:cracked_deepslate_bricks"],
    "DeepslateTiles": ["minecraft:deepslate_tiles", "minecraft:cracked_deepslate_tiles"],
    "QuartzPillar": ["minecraft:quartz_pillar"],
    "ChiseledQuartz": ["minecraft:chiseled_quartz_block"],
    "QuartzBricks": ["minecraft:quartz_bricks"],
    "Lantern": ["minecraft:lantern", "minecraft:soul_lantern"],
    "RedstoneBlock": ["minecraft:redstone_block"],
    "Lapis": ["minecraft:lapis_block"],
    "Coal": ["minecraft:coal_block"],
    "Iron": ["minecraft:iron_block"],
    "Gold": ["minecraft:gold_block"],
    "Diamond": ["minecraft:diamond_block"],
    "Emerald": ["minecraft:emerald_block"],
    "Netherite": ["minecraft:netherite_block"],
    "Amethyst": ["minecraft:amethyst_block"],
    "AncientDebris": ["minecraft:ancient_debris"],
    "RawCopper": ["minecraft:raw_copper_block"],
    "RawGold": ["minecraft:raw_gold_block"],
    "RawIron": ["minecraft:raw_iron_block"],
    "Bone": ["minecraft:bone_block"],
    "Stonecutter": ["minecraft:stonecutter_block"],
    "NetherBrickFence": ["minecraft:nether_brick_fence"],
    "Mud": ["minecraft:mud"],
    "ShulkerBox": [f"minecraft:{c}_shulker_box" for c in COLOURS] + ["minecraft:undyed_shulker_box"],
    "EnchantingTable": ["minecraft:enchanting_table"],
    "Grindstone": ["minecraft:grindstone"],
    "IronBars": ["minecraft:iron_bars"],
    "DragonEgg": ["minecraft:dragon_egg"],
    "Magma": ["minecraft:magma"],
    "Calcite": ["minecraft:calcite"],
    "Dripstone": ["minecraft:dripstone_block"],
    "SmoothBasalt": ["minecraft:smooth_basalt"],
    "Purpur": ["minecraft:purpur_block"],
    "PurpurPillar": ["minecraft:purpur_pillar"],
    "Tuff": ["minecraft:tuff", "minecraft:chiseled_tuff"],
    "TuffBricks": ["minecraft:tuff_bricks", "minecraft:chiseled_tuff_bricks"],
    "PolishedTuff": ["minecraft:polished_tuff"],
    "ResinBricks": ["minecraft:resin_bricks", "minecraft:chiseled_resin_bricks"],
    "IronChain": ["minecraft:iron_chain"],
    "CopperBars": [],  # famiglia di rame (COPPER_FAMILY)
    "CopperChain": [],
    "CopperGrate": [],
    "CopperLantern": [],
    "CopperTrapdoor": [],
    "CopperDoor": [],
    "Cinnabar": ["minecraft:cinnabar", "minecraft:chiseled_cinnabar"],
    "CinnabarBricks": ["minecraft:cinnabar_bricks"],
    "PolishedCinnabar": ["minecraft:polished_cinnabar"],
    "Sulfur": ["minecraft:sulfur", "minecraft:chiseled_sulfur"],
    "SulfurBricks": ["minecraft:sulfur_bricks"],
    "PolishedSulfur": ["minecraft:polished_sulfur"],
    "NetherSprouts": ["minecraft:nether_sprouts"],
    "Vines": ["minecraft:vine"],
    "Cobweb": ["minecraft:web"],
    "DeadBush": ["minecraft:deadbush"],
    "Cake": ["minecraft:cake"],
}

# Voci scritte a mano (nome Bedrock -> voce) dove il parser non arriva o il ramo letto non e' quello giusto.
# Formato: (strumento|None, livello, [(item, min, max, chance)])
EXCEPTIONS = {
    # Stone: senza tocco di seta lascia pietrisco (silkTouchOneOf(Cobblestone, Stone)); il parser legge il ramo Smooth.
    "minecraft:stone": ("pickaxe", 1, [("minecraft:cobblestone", 1, 1, 1.0)]),
    # Deepslate normale: silkTouchOneOf(CobbledDeepslate, ...)
    "minecraft:deepslate": ("pickaxe", 1, [("minecraft:cobbled_deepslate", 1, 1, 1.0)]),
    # Quartz non liscio / sabbia: oneOf(self), stesso requisito
    "minecraft:quartz_block": ("pickaxe", 1, [("minecraft:quartz_block", 1, 1, 1.0)]),
    "minecraft:sandstone": ("pickaxe", 1, [("minecraft:sandstone", 1, 1, 1.0)]),
    "minecraft:red_sandstone": ("pickaxe", 1, [("minecraft:red_sandstone", 1, 1, 1.0)]),
    "minecraft:chiseled_sandstone": ("pickaxe", 1, [("minecraft:chiseled_sandstone", 1, 1, 1.0)]),
    "minecraft:cut_sandstone": ("pickaxe", 1, [("minecraft:cut_sandstone", 1, 1, 1.0)]),
    "minecraft:chiseled_red_sandstone": ("pickaxe", 1, [("minecraft:chiseled_red_sandstone", 1, 1, 1.0)]),
    "minecraft:cut_red_sandstone": ("pickaxe", 1, [("minecraft:cut_red_sandstone", 1, 1, 1.0)]),
    # Ghiaia: selce al 10% (senza fortuna), altrimenti ghiaia. Le due voci sommano 1: alternative esclusive.
    "minecraft:gravel": (None, 0, [("minecraft:flint", 1, 1, 0.1), ("minecraft:gravel", 1, 1, 0.9)]),
    # Foglie: Dragonfly da' la foglia solo con le cesoie; senza non da' sempre qualcosa (sapling/bastoni/mela a probabilita'
    # che qui non si modellano). Si tiene "serve cesoie -> la foglia" e basta.
    # Ragnatela: cesoie -> ragnatela (con la spada da' filo: non rappresentabile qui, quindi manca).
    "minecraft:web": ("shears", 0, [("minecraft:web", 1, 1, 1.0)]),
    # Cespuglio secco: senza cesoie 0-2 bastoni (rand.IntN(3)); con le cesoie darebbe se stesso (non rappresentabile).
    "minecraft:deadbush": (None, 0, [("minecraft:stick", 0, 2, 1.0)]),
    # Torce spente/forni accesi: si rompono lasciando l'oggetto "normale", non il nome dello stato Bedrock.
    "minecraft:lit_furnace": (None, 0, [("minecraft:furnace", 1, 1, 1.0)]),
    "minecraft:lit_blast_furnace": (None, 0, [("minecraft:blast_furnace", 1, 1, 1.0)]),
    "minecraft:lit_smoker": (None, 0, [("minecraft:smoker", 1, 1, 1.0)]),
    "minecraft:unlit_redstone_torch": (None, 0, [("minecraft:redstone_torch", 1, 1, 1.0)]),
    # Zucca intagliata/lanterna di zucca: oneOf(self) e il nome coincide: nessuna voce.
    # Cottura del cibo non c'entra. Torta: neverHarvestable + simpleDrops() = niente.
    "minecraft:cake": (None, 0, []),
    # Farmland: oneOf(Dirt{})
    "minecraft:farmland": (None, 0, [("minecraft:dirt", 1, 1, 1.0)]),
    # Blocchi di corallo: silkTouchOneOf(CoralBlock{Dead: true}, self): senza tocco di seta lasciano la versione morta.
    **{f"minecraft:{t}_coral_block": ("pickaxe", 1, [(f"minecraft:dead_{t}_coral_block", 1, 1, 1.0)]) for t in ["tube", "brain", "bubble", "fire", "horn"]},
    **{f"minecraft:dead_{t}_coral_block": ("pickaxe", 1, [(f"minecraft:dead_{t}_coral_block", 1, 1, 1.0)]) for t in ["tube", "brain", "bubble", "fire", "horn"]},
    # Blackstone: ramo `Polished` solo cambia la durezza; Gilded da' 2-5 pepite d'oro con il 10% (senza fortuna), altrimenti se stesso
    "minecraft:blackstone": ("pickaxe", 1, [("minecraft:blackstone", 1, 1, 1.0)]),
    "minecraft:polished_blackstone": ("pickaxe", 1, [("minecraft:polished_blackstone", 1, 1, 1.0)]),
    "minecraft:gilded_blackstone": ("pickaxe", 1, [("minecraft:gold_nugget", 2, 5, 0.1), ("minecraft:gilded_blackstone", 1, 1, 0.9)]),
}

# Distribuzione binomiale di Dragonfly `fortuneBinomial(3)`: tre tentativi indipendenti, ciascuno riuscito con probabilita' 8/15
# (`rand.IntN(15) < 8`). Si rappresenta con tre voci da 1 pezzo a probabilita' 8/15 (tiri indipendenti).
BINOMIAL_3 = 8 / 15


def at(key, lo, hi=None):
    """Condizione sullo stato del blocco: la voce vale solo se `key` e' tra `lo` e `hi` (inclusi)."""
    return (key, lo, lo if hi is None else hi)


def crop(seed, produce):
    """`cropSeedDrops` di Dragonfly (grano, barbabietola): sotto crescita 7 un seme, altrimenti il raccolto piu' 0-3 semi."""
    young = [(seed, 1, 1, 1.0, at("growth", 0, 6))]
    mature = [(produce, 1, 1, 1.0, at("growth", 7))] + [(seed, 1, 1, BINOMIAL_3, at("growth", 7))] * 3
    return (None, 0, young + mature)


def root(item, poison=False):
    """Carota e patata di Dragonfly: sotto crescita 7 uno, poi `1 + fortuneBinomial(3)` (1-4); la patata ha il 2% di patata velenosa."""
    drops = [(item, 1, 1, 1.0, at("growth", 0, 6)), (item, 1, 1, 1.0, at("growth", 7))] + [(item, 1, 1, BINOMIAL_3, at("growth", 7))] * 3
    if poison:
        drops.append(("minecraft:poisonous_potato", 1, 1, 0.02, at("growth", 7)))
    return (None, 0, drops)


def state_exceptions(blocks):
    """Voci scritte a mano per blocchi il cui oggetto ha un altro nome o i cui drop dipendono dallo stato (crescita, altezza).
    Fonte: Dragonfly server/block (BreakInfo di ciascun tipo) dove c'e'; dove Dragonfly non ha il blocco (neve a strati, bacche, arnesi di
    redstone) vale il vanilla. Quel che non si sa modellare giusto non e' qui: lascia niente (vedi la lista stampata da `main`)."""
    out = {}
    nothing = (None, 0, [])

    def oneof(item):
        return (None, 0, [(item, 1, 1, 1.0)])

    # nomi dei blocchi che non sono quelli dell'oggetto
    for n in sorted(blocks):
        m = re.fullmatch(r"minecraft:(\w+)_(standing|wall)_sign", n)
        if m:  # Sign.BreakInfo: oneOf(Sign{Wood}); `darkoak` e' il nome Bedrock della quercia scura
            out[n] = oneof(f"minecraft:{'dark_oak' if m.group(1) == 'darkoak' else m.group(1)}_sign")
        m = re.fullmatch(r"minecraft:(\w+)_candle_cake", n)
        if m:  # vanilla: la torta con la candela lascia la candela (la torta no)
            out[n] = oneof(f"minecraft:{m.group(1)}_candle")
        if re.fullmatch(r"minecraft:(dead_)?\w+_coral_wall_fan", n):
            out[n] = nothing  # come i ventagli di Dragonfly (`silkTouchOnlyDrop`: niente senza tocco di seta)
    out["minecraft:standing_sign"] = out["minecraft:wall_sign"] = oneof("minecraft:oak_sign")
    out["minecraft:candle_cake"] = oneof("minecraft:candle")
    out["minecraft:standing_banner"] = out["minecraft:wall_banner"] = oneof("minecraft:banner")  # Banner.BreakInfo: oneOf(b); il colore non si conserva
    out["minecraft:unpowered_repeater"] = out["minecraft:powered_repeater"] = oneof("minecraft:repeater")
    out["minecraft:unpowered_comparator"] = out["minecraft:powered_comparator"] = oneof("minecraft:comparator")
    out["minecraft:reeds"] = oneof("minecraft:sugar_cane")  # SugarCane.BreakInfo: oneOf(c)
    out["minecraft:lit_redstone_lamp"] = oneof("minecraft:redstone_lamp")
    out["minecraft:daylight_detector_inverted"] = oneof("minecraft:daylight_detector")
    out["minecraft:redstone_wire"] = oneof("minecraft:redstone")
    out["minecraft:trip_wire"] = oneof("minecraft:string")
    out["minecraft:melon_stem"] = oneof("minecraft:melon_seeds")  # MelonSeeds.BreakInfo: oneOf(m)
    out["minecraft:pumpkin_stem"] = oneof("minecraft:pumpkin_seeds")  # PumpkinSeeds.BreakInfo: oneOf(p)
    # colture: Dragonfly dipende dalla crescita (0-7)
    out["minecraft:wheat"] = crop("minecraft:wheat_seeds", "minecraft:wheat")
    out["minecraft:beetroot"] = crop("minecraft:beetroot_seeds", "minecraft:beetroot")
    out["minecraft:carrots"] = root("minecraft:carrot")
    out["minecraft:potatoes"] = root("minecraft:potato", poison=True)
    # CocoaBean.BreakInfo: eta' 2 -> 2-3 semi (`rand.IntN(2)+2`), altrimenti 1
    out["minecraft:cocoa"] = (None, 0, [("minecraft:cocoa_beans", 1, 1, 1.0, at("age", 0, 1)), ("minecraft:cocoa_beans", 2, 3, 1.0, at("age", 2))])
    # NetherWart.BreakInfo: eta' < 3 -> 1, altrimenti 2-4 (`fortuneDiscreteCount(2, 4, ...)`)
    out["minecraft:nether_wart"] = (None, 0, [("minecraft:nether_wart", 1, 1, 1.0, at("age", 0, 2)), ("minecraft:nether_wart", 2, 4, 1.0, at("age", 3))])
    # Bacche dolci: Dragonfly non ha il blocco. Vanilla: da maturo 2-3, a meta' crescita 1-2, prima niente (le fasi 0-7 di Bedrock
    # si sono mappate sull'eta' 0-3 di Java: 7 = pieno, 4-6 = meta'; la corrispondenza esatta non e' verificata).
    out["minecraft:sweet_berry_bush"] = (None, 0, [("minecraft:sweet_berries", 2, 3, 1.0, at("growth", 7)), ("minecraft:sweet_berries", 1, 2, 1.0, at("growth", 4, 6))])
    # Neve a strati: Dragonfly non ha il blocco. Vanilla: serve una pala, lascia tante palle di neve quanti sono gli strati (height 0 = 1 strato).
    out["minecraft:snow_layer"] = ("shovel", 0, [("minecraft:snowball", h + 1, h + 1, 1.0, at("height", h)) for h in range(8)])
    # Ammasso d'ametista: con il piccone 4 schegge (vanilla); senza il vanilla ne darebbe 2, qui niente. Gemme in crescita e generatori: niente.
    out["minecraft:amethyst_cluster"] = ("pickaxe", 0, [("minecraft:amethyst_shard", 4, 4, 1.0)])
    for n in ["minecraft:budding_amethyst", "minecraft:small_amethyst_bud", "minecraft:medium_amethyst_bud", "minecraft:large_amethyst_bud", "minecraft:mob_spawner", "minecraft:trial_spawner"]:
        out[n] = nothing
    for n in out:
        assert n in blocks, n
    return out


# Tipi che il parser non sa leggere: non hanno voce (il blocco lascia se stesso). Motivo per ciascuno.
UNSUPPORTED = {
    "Carrot": "drop dipende dalla crescita (stato) e da fortuna; il nome Bedrock e' 'carrots'",
    "Potato": "drop dipende dalla crescita e dal 2% di patata velenosa; 'potatoes'",
    "WheatSeeds": "cropSeedDrops: dipende dalla crescita; 'wheat'",
    "BeetrootSeeds": "cropSeedDrops: dipende dalla crescita; 'beetroot'",
    "NetherWart": "dipende dall'eta'",
    "CocoaBean": "dipende dall'eta'",
    "Campfire": "carbone o terra d'anime secondo il tipo; si legge solo con tocco di seta",
    "Cake": "gestito in EXCEPTIONS",
    "Candle": "dipende dal numero di candele",
    "Lectern": "dipende dal libro contenuto",
    "PinkPetals": "dipende dal numero",
    "SeaPickle": "dipende dal numero",
    "ShelfMushroom": "dipende dalla dimensione",
    "DecoratedPot": "dipende dalle decorazioni",
    "Skull": "nome dell'oggetto dipende dal tipo",
    "Sign": "nome dell'oggetto dipende dal legno",
    "Banner": "oggetto dipende dal colore",
    "Slab": "regole a famiglia sotto",
    "Stairs": "regole a famiglia sotto",
    "Wall": "regole a famiglia sotto",
}

COPPER_STATES = ["", "exposed_", "weathered_", "oxidized_"]
# Nomi di legno (per riconoscere scale/lastre/muri di legno: si rompono con qualsiasi strumento)
WOOD_BASES = {"oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak", "bamboo", "bamboo_mosaic", "crimson", "warped", "poplar"}


def split_args(text, start):
    """Da `text[start]` (subito dopo la parentesi aperta) restituisce la lista di argomenti di primo livello e la posizione di fine."""
    depth = 0
    args = []
    cur = []
    i = start
    in_str = None
    while i < len(text):
        ch = text[i]
        if in_str:
            cur.append(ch)
            if ch == "\\" and in_str == '"':
                i += 1
                cur.append(text[i])
            elif ch == in_str:
                in_str = None
        elif ch in "\"`'":
            in_str = ch
            cur.append(ch)
        elif ch in "({[":
            depth += 1
            cur.append(ch)
        elif ch in ")}]":
            if depth == 0:
                args.append("".join(cur).strip())
                return args, i
            depth -= 1
            cur.append(ch)
        elif ch == "," and depth == 0:
            args.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
        i += 1
    raise ValueError("parentesi non chiusa")


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def load_tiers(dragonfly):
    s = open(os.path.join(dragonfly, "server", "item", "tool.go"), encoding="utf8").read()
    tiers = {}
    for m in re.finditer(r"(ToolTier\w+) = ToolTier\{HarvestLevel: (\d+), Durability: \d+, BaseMiningEfficiency: [\d.]+, BaseAttackDamage: ([\d.]+), EnchantmentValue: \d+, Name: \"(\w+)\"\}", s):
        tiers[m.group(1)] = {"harvest": int(m.group(2)), "attack": float(m.group(3)), "name": m.group(4)}
    assert len(tiers) == 7, tiers
    return tiers


def load_item_names(dragonfly):
    """Tipo item Go -> nome Bedrock (solo EncodeItem con un solo letterale)."""
    out = {}
    for f in glob.glob(os.path.join(dragonfly, "server", "item", "*.go")):
        s = open(f, encoding="utf8").read()
        for m in re.finditer(r"func \((?:\w+ )?(\w+)\) EncodeItem\(\) \(name string, meta int16\) \{\n(.*?)\n\}\n", s, re.S):
            lits = re.findall(r'"(minecraft:[^"]*)"', m.group(2))
            if len(lits) == 1 and "+" not in m.group(2):
                out[m.group(1)] = lits[0]
    return out


# Tipi blocco usati come drop diversi da se stessi (espressione senza spazi -> nome Bedrock dell'oggetto)
BLOCK_DROP_ITEMS = {
    "Dirt{}": "minecraft:dirt",
    "Cobblestone{}": "minecraft:cobblestone",
    "Obsidian{}": "minecraft:obsidian",
    "Deepslate{Type:CobbledDeepslate()}": "minecraft:cobbled_deepslate",
    "RedstoneWire{}": "minecraft:redstone",
    "Bamboo{}": "minecraft:bamboo",
    "WheatSeeds{}": "minecraft:wheat_seeds",
}


def drop_item_name(expr, item_names):
    e = re.sub(r"\s+", "", expr)
    if e in BLOCK_DROP_ITEMS:
        return BLOCK_DROP_ITEMS[e]
    m = re.fullmatch(r"item\.(\w+)\{\}", e)
    if m and m.group(1) in item_names:
        return item_names[m.group(1)]
    return None


def parse_harvest(arg):
    a = arg.strip()
    if a == "alwaysHarvestable":
        return (None, 0)
    if a == "pickaxeHarvestable":
        return ("pickaxe", 1)
    if a == "neverHarvestable":
        return ("never", 0)
    m = re.search(r"ToolType\(\) == item\.Type(\w+) && t\.HarvestLevel\(\) >= item\.(ToolTier\w+)\.HarvestLevel", a)
    if m:
        return (m.group(1).lower(), ("tier", m.group(2)))
    m = re.fullmatch(r"func\(t item\.Tool\) bool \{\s*return t\.ToolType\(\) == item\.Type(\w+)\s*\}", a)
    if m:
        return (m.group(1).lower(), 0)
    return None


def parse_count(expr):
    expr = expr.strip()
    if re.fullmatch(r"\d+", expr):
        n = int(expr)
        return (n, n)
    m = re.fullmatch(r"rand\.IntN\((\d+)\)\s*\+\s*(\d+)", expr)
    if m:
        return (int(m.group(2)), int(m.group(2)) + int(m.group(1)) - 1)
    return None


def parse_drops(arg, recv, typ, item_names):
    """Restituisce 'SELF', una lista di (item, min, max, chance) oppure None se non capito."""
    a = arg.strip()
    m = re.match(r"(\w+)\(", a)
    if not m:
        return None
    fn = m.group(1)
    args, _ = split_args(a, m.end())

    def is_self(e):
        e = e.strip()
        return e == recv or re.fullmatch(typ + r"\{.*\}", e) is not None

    if fn == "oneOf":
        if len(args) == 1 and is_self(args[0]):
            return "SELF"
        if len(args) == 1:
            n = drop_item_name(args[0], item_names)
            return [(n, 1, 1, 1.0)] if n else None
        return None
    if fn == "simpleDrops":
        if not args or args == [""]:
            return []
        if len(args) == 1:
            sm = re.fullmatch(r"item\.NewStack\((.*),\s*(\d+)\)", args[0].strip())
            if sm and is_self(sm.group(1)) and sm.group(2) == "1":
                return "SELF"
        return None
    if fn == "silkTouchOneOf":
        n = drop_item_name(args[0], item_names)
        return [(n, 1, 1, 1.0)] if n else None
    if fn == "silkTouchOnlyDrop":
        return []
    if fn == "silkTouchDrop":
        sm = re.fullmatch(r"item\.NewStack\((.*),\s*(.*)\)", args[0].strip())
        if sm:
            cnt = parse_count(sm.group(2))
            n = drop_item_name(sm.group(1), item_names)
            if n and cnt:
                return [(n, cnt[0], cnt[1], 1.0)]
        return None
    if fn == "oreDrops":
        n = drop_item_name(args[0], item_names)
        return [(n, 1, 1, 1.0)] if n else None
    if fn == "multiOreDrops":
        n = drop_item_name(args[0], item_names)
        return [(n, int(args[2]), int(args[3]), 1.0)] if n else None
    if fn == "discreteDrops":
        n = drop_item_name(args[0], item_names)
        if n is None and args[0].strip() == "RedstoneWire{}":
            n = "minecraft:redstone"
        return [(n, int(args[2]), int(args[3]), 1.0)] if n else None
    if fn == "grassDrops":
        return [("minecraft:wheat_seeds", 1, 1, 0.125)]
    return None


def parse_blocks(dragonfly, tiers, item_names):
    records = {}
    for f in sorted(glob.glob(os.path.join(dragonfly, "server", "block", "*.go"))):
        s = open(f, encoding="utf8").read()
        for m in re.finditer(r"func \((\w+) \*?(\w+)\) BreakInfo\(\) BreakInfo \{\n(.*?)\n\}\n", s, re.S):
            recv, typ, body = m.group(1), m.group(2), m.group(3)
            k = body.find("newBreakInfo(")
            if k < 0:
                records[typ] = ("NOPARSE", None, None)
                continue
            args, _ = split_args(body, k + len("newBreakInfo("))
            if len(args) != 4:
                records[typ] = ("NOPARSE", None, None)
                continue
            h = parse_harvest(args[1])
            if h is None:
                records[typ] = ("NOPARSE", None, None)
                continue
            tool, tier = h
            if isinstance(tier, tuple):
                tier = tiers[tier[1]]["harvest"]
            records[typ] = (tool, tier, parse_drops(args[3], recv, typ, item_names))
    return records


def family_entries(blocks):
    """Scale, lastre, muri e rame (famiglie con molti nomi): regole sul nome Bedrock."""
    out = {}
    for n in sorted(blocks):
        m = re.fullmatch(r"minecraft:(.+?)(_double_slab|_slab|_stairs|_wall)", n)
        if not m:
            continue
        base, kind = m.group(1), m.group(2)
        if kind == "_wall":
            tool, tier = "pickaxe", 1
        elif base in WOOD_BASES or base.endswith("_wool") or base == "mud_brick":
            # Dragonfly: scale/lastre prendono il BreakInfo del blocco base: legno/lana/mattoni di fango si rompono con tutto.
            tool, tier = None, 0
        elif "copper" in base:
            tool, tier = "pickaxe", 2
        else:
            tool, tier = "pickaxe", 1
        if kind == "_double_slab" or "double_cut_copper_slab" in n:
            # i doppi di rame tagliato si chiamano `[stato_]double_cut_copper_slab`, non `..._double_slab`
            single = n[: -len("_double_slab")] + "_slab" if kind == "_double_slab" else n.replace("double_", "")
            assert single in blocks, single
            out[n] = (tool, tier, [(single, 2, 2, 1.0)])
        elif tool:
            out[n] = (tool, tier, "SELF")
    return out


def copper_family(blocks):
    """Dragonfly: blocchi di rame (Copper, grata, lanterna, botola, porta) livello pietra; sbarre/catena: qualsiasi piccone."""
    out = {}
    for n in sorted(blocks):
        m = re.fullmatch(r"minecraft:(waxed_)?(exposed_|weathered_|oxidized_)?(copper_block|cut_copper|chiseled_copper|copper_grate|copper_trapdoor|copper_door|copper_bulb)", n)
        if m:
            out[n] = ("pickaxe", 2, "SELF")
            continue
        m = re.fullmatch(r"minecraft:(waxed_)?(exposed_|weathered_|oxidized_)?(copper_bars|copper_chain|copper_lantern)", n)
        if m:
            out[n] = ("pickaxe", 1, "SELF")
    return out


def main(dragonfly, data):
    blocks_doc = {
        b["name"]: {st["key"]: st["values"] for st in b["states"] if st["type"] in ("int", "byte")}
        for b in json.load(open(os.path.join(data, "blocks.json"), encoding="utf8"))["blocks"]
    }
    blocks = set(blocks_doc)
    items = {l.split("\t")[0] for l in open(os.path.join(data, "items.txt"), encoding="utf8").read().splitlines() if l}
    tiers = load_tiers(dragonfly)
    item_names = load_item_names(dragonfly)

    # ---- blocchi
    records = parse_blocks(dragonfly, tiers, item_names)
    result = {}
    taken = 0
    unparsed = []
    for typ, (tool, tier, drops) in sorted(records.items()):
        if typ in ("Slab", "Stairs", "Wall", "Copper", "CopperBars", "CopperChain", "CopperGrate", "CopperLantern", "CopperTrapdoor", "CopperDoor", "Leaves"):
            continue
        if typ in UNSUPPORTED:
            continue
        if tool == "NOPARSE" or drops is None:
            unparsed.append(typ)
            continue
        if tool == "never":
            tool, tier = None, 0
            if drops != []:
                unparsed.append(typ)
                continue
        if tool is None and drops == "SELF":
            continue
        names = NAMES.get(typ)
        if names is None:
            names = ["minecraft:" + snake(typ)]
        for n in names:
            result[n] = (tool, tier, drops)
    unmapped = []
    for n, v in list(result.items()):
        if n not in blocks:
            unmapped.append(n)
            del result[n]
    result.update(family_entries(blocks))
    result.update(copper_family(blocks))
    # foglie: servono le cesoie, danno la foglia
    for n in sorted(blocks):
        if re.fullmatch(r"minecraft:(\w+_)?leaves(_\w+)?", n) and "leaf_litter" not in n:
            result[n] = ("shears", 0, "SELF")
    for n, (tool, tier, drops) in EXCEPTIONS.items():
        result[n] = (tool, tier, [tuple(d) for d in drops])
    for n, (tool, tier, drops) in state_exceptions(blocks_doc).items():
        result[n] = (tool, tier, drops)
    # normalizza SELF
    out = {}
    for n, (tool, tier, drops) in sorted(result.items()):
        if drops == "SELF":
            drops = [(n, 1, 1, 1.0)]
        entries = []
        for i, a, b, c, *when in drops:
            assert i in items, (n, i)
            entry = {"item": i, "min": a, "max": b, "chance": c}
            if when:
                key, lo, hi = when[0]
                values = blocks_doc[n][key]
                assert min(values) <= lo <= hi <= max(values), (n, key, lo, hi)
                entry["when"] = {"state": key, "min": lo, "max": hi}
            entries.append(entry)
        out[n] = {"tool": tool, "min_tier": tier, "drops": entries}
    # le voci dei tipi Go senza nome valido si segnalano ma non si tengono
    for n in out:
        assert n in blocks, n
    # Blocchi senza voce il cui nome non e' un oggetto: in gioco non lasciano niente. Si stampano perche' siano rivisti (non e' un errore:
    # acqua, fuoco, portali, blocchi di debug/educazione non lasciano niente nemmeno in vanilla).
    silent = sorted(n for n in blocks if n not in out and n not in items and not n.startswith(("minecraft:element_", "minecraft:hard_")))
    print(f"blocchi senza voce che non lasciano niente ({len(silent)}):", ", ".join(silent))
    doc = {"_license": LICENSE, "blocks": out}
    with open(os.path.join(data, "block_drops.json"), "w", encoding="utf8", newline="\n") as f:
        json.dump(doc, f, indent=1, ensure_ascii=False)
        f.write("\n")
    print(f"blocchi: {len(out)} voci; tipi Go letti {len(records)}; eccezioni a mano {len(EXCEPTIONS)}; non letti {unparsed}; nomi non in blocks.json {unmapped}")
    print("  non supportati (lasciano se stessi):", ", ".join(sorted(UNSUPPORTED)))

    # ---- strumenti
    tools = {}
    bonus = {}
    for kind in ["pickaxe", "axe", "shovel", "hoe", "sword"]:
        s = open(os.path.join(dragonfly, "server", "item", f"{kind}.go"), encoding="utf8").read()
        m = re.search(r"AttackDamage\(\) float64 \{\n\treturn \w\.Tier\.BaseAttackDamage( \+ (\d+))?\n", s)
        assert m, kind
        bonus[kind] = int(m.group(2) or 0)
    for t in tiers.values():
        for kind in bonus:
            n = f"minecraft:{t['name']}_{kind}"
            if n in items:
                tools[n] = {"kind": kind, "tier": t["harvest"], "attack_damage": t["attack"] + bonus[kind]}
    s = open(os.path.join(dragonfly, "server", "item", "shears.go"), encoding="utf8").read()
    assert "TypeShears" in s and "minecraft:shears" in s
    tools["minecraft:shears"] = {"kind": "shears", "tier": 0, "attack_damage": 1.0}
    with open(os.path.join(data, "tools.json"), "w", encoding="utf8", newline="\n") as f:
        json.dump({"_license": LICENSE, "tools": dict(sorted(tools.items()))}, f, indent=1, ensure_ascii=False)
        f.write("\n")
    print(f"strumenti: {len(tools)}")

    # ---- cibi
    foods = {}
    manual = 0
    # Cibi con ramo crudo/cotto: nome (letterale in EncodeItem) -> (punti, saturazione). Letti a mano dai rami di Consume.
    BRANCHES = {
        "beef.go": [("minecraft:beef", 3, 1.8), ("minecraft:cooked_beef", 8, 12.8)],
        "porkchop.go": [("minecraft:porkchop", 3, 1.8), ("minecraft:cooked_porkchop", 8, 12.8)],
        "mutton.go": [("minecraft:mutton", 2, 1.2), ("minecraft:cooked_mutton", 6, 9.6)],
        "chicken.go": [("minecraft:chicken", 2, 1.2), ("minecraft:cooked_chicken", 6, 7.2)],
        "cod.go": [("minecraft:cod", 2, 0.4), ("minecraft:cooked_cod", 5, 6.0)],
        "salmon.go": [("minecraft:salmon", 2, 0.4), ("minecraft:cooked_salmon", 6, 9.6)],
        "rabbit.go": [("minecraft:rabbit", 3, 1.8), ("minecraft:cooked_rabbit", 5, 6.0)],
    }
    for f in sorted(glob.glob(os.path.join(dragonfly, "server", "item", "*.go"))):
        s = open(f, encoding="utf8").read()
        sat = re.findall(r"Saturate\(([\d.]+), ([\d.]+)\)", s)
        if not sat:
            continue
        base = os.path.basename(f)
        always = re.search(r"AlwaysConsumable\(\) bool \{\n\treturn (\w+)\n", s)
        always_edible = bool(always and always.group(1) == "true")
        if base in BRANCHES:
            # verifica che i valori scritti a mano coincidano con quelli del sorgente
            got = sorted((float(a), float(b)) for a, b in sat)
            want = sorted((float(p), sa) for _, p, sa in BRANCHES[base])
            assert got == want, (base, got, want)
            for n, p, sa in BRANCHES[base]:
                foods[n] = {"points": p, "saturation": sa, "always_edible": always_edible}
                manual += 1
            continue
        lits = re.findall(r'"(minecraft:[^"]*)"', s)
        assert len(sat) == 1 and len(set(lits)) == 1, (base, sat, lits)
        foods[lits[0]] = {"points": int(sat[0][0]), "saturation": float(sat[0][1]), "always_edible": always_edible}
    # Cibi che Dragonfly non ha (o ha sbagliati) ma che in vanilla si mangiano: (punti, saturazione, sempre commestibile).
    # Valori vanilla noti (wiki): carota 3/3.6, patata 1/0.6, bacche dolci e luminose 2/0.4, frutto del coro 4/2.4 (si mangia anche a pancia
    # piena), alga secca 1/0.6 (Dragonfly ha 0.2), barbabietola 1/1.2 (gia' in Dragonfly: controllata sotto).
    EXTRA = {
        "minecraft:carrot": (3, 3.6, False),
        "minecraft:potato": (1, 0.6, False),
        "minecraft:sweet_berries": (2, 0.4, False),
        "minecraft:glow_berries": (2, 0.4, False),
        "minecraft:chorus_fruit": (4, 2.4, True),
        "minecraft:dried_kelp": (1, 0.6, False),
        "minecraft:beetroot": (1, 1.2, False),
    }
    for n, (p, sa, always_edible) in EXTRA.items():
        if n == "minecraft:beetroot":
            assert foods[n] == {"points": p, "saturation": sa, "always_edible": always_edible}, foods.get(n)
        foods[n] = {"points": p, "saturation": sa, "always_edible": always_edible}
    for n in foods:
        assert n in items, n
    with open(os.path.join(data, "foods.json"), "w", encoding="utf8", newline="\n") as f:
        json.dump({"_license": LICENSE, "foods": dict(sorted(foods.items()))}, f, indent=1, ensure_ascii=False)
        f.write("\n")
    print(f"cibi: {len(foods)} (di cui {manual} da rami crudo/cotto scritti a mano e verificati)")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
