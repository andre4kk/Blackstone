# Dragonfly -> Blackstone: note tappa 1 (piazzamento, rottura, creativo)

Fonte: `vendor/dragonfly` (gitignored), MIT (LICENSE verificata: "MIT License, Copyright (c) 2019 Dragonfly Tech").
Percorsi sotto: `B/` = `vendor/dragonfly/server/block/`, `P/` = `server/player/player.go`, `S/` = `server/session/`.

## 1. Commit e protocollo

| Voce | Valore |
|---|---|
| HEAD usato | `0f9a3a5736267db60bf46e92d408f81be1de9a64` (2026-11-07, "debug/shape.go: Add MaxRenderDistance") |
| Commit che porta a 1.26.60 | `51dd9c1d` "dragonfly: Support 1.26.60 (#1481)" (2026-10-28), tocca go.mod |
| go.mod:16 | `github.com/sandertv/gophertunnel v1.63.0` |
| gophertunnel v1.63.0 `minecraft/protocol/consts.go:5,7` | `CurrentProtocol = 2225`, `CurrentVersion = "1.26.60"` (letto via raw.githubusercontent) |
| Tag Dragonfly | l'ultimo tag e v0.9.9, vecchio: usare HEAD/`51dd9c1d` o successivi, NON i tag |
| Licenza | MIT |

## 2. Convenzioni base (da capire prima delle tabelle)

| Concetto | Definizione | Fonte |
|---|---|---|
| `cube.Face` | Down=0, Up=1, North=2, South=3, West=4, East=5 (stesso ordine di Bedrock `facing_direction`) | `cube/face.go:5-15` |
| `cube.Direction` | North=0, South=1, West=2, East=3. `Face()`=d+2. | `cube/direction.go:7-14` |
| Direzione giocatore `Rotation().Direction()` | yaw (0=sud, 90=ovest, -90=est, +-180=nord): `45<yaw<=135` West; `-45<yaw<=45` South; `-135<yaw<=-45` East; resto North | `cube/rotation.go:52-65` |
| `Rotation().Orientation()` | 16 settori: `round(yaw/22.5)` (se yaw<-11.25 si somma 360). `Opposite()` = +180 deg via `OrientationFromYaw` (puo dare 16: UNVERIFIED se fa wrap) | `cube/rotation.go:69-77`, `cube/orientation.go:10-23` |
| Posizione server | `pk.Position - (0,1.62,0)`: Dragonfly tiene i PIEDI, non gli occhi | `S/handler_player_auth_input.go:~58` |
| `horizontalDirection(d)` (legacy "direction" 0..3) | South->0, West->1, North->2, East->3 (cioe Bedrock 0=S,1=W,2=N,3=E) | `B/block.go:212-225` |
| `firstReplaceable` | se il blocco cliccato e sostituibile: pos = cliccato, face forzata a **Up** (!); altrimenti pos = `pos.Side(face)` se sostituibile; altrimenti fallisce | `B/block.go:186-197` |
| `replaceableWith` | fuori range Y -> false; blocco deve implementare `Replaceable` e `ReplaceableBy(with)` e non essere gia uguale (`b == with`); se c'e liquido nello stesso pos deve essere sostituibile a sua volta | `B/block.go:165-183` |

## 3. Tabella stati di orientamento

`Dir` = direzione del giocatore (`Rotation().Direction()`); `Opp` = `.Opposite()`; `face` = faccia cliccata; `clickY` = `clickPos[1]` (0..1 nel blocco).
Tutti i valori stringa/int sono quelli emessi da `EncodeBlock`.

