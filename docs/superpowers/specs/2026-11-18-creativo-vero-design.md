# Tappa 1 "creativo vero" — design

Data: 18/11/2026. Brainstorming: `docs/brainstorm/stato-gameplay.md`, catalogo `docs/brainstorm/catalogo-gameplay.html`.

## Perché

VersMCBE deve diventare un server stile PocketMine-MP ma migliore, con un'API dei plugin in futuro. Dopo il mondo
generato (T1) l'utente vuole i comportamenti normali di Minecraft. Questa tappa è il primo pezzo: un creativo in cui si
costruisce davvero, con tutto controllato dal server e salvato. Serve a portare presto nel gioco le parti di protocollo
più rischiose (input completo del giocatore, inventario `ItemStackRequest`, aggiornamento dei blocchi) su un pezzo
piccolo, prima della sopravvivenza.

Oggi: si entra nel mondo generato, il movimento è calcolato dal client (salto e volo funzionano), il menu creativo è
vuoto (`CreativeContent::default()`), le rotture e i piazzamenti del client sono ignorati (il pacchetto 30
`InventoryTransaction` finisce in "non gestito"), non si salva nulla del giocatore.

## Obiettivi

1. **Menu creativo pieno**: tutti gli oggetti vanilla, nei loro gruppi, presi in mano e messi nella barra.
2. **Rompere blocchi** in creativo: istantaneo, deciso dal server, aggiornato nel chunk, salvato, inviato agli altri
   giocatori che vedono quel chunk.
3. **Piazzare blocchi** con lo stato giusto: regole per tipo di stato più casi speciali (vedi sotto).
4. **Interazioni col tasto destro**: porte, botole, cancelletti, leve, pulsanti (aprire/chiudere, accendere; i pulsanti
   tornano su da soli dopo il tempo vanilla).
5. **Dati del giocatore salvati** nel LevelDB in formato Bedrock: posizione, rotazione, modalità di gioco, inventario,
   casella selezionata. Al rientro si ritrovano.
6. **`/gamemode`** (creative, survival, adventure, spectator) per sé stessi.
7. **Punto unico a eventi annullabili** per rottura, piazzamento e interazione (aggancio della futura API plugin).

## Non obiettivi (tappa 2 o dopo)

- Tempi di rottura, attrezzi, drop, oggetti a terra, raccolta, salute, fame, cadute, morte.
- Banco da lavoro, forni, bauli e qualsiasi contenitore; cartelli modificabili.
- Fisica dei blocchi (acqua e lava che scorrono, sabbia che cade, redstone oltre leve e pulsanti che si accendono).
- Altri giocatori visibili; controlli anti-trucco sul movimento.
- La lista dei biomi completa (resta spenta: `SEND=biomes`).
- Verifica del login (il giocatore resta "NON verificato").

## Anti-obiettivi (falliremmo anche se "funziona")

- Un client modificato può rompere o piazzare dove il server non lo permetterebbe.
- Un'azione rifiutata resta sullo schermo del client: ogni rifiuto rimanda al client lo stato vero del blocco
  (`UpdateBlock`) o dell'inventario (risposta `ItemStackResponse` di errore + contenuto vero).
- Il client si disconnette per un pacchetto malformato (come la lista dei biomi del 18/11).

## Vincoli

- Server MIT in `crates/`; il generatore (AGPL) non si tocca in questa tappa.
- Protocollo 2225 (client 1.26.6x). I pacchetti server → client si generano con `tools/protocolgen.py` dagli schemi
  Mojang (bedrock-protocol-docs), che vanno **riscaricati** (EULA: non entrano nel repository). I decoder client →
  server si scrivono a mano, come quelli esistenti in `crates/protocol/src/packets.rs`.
- ID di rete dei blocchi: hash FNV-1a dell'NBT `{name, states}` (`world::block_hash`), come oggi.
- Dati liberi: pmmp/BedrockData (CC0) per oggetti creativi, stati canonici dei blocchi e associazione oggetto → blocco.
  `crates/server/data/items.txt` (bedrock-samples) resta per il registro degli oggetti.
