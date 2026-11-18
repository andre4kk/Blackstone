# Protocollo tappa 1: layout dei pacchetti per input, inventario creativo, blocchi

Riferimento per decoder/encoder a mano. Protocollo 2225 (client 1.26.6x).

## 0. Fonti e versione

| Fonte | Versione usata |
|---|---|
| Mojang `bedrock-protocol-docs` (clonato in `vendor/bedrock-protocol-docs`, ignorato da git, EULA: non copiare nulla di tracciato) | tag **v1.26.65**, commit `b0c990c8536910755aa975baae673c7bc6130f89` (checkout detached). Il branch `main` e' piu' nuovo (1.26.70-preview.26): NON usarlo. I tag v1.26.61/62/65 hanno tutti `x-protocol-version: 2225`; gli schemi dei pacchetti qui trattati sono identici tra v1.26.61 e v1.26.65 (git diff vuoto). I JSON dicono ancora `x-minecraft-version: 1.26.60-beta.0`. |
| gophertunnel (MIT) | HEAD `e2251a60` (2026-11-12), `CurrentProtocol = 2225`, `CurrentVersion = "1.26.60"`. Storia utile: `b36ddad` (1.26.40: PAI input flags a lista, ItemStackRequest a varianti), `7a556a0` (1.26.50: presenza opzionali a 1 byte invece di 2). |
| Mojang `additional_docs/BlockBreakingOverview.md`, `ServerAuthInventory/*` | flusso block breaking (parafrasato qui). |
| PMMP BedrockProtocol (solo 1.26.30, per cross-check storico) e Dragonfly (usa gophertunnel 1.63) | solo per slot 50 e per i pacchetti che il server manda. |

Percorsi schemi: `vendor/bedrock-protocol-docs/json/<Nome>PacketPayload.json` (+ tipi referenziati). gophertunnel: `minecraft/protocol/packet/*.go`, `minecraft/protocol/*.go`.

## 1. Regole di codifica (come le legge `tools/protocolgen.py`)

- Interi: fissi little-endian; con `Compression` varint (con segno = zigzag). La `Compression` su 8 bit non cambia nulla. `Enum-as-Value` non cambia la codifica, solo il significato.
- Proprieta' NON in `required` = **opzionali**: 1 byte di presenza (0/1) + valore. Vale per Mojang e gophertunnel (`OptionalFunc` = bool + valore).
- Array: contatore varuint32 (u32 fisso solo con `No size compression`). Stringa: lunghezza varuint32 + byte UTF-8. Float: f32 LE. Bool: 1 byte.
- `oneOf` con `x-control-value-type: uint32`: prima un varuint32 di selezione, poi il corpo.
- `BlockPos` Mojang = 3 interi `int32` compressi, cioe' **3 varint zigzag** (anche Y). Confermato dalla cattura (sez. 3).

Bug noti del generatore `protocolgen.py` (verificati lanciandolo su copia scratch, senza toccare il repo):

1. **Enum inline stringa** (`"type":"string","enum":[...], x-underlying-type: uint8, x-enum-binary-value`) viene trattato come stringa. Effetto reale: `CreativeGroupInfoPayload.creative_category` in `crates/protocol/src/generated.rs` e' scritto come stringa, ma sul filo e' **u8** (gophertunnel `r.Uint8(&x.Category)`; valori Mojang Construction=1, Nature=2, Equipment=3, Items=4, ItemCommandOnly=5; gophertunnel aggiunge All=0, Undefined=6). Stesso difetto per `ItemStackResponseInfo.result` (deve essere u8).
2. `NetworkItemStackDescriptor` fa crashare il generatore (`KeyError: 'x-underlying-type'` su `ItemStackNetIdVariant`, tipo integer senza tipo sottostante). InventoryContent, InventorySlot, MobEquipment non si generano finche' non si gestisce.
3. `CreativeContent` generato ha item = `NetworkItemInstanceDescriptorData` (id varint32, count u16, aux varuint32, block_runtime_id varint32 zigzag, user data stringa-byte): coincide con `r.Item(ItemStack)` di gophertunnel. Il campo `user_data_buffer: String` e' scritto come varuint len + byte: per item non-aria vedere sez. 2.3.

## 2. Tipi comuni

### 2.1 Enum utili