| Stato | Dominio | Regola (input -> valore) | Blocchi Dragonfly | File:riga |
|---|---|---|---|---|
| `pillar_axis` | "x","y","z" | `Axis = face.Axis()`: Up/Down->y, East/West->x, North/South->z | log/stem, wood/hyphae, bamboo_block, basalt, bone_block, hay_block (+`deprecated`=0), froglight, chain (iron/copper), deepslate, muddy_mangrove_roots, purpur_pillar, quartz_pillar, infested_deepslate | `log.go:64`, `bamboo_block.go:43`, `bone.go:30`, `hay_bale.go:38`, `iron_chain.go:31`, `purpur.go:47`, `quartz.go:39`, `wood.go:58`. Eccezione: purpur_block/quartz_block/smooth/chiseled emettono sempre `"y"` (`purpur.go:38`, `quartz.go:92-99`) |
| `minecraft:cardinal_direction` (volto verso il giocatore) | "north","south","east","west" | `Facing = Dir.Opposite()`, stringa = `Facing.String()` | chest, ender_chest, furnace/blast_furnace/smoker (+ varianti `lit_*`), stonecutter_block, lectern, loom*, carved_pumpkin/pumpkin, lit_pumpkin, end_portal_frame, pink_petals, campfire, copper_golem_statue, shelf (shelf_mushroom: `face.Direction()`) | `chest.go:179`, `furnace.go:78`, `stonecutter.go:45`, `lectern.go:60`, `pumpkin.go:35`, `lit_pumpkin.go:29`, `end_portal_frame.go:50`, `pink_petals.go:54`, `campfire.go:165`, `copper_golem_statue.go:60`, `shelf_mushroom.go:81` (*loom usa `direction`, vedi sotto) |
| `minecraft:cardinal_direction` (variante anvil) | idem | `Facing = Dir.RotateLeft()` | anvil | `anvil.go:48`, enc `:90` |
| `minecraft:cardinal_direction` (porte) | idem | `Facing = Dir`; stringa emessa = `Facing.RotateRight().String()` (es. giocatore verso sud -> "west") | wooden_door, *_door, copper_door | `wood_door.go:74,124-126`, `copper_door.go:106,157` |
| `minecraft:cardinal_direction` (fence gate) | idem | `Facing = Dir` (stringa diretta). In `Activate`, se apri e il giocatore guarda `Facing.Opposite()`, `Facing` si inverte | fence_gate, *_fence_gate | `wood_fence_gate.go:57,82-85,111-113` |
| `facing_direction` (int 0..5) generico "verso sud/nord" via `face` | 0..5 | Down0 Up1 N2 S3 W4 E5. Regole per blocco: (a) **hopper**: vedi nota sotto (`Facing=Down; se face!=Down -> face.Opposite()`); (b) **end_rod**: `=face`, se dietro c'e end_rod con stesso facing -> `face.Opposite()`; enc: `Facing` o `Facing.Opposite()` (due rami) ; (c) **barrel**: `calculateFace(user,pos)` (Up/Down se giocatore e entro 1 blocco orizzontale e occhi > 2 sopra/sotto, altrimenti `Dir.Opposite().Face()`); (d) **ladder** (vedi sotto) | hopper, end_rod, barrel | `hopper.go:119-133,250`, `end_rod.go:21-35,65-67`, `barrel.go:121`, `block.go:129-147` |
| `facing_direction` (da `Dir.Opposite()`) | 2..5 | `int32(2 + Facing)` con `Facing=Dir.Opposite()` | glazed_terracotta (`facing_direction`=0 per unknown) | `glazed_terracotta.go:45,34-36` |
| `facing_direction` (muro attaccato) | 2..5 | = id della faccia cliccata (`face.Direction()+2`) | wall_sign, wall_banner, wall skull (head), ladder (`l.Facing = face.Direction()` -> enc `Facing+2`) | `sign.go:154,184`, `banner.go:59,83`, `skull.go:63,111`, `ladder.go:51,89` |
| `facing_direction` (item_frame) | 0..5 | `Facing = face.Opposite()`, enc `int32(Facing.Opposite())` = faccia cliccata | frame, glow_frame | `item_frame.go:80,111` |
| `facing_direction` (stem) | 0..5 | `Direction` verso la zucca/anguria vicina o `FaceDown`(0) se nessuna | melon_stem, pumpkin_stem | `melon_seeds.go:32,56,107` |
| `minecraft:facing_direction` (con prefisso, stringa) | "down".."west" | **NON usato da Dragonfly** (solo osservatore `minecraft:observer` in block_states.nbt). Dragonfly non ha observer. UNVERIFIED regola | observer (non implementato) | `world/block_states.nbt` |
| `minecraft:block_face` (stringa) | "down".."west" | **NON usato da Dragonfly** (solo amethyst bud/cluster, ice_crystal in block_states.nbt: blocchi non implementati). Regola ovvia: faccia cliccata = `face.String()` -> UNVERIFIED | amethyst_cluster, *_amethyst_bud, ice_crystal | `world/block_states.nbt` |
| `direction` (legacy 0..3) | 0..3 (domino nbt 0..15 solo per bed/trapdoor etc, ma D. usa 0..3) | `int32(horizontalDirection(Facing))`, con `Facing`: **bed** `Dir`; **loom, decorated_pot, grindstone** `Dir.Opposite()`; **cocoa** `face.Opposite().Direction()` | bed, loom, decorated_pot, grindstone, cocoa | `bed.go:80,235`, `loom.go:46,58`, `decorated_pot.go:153,139`, `grindstone.go:42,81`, `cocoa_bean.go:71,111` |
| `direction` (trapdoor) | 0..3 | `Facing = Dir.Opposite()`, valore = `abs(Facing - 3)` : N->3, S->2, W->1, E->0 (quindi giocatore sud=>Facing nord=>3) | trapdoor, *_trapdoor, copper trapdoor | `wood_trapdoor.go:51,99-101`, `copper_trapdoor.go:45,119` |
| `weirdo_direction` (scale) | 0..3 | `Facing = Dir`; valore `3 - Facing`: N->3, S->2, W->1, E->0 (sale verso dove guarda il giocatore) | *_stairs (97 tipi) | `stairs.go:35,139-146` |
| `upside_down_bit` (scale) | 0/1 | `face==Down || (clickY>0.5 && face!=Up)` | stairs | `stairs.go:36-38` |
| `upside_down_bit` (trapdoor) | 0/1 | `(clickY>0.5 && face!=Up) || face==Down` (identico), alias `Top` | trapdoors | `wood_trapdoor.go:52` |
| `minecraft:corner` (NOVITA 1.26.60 per scale) | "none","inner_left","inner_right","outer_left","outer_right" | calcolato dai vicini (`calculateCorner`), ricalcolato in `NeighbourUpdateTick` e `DeriveState` | stairs (stato nuovo, va incluso nel nostro enc!) | `stairs.go:40,46-56,140`, `stairs_corner.go:58-72` |
| `minecraft:vertical_half` | "bottom","top" | `Top = face==Down || (clickY>0.5 && face!=Up)`. Vale per slab e double slab (stesso stato) | *_slab, *_double_slab (202 blocchi) | `slab.go:53-55,160` |
| `top_slot_bit` | n/a | **non esiste in Dragonfly** ne in block_states.nbt (0 blocchi). In 1.26.60 non c'e piu: non serve. | - | verificato con parser su `block_states.nbt` |
| `torch_facing_direction` | "top","east","north","south","west","unknown" | `Facing = face.Opposite()` (verso il blocco di supporto); enc: Down->"top", `unknownFace`(=Up, non piazzabile)->"unknown", altrimenti `Facing.String()`. Click su Down vietato. Se il supporto cliccato non e solido, cerca tra [S,W,N,E,Down] il primo solido (face=quel lato.opposite) | torch, soul_torch, copper_torch, redstone_torch (+unlit), colored_torch_* (non impl.) | `torch.go:37-62,89-103`, `copper_torch.go:51,86`, `redstone_torch.go:69,271` |
| `lever_direction` | "north","south","east","west", "down_east_west","down_north_south","up_east_west","up_north_south" | `Facing = face` (cliccata). Se face e Up/Down: asse "east_west" se `Dir.Face().Axis()==X`, altrimenti "north_south"; enc = `face.String()` + `_`+asse per Up/Down. Richiede supporto solido su `pos.Side(face.Opposite())`. `open_bit`=Powered | lever | `lever.go:51-66,89-99` |
| `ground_sign_direction` | 0..15 | `StandingAttachment(Orientation().Opposite())` valore `int32(Attach.o)`; click su Down vietato; Up -> standing, lato -> wall (facing_direction) | *_standing_sign, standing_banner | `sign.go:145-157,184-186`, `banner.go:55,85` |
| `ground_sign_direction` (skull) | 0..15 | skull standing: `Orientation()` NON opposta. Enc: Up -> `facing_direction`=1, muro `facing_direction=face id`, Down? =0 | mob heads | `skull.go:61-63,105-113` |
| `vine_direction_bits` | bitmask 0..15 | per faccia cliccata `face` (no Y): attaccamento nel `face.Direction().Opposite()`; bit: 1=South, 2=West, 4=North, 8=East (`i=0..3` su [S,W,N,E]) | vine | `vine.go:108-125,271-277` |
| `hanging` (lanterne) | bool | `Hanging = (face == Down)` (dopo fallback, vedi 4) | lantern, soul_lantern, copper_lantern | `lantern.go:48-69,100-106` |
| `open_bit` | bool | toggle con `Activate` (vedi 5) | door, trapdoor, fence_gate, barrel (stato aperto da contenitore) | vari |
| rail directions | - | **Dragonfly NON ha binari**: nessun file rail*.go in `server/block` (verificato con grep/ls). Niente da portare. | - | - |

