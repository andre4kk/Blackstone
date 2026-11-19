# Generazione del mondo Java per Blackstone: design

Data: 2026-11-17. Stato: bozza da approvare.
Catalogo delle opzioni valutate: `docs/brainstorm/catalogo-worldgen.html` (pubblicato come artifact).
Stato delle decisioni: `docs/brainstorm/stato.md`.

## Intento

Blackstone vuole diventare un server Bedrock in stile PocketMine-MP, ma migliore. In futuro avrà un'API per i plugin.
Entrando dal client Bedrock (1.26.6x, NetherNet) il giocatore deve trovare un mondo Minecraft "vero", generato con
l'algoritmo di **Java Edition**: stesso seed, stesso mondo di Java 26.3. Oggi il mondo è un unico chunk piatto ripetuto.

La generazione è presa da **SteelMC**, che dichiara parità blocco per blocco con vanilla su 7.500 chunk. Le mappature
Java → Bedrock sono quelle di **GeyserMC**, generate esattamente per Java 26.3 e Bedrock 1.26.60.

## Obiettivi (traguardo = T1 + T2 + T3)

- **T1, Overworld infinito**
  - Il generatore esterno è avviato e controllato dal server.
  - Il seed è configurabile.
  - Blocchi e biomi vengono tradotti in Bedrock e la `BiomeDefinitionList` è completa (sparisce il crash attuale).
  - I chunk vengono inviati mentre il giocatore si muove (`PlayerAuthInput`).
  - Lo spawn è calcolato come in Java.
  - I chunk vengono salvati in LevelDB Bedrock e ricaricati.
- **T3, Nether ed End**
  - Generati da Steel.
  - Si cambia dimensione con un comando (`/dim` o simile) e con i portali.
  - Le altezze sono quelle di Bedrock: Nether da 0 a 128, End da 0 a 256.
- **T2, sopravvivenza (M2)**
  - Modalità sopravvivenza e creativa.
  - Rottura dei blocchi con tempi calcolati per blocco e attrezzo (in creativa la rottura è istantanea).
  - Posa dei blocchi.
  - Drop come entità oggetto che si raccolgono.
  - Inventario con transazioni `ItemStackRequest`.
  - Un minimo di fame e di danno da caduta.
  - Le modifiche vengono salvate e propagate agli altri giocatori nei chunk interessati.

Ordine proposto: **audit, poi T1, poi T3, poi T2**. Motivo: T3 riusa direttamente il lavoro di T1 sul generatore,
mentre T2 è gameplay e poggia su un mondo già stabile.

## Non obiettivi (per ora)

- Entità generate con il mondo (abitanti dei villaggi, animali) e loro comportamento.
- Bottino delle casse e degli altri blocchi con dati: i blocchi compaiono, il contenuto arriva dopo.
- Luce calcolata dal server: in Bedrock la calcola il client.
- Giocatori visibili tra loro: è un lavoro separato. In T2 si propagano solo le modifiche ai blocchi.
- API per i plugin: arriva dopo, ma l'architettura non deve ostacolarla.
- Importare mondi Java esistenti (Anvil): utile più avanti.

## Anti-obiettivi (sarebbe un fallimento anche se "funziona")

- Il server principale che diventa AGPL o passa a Rust nightly.
- Il tick di gioco che si blocca aspettando il generatore.
- Un mondo diverso da Java 26.3 con lo stesso seed (a parte le differenze di traduzione inevitabili).
- Il server che va in crash o si blocca se il generatore cade.

## Vincoli

- **Licenze.**
  - `blackstone-protocol`, `blackstone-net` e `blackstone-server` restano MIT su Rust stable.
  - Il generatore `blackstone-gen` è AGPL-3.0-or-later perché collega `steel-core`.
  - I due comunicano solo tramite un protocollo su socket. Il codice di Steel non entra mai nel server.
