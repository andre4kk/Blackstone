# Tappa 3 "sopravvivenza base" Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In sopravvivenza i blocchi rotti lasciano oggetti, il giocatore ha vita e fame, prende danno (caduta, vuoto, colpi), mangia, muore e rinasce.

**Architecture:** Vita, fame e caduta vivono nel `Player` (`crates/server/src/lib.rs`) e si salvano in `playerdata`; la logica sta in un modulo nuovo `crates/server/src/survival.rs` (danno, cura, fame, morte, rinascita) chiamato dai gestori dei pacchetti e da `Server::tick`. Le tabelle (oggetti lasciati dai blocchi, strumenti, cibi, danni delle armi) sono JSON in `crates/server/data/` prodotti da script Python che leggono Dragonfly. I pacchetti nuovi stanno in `crates/protocol` (scritti a mano dove serve, byte da gophertunnel).

**Tech Stack:** Rust (workspace `crates/protocol`, `crates/server`), Python per gli script in `tools/`, protocollo Bedrock 2225, test con `Harness` (`crates/server/src/tests.rs`).

**Spec:** `docs/superpowers/specs/2026-11-19-sopravvivenza-design.md`

## Global Constraints

- Protocollo 2225 (client 1.26.6x). **Per i byte vale gophertunnel v1.63.0** (`vendor/gophertunnel/minecraft/protocol` e `.../packet`), non gli schemi Mojang (hanno sbagliato flag e skin nella tappa 2). Dragonfly 1.26.60 (`vendor/dragonfly`) per comportamento, numeri e ordine dei pacchetti.
- Varianti con etichetta = indice di variante varuint32, poi il corpo (vedi `crates/protocol/src/actor.rs` e `entity.rs`). Flag delle entità dopo CAN_DASH(46) spostati di uno.
- Enum sul filo: mai mandare un valore fuori dall'enum (la tappa 2 è crollata su `BuildPlatform` 0). Ogni campo enum nuovo ha un test che ne fissa il valore.
- `generated.rs` non si modifica a mano: si rigenera con `python tools/protocolgen.py vendor/blackstone-protocol-docs/json crates/protocol/src/generated.rs <elenco>` (elenco nel README, "Pacchetti generati"); se lo schema generato differisce da gophertunnel, scrivere il pacchetto a mano in `crates/protocol/src/` e dirlo nel commento.
- Codifiche non confermabili dai test: log esadecimale per le prime `HEX_LOGS` (3) volte per giocatore e voce nella lista di verifica (Task 9).
- Ogni azione nuova su mondo o giocatore passa da `Server::allow(&WorldAction)`; se un gancio dice no, nulla cambia.
- Creativo: nessun danno tranne il vuoto, nessuna fame, nessun oggetto dai blocchi. Spettatore: nessun danno. Sopravvivenza e avventura: tutto attivo.
- Ciclo unico a 20 Hz (`Server::tick`), nessun thread nuovo. Commenti e log in italiano; rustfmt 160; `cargo clippy --workspace --all-targets -- -D warnings` pulito; `cargo test --workspace` verde (se `target/debug/blackstone-server.exe` è bloccato dal server dell'utente, almeno `cargo test -p blackstone-protocol` e `cargo test -p blackstone-server --lib`, e dirlo nel report).
- Commit che finiscono con `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` (o il modello che lavora). Nessun push.
- Licenze: server MIT; tabelle e numeri da Dragonfly (MIT) citati nei commenti; nessun codice dal generatore AGPL.

## Review Focus

1. Morte con l'inventario pieno → gli oggetti cadono a terra una volta sola e l'inventario è vuoto (anche nei dati salvati); con `keep_inventory = true` restano. (Test nel Task 5.)
2. Il giocatore esce e rientra con 7 punti vita e fame 12 → li ritrova; un giocatore morto che esce prima di rinascere rientra vivo al punto di partenza, non bloccato nella schermata di morte. (Test nei Task 3 e 5.)
3. Volo in creativo a 100 blocchi, cambio a sopravvivenza a mezz'aria e atterraggio → il danno conta solo dalla quota in cui è iniziata la caduta in sopravvivenza (la caduta si azzera finché si vola o si è in creativo). (Test nel Task 4.)
4. Due colpi nello stesso tick o entro l'invulnerabilità → un solo danno (vanilla: un colpo più forte durante l'invulnerabilità fa solo la differenza). (Test nel Task 3.)
5. Blocco rotto senza lo strumento giusto (pietra a mano, ferro col piccone di legno) → nessun oggetto; con lo strumento giusto esattamente gli oggetti della tabella. (Test nel Task 2.)