Nota: la riga hopper sopra e scritta male; regola esatta (`hopper.go:126-130`): `Facing=Down; if Facing != face { Facing = face.Opposite() }`. Quindi clic su Down => Down; clic su qualsiasi altra faccia (anche Up) => `face.Opposite()`.

## 4. Casi speciali

### 4.1 Porte (`door_placement.go`, `wood_door.go`, `copper_door.go`)

| Aspetto | Regola | File:riga |
|---|---|---|
| Posizione | `firstReplaceable`; deve essere `face==Up` (dopo remap), il blocco sopra deve essere sostituibile, e sotto deve avere faccia superiore solida (`FaceSolid(below, Up)`) | `door_placement.go:10-19` |
| Stati | `Facing=Dir`; enc `minecraft:cardinal_direction = Facing.RotateRight()`, `door_hinge_bit`=Right, `open_bit`, `upper_block_bit`=Top | `wood_door.go:74,124-126` |
| Cerniera (`door_hinge_bit`) | `left = pos+Facing.RotateLeft`, `right = pos+Facing.RotateRight`; `Right = (leftDoor && !rightDoor) || rightCount > leftCount`, con `count` = blocchi pieni opachi (full cube, `LightDiffusion==15`) a quel lato su entrambe le meta (pos e pos+Up), `door` = se c'e una porta (Model Door) in uno dei due. | `door_placement.go:22-52` |
| Mezza superiore | `Top:true, Right:d.Right, Facing:d.Facing`, stessa `Open=false`; `ctx.IgnoreBBox=true` (la porta ignora bbox del giocatore); `CountSub=1` | `wood_door.go:76-81` |
| Rottura/vicini | meta alta: se sotto non e porta -> rompe senza drop; meta bassa: se sotto non solido -> rompe con drop; se sopra non e porta -> rompe senza drop | `wood_door.go:55-64` |
| Toggle | `Activate`: inverte `Open` e imposta lo stesso `Open` sull'altra meta (`pos.Side(Face(boolByte(!Top)))`: sotto se Top, sopra se bassa); suono. NON c'e check redstone/iron: **iron_door non esiste in Dragonfly** | `wood_door.go:85-100`, `copper_door.go:116-125` |
| Copper door | stessa logica, apribile a mano | `copper_door.go:100-125` |

