# bedrock-server

Server Minecraft Bedrock in Rust, su trasporto NetherNet (WebRTC), quello usato dalle versioni 26.60+.
Il mondo è un Overworld infinito, generato da un processo separato (`versmc-gen`, basato su SteelMC) con le stesse regole di
Java 26.3, e salvato su disco nel formato LevelDB di Bedrock.

## Architettura

```
crates/
  protocol/  bedrock-protocol  Pacchetti, batch, compressione, lettura del login. Nessun I/O: solo byte <-> struct.
                               Qui vivono le costanti di versione (PROTOCOL, VERSION).
  net/       bedrock-net       Trasporto: signaling HTTP, identità, WebRTC. Espone Event::{Open,Message,Close} e
                               Sender::{send,kick}. Non sa cosa c'è nei messaggi.
  server/    bedrock-server    Logica di gioco: un handler per pacchetto (handlers.rs). Contiene l'eseguibile.
  vgen/      vgen              Protocollo locale (TCP, solo std) tra il server e il generatore: Hello, palette dei blocchi,
                               Generate (chunk), Spawn. Licenza MIT: lo usano entrambi i lati.
generator/                      `versmc-gen`: processo che genera i chunk (SteelMC) e li traduce in blocchi e biomi Bedrock.
                               Workspace Cargo separato, toolchain nightly, licenza AGPL-3.0-or-later (vedi "Licenze").
```

Dipendenze: `server -> net`, `server -> protocol`, `server -> vgen`, `generator -> vgen`. `net` e `protocol` non si conoscono.
Il server NON dipende dal codice del generatore: lo avvia come programma (`generator` in `server.toml`), legge `READY <porta>`
dal suo stdout, si connette in TCP su localhost e parla `vgen`. Se il generatore cade lo riavvia e rimanda le richieste.

## Compilare ed eseguire

Server (Rust stable):

```
cargo run                 # legge server.toml (lo crea da server.example.toml se manca), TCP+UDP 19132
```

Generatore (cartella `generator/`, Rust nightly scelto da `rust-toolchain.toml`: rustup lo scarica da solo):

```
cd generator
cargo build --release     # produce generator/target/release/versmc-gen(.exe)
```

- La prima compilazione richiede Internet: la build di Steel scarica da Mojang il jar del server Java 26.3 per estrarne i dati.
- Una compilazione release da zero richiede circa 12 minuti (LTO); le successive sono molto più rapide.
- Il server cerca il generatore dove dice `generator` in `server.toml` (default `generator/target/release/versmc-gen`, su Windows
  con `.exe`). Se non lo trova scrive l'errore nel log e usa un mondo piatto di ripiego, senza salvarlo.
- Compilare il generatore NON serve per `cargo test --workspace` alla radice (usa un generatore finto); si prova a parte con
  `cargo test` dentro `generator/`.

## `server.toml`

Va nella cartella da cui si avvia il server. Se manca viene scritto con i valori di default (vedi `server.example.toml`).

| Campo | Default | Significato |
|---|---|---|
| `world` | `"Mondo"` | Nome del mondo; i dati stanno in `worlds/<world>` (LevelDB di Bedrock + `level.dat`). |
| `seed` | casuale | Seme, usato solo quando il mondo viene creato (poi vale quello salvato in `level.dat`). Massimo 9223372036854775807. |
| `generator` | `generator/target/release/versmc-gen` | Programma del generatore (`.exe` aggiunto da solo su Windows). |
| `view_distance` | `6` | Raggio di chunk inviato a ogni giocatore (1-32). |
| `port` | `19132` | Porta TCP (signaling) e UDP (gioco). |

Un `server.toml` non valido viene segnalato nel log e il server parte con i valori di default senza modificarlo.

## Flusso di ingresso (stato)

