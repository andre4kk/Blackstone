# Decisioni prese durante l'esecuzione del piano T1 (notte 17-18/11/2026)

Durante l'esecuzione l'utente dormiva e aveva chiesto di proseguire senza fermarsi. Queste sono le decisioni prese al
suo posto. Per ognuna è indicato quanto costa se si rivela sbagliata.

## Decisioni

1. **Ordine dei task: 1, 3, 2, 4, 5, 6, 7, 8, 9.** Il Task 2 usa i tipi di `vgen`, che nascono nel Task 3. Costo se
   sbagliata: nessuno.
2. **Messaggio `FromGen::Palette` (hash → NBT `{name, states}`).** Aggiunto nel Task 3 e inviato dal generatore subito
   dopo `Hello`. Serve al salvataggio su disco. Costo: piccolo.
3. **Cancello della prova.** Si proseguiva anche se il generatore era solo lento. Se Steel non fosse stato usabile come
   libreria, se ne sarebbe copiata una versione in locale. Non è servito: 55 ms a chunk, nessuna modifica a Steel.
4. **Modelli per ruolo.** Implementatori Sonnet; Opus solo per i task sulle parti interne di Steel (2 e 4) e per la
   revisione finale; revisori Sonnet; ricontrolli piccoli Haiku. Costo: qualche giro di correzioni in più.
5. **Errore mio sulla formattazione.** `rustfmt.toml` era rimasto a 180 colonne invece di 160. Corretto con un commit
   di sola formattazione. Costo: nessuno.
6. **Task 2b aggiunto: strato d'acqua.** Le alghe, la posidonia e i blocchi con `waterlogged=true` hanno l'acqua nel
   secondo strato del sotto-chunk, inviato al client e salvato su disco. Senza, gli oceani avrebbero buchi asciutti.
   Costo: piccolo.
7. **Il generatore lavora a regioni 4×4 con una cache di 1024 chunk.** Risponde comunque a richieste per singolo chunk,
   ma generare a regioni costa circa 10 volte meno per chunk. Costo: memoria.
8. **Client del generatore con due modalità.** `Launch::Process` avvia il vero programma; `Launch::Connect` si collega
   a un indirizzo e serve ai test con un generatore finto. Costo: il riavvio del vero processo è provato solo dal test
   `#[ignore]` (`VERSMC_GEN=...`).
9. **Sezioni con un solo tipo di blocco.** Si scrivono senza la dimensione della palette, come fanno gophertunnel e
   Dragonfly e come già facevamo per i biomi verificati col client. Costo se sbagliata: il client rifiuterebbe quelle
   sezioni (va visto nella prova nel gioco).
10. **Rimosso l'interruttore `SUBCHUNK_V8`.** Il server usa solo la versione 9.
11. **Posizione del giocatore in `PlayerAuthInput`.** Il test è costruito dallo schema ufficiale Mojang v1.26.61, non
    da un pacchetto catturato. Costo se sbagliata: il giocatore non riceverebbe i chunk nuovi camminando (va visto nel
    gioco).
12. **Biomi.** I dati vengono da pmmp/BedrockData (1.26.30). Il bioma `dappled_forest` manca lì e viene preso da
    bedrock-samples 1.26.60.5; il colore dell'acqua è quello della pianura.
13. **La prova nel client vero non può farla un agente.** La lista di controlli è in `2026-11-18-verifica-t1.md`.
14. **Dopo un crash del generatore le richieste in corso ripartono una alla volta**, dalla meno sospetta. Così le
    richieste innocenti non vengono scartate. Dopo 3 cadute di fila senza ripresa, il server passa al mondo piatto.
    Costo: recupero più lento dopo un crash.

## Note minori rimandate (dalle revisioni)

- **Generatore:**
  - `unmapped()` conta per stato e per sezione;
  - teste a muro con `Rotation` 0;
  - stendardi senza disegni;
  - cartelli senza `TextIgnoreLegacyBugResolved`;
  - casi limite del parser JSON dei cartelli;
  - `vgen::VERSION` non incrementato (nessuna versione distribuita).
- **Generatore (servizio):**
  - un solo `Condvar` per tre `Mutex`;
  - un errore temporaneo dello spawn resta in memoria;
  - canale delle risposte senza limite;
  - test da irrobustire (timeout, figli orfani);
  - lo spawn di Nether ed End risponde con errore (arriverà con T3).
- **Client del generatore:** attesa per evento nei test; controllo `tasklist` globale; il test reale si salta in
  silenzio senza `VERSMC_GEN`.
- **Chunk:**
  - nessun `debug_assert` su indici di palette fuori range;
  - mancano test per 3, 6 e 16 bit.
- **Salvataggio:**
  - il seed di `server.toml` viene ignorato per un mondo esistente (c'è una riga di log);
  - nomi NBT oltre 65535 byte troncati;
  - la chiusura blocca il runtime;
  - niente `fsync`;
  - ordine della heightmap in `Data3D` non verificato (va visto nel gioco).
- **Streaming:**
  - chiave in coda irraggiungibile con dimensione non valida;
  - `SpawnFailed` può sovrascrivere uno spawn noto.
- **Correzioni finali:**
  - con un generatore inchiodato, la ripresa una alla volta rallenta le richieste nuove;
  - `Instant` di std usato con un intervallo tokio;
  - il timeout di 60 s può scadere per chunk lenti in coda.
- **Test end-to-end:**
  - controlla un solo chunk del nuovo bordo;
  - pulizia delle cartelle temporanee su Windows;
  - porte fisse.
