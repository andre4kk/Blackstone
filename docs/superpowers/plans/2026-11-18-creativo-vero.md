# Tappa 1 "creativo vero" — piano di implementazione

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Nel mondo di Blackstone si costruisce davvero in creativo: menu creativo pieno, rompere e piazzare decisi e
controllati dal server (con l'orientamento giusto), porte/botole/cancelletti/leve/pulsanti, giocatore salvato, `/gamemode`.

**Architecture:** Il server (crate `blackstone-server`, ciclo unico a 20 tick in `lib.rs`) riceve le azioni del client
(`PlayerAction`, `PlayerAuthInput`, `InventoryTransaction`, `ItemStackRequest`), le valida, le fa passare da un punto a
eventi annullabili e cambia i chunk in memoria, mandando `UpdateBlock` a chi vede il chunk. Blocchi e oggetti vengono da
un registro caricato da dati estratti (Dragonfly 1.26.60). Le regole di piazzamento sono tabelle per stato + eccezioni per
famiglia + casi speciali tradotti da Dragonfly.

**Tech Stack:** Rust (workspace esistente), Python 3 per gli script di estrazione in `tools/`, LevelDB (`rusty-leveldb`).

**Spec:** `docs/superpowers/specs/2026-11-18-creativo-vero-design.md`

**Note di ricerca (leggere quelle indicate da ogni task):**
- `docs/superpowers/notes/2026-11-18-protocollo-tappa1.md` — layout dei pacchetti (sezioni citate come P§n)
- `docs/superpowers/notes/2026-11-18-dragonfly-tappa1.md` — regole di gioco Dragonfly (D§n)
- `docs/superpowers/notes/2026-11-18-dati-tappa1.md` — dati di blocchi, creativo, giocatore (DT§n)

## Global Constraints

- Server MIT in `crates/`; il generatore (`generator/`, AGPL) non si tocca.
- Protocollo 2225 (client 1.26.6x). Schemi Mojang in `vendor/blackstone-protocol-docs` (tag v1.26.65, gitignored, EULA: non
  copiarne il contenuto in file tracciati). Dragonfly in `vendor/dragonfly` (commit `0f9a3a5736267db60bf46e92d408f81be1de9a64`, MIT).
- ID di rete dei blocchi = hash FNV-1a dell'NBT LE `{name, states}` (`world::block_hash`); `minecraft:air` = -604749536.
- ID degli oggetti solo da `crates/server/data/items.txt`; mai da `required_item_list.json` di BedrockData (sfasato).
- Commenti, log e messaggi in italiano; rustfmt a 160 colonne; `cargo clippy --workspace --all-targets -- -D warnings` pulito.
- Ogni pacchetto client→server malformato si ignora con un log (`[#pid] ... illeggibile`), mai disconnessione né panico.
- Ogni azione rifiutata rimanda al client lo stato vero (`UpdateBlock` per i blocchi, `ItemStackResponse` di errore +
  `InventorySlot` per l'inventario).
- Portata: distanza occhi→centro del blocco ≤ 14.0 in creativo, ≤ 8.0 nelle altre modalità (D§6).
- Comandi: `cargo test --workspace` dalla radice (il generatore ha il suo workspace e qui non serve).

## Review Focus

1. Pacchetto client con campi opzionali assenti o lista vuota (PAI senza azioni, `ItemStackRequest` con 0 azioni) → nessun panico, nessun effetto.
2. Azione su chunk non caricato o fuori dal mondo (y < -64 o y ≥ 320, coordinate enormi) → rifiutata, niente creazione di chunk.
3. Stesso blocco rotto due volte nello stesso tick (PlayerAction 13 e UseItem Destroy insieme) → una sola rottura, un solo evento.
4. Oggetto in mano che il server non conosce o diverso da quello del client → nessun piazzamento, `UpdateBlock` di ripristino.
5. Giocatore che esce e rientra mentre il disco è lento → i dati letti arrivano prima di StartGame; nessun inventario vuoto salvato sopra uno pieno.

Ognuno ha il suo test nel task indicato (cercare "Review Focus N").

## Decisioni prese scrivendo il piano (da riportare all'utente)

1. **Fonte dei dati: Dragonfly 1.26.60** (`server/world/block_states.nbt`, `server/item/creative/creative_items.nbt`,
   `server/world/vanilla_items.nbt` per il controllo degli ID), non BedrockData (fermo a 1.26.30, mancano 109 oggetti).
   BedrockData resta solo per confronto.
2. **Rottura decisa dal server in StartGame: spenta in questa tappa.** Per Mojang il creativo è comunque deciso dal
   client (arriva `PlayerAction` 13) e il server convalida e ripristina; l'opzione accesa serve alla sopravvivenza e
   richiede di seguire il progresso della rottura: si accende nella tappa 2. Il server resta autoritativo: controlla e
   annulla ogni rottura.
3. **Codifica dell'ID di pila nelle richieste: come gophertunnel** (i32 fisso nelle `SlotInfo`, presenza + varint nelle
   `ItemInstance`), con log esadecimale delle prime richieste reali per verificarla nel gioco.
4. **Pulsanti** (assenti in Dragonfly): valori vanilla, legno 30 tick, pietra 20 tick.
5. **Regole di orientamento:** regola base per stato + tabella di eccezioni per famiglia (D§3 mostra che lo stesso stato
   ha convenzioni diverse tra famiglie).

---

### Task 1: Dati di blocchi, oggetti e menu creativo + registro nel server

**Files:**
- Create: `tools/extract_blocks.py`, `tools/extract_creative.py`
- Create: `crates/server/data/blocks.json`, `crates/server/data/creative.json` (generati, committati con `_license`)
- Create: `crates/server/src/registry.rs`
- Modify: `crates/server/src/lib.rs` (mod), `crates/server/src/storage.rs` (palette di ripiego)

Leggere DT§2-4, D§8.1, `tools/extract_biomes.py` (stile degli script).

**Interfaces:**
- Produces (in `registry.rs`):
  - `pub(crate) enum StateValue { Byte(u8), Int(i32), Str(String) }`
  - `pub(crate) struct BlockState { pub name: String, pub states: Vec<(String, StateValue)> }` (stati in ordine alfabetico)
    con `fn hash(&self) -> i32` (= `world::block_hash`), `fn get(&self, key: &str) -> Option<&StateValue>`,
    `fn with(&self, key: &str, value: StateValue) -> BlockState`, `fn nbt(&self) -> Vec<u8>` (NBT LE `{name, states}`).
  - `pub(crate) struct Blocks` con `fn state(&self, hash: i32) -> Option<&BlockState>`, `fn default_hash(&self, name: &str) -> Option<i32>`,
    `fn values(&self, name: &str, key: &str) -> Option<&[StateValue]>`, `fn is_valid(&self, s: &BlockState) -> bool`.
  - `pub(crate) struct Items` con `fn id(&self, name: &str) -> Option<i16>`, `fn name(&self, id: i16) -> Option<&str>`,
    `fn block_of(&self, name: &str) -> Option<i32>` (hash dello stato di default del blocco con lo stesso nome; `None` per i non-blocchi).
  - `pub(crate) struct CreativeItem { pub name: String, pub meta: u32, pub block: Option<i32>, pub nbt: Option<Vec<u8>>, pub group: u32 }`,
    `pub(crate) struct CreativeGroup { pub category: u8, pub name: String, pub icon: Option<CreativeItem> }`.
  - `pub(crate) fn blocks() -> &'static Blocks`, `pub(crate) fn items() -> &'static Items`,
    `pub(crate) fn creative() -> &'static (Vec<CreativeGroup>, Vec<CreativeItem>)` (tutti `OnceLock` da `include_str!`).

- [ ] **Step 1: Script di estrazione.** `extract_blocks.py <vendor/dragonfly> crates/server/data/blocks.json`: legge
  `server/world/block_states.nbt` (NBT di rete varint, compound concatenati, DT§2), ricalcola l'hash e verifica air =
  -604749536 e unicità; esce `{"_license": "...", "blocks": [{"name", "states": [{"key", "type", "values"}], "default": <hash>}]}`
  con default = primo stato del gruppo. `extract_creative.py <vendor/dragonfly> crates/server/data/items.txt crates/server/data/creative.json`:
  legge `creative_items.nbt` (vedi `server/item/creative/creative.go` per il formato e le categorie), tiene l'ordine,
  scarta e stampa i nomi assenti da `items.txt`; esce gruppi `{category, name, icon}` e voci `{name, meta, block_states|null, nbt_b64|null, group}`.
  Entrambi stampano un conteggio finale e falliscono (`sys.exit`) su incoerenze. Controllo incrociato: stampare quanti
  ID di `vanilla_items.nbt` differiscono da `items.txt` (atteso 0).
- [ ] **Step 2: Generare i dati** e committarli con gli script.
  Run: `python tools/extract_blocks.py vendor/dragonfly crates/server/data/blocks.json` e l'analogo per il creativo.
  Expected: conteggi stampati, nessun errore.
- [ ] **Step 3: Test che falliscono** in `registry.rs`:
  - `air_and_stone_hashes`: `blocks().default_hash("minecraft:air") == Some(-604749536)`, stone presente, e
    `blocks().state(h).unwrap().hash() == h` per entrambi.
  - `stairs_have_corner_state`: `blocks().values("minecraft:oak_stairs", "minecraft:corner")` contiene `"outer_left"` (stato 1.26.60).
  - `every_creative_block_item_places_a_known_state`: per ogni voce creativa con `block = Some(h)`, `blocks().state(h).is_some()`.
  - `creative_items_all_have_ids`: ogni voce creativa ha `items().id(name).is_some()`.
- [ ] **Step 4: Implementare `registry.rs`** (serde_json sui due file + `items.txt`).
- [ ] **Step 5: Palette di ripiego nel salvataggio.** In `storage.rs` quando un hash non è nella palette ricevuta dal
  generatore si usa `registry::blocks().state(hash).nbt()`. Test `save_and_load_block_not_in_generator_palette`: un
  chunk con `minecraft:oak_stairs` (stato non predefinito) salvato senza palette del generatore si rilegge uguale.
- [ ] **Step 6:** `cargo test -p blackstone-server` verde; commit `feat(server): registro di blocchi, oggetti e menu creativo (dati Dragonfly 1.26.60)`.

### Task 2: Codec del protocollo: oggetti, pacchetti nuovi, correzioni del generatore

**Files:**
- Modify: `tools/protocolgen.py` (enum stringa inline → intero; tipo intero senza tipo sottostante)
- Modify: `crates/protocol/src/generated.rs` (rigenerato)
- Create: `crates/protocol/src/reader.rs`, `crates/protocol/src/item.rs`
- Modify: `crates/protocol/src/lib.rs`, `crates/protocol/src/packets.rs` (id nuovi)

Leggere P§1, P§2, P§5.4, P§6, P§7.

**Interfaces:**
- Produces:
  - `reader::Reader<'a>(pub &'a [u8])` con `u8, i8, bool, i16, u16, i32, u32, i64, f32, var_u32, var_i32, var_u64, var_i64, string, bytes, block_pos -> [i32; 3], vec3 -> [f32; 3], optional<T>(f) -> Option<Option<T>>`, tutti `-> Option<_>` (None = finiti i byte).
  - `item::ItemStack { pub id: i16, pub count: u16, pub aux: u32, pub net_id: Option<i32>, pub block_runtime_id: u32, pub user_data: Vec<u8> }`
    (forma `NetworkItemStackDescriptor`, P§2.3) con `ItemStack::air()`, `fn encode(&self, w: &mut Writer)`,
    `fn decode(r: &mut Reader) -> Option<Self>`, `fn empty_user_data() -> Vec<u8>` (10 byte: i16 0, u32 0, u32 0) e
    `fn user_data_with_nbt(nbt_le: &[u8]) -> Vec<u8>`.
  - Encoder server→client (generati se possibile, a mano in `packets.rs` se il generatore non ce la fa): `CreativeContent`
    (categoria **u8**), `InventoryContent`, `InventorySlot`, `MobEquipment`, `PlayerHotbar`, `UpdateBlock`,
    `SetPlayerGameType`, `UpdatePlayerGameType`, `UpdateAbilities`, `LevelEvent`, `LevelSoundEvent`, `ItemStackResponse`
    (risultato **u8**), `BlockActorData` (id 56: posizione + NBT di rete). Nomi dei campi = quelli del generatore.
  - `id::` per tutti i pacchetti nuovi (P§7: 21, 25, 30, 31, 36, 48, 49, 50, 56, 62, 123, 145, 147, 148, 151, 187).

- [ ] **Step 1: Test che falliscono** in `item.rs`: `air_is_eight_zero_bytes` (encode di `ItemStack::air()` = `[0;8]`,
  decode inverso), `stone_round_trip` (id 1, count 64, net_id Some(5), block rt id = hash di stone, user data 10 byte),
  `decode_rejects_truncated`. In `generated.rs`/test del protocollo: `creative_category_is_one_byte` (un gruppo con
  categoria 1 e nome "a" codifica la categoria come singolo byte `01`), `item_stack_response_result_is_one_byte`.
- [ ] **Step 2: Correggere `protocolgen.py`** e rigenerare i pacchetti elencati (comando in testa allo script, schemi in
  `vendor/blackstone-protocol-docs/json`). Per `NetworkItemStackDescriptor` il generatore può usare `item::ItemStack` come
  tipo esterno.
- [ ] **Step 3: Implementare `reader.rs` e `item.rs`**; i test di Step 1 e quelli esistenti passano.
- [ ] **Step 4:** `cargo test -p blackstone-protocol` verde; commit `feat(protocol): ItemStack, Reader e pacchetti di inventario e blocchi`.

### Task 3: Decoder client→server: input, transazioni, richieste di inventario

**Files:**
- Create: `crates/protocol/src/input.rs`
- Modify: `crates/protocol/src/packets.rs` (sostituire il vecchio `PlayerAuthInput::decode`, che si sposta qui)

Leggere P§3, P§4, P§5.1-5.2, P§7 (PlayerAction, MobEquipment).

**Interfaces:**
- Produces (in `input.rs`, tutti `fn decode(body: &[u8]) -> Option<Self>`):
  - `PlayerAuthInput { pitch, yaw, position: [f32; 3], move_vector: [f32; 2], head_yaw, flags: u128, input_mode: u32, play_mode: u32, interaction_model: i32, interact_rotation: [f32; 2], tick: u64, delta: [f32; 3], item_use: Option<UseItem>, stack_request: Option<StackRequest>, block_actions: Vec<BlockAction> }`
    con `fn has(&self, flag: u32) -> bool` e costanti `flag::JUMPING = 6, SNEAKING = 8, START_SNEAKING = 27, STOP_SNEAKING = 28, PERFORM_ITEM_INTERACTION = 34, PERFORM_BLOCK_ACTIONS = 35, PERFORM_ITEM_STACK_REQUEST = 36`.
    I campi dopo `block_actions` si leggono ma non si conservano.
  - `UseItem { action: i32, trigger: u8, pos: [i32; 3], face: u8, hotbar_slot: i32, hand: u8, held: ItemStack, player_pos: [f32; 3], click_pos: [f32; 3], block_runtime_id: u32, prediction: u8, cooldown: u8 }`, `use_item::{PLACE = 0, USE = 1, DESTROY = 2}`.
  - `InventoryTransaction { legacy_request_id: i32, kind: u32, use_item: Option<UseItem> }` (altri tipi: `use_item = None`).
  - `BlockAction { action: i32, pos: [i32; 3], face: i32 }`; `PlayerAction { runtime_id: u64, action: i32, pos: [i32; 3], result_pos: [i32; 3], face: i32 }`; `player_action::{START_BREAK = 0, ABORT_BREAK = 1, STOP_BREAK = 2, CREATIVE_DESTROY = 13, CRACK = 18, PREDICT_DESTROY = 26, CONTINUE_DESTROY = 27}`.
  - `SlotRef { container: u8, dynamic_id: Option<u32>, slot: u8, stack_id: i32 }`;
    `StackAction { Take { count: u8, src: SlotRef, dst: SlotRef }, Place { count: u8, src: SlotRef, dst: SlotRef }, Swap { src: SlotRef, dst: SlotRef }, Drop { count: u8, src: SlotRef, randomly: bool }, Destroy { count: u8, src: SlotRef }, CraftCreative { net_id: u32, crafts: u8 }, Other(u8) }`;
    `StackRequest { request_id: i32, actions: Vec<StackAction> }`; `ItemStackRequestPacket { requests: Vec<StackRequest> }`.
    Un'azione `Other` con payload sconosciuto rende illeggibile il resto della richiesta: la richiesta si scarta intera.
  - `MobEquipment { runtime_id: u64, item: ItemStack, slot: u8, selected_slot: u8, container_id: u8 }` (anche `encode`, se non fatto nel Task 2).

- [ ] **Step 1: Test che falliscono:**
  - `inventory_transaction_real_capture`: i 53 byte di P§4.4 → `kind = 2`, `use_item.action = 0`, `pos = [-155, 72, -2]`,
    `face = 2`, `held = ItemStack::air()`, `block_runtime_id = 0x553014dd`, `prediction = 1`.
  - `player_auth_input_minimal`: corpo costruito nel test con il `Writer` (pitch 0, yaw 90, posizione (1, 2, 3), lista
    flag vuota, nessun opzionale) → `flags == 0`, `item_use None`, `block_actions` vuoto. **Review Focus 1.**
  - `player_auth_input_with_block_actions`: flag 35 attivo e due azioni (0 e 2) con posizione e faccia → lette entrambe.
  - `player_auth_input_with_creative_request`: flag 36 e una richiesta `[CraftCreative{net_id 7, crafts 1}, Place{64, (60, None, 50, id), (28, None, 0, 0)}]` → decodificata.
  - `truncated_bodies_return_none`: ogni prefisso dei corpi sopra restituisce `None` senza panico.
  - `item_stack_request_with_zero_actions_is_empty`: richiesta con 0 azioni → `actions` vuoto (**Review Focus 1**).
- [ ] **Step 2: Implementare `input.rs`** secondo P§3-5 (presenza opzionali a 1 byte; flag = lista di id varint).
- [ ] **Step 3:** `cargo test -p blackstone-protocol` verde; commit `feat(protocol): decoder di input, transazioni e richieste di inventario`.

### Task 4: Inventario, menu creativo e modalità di gioco al login

**Files:**
- Create: `crates/server/src/inventory.rs`
- Modify: `crates/server/src/lib.rs` (`Player`: inventario, modalità), `crates/server/src/join.rs`, `crates/server/src/handlers.rs`

Leggere P§5.3-5.4, P§6, P§7.2, D§8.2.

**Interfaces:**
- Consumes: `registry::{items, creative}`, `item::ItemStack`, decoder del Task 3, encoder del Task 2.
- Produces:
  - `inventory::Item { pub id: i16, pub count: u16, pub aux: u32, pub block: Option<i32>, pub nbt: Option<Vec<u8>> }` con
    `fn is_empty(&self) -> bool`, `fn stack(&self, net_id: i32) -> ItemStack`, `fn same_kind(&self, other: &Item) -> bool` (id, aux, block).
  - `inventory::Inventory` con 36 caselle (0-8 barra), mano secondaria, 4 di armatura, cursore, casella creata (slot 50
    di `CreatedOutput`), `selected: u8`; `fn held(&self) -> &Item`, `fn handle(&mut self, req: &StackRequest, creative: bool) -> StackResult`
    dove `StackResult { ok: bool, changed: Vec<(u8 /*container*/, u8 /*slot*/)> }` (in errore lo stato torna com'era).
  - `GameMode { Survival = 0, Creative = 1, Adventure = 2, Spectator = 6 }` in `lib.rs`, campo `Player::mode` (default Creative).
  - `Server::send_inventory(pid)` (InventoryContent di inventario, mano secondaria, armatura) e `Server::send_abilities(pid)`
    (`UpdateAbilities` layer Base con i bit di P§7.2 per la modalità; `SetPlayerGameType`).
- [ ] **Step 1: Test che falliscono** (unità in `inventory.rs`):
  - `craft_creative_then_place_into_hotbar`: `CraftCreative{net_id di minecraft:stone}` + `Place{64, created 50 → barra 0}` → barra 0 = 64 stone, `ok`.
  - `swap_and_destroy`, `take_more_than_present_fails_and_rolls_back`, `craft_creative_refused_in_survival`.
  - `stack_id_mismatch_is_rejected` (id di pila diverso da quello del server → errore, nessuna modifica).
- [ ] **Step 2: Implementare `inventory.rs`.** ID di pila dal contatore del server (positivi, mai 0 per pile piene); gli
  ID negativi nelle richieste si risolvono con le modifiche della stessa richiesta o di quelle precedenti non ancora
  confermate (D§8.2).
- [ ] **Step 3: Login.** `join.rs` manda `CreativeContent` pieno (net id = indice + 1; icone non aria; oggetti con NBT →
  `user_data_with_nbt`, gli altri `empty_user_data`), `InventoryContent`, `PlayerHotbar`, `SetPlayerGameType` +
  `UpdateAbilities`; `StartGame.game_type` = modalità del giocatore. Test di integrazione in `crates/server/src/tests.rs`
  `join_sends_full_creative_menu`: dopo il login il client finto riceve `CreativeContent` con tante voci quante
  `registry::creative().1`.
- [ ] **Step 4: Gestori.** `ItemStackRequest` (147) e quello dentro PAI (flag 36) → `Inventory::handle` → `ItemStackResponse`
  (ok con le caselle cambiate, o errore + `InventorySlot` delle caselle toccate). `MobEquipment` dal client → `selected`
  (se lo slot è 0-8, altrimenti si ignora). Log esadecimale (al più 3 per giocatore) delle prime `ItemStackRequest` e dei
  primi PAI con flag 34/35/36, per verificare nel gioco le codifiche non sicure (P§9).
- [ ] **Step 5:** test verdi; commit `feat(server): inventario creativo, menu pieno e modalità al login`.

> Punto di prova nel gioco (facoltativo, non blocca): menu creativo pieno, oggetti nella barra, nessuna disconnessione.

### Task 5: Modifica dei blocchi, eventi annullabili, salvataggio automatico

**Files:**
- Modify: `crates/server/src/world.rs` (`Chunk::block`, `Chunk::set_block`)
- Create: `crates/server/src/events.rs`
- Modify: `crates/server/src/lib.rs` (`Server::block_at`, `Server::set_blocks`, autosalvataggio nel tick)

**Interfaces:**
- Produces:
  - `Chunk::block(&self, lx: usize, y: i32, lz: usize) -> i32` (hash; aria se sezione assente) e
    `Chunk::water(&self, lx, y, lz) -> bool`; `Chunk::set_block(&mut self, lx, y, lz, hash: i32, water: bool)` (crea la
    sezione se manca, aggiunge alla palette, aggiorna la maschera dell'acqua, `dirty = true`).
  - `Server::block_at(&self, dim: Dim, pos: [i32; 3]) -> Option<(i32, bool)>` (`None` se chunk non in memoria o y fuori dal mondo).
  - `Server::set_blocks(&mut self, dim: Dim, changes: &[BlockChange]) ` con `BlockChange { pos: [i32; 3], hash: i32, water: bool }`:
    applica, manda `UpdateBlock` (strato 0, flag 3; strato 1 acqua o aria) a ogni giocatore con il chunk in `sent`.
  - `Server::resend_blocks(&self, pid: u64, dim: Dim, positions: &[[i32; 3]])` (stato vero al solo giocatore).
  - `events::WorldAction { Break { pid: u64, pos: [i32; 3], old: i32 }, Place { pid: u64, changes: Vec<BlockChange> }, Interact { pid: u64, changes: Vec<BlockChange> } }`
    e `Server::hooks: Vec<Box<dyn FnMut(&WorldAction) -> bool + Send>>` con `Server::allow(&mut self, a: &WorldAction) -> bool` (false se un hook annulla).
- [ ] **Step 1: Test che falliscono:** `set_block_in_missing_section_creates_it`, `set_block_grows_palette_and_bits`
  (da 1 a 17 voci: i byte di rete restano decodificabili), `waterlogged_block_keeps_water`,
  `set_blocks_sends_update_only_to_viewers` (due giocatori, uno solo ha il chunk in `sent`),
  `block_at_out_of_world_or_unloaded_is_none` (**Review Focus 2**), `cancelled_action_changes_nothing`.
- [ ] **Step 2: Implementare.** Autosalvataggio: ogni 1200 tick (60 s) i chunk `dirty` vanno al disco e tornano puliti.
  Chunk del mondo piatto di ripiego (`self.flat`): mai salvati; al primo cambiamento un log
  `[mondo] mondo piatto di ripiego: le modifiche non si salvano`.
- [ ] **Step 3:** test verdi; commit `feat(server): modifica dei blocchi, eventi annullabili e salvataggio automatico`.

### Task 6: Rottura dei blocchi

**Files:**
- Create: `crates/server/src/breaking.rs`
- Modify: `crates/server/src/handlers.rs`, `crates/server/src/join.rs` (`server_authoritative_block_breaking = false`, vedi Decisioni 2)

Leggere P§8, D§6, D§7.

**Interfaces:**
- Consumes: `Server::{block_at, set_blocks, resend_blocks, allow}`, decoder del Task 3, `Player::mode`, posizione occhi.
- Produces: `Server::break_block(&mut self, pid: u64, pos: [i32; 3])`; `fn in_reach(eyes: [f32; 3], pos: [i32; 3], creative: bool) -> bool`.
- [ ] **Step 1: Test che falliscono:** `creative_break_sets_air_and_notifies` (PlayerAction 13), `break_via_use_item_destroy`
  (PAI flag 34 con `UseItem` azione 2), `break_out_of_reach_is_reverted` (`UpdateBlock` con lo stato vero al solo autore),
  `spectator_and_adventure_cannot_break`, `breaking_waterlogged_kelp_leaves_water`,
  `double_break_same_tick_happens_once` (**Review Focus 3**: PlayerAction 13 + UseItem Destroy sulla stessa posizione nello
  stesso tick → un solo `WorldAction::Break`), `break_air_does_nothing`.
- [ ] **Step 2: Implementare.** Percorsi: `PlayerAction` 13, azioni PAI `STOP_BREAK`/`PREDICT_DESTROY` e `UseItem` DESTROY
  (in PAI o in `InventoryTransaction`). Effetti: `LevelEvent` 2001 (data = hash del blocco rotto) a chi vede il chunk.
  Sopravvivenza in questa tappa: rottura istantanea, senza drop.
- [ ] **Step 3:** test verdi; commit `feat(server): rottura dei blocchi decisa e controllata dal server`.

> Punto di prova nel gioco (facoltativo): rompere in creativo; il blocco resta rotto dopo il riavvio.

### Task 7: Piazzamento: regole generali per stato

**Files:**
- Create: `crates/server/src/placement/mod.rs`, `crates/server/src/placement/rules.rs`
- Modify: `crates/server/src/handlers.rs` (InventoryTransaction → piazzamento)

Leggere D§2, D§3, D§4.7, D§5 (ramo generico), D§6.

**Interfaces:**
- Consumes: `registry::{blocks, BlockState, StateValue}`, `Server::{block_at, set_blocks, resend_blocks, allow}`, `Inventory::held`.
- Produces:
  - `placement::PlaceCtx { pub clicked: [i32; 3], pub face: u8, pub click: [f32; 3], pub yaw: f32, pub feet: [f32; 3], pub block: i32, pub sneaking: bool }`
  - `pub(crate) trait BlockView { fn block(&self, pos: [i32; 3]) -> Option<i32>; }` (implementato da `Server` per la dimensione del giocatore).
  - `placement::place(ctx: &PlaceCtx, view: &impl BlockView) -> Option<Vec<BlockChange>>` (`None` = rifiutato).
  - `placement::replaceable(hash: i32) -> bool` (D§4.7: aria, acqua, lava, erba corta e alta, felce, cespuglio secco,
    fuoco, luce, nether sprouts, rampicanti, red_shrub).
  - `rules::orient(state: &BlockState, ctx: &PlaceCtx) -> BlockState`: regola base per stato + eccezioni per famiglia (tabella D§3).
- [ ] **Step 1: Test che falliscono** (`rules.rs`, giocatore che guarda verso sud = yaw 0, ovest = 90, est = -90, nord = 180):
  - `log_axis_from_face`: tronco cliccato sulla faccia est → `pillar_axis = "x"`; sopra → `"y"`.
  - `stairs_face_player_and_flip`: giocatore verso nord → `weirdo_direction = 3`; clic con y del punto 0.7 su faccia laterale → `upside_down_bit = 1`.
  - `slab_top_half`: clic sulla faccia inferiore → `minecraft:vertical_half = "top"`.
  - `chest_and_furnace_face_player`: giocatore verso sud → `minecraft:cardinal_direction = "north"`.
  - `trapdoor_direction`: giocatore verso sud → `direction = 3`.
  - `glazed_terracotta_and_ladder_and_wall_sign` secondo D§3.
  - `every_rule_yields_a_valid_state`: per ogni blocco del registro e per 4 direzioni × 6 facce, `blocks().is_valid(&orient(...))`.
  E in `mod.rs`: `place_on_replaceable_grass_goes_in_place`, `place_into_occupied_is_refused`,
  `place_inside_player_is_refused` (bbox giocatore 0.6 × 1.8 dai piedi), `place_in_water_waterlogs`,
  `unknown_held_item_is_refused` (**Review Focus 4**).
- [ ] **Step 2: Implementare** regole e piazzamento generico. Gestore: `InventoryTransaction` tipo 2 azione PLACE →
  controlli (portata, modalità, oggetto in mano del server uguale a `held` del client con `Item::same_kind`, oggetto con
  blocco) → `place` → `allow` → `set_blocks` + `LevelSoundEvent "place"` (data = hash). Rifiuto → `resend_blocks`
  (cliccato e vicino sulla faccia). In creativo l'oggetto non si consuma.
- [ ] **Step 3:** test verdi; commit `feat(server): piazzamento con regole di orientamento per stato`.

> Punto di prova nel gioco (facoltativo): scale, tronchi, mezzi blocchi e bauli orientati come in vanilla.

### Task 8: Piazzamento: casi speciali e aggiornamento dei vicini

**Files:**
- Create: `crates/server/src/placement/special.rs`, `crates/server/src/placement/neighbors.rs`
- Modify: `crates/server/src/placement/mod.rs`, `crates/server/src/lib.rs` (dopo ogni `set_blocks` si applicano gli aggiornamenti dei vicini)

Leggere D§4 (tutto), D§3 (`minecraft:corner`).

**Interfaces:**
- Produces: `special::place_special(ctx, view) -> Option<Option<Vec<BlockChange>>>` (`None` = non è un caso speciale,
  si usa il ramo generico); `neighbors::updates(changed: &[[i32; 3]], view) -> Vec<BlockChange>` (blocchi che devono
  cambiare o sparire perché è cambiato un vicino; applicati fino a 2 giri).
- [ ] **Step 1: Test che falliscono:** `door_places_two_halves_with_hinge`, `door_needs_solid_floor_and_free_top`,
  `bed_places_head_and_foot` (+ `BlockActorData` con il colore dall'aux dell'oggetto), `slab_on_same_slab_becomes_double`,
  `torch_on_wall_and_floor_refused_on_ceiling`, `lever_on_ceiling_axis`, `lantern_hanging_from_block`,
  `button_attaches_to_face`, `wall_connects_to_neighbors_and_post`, `stairs_corner_outer_left`,
  `flower_needs_soil`, `sugar_cane_on_sand`, `ladder_needs_wall`, `torch_breaks_when_support_removed`,
  `door_upper_half_removed_with_lower`, `wall_updates_when_neighbor_placed`.
- [ ] **Step 2: Implementare** traducendo da Dragonfly i file citati in D§4; pulsanti con `facing_direction` come le
  leve (faccia cliccata) e supporto solido. Nessun drop quando un blocco sparisce per mancanza di appoggio (tappa 1).
- [ ] **Step 3:** test verdi; commit `feat(server): casi speciali di piazzamento e aggiornamento dei vicini`.

### Task 9: Interazioni col tasto destro

**Files:**
- Create: `crates/server/src/interact.rs`
- Modify: `crates/server/src/handlers.rs`, `crates/server/src/lib.rs` (timer dei pulsanti nel tick)

Leggere D§5.

**Interfaces:**
- Produces: `interact::activate(state: &BlockState, pos: [i32; 3], ctx: &PlaceCtx, view) -> Option<Vec<BlockChange>>`
  (`None` = blocco non interattivo); `Server::schedule(&mut self, due_tick: u64, dim: Dim, pos: [i32; 3], expect: i32, then: i32)`.
- [ ] **Step 1: Test che falliscono:** `door_toggles_both_halves`, `iron_door_and_iron_trapdoor_ignore_hand`,
  `trapdoor_and_fence_gate_toggle` (cancelletto aperto dal lato opposto inverte la direzione), `lever_toggles`,
  `wooden_button_resets_after_30_ticks`, `stone_button_resets_after_20_ticks`,
  `button_reset_skipped_if_block_changed`, `sneaking_with_block_places_instead_of_opening`.
- [ ] **Step 2: Implementare.** Ordine del gestore `PLACE`: se il blocco cliccato è interattivo e (non accovacciato o mano
  vuota) → `activate` → `allow(Interact)` → `set_blocks` + suono; altrimenti piazzamento (Task 7/8).
- [ ] **Step 3:** test verdi; commit `feat(server): porte, botole, cancelletti, leve e pulsanti`.

### Task 10: Dati del giocatore salvati nel mondo

**Files:**
- Create: `crates/server/src/playerdata.rs`
- Modify: `crates/server/src/storage.rs`, `crates/server/src/disk.rs`, `crates/server/src/handlers.rs`, `crates/server/src/join.rs`, `crates/server/src/lib.rs`

Leggere DT§5.

**Interfaces:**
- Produces:
  - `playerdata::PlayerData { pub feet: [f32; 3], pub yaw: f32, pub pitch: f32, pub dim: i32, pub mode: GameMode, pub inventory: Inventory }`
    con `fn to_nbt(&self) -> Vec<u8>` e `fn from_nbt(b: &[u8]) -> Option<Self>` (campi: `Pos` piedi, `Rotation`,
    `DimensionId`, `PlayerGameType` (si legge anche `PlayerGameMode`), `Inventory` con `Slot`/`Name`/`Count`/`Damage`/`Block`/`tag`,
    `Offhand`, `Armor`, `SelectedInventorySlot`).
  - `playerdata::identity(info: &PlayerInfo) -> String` (XUID se c'è, altrimenti `name:<nome>`).
  - `World::load_player(&mut self, identity: &str) -> Option<Vec<u8>>`, `World::save_player(&mut self, identity: &str, nbt: &[u8]) -> io::Result<()>`
    (chiave `player_<identity>` con `{MsaId, SelfSignedId, ServerId}`, `ServerId = "player_server_<uuid>"` generato con
    `getrandom` la prima volta; dati in `player_server_<uuid>`).
  - `Disk::{load_player, save_player}` e `DiskEvent::PlayerLoaded(u64 /*pid*/, Option<Vec<u8>>)`.
- [ ] **Step 1: Test che falliscono:** `player_data_round_trip` (posizione, modalità survival, 3 caselle piene, casella
  selezionata 4), `save_load_player_in_world` (chiavi `player_*` e `player_server_*` presenti, rilettura uguale),
  `same_identity_reuses_server_id`, `login_waits_for_player_data_before_start_game` (**Review Focus 5**: con il disco
  lento StartGame parte solo dopo `PlayerLoaded`; la posizione salvata vale al posto dello spawn),
  `quit_before_data_loaded_does_not_overwrite` (**Review Focus 5**: uscita prima del caricamento → nessun salvataggio).
- [ ] **Step 2: Implementare.** Lettura dopo il login (prima di StartGame), salvataggio all'uscita e alla chiusura del
  server (prima del flush). Senza mondo su disco (test, `run`) si parte vuoti e non si salva.
- [ ] **Step 3:** test verdi; commit `feat(server): dati del giocatore salvati nel mondo in formato Bedrock`.

### Task 11: `/gamemode`

**Files:**
- Modify: `crates/server/src/chat.rs`, `crates/server/src/handlers.rs`

**Interfaces:**
- Consumes: `Server::send_abilities`, `Server::send_inventory`, `GameMode`.
- [ ] **Step 1: Test che falliscono:** `gamemode_survival_updates_client` (risposta con `SetPlayerGameType` 0 e
  `UpdateAbilities` senza volo né costruzione istantanea), `gamemode_accepts_names_numbers_and_short_forms`
  (`creative`, `c`, `1`, `spectator`, `sp`, `6`…), `gamemode_unknown_shows_usage`, `gamemode_is_listed_in_help`.
  Il pacchetto `SetPlayerGameType` mandato dal client (cambio dal menu) → si risponde con `UpdatePlayerGameType` della
  modalità vera (il client non cambia modalità da solo).
- [ ] **Step 2: Implementare**; la modalità si salva con i dati del giocatore (Task 10).
- [ ] **Step 3:** test verdi; commit `feat(server): comando /gamemode`.

### Task 12: Verifica d'insieme e lista per la prova nel gioco

**Files:**
- Create: `docs/superpowers/notes/2026-11-19-verifica-tappa1.md`
- Modify: `README.md` (sezione comandi e cosa funziona)

- [ ] **Step 1:** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`: tutti puliti.
- [ ] **Step 2: Prova senza client:** avviare il server con il generatore (`cargo run`) e il test end-to-end di join
  esistente esteso: il client finto riceve menu creativo, inventario e abilità.
- [ ] **Step 3: Lista per l'utente** in `2026-11-19-verifica-tappa1.md`: cosa provare nel gioco (menu, barra, rompere,
  piazzare scale/tronchi/porte/letti/muri, interazioni, uscita e rientro, `/gamemode`), quali righe di log controllare
  (esadecimali delle prime richieste) e gli interruttori di diagnostica.
- [ ] **Step 4:** commit `docs: verifica della tappa 1 e README`.