```
RequestNetworkSettings -> NetworkSettings                          verificato con Minecraft
Login                  -> PlayStatus + ResourcePacksInfo           verificato con Minecraft
ResourcePackClientResponse(DownloadingFinished) -> ResourcePackStack   verificato con Minecraft
ResourcePackClientResponse(StackFinished) -> JigsawStructureData, VoxelShapes, StartGame, ItemRegistry   accettati dal client
RequestChunkRadius -> ChunkRadiusUpdated, PlayStatus(PlayerSpawn), CreativeContent   verificato con Minecraft
SetLocalPlayerAsInitialized -> BiomeDefinitionList, NetworkChunkPublisherUpdate, LevelChunk...   VERIFICATO: il giocatore entra nel mondo piatto
```

`BiomeDefinitionList` (tutti i biomi, da `crates/server/data/biomes.json`) oggi è spenta di default (`SEND=biomes` per inviarla: il client l'ha rifiutata); non è ancora stata riprovata con il client
dopo il passaggio dal solo bioma "pianura". `StartGame` aspetta che lo spawn del mondo sia noto (dal `level.dat` o dal generatore).
Il test end-to-end `crates/server/tests/join.rs` copre tutto il flusso con un client WebRTC finto, sia sul mondo piatto sia con un
generatore finto (biomi, chunk attorno allo spawn, nuovi chunk dopo un `PlayerAuthInput` spostato di 5 chunk). La prova con il
client vero e la lista di controllo della tappa 1 (creativo) sono in `docs/superpowers/notes/2026-11-19-verifica-tappa1.md`;
`2026-11-18-verifica-t1.md` è la verifica precedente della generazione del mondo.

La sequenza segue quella dei server di riferimento (gophertunnel / Dragonfly).

## Cosa fa il server oggi

- Ciclo di gioco a 20 tick al secondo (`Server::tick`), con misura di TPS e durata media del tick (`/tps`).
- Chat tra i giocatori. Il nome viene dal login e non dal messaggio, ma le firme del login non sono ancora verificate (vedi "Sicurezza").
- Comandi: `/help`, `/list`, `/tps`, `/say <testo>`, `/gamemode <modo>` (`/gm`), `/bot [nome]` (crea un giocatore finto) e `/bot rimuovi`. Nome del bot: senza `§` e caratteri di controllo, al massimo 16 caratteri, diverso da quello di ogni giocatore e bot; al massimo 8 bot. Per ora chiunque può crearli e toglierli tutti (non c'è ancora un sistema di permessi).
- Annunci di ingresso e uscita.
- Movimento: dal `PlayerAuthInput` si legge la posizione (occhi); a ogni cambio di chunk il client riceve il nuovo centro
  (`NetworkChunkPublisherUpdate`) e il server manda i chunk mancanti (al massimo 8 per giocatore per tick, dal più vicino).
  I chunk lontani da tutti i giocatori (raggio + 2) si scaricano dalla memoria, salvandoli se modificati.
- Mondo: `worlds/<world>` in formato LevelDB di Bedrock. I chunk si leggono dal disco (senza bloccare il tick) e, se
  mancano, si chiedono al generatore `versmc-gen` (processo avviato dal server, vedi `server.toml`); quelli generati si salvano.
  Il seme è quello salvato nel mondo; `seed` in `server.toml` vale solo alla creazione.
- Se il generatore manca (programma non trovato) il server lo scrive nel log e usa il mondo piatto di ripiego
  (bedrock a y=-64, terra a -63/-62, erba a -61, spawn in 0,-60,0), senza salvare quei chunk.
- Creativo: ingresso in creativo con menu creativo completo, barra e inventario, rottura (validata dal server) e posa di blocchi
  con regole di orientamento (scale, lastre, porte, letti, torce, muri, ecc.), aggiornamento dei vicini, interazioni
  (porte, botole, cancelli, leve, bottoni). Le modifiche si salvano ogni 60 s e alla chiusura; l'inventario e la modalità
  del giocatore sono salvati nel LevelDB in formato Bedrock (un nuovo ingresso con la stessa identità espelle il vecchio).
- Tappa 2 (entità e giocatori): i giocatori si vedono tra loro (`PlayerList`, `AddPlayer`, movimento, testa, braccio,
  accovacciamento, oggetto in mano, skin), gli oggetti a terra si lanciano e si raccolgono. `/bot [nome]` crea un giocatore
  finto con la tua skin che gira in cerchio (raggio 3), salta, agita il braccio, si accovaccia, cambia oggetto e lancia pietre
  (create dal nulla): serve a provare tutto con un solo account. I bot restano anche se chi li ha creati esce; `/bot rimuovi` li toglie.
  Lista di prova nel gioco: `docs/superpowers/notes/2026-11-19-verifica-tappa2.md`.
- Ancora NON gestiti: sopravvivenza (danni, fame), mob. Limiti noti della tappa 1: `docs/superpowers/notes/2026-11-19-verifica-tappa1.md`.

Gli ID di rete dei blocchi sono hash FNV-1a dello stato (`crates/server/src/world.rs`): non serve nessuna tabella dei blocchi.

## Pacchetti generati

`crates/protocol/src/generated.rs` è generato dagli schemi JSON di Mojang: **non modificarlo a mano**.

```
git clone --depth 1 --branch v1.26.61 https://github.com/Mojang/bedrock-protocol-docs ../bedrock-protocol-docs
python tools/protocolgen.py ../bedrock-protocol-docs/json crates/protocol/src/generated.rs \
    StartGame ItemRegistry LevelChunk ChunkRadiusUpdated NetworkChunkPublisherUpdate BiomeDefinitionList \
    AvailableActorIdentifiers CreativeContent VoxelShapes JigsawStructureData InventoryContent InventorySlot \
    MobEquipment PlayerHotbar UpdateBlock SetPlayerGameType UpdatePlayerGameType UpdateAbilities LevelEvent \
    LevelSoundEvent ItemStackResponse BlockActorData ContainerOpen ContainerClose UpdateAttributes \
    TakeItemActor MoveActorAbsolute RemoveActor SetActorMotion MobArmorEquipment
```

Per aggiungere un pacchetto server -> client basta aggiungerne il nome all'elenco e rigenerare. Il generatore
produce solo la codifica; la lettura dei pacchetti client -> server è scritta a mano in `packets.rs` e `input.rs`.
`AddPlayer`, `AddItemActor` (dati sincronizzati, `oneOf`) e `Animate` (origine stringa, mano u8: il generatore sbaglia i tipi) sono scritti a mano in `crates/protocol/src/entity.rs`.
Il risultato è stato confrontato byte per byte con un'implementazione indipendente per la 1.26.60.

## Dati di gioco

`crates/server/data/items.txt` (nome e ID degli oggetti) si rigenera con:

```
git clone --depth 1 --branch v1.26.60.5 https://github.com/Mojang/bedrock-samples ../bedrock-samples
python tools/extract_items.py ../bedrock-samples crates/server/data/items.txt
```

`crates/server/data/biomes.json` (biomi della `BiomeDefinitionList`) si rigenera con:

```
git clone --depth 1 https://github.com/pmmp/BedrockData ../BedrockData
python -I tools/extract_biomes.py ../BedrockData ../bedrock-samples crates/server/data/biomes.json
```

I biomi vengono da pmmp/BedrockData (CC0, 1.26.30); quelli che mancano lì (oggi `minecraft:dappled_forest`, ID 195 dalla tabella di
Geyser in `generator/data/geyser/biomes.json`) da bedrock-samples v1.26.60.5, con colore dell'acqua della pianura (ipotesi: Mojang non lo pubblica).

Schemi e dati Mojang sono soggetti all'EULA di Minecraft: prima di distribuire il server verificare cosa si può
includere e cosa far scaricare o generare all'utente.

## Diagnostica

Variabili d'ambiente (PowerShell: `$env:NO_CHUNKS="1"; cargo run`, poi `Remove-Item Env:NO_CHUNKS`):

- `NO_CHUNKS=1`: dopo PlayerSpawn non invia nessun chunk (serve a capire se i chunk causano la disconnessione).
- `CHUNKS_EARLY=1`: manda biomi e chunk subito dopo PlayerSpawn, senza aspettare `SetLocalPlayerAsInitialized`.
- `SKIP=jigsaw,voxel,items,creative,publisher`: non invia i pacchetti elencati (si possono combinare).
- `VERSMC_DEBUG=nozero,nowater,plains,nobe`: altera o disattiva parti dei chunk (`plains` forza il bioma pianura) per isolare cosa rifiuta il client.
- `SEND=biomes`: invia la lista biomi (spenta di default: il client l'ha rifiutata).
- `SEND=actors`: invia anche `AvailableActorIdentifiers`, spento di default perché con i dati minimi che inventiamo
  (nessuna entità) il client crolla.
- `PUBLIC_IP=x.x.x.x`: IP pubblico da pubblicare come candidato ICE (gioco da Internet).

Ogni riga del log ha davanti i secondi dall'avvio. Il log mostra gli stati ICE, la chiusura dei data channel e il contenuto dei pacchetti non gestiti (se brevi).

## Comandi

```
cargo run                 # avvia il server (TCP+UDP 19132)
cargo test --workspace    # unit test del protocollo + test end-to-end con un client WebRTC finto e un generatore finto
cargo clippy --workspace --all-targets
cargo fmt --all

cd generator && cargo test && cargo clippy --all-targets   # il generatore ha i suoi test (nightly)
```

Per giocare da Internet: `$env:PUBLIC_IP="il.tuo.ip"` (PowerShell) e apri TCP+UDP 19132 sul router.

## Aggiornare a una nuova versione del gioco

1. Cerca la release su github.com/Mojang/bedrock-protocol-docs e leggi "Network protocol version".
2. Aggiorna `PROTOCOL` e `VERSION` in `crates/protocol/src/lib.rs`.
3. Controlla i pacchetti cambiati (gli schemi JSON del repository) e aggiorna `packets.rs`.
4. `cargo test --workspace`.

## Aggiungere un pacchetto

1. `crates/protocol/src/packets.rs`: ID in `id`, struct con `decode`/`encode`, test.
2. `crates/server/src/handlers.rs`: un ramo in `handle()` e un metodo che lo gestisce.

## File sensibili

`identity.key` è l'identità del server: i giocatori si fidano di questa chiave. Non cancellarla e non condividerla.

## Sicurezza: cosa NON è ancora verificato

- Le firme dei JWT del login non vengono controllate (`crates/protocol/src/login.rs`): chiunque può dichiararsi
  chiunque. Va bene in sviluppo, non per un server pubblico.
- L'offerta SDP del client (`a=identity`) viene scartata senza verifica.
- Il signaling è HTTP semplice, senza TLS.

## Licenze

- Server e crate della radice (`crates/*`, compreso `vgen`): MIT (`LICENSE`).
- Generatore (`generator/`): AGPL-3.0-or-later (`generator/LICENSE`), perché usa SteelMC (AGPL-3.0). Il server lo avvia come
  programma separato e ci parla via TCP: non lo include. Se distribuisci il binario `versmc-gen` valgono le condizioni dell'AGPL.
- `generator/data/geyser/` (tabelle di mappatura dei blocchi e dei biomi): GeyserMC/mappings, MIT (`generator/data/geyser/LICENSE`).
- `crates/server/data/biomes.json`: da pmmp/BedrockData, CC0.
- Dati e schemi di Mojang (jar del server Java, bedrock-protocol-docs, bedrock-samples): soggetti all'EULA di Minecraft.