### 4.2 Letti (`bed.go`)

| Aspetto | Regola | File:riga |
|---|---|---|
| Placement | `firstReplaceable`; `Facing = Dir`; il piede e alla pos cliccata, la testa a `pos.Side(Facing.Face())` (davanti al giocatore); entrambe devono essere sostituibili e avere supporto solido sotto (`FaceSolid(below,Up)`); piazza prima la testa poi il piede, `IgnoreBBox=true` | `bed.go:72-95,336-339` |
| Stati | `direction=horizontalDirection(Facing)`, `head_piece_bit`=Head, `occupied_bit`=Occupied (di default false); colore = item meta (`EncodeItem` -> `minecraft:bed`, meta=colore) | `bed.go:231-238` |
| Rottura | `NeighbourUpdateTick`: se l'altra meta manca -> `breakBlockNoDrops` | `bed.go:~221-226` |
| Nota | bed in 1.26.60 e un solo nome `minecraft:bed` con meta colore dalla block entity NBT (`EncodeNBT` "Bed" + color) -> serve block entity | `bed.go:241-248` |
| straw_bed | nuovo letto dragonfly con `minecraft:cardinal_direction`, non vanilla (custom) - ignorare | `straw_bed.go` |

### 4.3 Slab -> doppia slab (`slab.go:25-58`, `slab_type.go:11-16`)

1. Click su uno slab *mezzo* dello stesso tipo, sulla faccia Up se `!Top` oppure Down se `Top`: diventa `Double=true` (stesso pos).
2. Se il blocco cliccato non e sostituibile e il vicino `pos.Side(face)` e uno slab mezzo dello stesso tipo: diventa doppio *nel vicino*.
3. Altrimenti `firstReplaceable` e `Top` come tabella.
4. **Nome del doppio**: `minecraft:X_slab` -> `minecraft:X_double_slab` (stesso `minecraft:vertical_half`); copper: `cut_copper_slab` -> `double_cut_copper_slab` (`slab_type.go:46-52`). Confronto "stesso tipo" via `EncodeItem()` (nome item + meta).

### 4.4 Torce / lanterne / pulsanti / leve: attacco e supporto

| Blocco | Posizione consentita | Check supporto (vicini) | File:riga |
|---|---|---|---|
| Torch | clic non-Down. Se supporto `pos.Side(face.Opposite())` non e `FaceSolid(face)`, prova [S,W,N,E,Down] e prende il primo con faccia solida | `NeighbourUpdateTick`: `pos.Side(Facing)` deve avere `FaceSolid(Facing.Opposite())` altrimenti `breakBlock` (con drop) | `torch.go:37-62,67-72` |
| Lantern | faccia Down: appeso, se sopra non e IronChain ne solido-in-giu -> forza face=Up. Se non-Down: sotto deve avere faccia Up solida (altrimenti false) | Tick: hanging -> sopra IronChain o solido; altrimenti sotto solido | `lantern.go:30-69` |
| Lever | `firstReplaceable`; supporto `pos.Side(face.Opposite())` con `FaceSolid(face)`; qualsiasi face (anche Up/Down) | Tick: stesso check, altrimenti `breakBlock` | `lever.go:45-66` |
| Button | **Dragonfly NON ha bottoni** (nessun file; `button_pressed_bit` presente solo in block_states.nbt, 15 blocchi). Reset ticks per materiale: NON disponibile -> UNVERIFIED, usare valori vanilla (legno 30t, pietra 20t) da altra fonte | - | verificato con grep |
| Pressure plate / snow_layer / iron door / rails | **assenti** in Dragonfly | - | grep |
| Ladder | non Up/Down; se il dietro e `LightDiffuser` (non pieno), cerca [S,N,E,W] con `LightDiffusionLevel()==15` o non-diffuser; `Facing=face.Direction()` | (tick non letto) UNVERIFIED | `ladder.go:30-52` |
| Vine | non Y; `canSpreadTo(pos)` (cubo pieno) richiesto; non sovrascrive altre `Vines` esistenti; `WithAttachment(face.Direction().Opposite())`. Tick: toglie lato se non supportato e sopra non e vine con lo stesso lato | `vine.go:108-139` |
| Sign | non Down; Up -> standing (Orientation opp); lato -> wall | `sign.go:145-157` |
| Cocoa | attacca a jungle log/wood sul lato opposto del click | `cocoa_bean.go:53-75` |