- **Toolchain del generatore:** nightly fissato alla stessa data di Steel (oggi `nightly-2026-10-14`), con un suo
  `rust-toolchain.toml` nella cartella del generatore. Il generatore va **fuori dal workspace principale**: un
  workspace separato, così `cargo build` del server resta stable.
- **Rete alla compilazione:** la build di Steel scarica dal sito di Mojang il server jar di Java 26.3 per estrarre i
  dati vanilla. Il generatore va quindi compilato una volta con internet. L'EULA di Minecraft si applica a quei dati.
- **Versioni:** il client è 1.26.63 (`VERSION`), le mappature Geyser sono per 1.26.60. Va verificato che gli hash dei
  blocchi coincidano: i blocchi sconosciuti finiscono in un blocco di ripiego e vengono contati nel log.
- **Steel è pre-alpha:** le sue API interne cambiano. Il generatore fissa un commit preciso di Steel e lo aggiorna
  apposta, mai in automatico.

## Approcci considerati

| Opzione | Cosa | Esito |
|---|---|---|
| A | `steel-core` come libreria dentro il server | Scartata: rende il server AGPL e nightly, e lega i plugin all'AGPL |
| B | Copiare il generatore di Steel nel progetto | Scartata: 150-200k righe da mantenere a mano |
| C | Solo `steel-worldgen` (terreno), il resto dopo | Scartata: mondo senza alberi né strutture, e alla fine diventa B |
| **D** | **Generatore in un processo separato** | **Scelta dall'utente**: server MIT e stable, plugin liberi |

Varianti di D:

| Variante | Cosa | Esito |
|---|---|---|
| **D1** | **Binario nostro su `steel-core`, chunk su richiesta** | **Scelta**: mondo infinito, protocollo nostro e stabile |
| D2 | Server Java vanilla + Chunky, si leggono i file Anvil | Scartata per ora: area finita, serve Java. Utile come importatore futuro |
| D3 | SteelMC originale pregenera, si leggono i file STLR | Scartata: il formato STLR (v23) cambia spesso |
| D4 | Il server si collega a SteelMC come client Java | Scartata: equivale a scrivere un Geyser |

Salvataggio:

| Opzione | Esito |
|---|---|
| **S1: LevelDB Bedrock** | **Scelta**: mondi apribili in Bedrock, BDS, Amulet e Chunker |
| S2: formato nostro | Scartata |
| S3: niente salvataggio | Scartata |

## Architettura

```
                  TCP 127.0.0.1 (protocollo "vgen")
blackstone-server  <---------------------------------->  blackstone-gen (AGPL, nightly, workspace separato)
  (MIT, stable)     richiesta: dim, x, z                 steel-core: ChunkMap::generate_region
  - avvia/riavvia   risposta: chunk Bedrock               + traduzione blocchi/biomi (mappature Geyser)
  - cache + LevelDB  (sezioni con palette di hash Bedrock,
  - streaming          biomi 3D, block entity minime)
```

### Generatore `blackstone-gen` (cartella `generator/`, workspace a parte)

- Si avvia con `blackstone-gen --port N --seed S`. Accetta una sola connessione: quella del server.
- Tiene una `ChunkMap` di Steel per ciascuna delle tre dimensioni. Per ogni richiesta genera fino allo stato Full.
  - Nota per chi implementa: `generate_region` richiede una mappa vuota. Va capito nella fase di prova se generare a
    regioni (es. 8×8 chunk) con una mappa nuova ogni volta, pagando la fascia di dipendenze, oppure usare il
    normale sistema a ticket di Steel con una mappa persistente. Il criterio è la latenza per un giocatore che cammina.
- Traduzione, fatta al momento della compilazione del generatore:
  - dagli ID di stato Java di Steel all'hash Bedrock, con `blocks.nbt` di Geyser;
  - dai biomi Java agli ID Bedrock, con `biomes.json` di Geyser.