- Il mondo deve funzionare anche senza generatore (mondo piatto): le regole dei blocchi non dipendono dalla palette che
  manda il generatore.
- Codice e test in stile del progetto: italiano nei commenti e nei log, rustfmt a 160 colonne, clippy pulito.

## Approcci considerati

**Fonte della logica** — scelto: **tradurre in Rust da Dragonfly** (Go, MIT): server Bedrock completo con stesse
regole di protocollo e licenza senza vincoli. Scartati: PocketMine-MP (PHP, LGPL-3: il derivato resta LGPL), logica
Java di Steel (stati Java diversi, ritardo del processo separato, AGPL), da zero (mesi a riscoprire regole note).

**Autorità** — scelto: **il server decide** (rottura decisa dal server attiva in StartGame, input completo letto e
controllato). Scartato: fidarsi del client (trucchi possibili, i plugin non potrebbero annullare nulla).

**Stato dei blocchi piazzati** — scelto: **regole per tipo di stato + casi speciali da Dragonfly**. Scartati: traduzione
blocco per blocco da Dragonfly (centinaia di file e un modello dei blocchi in Rust, più notti), solo stato predefinito
(scale e tronchi tutti nello stesso verso).

**Dati del giocatore** — scelto: **LevelDB in formato Bedrock**. Scartati: niente salvataggio (rientro sempre a vuoto),
file JSON nostro (il mondo non resterebbe completo per gli strumenti Bedrock).

## Cosa decide l'utente (fissato)

- Logica da Dragonfly; server autoritativo; solo tappa 1; regole per stato + casi speciali; dati del giocatore in
  LevelDB Bedrock (risposte 1–5 del 18/11).

## Il come, a grandi linee

### Rottura (creativo)

- StartGame: rottura decisa dal server attiva (`server_authoritative_block_breaking`).
- Il client manda in `PlayerAuthInput` le azioni sui blocchi (inizio, avanzamento, interruzione, rottura prevista) e
  l'eventuale interazione con un oggetto. `PlayerAuthInput` va letto **tutto** come fa Dragonfly (oggi si leggono solo
  i primi 20 byte); un pacchetto che non si riesce a leggere si ignora con un log, non disconnette.
- Controlli: distanza dagli occhi entro il limite vanilla del creativo, chunk caricato, blocco non aria, altezza dentro
  il mondo, modalità che permette di rompere (spectator e adventure no).
- Esito accettato: il blocco diventa aria (strato acqua compreso, se c'era: con un blocco "waterlogged" resta l'acqua),
  il chunk diventa `dirty`, `UpdateBlock` a tutti i giocatori che hanno quel chunk, particelle e suono di rottura.
  Rifiutato: `UpdateBlock` con lo stato vero al solo autore.

### Piazzamento

- Arriva come `InventoryTransaction` di tipo "uso oggetto, clic su blocco" (pacchetto 30): posizione cliccata, faccia,
  punto del clic, oggetto in mano, posizione del giocatore.
- Posizione del nuovo blocco: quella cliccata se il blocco cliccato è sostituibile (erba alta, acqua, aria, neve
  sottile…), altrimenti quella accanto sulla faccia.
- Controlli: distanza, oggetto in mano uguale a quello dell'inventario del server, l'oggetto è un blocco, posizione
  libera o sostituibile, il blocco non si sovrappone a un giocatore (bounding box), altezza nel mondo, modalità.
- **Regole per tipo di stato** (applicate a qualunque blocco abbia quello stato):
  - asse (`pillar_axis`): dall'asse della faccia cliccata;
  - direzioni (`minecraft:cardinal_direction`, `facing_direction`, `direction`, `weirdo_direction`,
    `minecraft:facing_direction`, `ground_sign_direction`…): dalla direzione in cui guarda il giocatore o dalla faccia,
    secondo la convenzione di Dragonfly per quello stato;
  - metà sopra/sotto (`upside_down_bit`, `minecraft:vertical_half`, `top_slot_bit`): dal punto del clic e dalla faccia;
  - gli altri stati restano al valore predefinito del blocco.