| Enum | Valori |
|---|---|
| `PlayerActionType` (int32 varint) | Unknown -1, StartDestroyBlock 0, AbortDestroyBlock 1, StopDestroyBlock 2, StartSleeping 5, StopSleeping 6, Respawn 7, StartJump 8, StartSprinting 9, StopSprinting 10, StartSneaking 11, StopSneaking 12, **CreativeDestroyBlock 13**, ChangeDimensionAck 14, StartGliding 15, StopGliding 16, DenyDestroyBlock 17, **CrackBlock 18**, StartSwimming 21, StopSwimming 22, StartSpinAttack 23, StopSpinAttack 24, **PredictDestroyBlock 26**, **ContinueDestroyBlock 27**, StartItemUseOn 28, StopItemUseOn 29, HandledTeleport 30, MissedSwing 31, StartCrawling 32, StopCrawling 33, StartFlying 34, StopFlying 35, StartUsingItem 37, InternalUpdate 38 |
| `ItemUseActionType` (varint32) | Place/ClickBlock 0, Use/ClickAir 1, Destroy/BreakBlock 2, UseAsAttack 3 |
| `ItemUseTriggerType` (u8) | Unknown 0, PlayerInput 1, SimulationTick 2 |
| `HandSlot` (u8) | Mainhand 0, Offhand 1 |
| `ItemUsePredictedResult` (u8) | Failure 0, Success 1; `ItemUseClientCooldownState` (u8) Off 0, On 1 |
| Transaction type (varuint32) | Normal 0, Mismatch 1, ItemUse 2, ItemUseOnActor 3, ItemRelease 4 |
| Facce | 0 giu', 1 su, 2 nord (-Z), 3 sud (+Z), 4 ovest (-X), 5 est (+X) (convenzione Bedrock; non dichiarata nello schema: UNVERIFIED nello schema, ma coerente con la cattura: face 2 su blocco z=-2 con giocatore a z=-3.78) |
| `GameType` (varint32) | Survival 0, Creative 1, Adventure 2, Default 5, Spectator 6 |
| `ContainerEnumName` (u8) | ArmorContainer 6, CombinedHotbarAndInventory 12, **HotbarContainer 28**, **InventoryContainer 29**, OffhandContainer 34, **CursorContainer 59**, **CreatedOutputContainer 60**, CraftingInput 13, CraftingOutputPreview 14, DynamicContainer 63 (lista completa 0-70 nello schema `ContainerEnumName.json`) |
| Window id legacy (gophertunnel) | Inventory 0, OffHand 119, Armour 120, UI 124 |

### 2.2 FullContainerName
`container_name` u8 (ContainerEnumName) + **opzionale** `dynamic_id` (presenza + u32 fisso LE). Fonte: `FullContainerName.json`, gophertunnel `container.go`.

### 2.3 NetworkItemStackDescriptor (ItemInstance) e NetworkItemInstanceDescriptor

Due forme diverse (non confonderle):

**NetworkItemStackDescriptor** (InventoryContent/Slot, MobEquipment, InventoryAction, UseItem.Item). Schema `cerealizer_NetworkItemStackDescriptor___SerializedData.json`, gophertunnel `Reader.ItemInstance`:

| # | Campo | Codifica |
|---|---|---|
| 0 | id | **i16 fisso LE** (0 = aria/non valido) |
| 1 | count | u16 fisso LE |
| 2 | aux (metadata) | varuint32 |
| 3 | net id variant | opzionale: presenza (bool) + varint32 zigzag (server net id > 0; vedi rischio 1 in sez. 9) |
| 4 | block runtime id | varuint32 (0 se non e' un blocco; per i blocchi e' l'hash FNV, `block_network_ids_are_hashes = true`) |
| 5 | user data | varuint32 len + byte |

L'**aria** sono quindi **8 byte a zero** (2+2+1+1+1+1), non 1 byte come nei protocolli vecchi: verificato dalla cattura. User data per item non-aria (gophertunnel `itemUserData`): se non c'e' NBT i byte sono `i16 0` + `u32 0` (can place on) + `u32 0` (can break) = 10 byte, preceduti da len=10; con NBT `i16 -1`, `u8 1`, NBT LE, poi liste. Lettura gophertunnel accetta anche len=0. UNVERIFIED se il client vanilla accetta len=0 per item non-aria (il nostro `CreativeContent::default()` e' vuoto, non lo mette alla prova).

**NetworkItemInstanceDescriptor** (CreativeContent, icone gruppi): id **varint32** zigzag, count u16, aux varuint32, block runtime id **varint32 zigzag**, user data (len + byte). Niente net id variant. Gia' generato correttamente (`NetworkItemInstanceDescriptorData`).

## 3. PlayerAuthInput (client -> server, id 144)

Fonti: `PlayerAuthInputPacketPayload.json`, `PackedItemUseLegacyInventoryTransaction.json`, `PlayerBlockActionData.json`, gophertunnel `packet/player_auth_input.go`, `protocol/input_flags.go`. Nota Mojang `developer_notes/player_auth_input_cereal_performance.md`: formato cambiato (marcatori di presenza ridondanti rimossi da Input Data, Item Use Transaction, Item Stack Request, Block Actions, Vehicle Rotation, Client Predicted Vehicle).

Ordine dei campi (concorda tra Mojang e gophertunnel, tranne dove indicato):