---

### Task 1: Tabelle da Dragonfly (blocchi, strumenti, cibi, armi)

**Files:**
- Create: `tools/extract_survival.py`, `crates/server/data/block_drops.json`, `crates/server/data/tools.json`, `crates/server/data/foods.json`
- Create: `crates/server/src/tables.rs` (caricamento con `include_str!` + `serde_json`, `OnceLock`)
- Modify: `crates/server/src/lib.rs` (mod), README (sezione dati)

**Interfaces:**
- Produces (in `crate::tables`):
  - `pub(crate) enum ToolKind { Pickaxe, Axe, Shovel, Hoe, Sword, Shears }` e `pub(crate) struct ToolInfo { pub kind: ToolKind, pub tier: u8 /* 0 nessuno, 1 legno/oro, 2 pietra, 3 ferro, 4 diamante, 5 netherite: livello di raccolta */, pub attack_damage: f32 }`
  - `pub(crate) fn tool(item_name: &str) -> Option<&'static ToolInfo>`
  - `pub(crate) struct Drop { pub item: String, pub min: u16, pub max: u16, pub chance: f32 }` e `pub(crate) struct BlockDrops { pub tool: Option<ToolKind>, pub min_tier: u8, pub drops: Vec<Drop> }`
  - `pub(crate) fn block_drops(block_name: &str) -> Option<&'static BlockDrops>` (nome senza stati, es. `minecraft:stone`). `None` = il blocco lascia se stesso (1) senza requisiti.
  - `pub(crate) struct Food { pub points: u8, pub saturation: f32, pub always_edible: bool }` e `pub(crate) fn food(item_name: &str) -> Option<&'static Food>`
  - `pub(crate) fn attack_damage(item_name: &str) -> f32` (1.0 se non è un'arma/strumento)

- [ ] **Step 1:** Scrivere lo script. Legge `vendor/dragonfly/server/block/*.go` (funzioni `BreakInfo`, `newBreakInfo(hardness, harvestable, effective, drops)` con gli aiutanti di `break_info.go`: `oneOf`, `simpleDrops`, `silkTouchOneOf` (si prende il primo, quello senza tocco di seta), `silkTouchOnlyDrop` (nessun oggetto), `oreDrops`, `pickaxeHarvestable`, `alwaysHarvestable`, funzioni inline con `HarvestLevel() >= item.ToolTierX.HarvestLevel`), i nomi Bedrock dai `EncodeBlock`/`EncodeItem`, `vendor/dragonfly/server/item/tool.go` e i file degli strumenti per livelli e `AttackDamage`, e i `Consume` dei cibi (`AddFood(points, saturation)` / `Saturate`). Quel che lo script non capisce va in un dizionario di eccezioni scritto a mano nello script, con un commento per voce; lo script stampa quante voci ha preso e quante eccezioni. Probabilità semplici note (ghiaia → selce 10%, foglie → nulla senza cesoie, erba alta → semi 12,5%) come `chance`.
- [ ] **Step 2: Test che falliscono** in `tables.rs`: `stone_needs_a_pickaxe_and_drops_cobblestone` (`block_drops("minecraft:stone")` = piccone, livello ≥1, `minecraft:cobblestone` x1), `iron_ore_needs_stone_tier` (min_tier 2, `minecraft:raw_iron`), `grass_block_drops_dirt_by_hand`, `dirt_has_no_entry_and_drops_itself` (o voce identica), `glass_drops_nothing`, `diamond_sword_damage` (`attack_damage("minecraft:diamond_sword")` = 8 come Dragonfly), `fist_damage_is_one`, `bread_food` (5 punti, saturazione 6.0), `golden_apple_always_edible`, `wooden_pickaxe_tier_1`.
- [ ] **Step 3:** Generare i JSON con lo script e scrivere `tables.rs`. I test passano.
- [ ] **Step 4:** fmt, clippy, test; commit `feat(server): tabelle di sopravvivenza da Dragonfly`.

### Task 2: I blocchi rotti in sopravvivenza lasciano oggetti

**Files:** Modify `crates/server/src/breaking.rs`, `crates/server/src/items.rs` (se serve un aiutante di spawn), `crates/server/src/tests.rs`.

**Interfaces:**
- Consumes: `tables::{block_drops, tool}`, `Server::spawn_item(item, pos, velocity, pickup_delay) -> u64`, `inventory::Item` (con `Item::from_name(name, count)` o equivalente: se manca, aggiungerlo in `inventory.rs` usando il registro degli oggetti).
- Produces: `pub(crate) fn drops_for(block_name: &str, held: &Item, roll: &mut impl FnMut() -> f32) -> Vec<Item>` in `breaking.rs` (pura, testabile; `roll` dà numeri in [0,1)).

- [ ] **Step 1: Test che falliscono:** `drops_for` (pietra a mano → niente; pietra col piccone di legno → 1 ciottolo; minerale di ferro col piccone di legno → niente, di pietra → ferro grezzo; ghiaia con roll 0.05 → selce, con 0.5 → ghiaia); nel `Harness`: un giocatore in sopravvivenza rompe la pietra col piccone → nasce un oggetto a terra al centro del blocco (pos + 0.5) con ciottolo x1; in creativo nessun oggetto; con un gancio che rifiuta `Break` nessun oggetto; la stessa rottura doppia nello stesso tick → un solo oggetto.
- [ ] **Step 2:** Implementare: dopo una rottura accettata in sopravvivenza/avventura, `drops_for` col nome del blocco rotto (dal registro dei blocchi: hash → nome) e l'oggetto in mano; ogni oggetto nasce al centro del blocco con spinta casuale piccola (come Dragonfly `world.Tx.AddEntity` per i drop: velocità x/z in ±0.1, y 0.1) e ritardo di raccolta 10 tick. Aggiungere esaurimento 0.005 per la rottura solo se il Task 6 c'è già (altrimenti lo fa il Task 6).
- [ ] **Step 3:** fmt, clippy, test; commit.

### Task 3: Vita, danno e cura

**Files:** Create `crates/server/src/survival.rs`; Modify `lib.rs` (campi `Player`), `playerdata.rs` (salvataggio), `events.rs`, `join.rs` (attributi allo spawn con i valori veri), `crates/protocol/src/` (pacchetto `ActorEvent` se manca: in `entity.rs`), `tests.rs`.

**Interfaces:**
- Produces:
  - Campi `Player`: `health: f32` (20), `food: u8` (20), `saturation: f32` (5), `exhaustion: f32` (0), `invulnerable_until: u64` (tick), `last_damage: f32`, `dead: bool`, `fall_start_y: Option<f32>`.
  - `pub(crate) enum DamageCause { Fall, Void, Starvation, Attack { attacker: u64 }, Kill }` in `survival.rs`.
  - `WorldAction::Damage { pid: u64, amount: f32, cause: DamageCause }` (con `DamageCause: Clone + Debug + PartialEq`; `Eq` di `WorldAction` va tolto o `amount` va in millesimi interi: scegliere e dirlo).
  - `impl Server { pub(crate) fn hurt(&mut self, pid: u64, amount: f32, cause: DamageCause) -> bool; pub(crate) fn heal(&mut self, pid: u64, amount: f32); pub(crate) fn send_vitals(&self, pid: u64) }` — `send_vitals` manda `UpdateAttributes` con vita, fame, saturazione, esaurimento (stessi nomi e massimi di `join::spawn_attributes`).
  - `blackstone_protocol::entity::actor_event(runtime_id: u64, event: u8, data: i32) -> Packet` (ActorEvent: byte da gophertunnel `packet/actor_event.go`; HURT e DEATH dai valori di gophertunnel).
  - `PlayerData` aggiunge `health`, `food`, `saturation`, `exhaustion` (tag NBT come i mondi vanilla se Dragonfly/vanilla li nomina, altrimenti nomi propri documentati); valori mancanti → pieni.
- [ ] **Step 1: Test che falliscono:** `hurt` in sopravvivenza toglie vita, manda `UpdateAttributes` a sé e `ActorEvent` HURT a sé e agli spettatori; secondo colpo entro 10 tick più debole → nessun danno, più forte → solo la differenza (Dragonfly `Hurt`); creativo/spettatore → nessun danno (il vuoto sì in creativo); gancio che rifiuta → nulla; vita salvata e ricaricata (7.0 resta 7.0); dati vecchi senza vita → 20; `heal` non supera 20.
- [ ] **Step 2:** Implementare seguendo `vendor/dragonfly/server/player/player.go` `Hurt` (riga ~625) e `Heal` (~544). A vita ≤ 0 chiamare `self.die(pid, cause)` — nel Task 3 è un segnaposto che mette `dead = true` e logga; il Task 5 lo completa.
- [ ] **Step 3:** fmt, clippy, test; commit.

### Task 4: Danno da caduta e dal vuoto

**Files:** Modify `survival.rs`, `handlers.rs` (`player_auth_input`), `tests.rs`.

**Interfaces:** Consumes `hurt`. Produces `pub(crate) fn fall_tick(&mut self, pid: u64, feet_y: f32, on_ground: bool, in_water: bool)` (o equivalente) e un controllo del vuoto in `Server::tick`.

- [ ] **Step 1: Test che falliscono:** caduta da 10 blocchi in sopravvivenza (sequenza di `PlayerAuthInput` con y che scende e poi a terra) → danno 7 (distanza − 3, come Dragonfly `fall`); da 3 blocchi → 0; atterraggio in acqua → 0; volo in creativo a 100, cambio a sopravvivenza in aria, caduta di 5 → danno 2; planata (flag di planata) → caduta azzerata; sotto il fondo del mondo (y < −64 − 64 come Dragonfly, verificarlo) → 4 di danno ogni 10 tick anche in creativo, non in spettatore.
- [ ] **Step 2:** Implementare. "A terra" dal client: cercare in gophertunnel `PlayerAuthInput` i flag `InputFlagVerticalCollision`/`OnGround`-equivalenti (numeri da `packet/player_auth_input.go`) e usare la stessa idea di Dragonfly (`player.go` `fall` ~574 e il suo chiamante nella sessione). Acqua: il blocco ai piedi è acqua (registro). Logga in esadecimale nulla di nuovo se i flag sono già letti.
- [ ] **Step 3:** fmt, clippy, test; commit.

### Task 5: Morte, rinascita, `/kill`, bot mortali

**Files:** Modify `survival.rs`, `handlers.rs` (pacchetto `Respawn` e `PlayerAction` di rinascita se servono), `chat.rs` (`/kill`), `bot.rs`, `config.rs` + `server.example.toml` (`keep_inventory = false`), `crates/protocol` (`Respawn` se manca), `tests.rs`.

**Interfaces:**
- Produces: `pub(crate) fn die(&mut self, pid: u64, cause: DamageCause)`, `pub(crate) fn respawn(&mut self, pid: u64)`, `WorldAction::Death { pid: u64 }` (annullabile: se un gancio dice no la vita torna a 1? — scelta: la morte non si annulla, il gancio solo osserva; dirlo nel commento), `ServerConfig::keep_inventory: bool`.
- [ ] **Step 1: Test che falliscono:** a vita 0 → messaggio in chat a tutti (testi in italiano per causa: caduta "X è caduto da troppo in alto", vuoto "X è caduto fuori dal mondo", fame "X è morto di fame", attacco "X è stato ucciso da Y", kill "X è morto"), inventario a terra (un oggetto a terra per casella non vuota, inventario vuoto, armatura compresa), `ActorEvent` DEATH agli spettatori, `UpdateAttributes` vita 0 a sé, pacchetto `Respawn` come Dragonfly (`session/player.go` ~87); il client manda `Respawn` con stato ClientReadyToSpawn → vita 20, fame 20, saturazione 5, posizione al punto di partenza del mondo, `MovePlayer`/teletrasporto come fa Dragonfly, corpo ricomparso per gli altri; con `keep_inventory = true` l'inventario resta; un morto che esce e rientra entra vivo al punto di partenza; mentre è morto non prende danno, non si muove e non rompe/piazza; `/kill` uccide chi lo scrive; un bot colpito a morte sparisce (RemoveActor e PlayerList rimossa).
- [ ] **Step 2:** Implementare seguendo Dragonfly `player.go` `kill` (~899) e `Respawn` (~984), `session/handler_respawn.go`. Log esadecimale dei primi `Respawn` ricevuti.
- [ ] **Step 3:** fmt, clippy, test; commit.

### Task 6: Fame

**Files:** Modify `survival.rs`, `handlers.rs`, `breaking.rs`, `tests.rs`.

**Interfaces:** Produces `pub(crate) fn exhaust(&mut self, pid: u64, points: f32)` e `pub(crate) fn hunger_tick(&mut self)` chiamato da `Server::tick`.

- [ ] **Step 1: Test che falliscono** (numeri da `vendor/dragonfly/server/player/hunger.go` e dai chiamanti di `Exhaust` in `player.go`): esaurimento ≥ 4 → −1 saturazione o, a saturazione 0, −1 fame; corsa e salto aggiungono esaurimento (salto 0.05, salto in corsa 0.2, corsa 0.1 per blocco), rottura 0.005, attacco 0.1, danno 0.1; con fame ≥ 18 (e saturazione) si recupera 1 vita ogni 80 tick con esaurimento 6 (o la regola rapida di Dragonfly con fame 20 e saturazione); a fame 0 si perde 1 vita ogni 80 tick fino a 1 (normale); creativo nessun effetto; `UpdateAttributes` solo quando un valore cambia.
- [ ] **Step 2:** Implementare. Il movimento orizzontale per la corsa si misura dalle posizioni di `PlayerAuthInput`; il salto dal flag di inizio salto (numero da gophertunnel).
- [ ] **Step 3:** fmt, clippy, test; commit.

### Task 7: Mangiare

**Files:** Modify `survival.rs`, `handlers.rs`, `crates/protocol/src/input.rs` (se servono letture nuove: rilascio dell'oggetto), `tests.rs`.

**Interfaces:** Consumes `tables::food`. Produces campo `Player::eating_since: Option<u64>` (tick) e `WorldAction::Eat { pid: u64, item: Item }`.

- [ ] **Step 1:** Leggere come Dragonfly gestisce l'uso prolungato e il consumo (`player.go` `UseItem`/`ReleaseItem`/`usingItem`, `session/handler_inventory_transaction.go` ReleaseItem e UseItem ClickAir, e il flag di `PlayerAuthInput` per l'uso dell'oggetto) e scrivere nel report il flusso dei pacchetti scelto.
- [ ] **Step 2: Test che falliscono:** con pane in mano, fame 10, uso iniziato e 32 tick dopo (1,6 s) → fame 15, saturazione +6 (con tetto alla fame), pane −1 nella casella (casella rimandata), `ActorEvent` di fine pasto agli spettatori se Dragonfly lo manda; uso interrotto prima di 32 tick → nulla; fame piena → non si mangia (mela d'oro sì); creativo → si mangia senza consumare (come vanilla) o non si mangia: seguire Dragonfly e dirlo.
- [ ] **Step 3:** Implementare; log esadecimale dei primi pacchetti di uso/rilascio. fmt, clippy, test; commit.

### Task 8: Combattimento corpo a corpo

**Files:** Modify `survival.rs`, `handlers.rs`, `crates/protocol/src/input.rs` (transazione UseItemOnEntity, byte da gophertunnel), `bot.rs`, `config.rs` + `server.example.toml` (`pvp = true`), `tests.rs`.

**Interfaces:** Consumes `hurt`, `tables::attack_damage`, `set_actor_motion`. Produces `WorldAction::Attack { pid: u64, target: u64 }`, `ServerConfig::pvp: bool`, vita dei bot (`Bot::health`).

- [ ] **Step 1: Test che falliscono:** transazione di attacco (byte costruiti come gophertunnel `UseItemOnEntityTransactionData`) su un giocatore a 2 blocchi → danno pari all'arma in mano (spada di diamante 8, mano 1), spinta (`SetActorMotion` alla vittima, forza e altezza da Dragonfly `knockBack`), `animate`/HURT agli spettatori, esaurimento 0.1 all'attaccante; bersaglio oltre la portata (Dragonfly: controlla la distanza; usare il suo valore) → nulla; `pvp = false` → nulla tra giocatori, i bot si colpiscono sempre; attaccante morto o spettatore → nulla; bersaglio in creativo → nessun danno; bot colpito 3 volte con la spada di diamante → muore e sparisce; il messaggio di morte dice chi ha ucciso.
- [ ] **Step 2:** Implementare seguendo Dragonfly `AttackEntity` (~1869) e `knockBack` (~771). Log esadecimale delle prime transazioni di attacco.
- [ ] **Step 3:** fmt, clippy, test; commit.

### Task 9: Documentazione e lista di verifica nel gioco

**Files:** Create `docs/superpowers/notes/2026-11-20-verifica-tappa3.md`; Modify `README.md`.

- [ ] **Step 1:** Lista di verifica per l'utente (italiano semplice, passi numerati): rompere pietra a mano e col piccone, minerali, ghiaia; caduta da 5 e da 20 blocchi; acqua; vuoto; fame (correre a lungo), mangiare pane; `/kill` e rinascita; morte con inventario pieno; uscire e rientrare ferito; colpire il bot fino a ucciderlo; PvP spento in config. Per ogni codifica loggata in esadecimale: cosa cercare nel log. Elenco delle "Scelte mie" del design da confermare.
- [ ] **Step 2:** README: sezione sopravvivenza (cosa c'è, cosa manca: crafting, armature, usura, mob), opzioni nuove della config. Commit.