- Restituisce le sezioni per l'altezza Bedrock della dimensione (Overworld da -64 a 320; Nether da 0 a 128,
  scartando la parte Java sopra 128; End da 0 a 256), le palette di biomi e i blocchi con dati essenziali (tipo e
  posizione, per casse e cartelli). Restituisce anche lo spawn calcolato da Steel quando lo si chiede.
- Le mappature Geyser sono MIT: si copiano in `generator/data/` con licenza e versione annotate.

### Protocollo "vgen"

- Messaggi con lunghezza in testa, formato binario semplice e versionato (byte di versione nel saluto iniziale).
- Messaggi:
  - `Hello{version, seed}`
  - `GenerateChunk{req_id, dim, x, z}` → `Chunk{req_id, payload}` oppure `Error{req_id, msg}`
  - `Spawn{dim}` → `SpawnPos{x, y, z}`
  - `GetGameData` → `GameData{...}` (durezza, attrezzi, drop in ID Bedrock; vedi sotto)
- Più richieste possono essere in volo insieme e le risposte possono arrivare in qualsiasi ordine (`req_id`).
- Il payload del chunk è neutro rispetto al formato di rete: sezioni con palette di hash Bedrock e indici. Il server
  lo trasforma sia in pacchetto di rete sia in record LevelDB. La serializzazione di rete resta solo nel server.

### Server

- **Gestore del generatore:** avvia il binario (percorso in configurazione), fa il saluto e lo riavvia se cade.
  Finché non risponde, i chunk richiesti restano in coda: non vengono mandati chunk finti al client. Se il
  generatore manca del tutto, il server si avvia con il mondo piatto attuale e lo segnala nel log.
- **Mondo** (`world/`):
  - una mappa di chunk caricati per dimensione;
  - un chunk in memoria = sezioni con palette + biomi + blocchi con dati;
  - ordine di ricerca di un chunk: memoria, poi LevelDB, poi generatore;
  - i chunk generati si salvano subito; quelli modificati vengono marcati e salvati periodicamente e alla chiusura;
  - i chunk lontani da tutti i giocatori vengono scaricati.
- **LevelDB:** `rusty-leveldb` con un compressore deflate raw registrato con id 4, come Bedrock. Chiavi e valori
  seguono il formato Bedrock: versione del chunk, sotto-chunk v9, `Data3D` per biomi e altezze, `BlockEntity`,
  stato di completamento. Dati del mondo in `level.dat` (seed, spawn, nome). Cartella: `worlds/<nome>/`.
- **Streaming:**
  - `PlayerAuthInput` aggiorna la posizione del giocatore;
  - quando il giocatore cambia chunk, il server calcola i chunk da mandare in ordine di distanza;
  - li chiede al mondo senza bloccare il tick (le risposte del generatore arrivano come eventi al ciclo principale);
  - aggiorna `NetworkChunkPublisherUpdate`;
  - ogni giocatore ha un limite di chunk inviati per tick.
- **Serializzazione di rete:** sotto-chunk v9 con bit per blocco variabili (1, 2, 3, 4, 5, 6, 8, 16) e biomi 3D per
  sezione. Generalizza l'attuale `flat_chunk`.
- **Biomi:** `BiomeDefinitionList` completa, ricavata da pmmp/BedrockData (CC0) al momento della compilazione.
  Questo rimuove l'interruttore `SEND=biomes`.
- **Dimensioni (T3):** pacchetti di cambio dimensione, altezze per dimensione nello `StartGame` e nei chunk, portali
  che si accendono e portano il giocatore. Il calcolo del portale di arrivo può essere semplificato (cerca o crea
  vicino a coordinate ×8 o /8).