### 4.5 Muri (`wall.go`)

Stati: `wall_connection_type_{north,east,south,west}` = "none"/"short"/"tall" (`wall_connection_type.go:36-46`), `wall_post_bit` (`wall.go:~50-58`).

`calculateConnections` (`wall.go:129-185`), per ogni lato (faccia orizzontale):
- connesso se il vicino ha `FaceSolid(sidePos, face.Opposite())`, oppure e un `Wall`, oppure e un `WoodFenceGate` con `gate.Facing.Face().Axis() != face.Axis()`, oppure il suo model e `model.Thin` (vetro/barre).
- se connesso: "short"; diventa "tall" se il blocco sopra ha una bbox che parte da y=0 e si sovrappone al braccio (per North: `xOverlap && bb.Max.Z>0.75`; East: `bb.Min.X<0.25 && zOverlap`; South: `xOverlap && bb.Min.Z<0.25`; West: `bb.Max.X>0.75 && zOverlap`, con `xOverlap = bb.Min.X<0.75 && bb.Max.X>0.25`, idem Z).

`calculatePost` (`wall.go:187-243`): `connections` = n. lati != none. Il blocco sopra decide `post`:
- Lantern: `!Hanging`; Sign: `!Attach.hanging` (ground sign); Torch: `Facing==Down` (a terra); WoodTrapdoor aperto: post se il lato `above.Facing` e connesso; Wall sopra: `above.Post`.
- Se `!post`: connections<2 -> post; N+S connessi -> post se E o W connessi; E+W connessi -> post se N o S connessi; altrimenti (angolo) -> post=true.
- Wall ricalcolato in `UseOnBlock` e `NeighbourUpdateTick` (anche per vicini).

### 4.6 Piante e suolo

| Aspetto | Regola | File:riga |
|---|---|---|
| Check | `supportsVegetation(veg, below)` = `below` implementa `Soil` e `SoilFor(veg)` | `dirt.go:97-106` |
| Dirt | erba/felce/doppia erba/arbusto/dead bush solo se `!Coarse`; fiori, doppi fiori, nether sprouts, pink petals, sugar cane, bamboo(+sapling), sapling sempre | `dirt.go:21-29` |
| Grass | tutto quanto sopra (short_grass, fern, double_tall_grass, flower, double_flower, nether_sprouts, pink_petals, sugar_cane, dead_bush, bamboo, sapling, red_shrub) | `grass.go:40-46` |
| Farmland | erbe/fiori/sapling/dead_bush/ecc **ma NON** sugar_cane/bamboo; crop (grano ecc.) richiede `Farmland` direttamente sotto | `farmland.go:22-28`, `crop.go:27`, `wheat_seeds.go:40` |
| Mud, muddy_mangrove_roots, podzol, mycelium | quasi tutte le erbe/fiori (liste in file) | `mud.go:11`, `muddy_mangrove_roots.go:24`, `podzol.go:12`, `mycelium.go:16` |
| Sand | cactus, dead_bush, sugar_cane, bamboo(+sapling) | `sand.go:20-25` |
| Gravel | solo bamboo(+sapling) | `gravel.go:19` |
| Soul sand/netherrack | solo wither_rose; soul_soil: nether_sprouts | `soul_sand.go:16`, `netherrack.go:15`, `soul_soil.go:11` |
| Terracotta (anche stained) | solo dead_bush | `terracotta.go:13`, `stained_terracotta.go:19` |

### 4.7 Blocchi sostituibili (`Replaceable`)

Embeddano `replaceable` (sempre sostituibili): Air, DeadBush, DoubleTallGrass, Fern, Fire, Light, NetherSprouts, RedShrub, ShortGrass, Vines (`block.go:245-251` e file rispettivi: `air.go deadbush.go double_tall_grass.go fern.go fire.go light.go nether_sprouts.go red_shrub.go short_grass.go vine.go`).
Acqua e Lava (`water.go:28-36`, `lava.go:37-45`): sostituibili da tutti i blocchi eccetto quelli `LiquidRemovable` che non sono `LiquidDisplacer`/`Liquid`. In pratica un blocco normale sostituisce l'acqua, ma piante `LiquidRemovable` (erbe marine, fiori ecc.) no.
**Snow layer non esiste** (solo `Snow` = blocco di neve intero, `snow.go`). Slab/carpet/ecc. NON sono `Replaceable`.
`firstReplaceable`: cliccando su un blocco sostituibile si piazza SU quella posizione (come se la faccia fosse Up).

### 4.8 Altri piazzamenti utili