- **Casi speciali** (traduzione mirata da Dragonfly): porte (due blocchi, cerniera), letti (due blocchi), mezzi blocchi
  che si uniscono nel blocco doppio, torce/lanterne/pulsanti/leve su muro, pavimento o soffitto (e rifiuto se non c'è
  appoggio), muretti (connessioni ricalcolate con i vicini), piante e fiori che richiedono terra sotto, scale a pioli
  e rampicanti su muro.
- Un blocco con `waterlogged` piazzato dentro l'acqua tiene l'acqua nel secondo strato (lo strato esiste già nei chunk).
- Esito accettato: chunk `dirty`, `UpdateBlock` a chi vede il chunk, suono di piazzamento. In creativo l'oggetto in mano
  non si consuma. Rifiutato: `UpdateBlock` con lo stato vero nelle posizioni toccate.

### Interazioni

- Clic su porte, botole, cancelletti: commuta lo stato aperto (porta: tutte e due le metà). Leve: commuta. Pulsanti:
  accesi, poi spenti dopo i tick vanilla (legno 30, pietra 20) tramite il tick del server.
- Un clic su un blocco interattivo non piazza nulla (salvo accovacciati, come vanilla).

### Inventario

- Al login: `CreativeContent` pieno (gruppi e oggetti da BedrockData), contenuto dell'inventario del giocatore
  (`InventoryContent`), casella selezionata.
- `ItemStackRequest`: gestite le azioni necessarie al creativo (prendere dal menu creativo, spostare, scambiare,
  dividere, buttare nel cestino/distruggere, lasciar cadere = distruggere in questa tappa). Ogni richiesta riceve
  `ItemStackResponse` ok o errore; in errore il server rimanda le caselle vere.
- Cambio della casella selezionata (`MobEquipment`/`PlayerHotbar`) seguito dal server: è l'oggetto "in mano" che il
  piazzamento controlla.
- Modello degli oggetti: nome, ID di rete (da `items.txt`), quantità, metadati, stato di blocco associato (da
  BedrockData). NBT degli oggetti fuori portata in questa tappa (si conserva grezzo se arriva).

### Dati del giocatore

- Salvati in LevelDB come fa il server dedicato Bedrock (`player_server_<id>` con NBT del giocatore), con
  un'associazione dall'identità del client (XUID se c'è, altrimenti nome) all'id. Contenuto minimo: posizione,
  rotazione, dimensione, modalità di gioco, inventario, casella selezionata.
- Salvati all'uscita del giocatore e alla chiusura del server. Al login, se ci sono, valgono al posto dello spawn.
- L'identità non è verificata (vedi non obiettivi): con server aperto chiunque potrebbe dichiararsi un altro.

### Modalità di gioco

- `/gamemode <creative|survival|adventure|spectator>` su sé stessi; aggiorna il client e si salva.
- In questa tappa, in sopravvivenza si rompe ancora in modo istantaneo e senza drop, e si piazza senza consumare
  oggetti (le regole vere arrivano con la tappa 2). Adventure e spectator non rompono né piazzano.

### Eventi annullabili

- Un modulo unico riceve "rottura", "piazzamento" e "interazione" già validati, con giocatore, posizione e blocchi
  coinvolti, e può annullarli prima che tocchino il mondo. In questa tappa nessuno li annulla; i test lo fanno per
  provare che l'annullamento rimanda al client lo stato vero.

## Precisazioni dopo la revisione del design (decisioni mie, segnalate all'utente)

1. **Registro dei blocchi:** l'autorità sono gli stati canonici di BedrockData (`canonical_block_states`, CC0), caricati
   nel server: per ogni blocco l'elenco degli stati con tipi e valori ammessi e lo stato predefinito. L'hash si calcola
   da lì come oggi. Criterio di accettazione: **ogni oggetto del menu creativo che è un blocco piazza uno stato presente
   nel registro** (test che scorre tutto il menu); un oggetto il cui blocco manca dal registro non si piazza (rifiuto +
   log), non si inventa uno stato.
2. **Menu creativo:** si manda la lista creativa di BedrockData così com'è, compresi gli oggetti non-blocco (attrezzi,
   cibo, uova) e quelli con NBT (libri incantati, pozioni, stendardi), con l'NBT fornito da BedrockData. Un oggetto che
   non ha ID in `items.txt` si toglie dal menu con un log all'avvio. Tutti gli oggetti si possono tenere nella barra e si
   salvano; solo quelli associati a un blocco si piazzano.
