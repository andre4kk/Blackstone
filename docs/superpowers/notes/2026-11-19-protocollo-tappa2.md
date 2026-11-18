# Protocollo tappa 2: pacchetti delle entità

Protocollo 2225. Fonte dei byte: schemi Mojang `vendor/bedrock-protocol-docs/json` (tag v1.26.65), indice `x-ordinal-index`.
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

**PlayerList (63) / PlayerSkin (93)**: non implementati qui.

## Numeri dei flag (`actor::flag`, chiave `FLAGS` = bitset i64)

SNEAKING 1, SPRINTING 3, GLIDING 32, SWIMMING 57. Gli schemi Mojang non contengono l'enum `ActorFlags` (solo
`ActorDataFlagComponent`, un bitset opaco), quindi non c'è un numero ufficiale da confrontare. Fonte: la numerazione già usata dal
crate e verificata sul gioco (CAN_CLIMB 19, BREATHING 35, HAS_COLLISION 48, HAS_GRAVITY 49, dopo lo spostamento di uno introdotto
da CAN_DASH 46 nel protocollo 560, changelog_560) e i nomi `EntityDataFlag*` di gophertunnel/Dragonfly
(`entity_metadata.go` li usa per sneaking, sprinting, swimming, gliding). I quattro valori seguono la stessa tabella
PocketMine `EntityMetadataFlags` (SNEAKING 1, SPRINTING 3, GLIDING 32, SWIMMING 56 nella tabella precedente al 560): NON verificati su una cattura del client. SWIMMING è 57 perché `changelog_560_11_10_22.md` lo elenca tra i flag spostati di uno da CAN_DASH(46); decisione del controller (le doc Mojang fanno fede).