| Blocco | Regola | File:riga |
|---|---|---|
| Chest | `Facing=Dir.Opposite()`; cerca accoppiamento su `Facing.RotateLeft()` poi `RotateRight()` (vicino chest compatibile) -> doppia | `chest.go:172-191` |
| Pumpkin/lit_pumpkin/carved | `Facing=Dir.Opposite()` | `pumpkin.go:30-36` |
| Glazed terracotta | `Dir.Opposite()` -> `facing_direction=2+dir` | `glazed_terracotta.go:40-46` |
| Grindstone | Dir.Opposite; clic su Up/Down -> attacco floor/hanging, lato -> wall con `face.Direction()` | `grindstone.go:42-47` |
| Shulker | `Facing=face` | `shulker_box.go:136` |
| Skull/banner | vedi tabella 3 | `skull.go:54-63`, `banner.go:48-59` |

## 5. Interazioni (click destro su blocco)

Ordine in `UseItemOnBlock` (`P/player.go:1775-1835`): (1) air o fuori portata -> resend e stop; (2) evento; (3) se blocco `Activatable` e (`!Sneaking || mano vuota`) -> `Activate`, se true consuma e stop; (4) item `UsableOnBlock` (anche i blocchi che implementano `UseOnBlock`) altrimenti (5) item = `world.Block` generico: `replacedPos = pos` se `ReplaceableBy`, sennò `pos.Side(face)`; se ancora non sostituibile o fuori range -> return; `DeriveState`; `placeBlock`; in creativo non toglie item.