3. **Modalità e abilità:** il cambio di modalità manda `SetPlayerGameType` e `UpdateAbilities` (abilità e "rottura
   istantanea" del creativo come fa Dragonfly); StartGame e il primo `UpdateAbilities` vanno controllati e allineati.
   Se servono pacchetti nuovi si generano con `protocolgen.py`.
4. **Dati del giocatore:** obiettivo = chiavi e nomi dei campi del server dedicato Bedrock (il motivo della scelta A è
   che il mondo resti completo per gli strumenti Bedrock). Si usa la chiave `player_server_<uuid>` con l'NBT del
   giocatore e la chiave di associazione dall'identità del client; i nomi esatti dei campi si prendono dal formato
   vanilla documentato. Il test minimo è salvare e rileggere; aprire il mondo nel server dedicato vanilla è una verifica
   desiderata, non un requisito della tappa.
5. **Dove vive lo stato:** c'è già un ciclo unico a 20 tick che possiede mondo e giocatori (`Server::tick`, eventi di
   rete nello stesso `select!`): timer dei pulsanti e invio di `UpdateBlock` a tutti stanno lì. "A chi vede il chunk" =
   i giocatori che hanno quel chunk in `sent`. Più giocatori insieme sono in portata (il codice già li gestisce), anche
   se non si vedono tra loro.
6. **Salvataggio dei chunk modificati:** oltre a quanto già c'è (all'uscita dal raggio e alla chiusura) si aggiunge un
   salvataggio automatico dei chunk `dirty` ogni 60 s. In caso di crash si perde al massimo l'ultimo minuto. Il formato
   su disco resta quello di oggi. I chunk del mondo piatto di ripiego (generatore assente) restano non salvati anche se
   modificati, con un avviso nel log al primo cambiamento: sono temporanei per scelta.
7. **Oggetto in mano:** il confronto tra l'oggetto della transazione e quello dell'inventario del server usa ID di rete,
   metadati e stato di blocco, non l'ID di rete della pila (`stack network id`), per non rifiutare piazzamenti validi.
8. **Versione di Dragonfly:** si usa l'ultima versione che supporta il protocollo 1.26.6x al momento del piano, e il
   piano ne annota il commit; le convenzioni degli stati si verificano comunque sugli stati canonici del punto 1.

## Rischi da provare presto nel gioco

1. Lettura completa di `PlayerAuthInput` (insieme di bit lungo, campi condizionali): se sbagliata, niente rotture.
2. `ItemStackRequest` e risposte: se sbagliate, il client mostra oggetti fantasma o si disconnette.
3. `CreativeContent` pieno: un oggetto malformato può far chiudere il client (come i biomi).
4. Hash degli stati prodotti dalle regole: uno stato con nome o tipo sbagliato dà un hash che il client non conosce.
5. NBT del giocatore nel formato Bedrock (nomi dei campi).

## Lasciato a chi costruisce

- Struttura dei moduli e dei tipi in Rust; come rappresentare gli stati dei blocchi (purché l'hash resti quello di oggi).
- Quali file di BedrockData usare e come generarne i dati nel repository (script in `tools/`, come per i biomi).
- Valori esatti dei limiti (distanza, tick) presi da Dragonfly.
- Ordine dei task, purché i rischi 1–3 si possano provare nel gioco il prima possibile.
