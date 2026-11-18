# bedrock-server

Server Minecraft Bedrock in Rust, su trasporto NetherNet (WebRTC), quello usato dalle versioni 26.60+.

## Architettura

```
crates/
  protocol/  bedrock-protocol  Pacchetti, batch, compressione, lettura del login. Nessun I/O: solo byte <-> struct.
                               Qui vivono le costanti di versione (PROTOCOL, VERSION).
  net/       bedrock-net       Trasporto: signaling HTTP, identità, WebRTC. Espone Event::{Open,Message,Close} e
                               Sender::{send,kick}. Non sa cosa c'è nei messaggi.
  server/    bedrock-server    Logica di gioco: un handler per pacchetto (handlers.rs). Contiene l'eseguibile.
```

Dipendenze: `server -> net`, `server -> protocol`. `net` e `protocol` non si conoscono.

## Flusso di ingresso (stato)

```
RequestNetworkSettings -> NetworkSettings                          verificato con Minecraft
Login                  -> PlayStatus + ResourcePacksInfo           verificato con Minecraft
ResourcePackClientResponse(DownloadingFinished) -> ResourcePackStack   verificato con Minecraft
ResourcePackClientResponse(StackFinished) -> JigsawStructureData, VoxelShapes, StartGame, ItemRegistry   accettati dal client
RequestChunkRadius -> ChunkRadiusUpdated, PlayStatus(PlayerSpawn), CreativeContent   verificato con Minecraft
SetLocalPlayerAsInitialized -> BiomeDefinitionList, NetworkChunkPublisherUpdate, LevelChunk...   VERIFICATO: il giocatore entra nel mondo piatto
```

`BiomeDefinitionList` (tutti i biomi, da `crates/server/data/biomes.json`) parte sempre; non è ancora stata riprovata con il client
dopo il passaggio dal solo bioma "pianura". `StartGame` aspetta che lo spawn del mondo sia noto (dal `level.dat` o dal generatore).

La sequenza segue quella dei server di riferimento (gophertunnel / Dragonfly).

## Cosa fa il server oggi

- Ciclo di gioco a 20 tick al secondo (`Server::tick`), con misura di TPS e durata media del tick (`/tps`).
- Chat tra i giocatori. Il nome viene dal login e non dal messaggio, ma le firme del login non sono ancora verificate (vedi "Sicurezza").
- Comandi: `/help`, `/list`, `/tps`, `/say <testo>`.
- Annunci di ingresso e uscita.
- Movimento: dal `PlayerAuthInput` si legge la posizione (occhi); a ogni cambio di chunk il client riceve il nuovo centro
  (`NetworkChunkPublisherUpdate`) e il server manda i chunk mancanti (al massimo 8 per giocatore per tick, dal più vicino).
  I chunk lontani da tutti i giocatori (raggio + 2) si scaricano dalla memoria, salvandoli se modificati.
- Mondo: `worlds/<world>` in formato LevelDB di Bedrock. I chunk si leggono dal disco (senza bloccare il tick) e, se
  mancano, si chiedono al generatore `versmc-gen` (processo avviato dal server, vedi `server.toml`); quelli generati si salvano.
  Il seme è quello salvato nel mondo; `seed` in `server.toml` vale solo alla creazione.
- Se il generatore manca (programma non trovato) il server lo scrive nel log e usa il mondo piatto di ripiego
  (bedrock a y=-64, terra a -63/-62, erba a -61, spawn in 0,-60,0), senza salvare quei chunk.
- Ancora NON gestiti (ignorati in silenzio): rottura/posa di blocchi, inventario, animazioni, visibilità tra giocatori.

Gli ID di rete dei blocchi sono hash FNV-1a dello stato (`crates/server/src/world.rs`): non serve nessuna tabella dei blocchi.

## Pacchetti generati

`crates/protocol/src/generated.rs` è generato dagli schemi JSON di Mojang: **non modificarlo a mano**.

```
git clone --depth 1 --branch v1.26.61 https://github.com/Mojang/bedrock-protocol-docs ../bedrock-protocol-docs
python tools/protocolgen.py ../bedrock-protocol-docs/json crates/protocol/src/generated.rs \
    StartGame ItemRegistry LevelChunk ChunkRadiusUpdated NetworkChunkPublisherUpdate BiomeDefinitionList \
    AvailableActorIdentifiers CreativeContent VoxelShapes JigsawStructureData
```

Per aggiungere un pacchetto server -> client basta aggiungerne il nome all'elenco e rigenerare. Il generatore
produce solo la codifica; la lettura dei pacchetti client -> server è scritta a mano in `packets.rs`.
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
- `SUBCHUNK_V8=1`: serializza i sotto-chunk in versione 8 (senza indice di sezione) invece della 9, che è il default.
- `CHUNKS_EARLY=1`: manda biomi e chunk subito dopo PlayerSpawn, senza aspettare `SetLocalPlayerAsInitialized`.
- `SKIP=jigsaw,voxel,items,creative,publisher`: non invia i pacchetti elencati (si possono combinare).
- `SEND=actors`: invia anche `AvailableActorIdentifiers`, spento di default perché con i dati minimi che inventiamo
  (nessuna entità) il client crolla. I biomi invece partono sempre (`SEND=biomes` non esiste più).
- `PUBLIC_IP=x.x.x.x`: IP pubblico da pubblicare come candidato ICE (gioco da Internet).

Ogni riga del log ha davanti i secondi dall'avvio. Il log mostra gli stati ICE, la chiusura dei data channel e il contenuto dei pacchetti non gestiti (se brevi).

## Comandi

```
cargo run                 # avvia il server (TCP+UDP 19132)
cargo test --workspace    # unit test del protocollo + test end-to-end con un client WebRTC finto
cargo clippy --workspace --all-targets
cargo fmt --all
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