- **Sopravvivenza (T2):**
  - durezza, attrezzo giusto e drop sono **dati**, non codice di Steel: il server li riceve dal generatore all'avvio
    (messaggio `GameData`, vedi le decisioni prese dopo la revisione);
  - le entità oggetto hanno una fisica minima e vengono raccolte quando il giocatore è vicino;
  - l'inventario con `ItemStackRequest` gestisce: prendere, posare, scambiare, dividere, buttare, creativo;
  - fame e danno da caduta minimi; alla morte si rinasce allo spawn.

## Cosa ho deciso io (l'utente può correggere)

- Ordine delle fasi: audit, T1, T3, T2.
- Generatore in un workspace a parte, nella cartella `generator/` dello stesso repository.
- Protocollo TCP locale e payload neutro; la serializzazione di rete resta nel server.
- I dati di gioco per la sopravvivenza (durezza, drop) vengono ricavati da dati, non presi dal codice di Steel.
- Blocchi Java senza equivalente: si usa un blocco di ripiego e si conta nel log.
- Se il generatore manca, il server usa il mondo piatto.
- Seed e percorso del generatore in un file di configurazione (`server.toml`).

### Decisioni prese dopo la revisione (mie, correggibili)

- **Portata dei piani.**
  - L'audit è un lavoro a sé, che precede il resto: bug, ottimizzazioni, `.gitignore`, `identity.key` pubblico.
  - Il primo piano copre la prova e T1. La prova fa da cancello: si procede solo se il generatore compila e produce
    chunk in tempi accettabili.
  - T3 e T2 avranno ciascuno il proprio piano, scritto quando si arriva lì.
- **Dati di gioco per T2.** La build del server non scarica il jar di Mojang. All'avvio il generatore espone tramite
  vgen i dati che Steel ha già nel suo registro (durezza, attrezzo giusto, drop, tabelle di bottino dei blocchi), già
  tradotti in ID Bedrock (messaggio `GameData`). Il server li tiene in memoria. Così niente dati Mojang entra nel
  repository o nella build del server.
- **Portali in T3.**
  - T3 comprende il comando `/dim`, il passaggio quando il giocatore entra in un blocco portale acceso e il ritorno
    attraverso il portale.
  - Accendere un portale costruito dal giocatore (acciarino) richiede l'uso degli oggetti, quindi arriva con T2.
  - Nell'End si entra con `/dim`; il portale dell'End nella fortezza funziona quando T2 permette di posare gli occhi
    di Ender.
- **Verifica "stesso mondo di Java".**
  - Ci si fida della parità dichiarata da Steel; si verifica solo la nostra parte.
  - Test "golden" sul generatore: seed fisso, alcuni chunk noti, hash dei blocchi tradotti confrontato con un valore
    salvato.
  - Più un controllo a mano una tantum: confronto visivo con Java 26.3, stesso seed e stesse coordinate.
  - Nessun server Java nei test.
- **Blocchi con dati in T1.** Si traducono quelli che cambiano l'aspetto delle strutture generate: casse (anche
  doppie), cartelli, letti (colore), stendardi, teste, vasi di fiori, campane, generatori di mostri (tipo). Il
  contenuto (bottino) resta fuori. Il payload di vgen porta questi blocchi come NBT Bedrock già pronto.

## Lasciato a chi implementa

- La strategia di generazione (regioni o ticket di Steel), dopo la prova di latenza.
- Il formato binario esatto di "vgen".
- Il limite di chunk per tick e la politica di scaricamento.
- Come dividere il server in moduli o crate (`world`, `storage`, `gen_client`, `inventory`).
- Quali dettagli del formato LevelDB servono per aprire il mondo in Bedrock e in Amulet: si verifica aprendolo davvero.

## Prima cosa da fare: una prova usa e getta

Compilare `blackstone-gen` contro un commit fissato di Steel su Windows e generare il chunk (0,0) con un seed noto.
Tradurlo e stamparlo. Misurare:

- tempo di compilazione;
- tempo per chunk (singolo e a regioni);
- quanti blocchi non hanno una mappatura.

Se fallisce, si torna a questo design prima del resto.