| Blocco | Interazione | File:riga |
|---|---|---|
| Door legno / copper | toggle `open_bit` su entrambe le meta | `wood_door.go:85`, `copper_door.go:116` |
| Trapdoor legno/copper | toggle `open_bit` | `wood_trapdoor.go:58-68`, `copper_trapdoor.go:85-91` |
| Fence gate | toggle; in apertura inverte `Facing` se il giocatore e dal lato opposto | `wood_fence_gate.go:81-95` |
| Lever | toggle `open_bit`(=Powered) + suono | `lever.go:71-80` |
| Iron door/iron trapdoor/button | **non implementati** (non c'e blocco); le iron non si aprono a mano per regola vanilla, ma Dragonfly non le ha | - |
| Fence gate in wall | `Lowered` (`in_wall_bit`) = muro a sinistra o destra del gate | `wood_fence_gate.go:73-79` |

## 6. Costanti di validazione

| Voce | Valore | File:riga |
|---|---|---|
| Portata (rottura, piazzamento, uso, pick) | `canReach(pos)`: `dist(EyePosition, centro_blocco) <= 8.0`; in creativo `<= 14.0`; richiede non morto e `AllowsInteraction()`. Per piazzare/rompere serve anche `AllowsEditing()` | `P/player.go:3343-3348`, `placeBlock` 2111, `BreakBlock` 2177... (via `canReach` righe 1778, 1968, 2111, 2177, 2256) |
| Occhi | 1.62 in piedi, 1.27 sneaking, 0.4 nuotando/strisciando/planando | `P/player.go:3100-3109` |
| Intersezione col giocatore (`obstructedPos`) | bbox del blocco da piazzare (`b.Model().BBox`) traslata; entita entro box +-3 attorno; ignora Item/Arrow/XPOrb e spettatori; bbox entita ridotta di 1e-4 (`Grow(-1e-4)`); se ostruisce solo il giocatore stesso -> rifiuta senza resend (`selfOnly`), se ostruisce un altro -> rifiuta e **resend** del blocco + 6 vicini | `P/player.go:2118-2125,2149-2181` |
| `IgnoreBBox` | porte, letti | `wood_door.go:78`, `bed.go:88` |
| Resend su rifiuto | `UpdateBlock` (layer 0, flags `BlockUpdateNetwork`; layer 1 se liquido) per pos + (opz.) vicini; solo se pos entro chunk radius del giocatore (anti-DoS) | `P/player.go:3549-3576`, `S/world.go:1005-1022` |
| Solo sicurezza sul raggio chunk | `withChunkRadius` prima di resend | `P/player.go:3565` |

## 7. Rottura blocchi (autorita server) e PlayerAuthInput

| Azione (`protocol.PlayerAction*`) | Gestione | File:riga |
|---|---|---|
| `StartBreak`, `ContinueDestroyBlock` | `StartBreaking(pos, face)`: aborta il precedente; se Air/fuori portata ritorna; se sopra c'e fuoco lo spegne; sword in creativo non rompe; chiama handler, `Punchable.Punch`; `breaking=true`; in survival invia `StartCrackAction{BreakTime}` ai viewer (LevelEvent crack) | `S/handler_player_action.go:~29-34`, `P/player.go:1966-2012` |
| `AbortBreak` | `AbortBreaking` -> `StopCrackAction` | `P/player.go:2058-2066` |
| `PredictDestroyBlock`, `StopBreak` | `FinishBreaking()`: se `!breaking` rimanda blocco; altrimenti `BreakBlock(breakingPos)` **senza verificare che il tempo di rottura sia trascorso** (fiducia nel client) | `P/player.go:2046-2053` |
| `CrackBreak`, `CreativePlayerDestroyBlock`, `InteractWithBlock`, `StartItemUseOn` | ignorate | `S/handler_player_action.go` |
| `MissedSwing` | `PunchAir` | idem |
| PlayerAuthInput `InputFlagPerformBlockActions` | itera `BlockActions` -> stesso `handlePlayerAction`, `entityRuntimeID = selfEntityRuntimeID` | `S/handler_player_auth_input.go:~160-166` |
| PlayerAuthInput `InputFlagPerformItemInteraction` + `UseItemTransactionData` | confronta item in mano col client (mismatch -> ignora); **solo** `UseItemActionBreakBlock` -> `BreakBlock(pos)`; ogni altro `ActionType` -> errore (kick) | `S/handler_player_auth_input.go:~137-153` |
| InventoryTransaction `UseItemTransactionData` | `VerifyAndSetHeldSlot`, reinvia inventario, poi `BreakBlock` / `UseItemOnBlock(pos, face, clickPos)` / `UseItem` | `S/handler_inventory_transaction.go:~170-205` |
| `InputFlagPerformItemStackRequest` | passa a `ItemStackRequestHandler.handleRequest` (stesso flusso, include `MineBlock`) | `S/handler_player_auth_input.go:~86-97` |

`BreakBlock(pos)` (`P/player.go:2170-2230`): Air -> stop; fuori portata o `!AllowsEditing` -> resend; non `Breakable` e non creativo -> resend; calcola drops (nessuno in creativo), xp, evento, poi `SetBlock(pos, nil)`, particella BlockBreak, `BreakHandler`, spawn drop come entita item (velocita random +-0.1, y 0.2), `Exhaust(0.005)`, durabilita utensile solo se hardness>0.

**Rottura istantanea creativo**: in creativo `StartBreaking` NON invia crack e ritorna subito dopo `breaking=true` (`P/player.go:2005-2007`); la rottura avviene al `PredictDestroyBlock/StopBreak` oppure via `UseItemActionBreakBlock`. Non c'e un check temporale server-side. In survival il tempo atteso e solo usato per l'animazione (`BreakDuration`, `block/break_info.go:51-115`; `Hardness<=0` -> 0 = istantaneo `BreaksInstantly:116`). `ContinueBreaking` e chiamato dal tick (`P/player.go:2724`), emette particelle/suono ogni 5 tick, aggiorna crack se cambia il tempo.

## 8. Item -> blocco e inventario creativo

### 8.1 Item -> blocco
- Un blocco che e anche item implementa `world.Item` (`EncodeItem() (name, meta)`) ed e registrato in `world.RegisterItem` (`block/register.go:269-...`). Non c'e una mappa item->blocco separata: **l'item E il blocco** (`item.Stack.Item() world.Block`).
- Sul filo il `ItemStack` porta `NetworkID` (runtime id item da `vanilla_items.nbt`), `MetadataValue` e **`BlockRuntimeID`** del blocco/stato di default dell'item (`S/player.go:1019-1040`). In ingresso (`stackToItem`, `S/player.go:1043-1063`) se `BlockRuntimeID > 0` il blocco viene ricostruito da `BlockByRuntimeID` (quindi lo **stato cliccato nell'item determina il blocco di partenza**: es. variante di legno/colore), poi `UseOnBlock` sovrascrive lo stato di orientamento.
- Selezione del percorso di piazzamento: se il blocco implementa `UseOnBlock` (vedi lista sopra) usa quella (`item.UsableOnBlock`), altrimenti il ramo generico `case world.Block` in `UseItemOnBlock` (`P/player.go:1816-1834`).
- `ItemByName(name, meta)` ripiega su meta=0 (`world/item.go:93-100`).

### 8.2 Creativo e ItemStackRequest

