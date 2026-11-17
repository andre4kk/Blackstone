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
SetLocalPlayerAsInitialized -> NetworkChunkPublisherUpdate, LevelChunk...   VERIFICATO: il giocatore entra nel mondo piatto
```

La sequenza segue quella dei server di riferimento (gophertunnel / Dragonfly).

## Cosa fa il server oggi

- Ciclo di gioco a 20 tick al secondo (`Server::tick`), con misura di TPS e durata media del tick (`/tps`).
- Chat tra i giocatori. Il nome viene dal login e non dal messaggio, ma le firme del login non sono ancora verificate (vedi "Sicurezza").
- Comandi: `/help`, `/list`, `/tps`, `/say <testo>`.
- Annunci di ingresso e uscita.
- Ancora NON gestiti (ignorati in silenzio): movimento (`PlayerAuthInput`), rottura/posa di blocchi, inventario,
  animazioni, visibilità tra giocatori, salvataggio del mondo.

Mondo: piatto (bedrock a y=-64, terra a -63/-62, erba a -61), identico in ogni chunk. Gli ID di rete dei blocchi sono
hash FNV-1a dello stato (`crates/server/src/world.rs`): non serve nessuna tabella dei blocchi.

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

Schemi e dati Mojang sono soggetti all'EULA di Minecraft: prima di distribuire il server verificare cosa si può
includere e cosa far scaricare o generare all'utente.

## Diagnostica

Variabili d'ambiente (PowerShell: `$env:NO_CHUNKS="1"; cargo run`, poi `Remove-Item Env:NO_CHUNKS`):

- `NO_CHUNKS=1`: dopo PlayerSpawn non invia nessun chunk (serve a capire se i chunk causano la disconnessione).
- `SUBCHUNK_V8=1`: serializza i sotto-chunk in versione 8 (senza indice di sezione) invece della 9, che è il default.
- `CHUNKS_EARLY=1`: manda biomi e chunk subito dopo PlayerSpawn, senza aspettare `SetLocalPlayerAsInitialized`.
- `SKIP=jigsaw,voxel,items,creative,publisher`: non invia i pacchetti elencati (si possono combinare).
- `SEND=biomes,actors`: invia anche `BiomeDefinitionList` e `AvailableActorIdentifiers`, spenti di default perché con i dati
  minimi che inventiamo (un solo bioma, nessuna entità) il client crolla. Da riabilitare con dati completi.
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
