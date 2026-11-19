# Protocollo tappa 2: pacchetti delle entità

Protocollo 2225. Fonte dei byte: schemi Mojang `vendor/blackstone-protocol-docs/json` (tag v1.26.65), indice `x-ordinal-index`.
Regole di codifica: `2026-11-18-protocollo-tappa1.md` §1 (interi fissi LE, `Compression` = varint/zigzag, opzionali = 1 byte di presenza,
array = contatore varuint32). Codice: `crates/protocol/src/entity.rs` (a mano) e `generated.rs` (generato).

## ID (`[cereal:packet]`)

AddPlayer 12, RemoveActor 14, AddItemActor 15, TakeItemActor 17, MoveActorAbsolute 18, MobArmorEquipment 32, SetActorMotion 40,
Animate 44, PlayerList 63, PlayerSkin 93. PlayerList e PlayerSkin: solo le costanti (la codifica è del Task 4 / Task 7).

## Campi, in ordine

**AddPlayer (12)**, scritto a mano:
0 UUID `mce::UUID` = due u64 fissi LE (bit alti, poi bassi); 1 nome stringa; 2 runtime id varuint64; 3 id chat piattaforma stringa;
4 posizione 3 f32 (altezza dell'attore già applicata); 5 velocità 3 f32; 6 rotazione Vec2 (pitch, yaw) 2 f32; 7 yaw testa f32;
8 oggetto in mano `NetworkItemStackDescriptor`; 9 tipo di gioco varint32 zigzag; 10 dati sincronizzati (contatore varuint32 + voci
`{ ID varuint32, Payload variante }`, stesso formato di `SetActorData`); 11 proprietà sincronizzate (lista int, lista float:
due contatori); 12 abilità `SerializedAbilitiesData` (target i64 fisso, permessi i8, comandi u8, contatore + livelli);
13 legami (contatore varuint32, qui 0); 14 blocco passeggero (opzionale: 1 byte di presenza, qui 0); 15 id dispositivo stringa;
16 piattaforma di build i32 fisso (nessuna `Compression`).
Gophertunnel ha lo stesso ordine (UUID, nome, runtime id, chat id, pos, vel, pitch, yaw, head yaw, oggetto, game type, metadata,
proprietà, abilità, legami, device id, piattaforma): l'unica aggiunta dello schema è il blocco passeggero opzionale (14).
Scelte: le abilità arrivano dal chiamante come elenco di livelli (`AddPlayerData::layers`, gli stessi di `join::ability_layers`);
il bersaglio è il runtime id, i permessi sono Member/Any. La skin non c'è: viaggia nella `PlayerList`.

**AddItemActor (15)**, a mano (dati sincronizzati): 0 id unico varint64 zigzag; 1 runtime id varuint64; 2 oggetto
`NetworkItemStackDescriptor`; 3 posizione 3 f32; 4 velocità 3 f32; 5 dati sincronizzati; 6 da pesca bool. (Il generatore
lascia `raw` i `DataItemEntry`, per questo non è generato.)

**TakeItemActor (17)**, generato: 0 runtime id dell'oggetto varuint64; 1 runtime id di chi raccoglie varuint64. L'ordine è
oggetto-poi-raccoglitore, come gophertunnel (`EntityRuntimeID` = oggetto, poi `TakerEntityRuntimeID`).

**MoveActorAbsolute (18)**, generato (`MoveActorAbsoluteData`): 0 runtime id varuint64; 1 intestazione u8; 2 posizione 3 f32;
3 rotazione X (pitch) u8; 4 rotazione Y (yaw) u8; 5 yaw testa u8. Angoli: `gradi / 360 * 256` in un byte (come gophertunnel
`byte(angle / (360/256))`). Intestazione: bit 0 a terra, bit 1 spostamento forzato/teletrasporto (nota Mojang us_1565354: il
vecchio `mTeleported` è stato rinominato `mForceMove`; gophertunnel: `OnGround 1`, `Teleport 2`). Non usiamo altri bit.

**RemoveActor (14)**, generato: 0 id unico varint64 zigzag (non il runtime id).

**SetActorMotion (40)**, generato: 0 runtime id varuint64; 1 moto 3 f32; 2 tick varuint64 (0 per entità non controllate dal client).

**MobArmorEquipment (32)**, generato: 0 runtime id; 1 testa; 2 busto; 3 gambe; 4 piedi; 5 corpo; tutti
`NetworkItemStackDescriptor`. Il corpo (cavalli/lupi) è sempre aria, come Dragonfly (che manda l'elmo..stivali e un corpo vuoto).

**Animate (44)**, generato: 0 azione **u8** (NoAction 0, Swing 1, WakeUp 3, CriticalHit 4, MagicCriticalHit 5); 1 runtime id varuint64;
2 dati f32; 3 origine dell'oscillazione opzionale u8; 4 mano opzionale u8. Differenza dai protocolli vecchi: l'azione non è più un
varint32. Il client manda lo stesso pacchetto: `input::Animate` legge solo azione e runtime id e ignora il resto. Il campo
`action` è `i32` (come da piano) ma si legge un solo byte.

**PlayerSkin (93)**: non implementato qui (Task 7). **PlayerList (63)**: vedi sezione dedicata sotto.

## Numeri dei flag (`actor::flag`, chiave `FLAGS` = bitset i64)

SNEAKING 1, SPRINTING 3, GLIDING 32, SWIMMING 57. Gli schemi Mojang non contengono l'enum `ActorFlags` (solo
`ActorDataFlagComponent`, un bitset opaco), quindi non c'è un numero ufficiale da confrontare. Fonte: la numerazione già usata dal
crate e verificata sul gioco (CAN_CLIMB 19, BREATHING 35, HAS_COLLISION 48, HAS_GRAVITY 49, dopo lo spostamento di uno introdotto
da CAN_DASH 46 nel protocollo 560, changelog_560) e i nomi `EntityDataFlag*` di gophertunnel/Dragonfly
(`entity_metadata.go` li usa per sneaking, sprinting, swimming, gliding). I quattro valori seguono la stessa tabella
PocketMine `EntityMetadataFlags` (SNEAKING 1, SPRINTING 3, GLIDING 32, SWIMMING 56 nella tabella precedente al 560): NON verificati su una cattura del client. SWIMMING è 57 perché `changelog_560_11_10_22.md` lo elenca tra i flag spostati di uno da CAN_DASH(46); decisione del controller (le doc Mojang fanno fede).

## SerializedSkin (implementata in `crates/protocol/src/skin.rs`, `Skin::encode`)

> **Superata dalla sezione "Confronto con gophertunnel v1.63.0" in fondo**: colore, PieceType, tinte e flag fidato di questa tabella erano sbagliati.

Ordine = `x-ordinal-index` di `SerializedSkin.json` (tipi di `SkinImage`, `AnimatedImageData`, `SerializedPersonaPieceHandle`, `TintMapColor`). Regole del generatore: struct = campi in ordine, array/mappe = contatore varuint32, wrapper a un solo intero (`Color`) = intero, interi senza `Compression` = fissi LE.

| # | Campo | Filo |
|---|---|---|
| 0 | ID | stringa |
| 1 | ResourcePatch | stringa (JSON) |
| 2 | ImageData (`SkinImage`) | width u32, height u32, ImageBytes (varuint32 + byte) |
| 3 | AnimatedImageData | contatore varuint32; per voce: SkinImage, AnimatedTextureType **varuint32** (Compression), Frames f32, AnimationExpression **varuint32** |
| 4 | CapeImageData | SkinImage (senza cape: 0, 0, 0 byte) |
| 5 | GeometryData | stringa (JSON) |
| 6 | GeometryDataMinEngineVersion | stringa |
| 7 | AnimationData | stringa |
| 8 | CapeID | stringa |
| 9 | FullID | stringa |
| 10 | ArmSize | **u8** (Slim 0, Wide 1), non stringa come in gophertunnel |
| 11 | SkinColor (`Color`) | i32 LE, ARGB (da "#AARRGGBB") |
| 12 | PersonaPieces | contatore; per voce: PieceId stringa, PieceType **stringa** (nome minuscolo, vedi sotto), PackId (`mce::UUID`: Most Significant u64 poi Least Significant u64, LE), IsDefaultPiece bool, ProductId stringa |
| 13 | PieceTintColors | mappa: contatore; per voce: chiave PieceType **stringa** (nome minuscolo), valore `TintMapColor` = array `Colors` (contatore varuint32 + i32 LE ciascuno; lo schema dice min=max=4 ma il generatore scrive comunque il contatore: **non verificato su cattura**) |
| 14-18 | IsPremium, IsPersona, IsPersonaCapeOnClassicSkin, IsPrimaryUser, OverridesPlayerAppearance | bool |
| 19 | TrustedSkinFlag | enum (Unset 0, False 1, True 2), **larghezza non verificata**: scritto come u8; il server manda 2 |
| 20 | ProfileHash | stringa (vuota) |

Incerti da confermare con una cattura reale del PlayerList: contatore dei `Colors`, larghezza di `TrustedSkinFlag`, endianness di `ArmSize` (qui u8 come da schema).

**PieceType e' una stringa sul filo** (varuint32 + UTF-8, nome minuscolo della tabella `PIECE_TYPES`: `unknown`, `skeleton`, `body`, ...), sia nei pezzi persona sia nelle tinte. Lo dice l'errore del client reale del 19/11/2026 ("This value is bound to 'persona::PieceType' and expects one of the following strings ... readNoHeader failed! packetId: 63"), anche se `SerializedPersonaPieceHandle.json` dichiara uint32 Enum-as-Value. Vale la risposta del client, non lo schema. Un nome fuori tabella si scrive come `unknown`.

UUID nel PlayerList (Task 4): `skin::uuid_bytes` restituisce i 16 byte **nell'ordine del testo** ("aabbccdd-eeff-..." -> aa bb cc dd ee ff ...). Bedrock scrive l'UUID come due u64 little-endian (Most Significant Bits poi Least Significant Bits, come `mce::UUID` sopra): quindi scrivere `u64::from_be_bytes(b[..8])` e `u64::from_be_bytes(b[8..])` con `Writer::u64` (little-endian), come fa `Skin::encode` per `PackId`. Ordine non verificato su gophertunnel (non e' in `vendor/`): controllare con la cattura.


## PlayerList (63), `entity::player_list_add` / `player_list_remove`

Schemi: `PlayerListPacketPayload.json`, `PlayerListAddEntry.json`, `PlayerListRemoveEntry.json`. Il corpo e' un array: contatore varuint32,
poi per ogni voce una **variante con etichetta** (`oneOf`, `x-control-value-type: uint32`, stessa regola di `ItemStackRequest`, tappa1 §1):
indice di variante varuint32 = posizione nel `oneOf` (**Remove 0, Add 1**), poi i campi della voce, che iniziano col suo byte `Action`
(u8: Add 0, Remove 1). Il server manda sempre liste di una sola specie.

| Voce | Campi in ordine (`x-ordinal-index`) |
|---|---|
| Add | 0 Action u8 = 0; 1 UUID (due u64 LE, bit alti poi bassi); 2 id unico varint64 zigzag (= `pid`); 3 nome; 4 XUID; 5 PlayFab ID; 6 id online piattaforma (chat); 7 piattaforma di build i32 fisso; 8 `SerializedSkin`; 9 insegnante bool; 10 host bool; 11 sub-client bool; 12 colore giocatore i32 fisso (ARGB) |
| Remove | 0 Action u8 = 1; 1 UUID |

Scelte: PlayFab ID = quello della skin (vuoto di norma); id chat piattaforma vuoto; piattaforma di build = `DeviceOS` del login passato
cosi' com'e' (-1 se manca); host, insegnante, sub-client falsi; colore = -1 (bianco opaco).
**Non verificato su cattura**: l'indice di variante (Remove 0 / Add 1, dalla posizione nel `oneOf`; se il client volesse l'`Action` come
indice sarebbero invertiti) e il valore di default del colore. La lista Add arriva prima dell'`AddPlayer` dello stesso giocatore.

## Confronto con gophertunnel v1.63.0

Riferimento: `vendor/gophertunnel` v1.63.0 (MIT, `CurrentProtocol` 2225, usato da Dragonfly 1.26.60 e provato su client veri). Ordine del
controller: dove gophertunnel e gli schemi JSON Mojang divergono, vale gophertunnel. Le tabelle precedenti di questo file (SerializedSkin,
PlayerList) sono sostituite da questa. Il client reale rifiutava il `PlayerList` (errore "invalid enum value readNoHeader failed! packetId: 63")
perche' la skin era disallineata: il commit c950c80 (PieceType come stringa) curava il sintomo, non la causa.

### Skin (`skin.go`: `Skin.Marshal`, `SkinAnimation.Marshal`, `PersonaPiece.Marshal`, `PersonaPieceTintColour.Marshal`)

| Campo | Nostro (prima) | gophertunnel | Esito |
|---|---|---|---|
| SkinColour | i32 LE di "#AARRGGBB" | `BEARGB`: int32 big-endian di `A \| R<<8 \| G<<16 \| B<<24`, cioe' byte B, G, R, A | corretto; in struct e' RGBA, dal login "#b37b62ff" = R G B A, "#0" = 0 |
| PersonaPiece.PieceType | stringa (c950c80) | `uint32` | corretto (annullata c950c80 per i pezzi) |
| Tabella `PieceType*` | 30 voci con "coco" prima di "unsupported" (unsupported = 29) | 29 voci, `Unsupported` = 28, nessun "coco" | corretto; "persona_hand" del login vale Hands (9) |
| PieceTintColours (contatore) | contatore varuint32 delle voci | `Slice` = contatore varuint32 | uguale |
| PersonaPieceTintColour.PieceType | stringa dal tipo numerico (`PIECE_TYPES`, fuori tabella "unknown") | stringa: `personaPieceTintWireType` (toglie "persona_", "persona_hand" -> "hands"); in lettura il contrario | corretto: il nome del login e' conservato come `String` |
| PersonaPieceTintColour.Colours | contatore varuint32 + i32 LE ciascuno | 4 colori `BEARGB`, **senza contatore** | corretto (array fisso `[[u8;4];4]`; mancanti = 0; tinte del login lette come "#AARRGGBB") |
| SkinAnimation.AnimationType / ExpressionType | varuint32 | varuint32 | uguale |
| Trusted | u8 (`TrustedSkinFlag`, 2) | stringa "true"/"false" (lettura: `EqualFold` con "true") | corretto: scriviamo "true"; in lettura la stringa si scarta |
| ProfileHash | stringa | stringa | uguale |
| PlayFabId | non nella skin | non nella skin (e' nella voce del PlayerList) | uguale |
| ArmSize | u8 (slim 0, wide 1) | `Uint8`, `ArmSizeSlim` 0, `ArmSizeWide` 1 | uguale |
| Resto (SkinID, ResourcePatch, immagini, cape, geometria, versione motore, AnimationData, CapeID, FullID, bool, ordine) | | | uguale |
| PersonaPiece.PackID | due u64 LE (bit alti, bassi) | `Writer.UUID`: ogni meta' invertita = due u64 LE | uguale |

`Skin::decode` legge esattamente `Skin.Marshal` (e quindi il corpo di `PlayerSkin`: UUID, skin, nome nuovo, nome vecchio, come `packet/player_skin.go`).

### PlayerList (`player.go` `PlayerListEntry.Marshal`, `packet/player_list.go`)

| Campo | Nostro (prima) | gophertunnel | Esito |
|---|---|---|---|
| Variante + azione | varuint32 variante (Remove 0, Add 1) + u8 azione (Add 0, Remove 1) | identico (`playerListAction`) | uguale |
| UUID, id unico varint64, nome, XUID, PlayFab, id chat, piattaforma i32 | uguali | uguali | uguale |
| Skin | disallineata (vedi sopra) | `Skin.Marshal` | corretto |
| Teacher, Host, SubClient | bool | bool | uguale |
| PlayerColour | i32 LE = -1 | `BEARGB` (B, G, R, A) | corretto (il bianco opaco da' gli stessi byte, ma ora con la funzione giusta) |
| Voce Remove | variante 0, azione 1, UUID | identico | uguale |

### Altri pacchetti della tappa 2

| Pacchetto | Nostro | gophertunnel | Esito |
|---|---|---|---|
| AddPlayer (12) | ordine, UUID, proprieta' sincronizzate 0/0, abilita' (i64, permessi u8 x2, livelli u16/u32/u32/f32 x3), legami 0, blocco passeggero assente, DeviceID, BuildPlatform | stesso ordine e tipi (`AbilityData`, `EntityProperties` = due `Slice`) | uguale; test esistente |
| AddItemActor (15) | id unico varint64, runtime id, oggetto, pos, vel, dati, da pesca | stesso | uguale |
| MoveActorAbsolute (18) | runtime id, flag u8 (1 a terra, 2 teletrasporto), pos, 3 angoli in un byte | `ByteFloat` (x / (360/256)), stessi flag | uguale |
| RemoveActor (14) | varint64 id unico | `ActorUniqueID` | uguale |
| TakeItemActor (17) | oggetto poi chi raccoglie | stesso | uguale |
| SetActorMotion (40) | runtime id, vec3, tick varuint64 | stesso | uguale |
| SetActorData (39) | runtime id, dati, 0 + 0 proprieta', tick varuint64 | stesso | uguale |
| Animate (44) | generato: origine `Option<u8>`, mano `Option<u8>` (1 byte di presenza + valore) | origine `Optional[string]` (nomi "build", "mine", "attack"...), mano `u8` **sempre presente** | corretto: scritto a mano (`entity::animate`). Con origine e mano assenti i byte erano gia' uguali (`animate_swing` non cambia). Il tipo generato `Animate` resta in `generated.rs` finche' non si rigenera (manca `../blackstone-protocol-docs`); tolto dall'elenco del README |
| Animate dal client | azione u8 + runtime id | stesso, poi dati f32, origine, mano (ignorati) | uguale |
| MobEquipment (31) | runtime id, oggetto, slot, slot barra, finestra (u8 x3) | stesso | uguale; test aggiunto |
| MobArmorEquipment (32) | runtime id, 4 pezzi + corpo (vuoto) | stesso | uguale |
| Oggetto (`ItemInstance`) | i16 id, u16 conteggio, varuint32 aux, bool + varint32 id pila, varuint32 hash, dati utente con lunghezza (i16 + 2 x u32) | stesso; aria = 8 zeri | uguale |
| Metadati (`EntityMetadata`) | contatore, per voce: chiave, variante varuint32, tipo u8, valore; tipi Float 3, Int64 7 (varint64) | identico; `EntityDataType` Float32 3, Int64 7 | uguale |
| Ordine delle voci dei metadati | ordine di chiamata | **chiavi in ordine crescente** (alcune funzioni lo richiedono) | corretto: `write_data` ordina |

### Numeri di `actor::flag` e `actor::key` contro `EntityDataFlag*` / `EntityDataKey*`

Tutti uguali: chiavi Flags 0, Width 53, Height 54; flag Sneaking 1, Sprinting 3, Climb 19, Gliding 32, Breathing 35, HasCollision 48, HasGravity 49,
Swimming 57. Fissati con un test (`flag_and_key_numbers_match_gophertunnel`).