| Voce | Comportamento | File:riga |
|---|---|---|
| Contenuto | `CreativeContent{Groups, Items}`; `CreativeItemNetworkID = index+1` (indice 1-based nell'elenco `creative.Items()`); icone dei gruppi e item con `Damage` rimosso (`deleteDamage`) | `S/player.go:1113-1145`, `S/session.go:227-228` |
| Dati creativi | `server/item/creative/creative_items.nbt` (file embedded) | `item/creative/` |
| `CraftCreativeStackRequestAction` | solo in gamemode con `CreativeInventory()`; `index = CreativeItemNetworkID-1`; stack = item creativo con **conteggio = MaxCount**; passa a `createResults` -> `handleCreate` che lo scrive nello slot `ContainerCreatedOutput` (slot `craftingResult`) | `S/handler_crafting.go:154-165`, `S/handler_item_stack_request.go:~286-316` |
| `DestroyStackRequestAction` | solo creativo: cancella `Count` dallo slot sorgente (cestino) | `S/handler_item_stack_request.go:~219-235` |
| `Take` / `Place` | `handleTransfer`: verifica slot, `Comparable`, count e MaxCount; setta slot da/a | `~158-196` |
| `Swap` | scambia due slot | `~198-215` |
| `Drop` | `c.Drop(...)`, setta slot | `~237-253` |
| `MineBlock` | aggiorna slot hotbar per correzione durabilita (risposta con `StackNetworkID`) | `~256-271` |
| `Create` | risultati multipli craft | `~272-296` |
| Altre (non creativo) | `CraftRecipe`/Auto/Optional, Loom, Grindstone, Smithing, Stonecutter, Enchant, Beacon -> non necessarie in tappa 1; `Consume`/`CraftResultsDeprecated` ignorate; `CraftReserved` errore | `~72-118` |
| Risposta (OK) | `ItemStackResponse{Status OK, RequestID, ContainerInfo[{Container, SlotInfo[{Slot, HotbarSlot, Count, StackNetworkID, DurabilityCorrection}]}]}` per ogni slot cambiato | `~417-437` (`resolve`) |
| Risposta (errore) | `Status Error` solo `RequestID`; poi ripristina server-side gli slot gia cambiati (`before`) | `~440-458` (`reject`) |
| StackNetworkID | contatore atomico globale `stackID` (`atomic.AddInt32`), assegnato a ogni `item.Stack` creato (`NewStack`, e `Grow`/split genera nuovi id); `id()` = 0 per stack vuoto; collegato al `session` via linkname `item_id` | `item/stack.go:71,401,470-487`, `S/player.go:1387-1390` |
| Verifica client | il client invia `StackNetworkID` per ogni slot: se >=0 deve combaciare con quello server (altrimenti "stack ID mismatch" -> reject); se **negativo** e un riferimento a una richiesta precedente: si risolve con `responseChanges[reqID][inv][slot]` | `~326-370` |
| Pulizia | le entry `responseChanges` si cancellano quando il client conferma lo slot con id >=0, o dopo 5s; limite 256 modifiche non confermate (errore) | `~371-405` |
| Rete | la richiesta viaggia anche dentro `PlayerAuthInput` (`InputFlagPerformItemStackRequest`) | `S/handler_player_auth_input.go:~86-97` |

## 9. Sorprese e rischi (top)

1. **Mancano in Dragonfly**: bottoni, pressure plates, rails, iron_door/iron_trapdoor, snow_layer, observer, amethyst, ice_crystal (stati in `block_states.nbt` ma nessuna logica). Per questi la fonte e vanilla/altra: i valori "reset ticks per materiale" restano UNVERIFIED.
2. **Convenzioni per blocco diverse**: porta = `cardinal_direction` con `Dir.RotateRight()`; trapdoor = `direction = abs(Facing.Opposite - 3)`; bed = `direction = horizontalDirection(Dir)`; loom/pot/grindstone = `Dir.Opposite()`; scale = `3 - Dir`; anvil = `Dir.RotateLeft()`; chest/furnace = `Dir.Opposite()`. Non esiste una regola unica generica.
3. **Stati nuovi 1.26.60**: scale hanno `minecraft:corner` (5 valori); porte/fence gate/chest/furnace ecc. usano `minecraft:cardinal_direction` (stringhe), `top_slot_bit` non esiste piu. Il nostro hash FNV-1a deve includere esattamente questi stati.
4. **Portata**: 8.0 blocchi survival, 14.0 creativo, misurata dall'occhio al CENTRO del blocco (non al punto cliccato); nessun check sul punto cliccato ne su raycast/occlusione. Nessun controllo del tempo di rottura (fiducia al client su `StopBreak`/`PredictDestroyBlock`).
5. **Piazzamento item in creativo**: l'item porta `BlockRuntimeID`, che seleziona lo stato di partenza; il ramo "blocco generico" scarta item se fuori range/non sostituibile; blocchi con `UseOnBlock` hanno la loro logica (DeriveState solo nel ramo generico). Il creativo usa `CraftCreativeStackRequestAction` con stack pieno (`MaxCount`) e id 1-based.
6. Posizione server = piedi (`pos - 1.62`); `Orientation()` (segni e wrap a 16) e `Opposite()` richiedono attenzione ai bordi (UNVERIFIED: wrap di 16 -> 0).
7. `firstReplaceable` forza `face = Up` quando si clicca un blocco sostituibile (es. erba alta): impatta torce/leve/lanterne/porte.
8. `UseItemTransactionData` da PlayerAuthInput accetta solo `BreakBlock`; il piazzamento passa da `InventoryTransaction` (click block/air).

## 10. UNVERIFIED
- Wrap di `Orientation().Opposite()` a 16.
- Tick di supporto di ladder, ground_sign, cocoa (non letti).
- Valori di reset dei bottoni, iron doors/trapdoors: assenti in Dragonfly.
- `minecraft:block_face`, `minecraft:facing_direction` (stringa): nessuna implementazione Dragonfly; regola dedotta.
- Riferimenti "~" a righe: stimati (file lunghi) ma entro +-5 righe.