| # | Campo | Codifica | Note |
|---|---|---|---|
| 1 | pitch | f32 | rotazione giocatore (Vec2: x=pitch, y=yaw) |
| 2 | yaw | f32 | |
| 3 | position | 3 x f32 | posizione OCCHI (gia' usata dal decoder attuale) |
| 4 | move vector | 2 x f32 | |
| 5 | head yaw | f32 | |
| 6 | **input data** | **varuint32 N (max 66) + N x varint32 zigzag = indici dei flag attivi** | NON un bitset dal 1.26.40: lista degli id. Id fuori range 0..65 o duplicati = non valido. |
| 7 | input mode | varuint32 | 0 undefined, 1 mouse, 2 touch, 3 gamepad (4 motion controller in gophertunnel; Count 5) |
| 8 | play mode | varuint32 | 0 Normal, 1 Teaser, 2 Screen, 7 ExitLevel (gophertunnel: 3 Viewer, 4 Reality, 5 Placement, 6 LivingRoom, 8 ExitLevelLivingRoom) |
| 9 | new interaction model | varint32 zigzag | 0 Touch, 1 Crosshair, 2 Classic |
| 10 | interact rotation | 2 x f32 | |
| 11 | client tick | varuint64 | |
| 12 | pos delta | 3 x f32 | velocita' prevista |
| 13 | item use transaction | **opz.**: presenza + struttura (sotto) | atteso se flag PerformItemInteraction (34) |
| 14 | item stack request | **opz.**: presenza + ItemStackRequest (sez. 5) | atteso se flag PerformItemStackRequest (36) |
| 15 | player block actions | **opz.**: presenza + varuint32 count (max 100) + azioni | atteso se flag PerformBlockActions (35) |
| 16 | vehicle rotation | **opz.**: presenza + 2 x f32 | solo se IsInClientPredictedVehicle (45) |
| 17 | client predicted vehicle | **opz.**: presenza + varint64 zigzag (ActorUniqueID) | idem |
| 18 | analog move vector | 2 x f32 | |
| 19 | camera orientation | 3 x f32 | |
| 20 | raw move vector | 2 x f32 | |

La presenza e' **1 byte** (gophertunnel: `OptionalFunc`; fino a 1.26.40 erano 2 byte, `DoubleOptionalFunc`). Il nostro decoder legge solo i primi 5 f32: va esteso per arrivare ai campi 6+ (i campi 4-5 sono 3 f32 = 12 byte prima della lista).

**Item use transaction dentro PAI** (`PackedItemUseLegacyInventoryTransaction`): `legacy_request_id` varint32 zigzag; `legacy_set_item_slots` opzionale (presenza + varuint32 count + [container u8, slot-array: varuint32 count + u8]); poi i campi di UseItem (sez. 4.2 da `actions` in poi, SENZA il tag del tipo transazione e SENZA il bool hasLegacy del pacchetto autonomo). Nota: nel pacchetto PAI il legacy set item slots e' opzionale con presenza, mentre in InventoryTransaction autonomo e' il bool `hasLegacy` (equivalente a un byte di presenza).

**Player block action** (ripetuta): `action` varint32 (PlayerActionType) + `block position` (3 varint zigzag) + `face` varint32 zigzag. Posizione e faccia sono sempre scritte (Mojang: tutti e tre `required`); StopDestroyBlock "senza dati aggiuntivi" ma i campi ci sono comunque nel layout (UNVERIFIED nei valori: tratta pos/face come ignorabili). Azioni ammesse nell'array (vincolo runtime Mojang): StartDestroyBlock 0, AbortDestroyBlock 1, StopDestroyBlock 2, CrackBlock 18, PredictDestroyBlock 26, ContinueDestroyBlock 27. CrackBlock solo con block breaking server-auth spento; Predict/Continue solo con acceso.

**Indici flag** (Mojang `PlayerAuthInputData`, coincidono con gophertunnel):

| Flag | Id | | Flag | Id |
|---|---|---|---|---|
| Ascend | 0 | | StartFlying | 42 |
| Descend | 1 | | StopFlying | 43 |
| (NorthJump, non nello schema) | 2 | | ClientAckServerData | 44 |
| JumpDown | 3 | | IsInClientPredictedVehicle | 45 |
| SprintDown | 4 | | BlockBreakingDelayEnabled | 48 |
| ChangeHeight | 5 | | HorizontalCollision / VerticalCollision | 49 / 50 |
| **Jumping** | **6** | | StartUsingItem | 53 |
| AutoJumpingInWater | 7 | | (CameraRelativeMovement, RotControlledByMoveDirection solo gophertunnel) | 54, 55 |
| **Sneaking** | **8** | | StartSpinAttack / StopSpinAttack | 56 / 57 |
| SneakDown | 9 | | IsHotbarOnlyTouch | 58 |
| Up/Down/Left/Right | 10/11/12/13 | | JumpReleasedRaw/PressedRaw/CurrentRaw | 59/60/61 |
| UpLeft/UpRight | 14/15 | | SneakReleasedRaw/PressedRaw/CurrentRaw | 62/63/64 |
| Sprinting | 20 | | InternalUpdate | 65 |
| StartSprinting/StopSprinting | 25/26 | | **StartSneaking** | **27** |
| **StopSneaking** | **28** | | StartSwimming/StopSwimming | 29/30 |
| **StartJumping** | **31** | | StartGliding/StopGliding | 32/33 |
| **PerformItemInteraction** | **34** | | **PerformBlockActions** | **35** |
| **PerformItemStackRequest** | **36** | | HandledTeleport | 37 |
| Emoting | 38 | | MissedSwing | 39 |
| StartCrawling/StopCrawling | 40/41 | | PaddlingLeft/Right, DownLeft/Right | 46/47, 51/52 |

Rischio: lo schema Mojang descrive "Input Data" come array di enum (non dice "bitset"); la lista di id e' confermata solo da gophertunnel (che la usa dal 1.26.40). Vedi rischio 3.

## 4. InventoryTransaction (id 30, client -> server; il server non lo manda)

### 4.1 Layout pacchetto autonomo
Fonti: `InventoryTransactionPacketPayload.json`, gophertunnel `packet/inventory_transaction.go`.

| # | Campo | Codifica |
|---|---|---|
| 1 | legacy request id | varint32 zigzag (0 = nessuno; negativi pari < -1 = richiesta legacy che attende ItemStackResponse) |
| 2 | legacy set item slots | presenza (1 byte, e' sempre scritto) + se 1: varuint32 count + [container u8 (ContainerEnumName), slot: varuint32 count + u8 ciascuno] |
| 3 | tipo transazione | varuint32 (0..4, sez. 2.1) |
| 4 | actions | varuint32 count (max 100) + `InventoryAction` |
| 5 | dati specifici del tipo | vedi 4.2 / 4.3 |

`InventoryAction`: `source_type` varuint32 (Container 0, Global 1, WorldInteraction 2, Creative 3, NonImplemented 99999); `container_id` opzionale (presenza + i8); `bit_flags` opzionale (presenza + varuint32; 1 = WorldInteractionRandom); `slot` varuint32; `from_item` e `to_item` NetworkItemStackDescriptor.

### 4.2 UseItem (tipo 2), dopo le `actions`

| # | Campo | Codifica |
|---|---|---|
| a | action type | varint32 zigzag (0 Place/ClickBlock, 1 Use/ClickAir, 2 Destroy/BreakBlock, 3 UseAsAttack) |
| b | trigger type | u8 |
| c | block position | 3 x varint32 zigzag |
| d | face | u8 |
| e | hotbar slot | varint32 zigzag |
| f | hand | u8 (0 main, 1 off) |
| g | held item | NetworkItemStackDescriptor (sez. 2.3) |
| h | player position (from) | 3 x f32 (posizione occhi) |
| i | click position | 3 x f32 (relativa al blocco, 0..1) |
| j | target block id | varuint32 (runtime id/hash del blocco colpito, per validazione) |
| k | client interact prediction | u8 (0 Failure, 1 Success) |
| l | client cooldown state | u8 (0 Off, 1 On) |

### 4.3 Altri tipi
- UseItemOnActor (3): `runtime id` varuint64, `action type` varint32 (Interact 0, Attack 1, ItemInteract 2), `slot` varint32, `hand` u8, `item` descriptor, `from position` 3 f32, `hit position` 3 f32. Ordine in gophertunnel identico (UNVERIFIED sul filo: Mojang elenca `Hand` dopo `Slot`, coerente).
- ItemRelease (4): `action type` varint32 (Release 0, Use 1), `slot` varint32, `item` descriptor, `from position` 3 f32, `hand` u8. Ordine Mojang (Slot, Item, From Position, Hand) = ordine gophertunnel.
- Normal (0) e Mismatch (1): nessun dato oltre alle `actions`.

### 4.4 Decodifica della cattura reale (53 byte)

`[00 00 02 00 00 01 b5 02 90 01 03 02 00 00 | 00x8 | a4 64 19 c3 ...]`

| Offset | Byte | Campo | Valore |
|---|---|---|---|
| 0 | 00 | legacy request id (varint32) | 0 |
| 1 | 00 | presenza legacy set item slots | assente |
| 2 | 02 | tipo transazione (varuint32) | UseItem |
| 3 | 00 | numero di actions | 0 |
| 4 | 00 | action type (varint32) | 0 = Place/ClickBlock |
| 5 | 01 | trigger type | PlayerInput |
| 6-7 | b5 02 | block x: varuint 309 -> zigzag | -155 |
| 8-9 | 90 01 | block y: 144 -> zigzag | 72 |
| 10 | 03 | block z: 3 -> zigzag | -2 |
| 11 | 02 | face | 2 (nord) |
| 12 | 00 | hotbar slot (varint32) | 0 |
| 13 | 00 | hand | mano principale |
| 14-21 | 00 x8 | held item: id i16 0, count u16 0, aux 0, hasNetId false, block rt id 0, user data len 0 | **aria, 8 byte** |
| 22-33 | a4 64 19 c3 / 58 8d 93 42 / f7 a0 71 c0 | player position f32 x3 | (-153.393, 73.776, -3.775) |
| 34-45 | 00 82 5b 3f / 80 43 0f 3f / c0 cc cc 3c | click position f32 x3 | (0.857, 0.560, 0.025) |
| 46-50 | dd a9 c0 a9 05 | target block id varuint32 | 1429214429 = 0x553014dd (hash di blocco) |
| 51 | 01 | client interact prediction | Success |
| 52 | 00 | client cooldown state | Off |

Totale 53 byte, nessun byte residuo. Plausibilita': occhi a y=73.78 (piedi 72.16), blocco cliccato (-155,72,-2), faccia nord (z minimo, click z=0.025), giocatore a z=-3.78 davanti alla faccia nord. Conferma che Y del BlockPos e' zigzag (144 grezzo darebbe y=144, incoerente) e che l'aria sono 8 byte.

## 5. ItemStackRequest (client -> server, id 147) e ItemStackResponse (server -> client, id 148)

Fonti: `ItemStackRequestPacketPayload.json`, `ItemStackRequest.json`, `ItemStackRequest*Action.json`, `ItemStackResponsePacketPayload.json`, gophertunnel `protocol/item_stack.go`, Mojang `developer_notes/us1418592.md`.

### 5.1 Pacchetto e richiesta
- Pacchetto 147: varuint32 count (max 100) + `ItemStackRequest`. Dentro PAI e' un solo `ItemStackRequest` opzionale (presenza + struttura).
- `ItemStackRequest`: `client_request_id` varint32 zigzag (valido: **negativo dispari**); `actions` varuint32 count (1..100); `strings_to_filter` varuint32 count + stringhe (max 1000 char); `strings_to_filter_origin` **i32 fisso LE** (enum TextProcessingEventOrigin; -1 = unknown).
- **Ogni azione** = `varuint32 indice di variante` + `u8 ActionType` + payload. L'indice di variante e' la posizione nella lista oneOf Mojang: coincide con ActionType per Take..Create (0-6) e vale ActionType - 2 da LabTable in poi (nessun id 7 e 8). Fonte: gophertunnel `stackRequestActionVariant` (la descrizione Mojang dice solo "un byte ActionType", ma il campo `x-control-value-type` e' uint32: il doppio prefisso e' confermato SOLO da gophertunnel: vedi rischio 2).

### 5.2 Azioni
ActionType (u8): Take 0, Place 1, Swap 2, Drop 3, Destroy 4, Consume 5, Create 6, ScreenLabTableCombine 9, ScreenBeaconPayment 10, ScreenHUDMineBlock 11, CraftRecipe 12, CraftRecipeAuto 13, **CraftCreative 14**, CraftRecipeOptional 15, CraftRepairAndDisenchant 16, CraftLoom 17, CraftReservedAction 18, CraftNonImplemented 19, CraftResults(deprecated) 20. Indice variante = id per 0-6, id-2 per id >= 9 (CraftCreative -> 12).

| Azione | Payload dopo variante+tipo |
|---|---|
| Take / Place | `amount` u8 (1..64), `source` SlotInfo, `destination` SlotInfo |
| Swap | `source` SlotInfo, `destination` SlotInfo |
| Drop | `amount` u8, `source` SlotInfo, `randomly` bool |
| Destroy / Consume | `amount` u8, `source` SlotInfo |
| Create | `results_index` u8 |
| **CraftCreative** | `creative_item_net_id` varuint32 (>= 1, quello di CreativeContent), `number_of_crafts` u8 |
| MineBlock (HUD) | `slot` varint32, `predicted_durability` varint32, `net_id_variant` (vedi sotto) |
| CraftRecipe | `recipe_net_id` varuint32, `number_of_crafts` u8 |
| CraftRecipeAuto | `recipe_net_id` varuint32, `number_of_crafts` u8, `ingredients` (array di RecipeIngredient) |
| CraftNonImplemented | nessun payload |
| CraftResultsDeprecated | array di risultati (descrittori) + `num_crafts` u8 |

`SlotInfo` = `FullContainerName` (u8 + opzionale u32) + `slot` u8 + `net id variant`. **Codifica del net id variant: vedi rischio 2.** Gophertunnel: i32 fisso LE (dal 1.26.40); PMMP 1.26.30: varint32 zigzag; Mojang us1418592: "variante cereal con tag" dei tre id (ItemStackNetId server > 0, ItemStackRequestId negativo dispari, LegacyRequestId negativo pari). UNVERIFIED.

### 5.3 Flusso creativo tipico (da Dragonfly/PMMP, non da schema Mojang)
Richiesta con azioni `[CraftCreative{net_id, 1}, Take|Place{count, source = (CreatedOutputContainer 60, slot 50), destination = (Hotbar/Inventory/Cursor...)}]`. Lo slot **50** di `CreatedOutputContainer` = risultato creato (PMMP `UIInventorySlotOffset::CREATED_ITEM_OUTPUT = 50`; Dragonfly scrive il risultato li'). Per eliminare un item (cestino) il client manda `Destroy` dal cursore o da uno slot. Il server deve rispondere con un `ItemStackResponse` e mandare anche un `InventorySlot` coerente.

### 5.4 ItemStackResponse (148)
`responses`: varuint32 count + per ogni risposta:

| Campo | Codifica |
|---|---|
| `result` | **u8** (Success 0, Error 1, InvalidRequestActionType 2, ActionRequestNotAllowed 3, ScreenHandlerEndRequestFailed 4, ItemRequestActionHandlerCommitFailed 5, InvalidRequestCraftActionType 6, InvalidCraftRequest 7, InvalidCraftRequestScreen 8, ... 67 voci in tutto, elenco in `ItemStackResponseInfo.json`; PlayerNotInCreativeMode, FailedToCraftCreative ecc.) |
| `client_request_id` | varint32 zigzag (lo stesso della richiesta) |
| `containers` | **opzionale**: presenza + varuint32 count + `{full_container_name, slots: varuint32 count + SlotInfo}` (con errore: presenza 0) |

Response `SlotInfo`: `requested_slot` u8, `slot` u8, `amount` u8, `item_stack_net_id` **opzionale** (presenza SEMPRE scritta; valore varint32 zigzag solo se amount > 0), `custom_name` stringa, `filtered_custom_name` opzionale (presenza + stringa) [`Bedrock::Safety::RedactableString`: unredacted + redacted opzionale], `durability_correction` varint32 (deve stare in i16). Nota: in gophertunnel `HotbarSlot` = secondo u8 e una volta doveva essere uguale a `slot`; ora non piu'.

## 6. CreativeContent (server -> client, id 145)

Gia' in `generated.rs` (`CreativeContent { groups, entries }`). Layout: `groups` varuint32 count + [`creative_category` **u8** (!), `name` string, `group_icon_item` NetworkItemInstanceDescriptor]; `entries` varuint32 count + [`creative_net_id` varuint32 (unico, >= 1), `item_instance` NetworkItemInstanceDescriptor, `group_index` varuint32]. Item = forma **varint** (sez. 2.3), non "raw". **Bug presente**: `creative_category` e' generato come stringa (sez. 1). L'item icona del gruppo non puo' essere aria (client rimuove item invalidi o con net id duplicato/0, doc Mojang `ItemStackPackets_Disabled.md`). Con `CreativeContent::default()` (nessun gruppo/item) il menu creativo e' vuoto.

## 7. Altri pacchetti server -> client e bidirezionali

| Pacchetto (id) | Layout | Fonte |
|---|---|---|
| **InventoryContent (49)** | `container_id` varuint32 (0 inventario, 119 offhand, 120 armatura, 124 UI), `slots` varuint32 count + NetworkItemStackDescriptor, `full_container_name` (non opz.), `storage_item` descriptor (non opz.; aria se nessuno) | `InventoryContentPacketPayload.json`, gophertunnel |
| **InventorySlot (50)** | `container_id` u8 (Mojang) = varuint32 (gophertunnel, identici per < 128), `slot` varuint32, `full_container_name` **opz.**, `storage_item` **opz.**, `item` descriptor | idem |
| **MobEquipment (31, ambo i versi)** | `target_runtime_id` varuint64, `item` descriptor, `slot` u8 (slot inventario), `selected_slot` u8 (slot hotbar), `container_id` u8 (window id, 0 = inventario) | `MobEquipmentPacketPayload.json` |
| **PlayerHotbar (48)** | `selected_slot` varuint32, `container_id` u8, `should_select_slot` bool | `PlayerHotbarPacketPayload.json` |
| **UpdateBlock (21)** | `position` 3 x varint32 zigzag, `block_runtime_id` varuint32 (hash), `flags` varuint32, `layer` varuint32 | `UpdateBlockPacketPayload.json` |
| **SetPlayerGameType (62)** | `game_type` varint32 (Creative 1) | `SetPlayerGameTypePacketPayload.json`; Mojang: il client lo manda quando cambia il menu, il server risponde con UpdatePlayerGameType (151) |
| **UpdateAbilities (187)** | `target_player_raw_id` **i64 fisso**, `player_permissions` i8 (Visitor 0, Member 1, Operator 2, Custom 3), `command_permissions` u8 (Any 0 ... Owner 4), `layers` varuint32 count + [`type` u16, `abilities_set` u32, `ability_values` u32, `fly_speed` f32, `vertical_fly_speed` f32, `walk_speed` f32] (tutti u32 fissi) | `UpdateAbilitiesPacketPayload.json`, gophertunnel `ability.go` |
| **LevelEvent (25)** | `event_id` varint32 zigzag, `position` 3 f32, `data` varint32 | `LevelEventPacketPayload.json` |
| **LevelSoundEvent (123)** | `sound_event` **stringa** (non piu' enum numerico), `position` 3 f32, `data` varint32, `actor_identifier` stringa, `is_baby` bool, `is_global` bool, `actor_unique_id` **i64 fisso**, `fire_at_position` opz. (presenza + 3 f32) | `LevelSoundEventPacketPayload.json`, gophertunnel |
| **PlayerAction (36, client -> server)** | `player_runtime_id` varuint64, `action` varint32 (PlayerActionType), `block_position` BlockPos, `result_position` BlockPos, `face` varint32 | `PlayerActionPacketPayload.json` |

### 7.1 Flags di UpdateBlock e layer
Gophertunnel (nessun enum nello schema Mojang, UNVERIFIED per i nomi Mojang): `Neighbours = 1`, `Network = 2`, `NoGraphics = 4`, `Priority = 8`. Valore comune: **3** (Neighbours | Network); Dragonfly manda solo 2. Layer: 0 = blocco normale, 1 = liquido.

### 7.2 Abilities (bit, gophertunnel `ability.go`)
Build 1<<0, Mine 1<<1, DoorsAndSwitches 1<<2, OpenContainers 1<<3, AttackPlayers 1<<4, AttackMobs 1<<5, OperatorCommands 1<<6, Teleport 1<<7, Invulnerable 1<<8, Flying 1<<9, MayFly 1<<10, **InstantBuild 1<<11**, Lightning 1<<12, FlySpeed 1<<13, WalkSpeed 1<<14, Muted 1<<15, WorldBuilder 1<<16, NoClip 1<<17, PrivilegedBuilder 1<<18, VerticalFlySpeed 1<<19. Tipo di layer: CustomCache 0, **Base 1**, Spectator 2, Commands 3, Editor 4, LoadingScreen 5. Valori base: fly 0.05, walk 0.1, vertical 1.0.

Come li imposta Dragonfly (riferimento): `abilities_set` = tutti i bit (0xFFFFF); `ability_values`:
- **Creativo**: MayFly | Invulnerable | InstantBuild | Build | Mine | DoorsAndSwitches | OpenContainers | AttackPlayers | AttackMobs (+ Flying se sta volando). Per l'operatore anche OperatorCommands/Teleport (UNVERIFIED).
- **Sopravvivenza**: Build | Mine | DoorsAndSwitches | OpenContainers | AttackPlayers | AttackMobs.
- Spettatore: layer extra (type 2) con Flying | NoClip | Invulnerable.
Dragonfly manda sempre `SetPlayerGameType` e poi `UpdateAbilities` (layer base) dopo il join e al cambio modalita'.

### 7.3 Sound e particelle per rottura/posa blocco
- **LevelEvent** (gophertunnel `level_event.go`; lo schema Mojang non ha l'enum): `ParticlesDestroyBlock = 2001` (data = block runtime id; suono + particelle di rottura), `ParticlesCrackBlock = 2014` (data = runtime id | (faccia << 24)), `ParticlesDestroyBlockNoSound = 2021`, `StartBlockCracking = 3600` (data = 65535 / (secondi di rottura x 20)), `StopBlockCracking = 3601`, `UpdateBlockCracking = 3602`, `ParticlesCrackBlockDown..East = 3603..3608`, `ParticleLegacyEvent = 0x4000`. Il doc Mojang BlockBreakingOverview cita `UpdateBlockCracking`, `ParticlesCrackBlock`, `ParticlesDestroyBlock` come effetti iniziati dal server.
- **LevelSoundEvent**: il tipo e' ora una stringa. Enum Mojang `LevelSoundEvent`.json (uint32, 571 voci) ha i nomi: `hit` = 1, `step` = 2, `break` = 5, `place` = 6, `item.use.on` = 0, `break.block` = 55 (UNVERIFIED se la stringa sul filo e' il nome del valore dell'enum o l'enum numerico: gophertunnel usa le stringhe "hit", "step", "break", "place", ...). Posa blocco: `sound_event "place"`, `data` = block runtime id. Rottura: Dragonfly NON manda un suono ma il LevelEvent 2001. Colpi durante la rottura: `hit` con data = runtime id.

## 8. StartGame: server authoritative block breaking

`StartGame.movement_settings` = `SyncedPlayerMovementSettings`: `rewind_history_size` varint32 zigzag (tick di storia per le correzioni; `player-rewind-history-size-ticks=40` nell'esempio Mojang) + `server_authoritative_block_breaking` bool. Gia' presente nel generatore. Il repo oggi manda `Default` (0 / false) in `crates/server/src/join.rs`. Non esiste piu' un campo "server auth movement mode": PAI e' sempre il pacchetto di input (ServerAuthoritativeV3, `MovementProtocolDeprecation.md`). `enable_item_stack_net_manager` e' true in `join.rs`: i pacchetti ItemStack* sono obbligatori.

**Cosa si aspetta il client (doc Mojang `BlockBreakingOverview.md`, parafrasato):**
- Toggle **spento** (consigliato per la tappa 1): il client manda azioni di rottura (Start/Abort/Crack) in PAI e una transazione legacy UseItem tipo Destroy (BreakBlock) dentro PAI per rompere il blocco e danneggiare l'item; il server esegue la rottura quando la transazione arriva, senza validare il progresso. Il server deve mandare gli effetti (crack) in risposta a `CrackBlock`.
- Toggle **acceso**: niente transazione legacy; il client predice la rottura e manda `PredictDestroyBlock` e `ContinueDestroyBlock` nelle block actions + un'azione `ScreenHUDMineBlock` (MineBlock) in ItemStackRequest dentro PAI per la durabilita'. Il server deve seguire il progresso, mandare LevelEvent (StartBlockCracking/UpdateBlockCracking/ParticlesCrackBlock/ParticlesDestroyBlock), correggere con UpdateBlock e InventorySlot, e rispondere con ItemStackResponse. Il bersaglio resta scelto dal client (solo controllo di distanza; `server-authoritative-block-breaking-pick-range-scalar=1.5` nell'esempio Mojang).
- **Creativo**: "Creative mode is an exception and is still client authoritative". Il client rompe subito in locale. La rottura creativa arriva come **PlayerAction (36) con action 13 CreativeDestroyBlock** (schema: "Expects server to destroy the block and optionally send new block or chunk information. Sent in Player Action"). Il server deve rompere il blocco (aria nel mondo), mandare UpdateBlock a tutti e LevelEvent 2001. UNVERIFIED se con toggle spento in creativo il client mandi invece (o anche) la UseItem Destroy dentro PAI (Dragonfly gestisce entrambe): per sicurezza gestire entrambi i percorsi, senza applicare due volte.

## 9. Rischi e punti NON verificati

1. **Net id variant** (ItemInstance dentro `NetworkItemStackDescriptor`, e SlotInfo/MineBlock nelle richieste): Mojang dice "variante cereal con tag" (us1418592) o "singolo varint con segno" (us1404359, precedente); gophertunnel scrive bool + varint32 negli ItemInstance ma **i32 fisso** nelle richieste. UNVERIFIED: va loggata la prima ItemStackRequest reale in esadecimale prima di fidarsi (il flusso creativo CraftCreative + Take/Place usa SlotInfo con net id).
2. **Doppio prefisso azione** (indice variante varuint32 + ActionType u8) in ogni azione: confermato solo da gophertunnel; da verificare con una cattura reale. Se sbagliato, il decoder deriva subito.
3. **Input Data come lista di id** invece del bitset: confermato solo da gophertunnel HEAD (dal 1.26.40) e dalla nota Mojang; da controllare con una cattura reale di PAI (primo byte dopo i 5 float + 2 float movimento + 1 float head yaw).
4. **Item non-aria con user data**: len=0 vs i 10 byte canonici (i16 0 + 2 x u32 0); `CreativeContent` non vuoto e `InventorySlot` con item reali dipendono da questo. UNVERIFIED sul client vanilla.
5. **Bug del generatore**: `creative_category` (e `ItemStackResponse.result`) come stringa invece di u8: un `CreativeContent` non vuoto verrebbe letto male dal client (disconnessione/menu rotto). `NetworkItemStackDescriptor` non si genera ancora.
6. Creativo con toggle spento: percorso esatto di rottura (PlayerAction 13 vs UseItem Destroy in PAI) UNVERIFIED; stringhe/valori di `LevelSoundEvent` per place/break e ordine dei flag di UpdateBlock UNVERIFIED nello schema Mojang (solo gophertunnel). Le facce 0..5 e lo slot 50 di CreatedOutput vengono da gophertunnel/PMMP, non dalle spec Mojang.
