# Verifica nel gioco della tappa 2 (entità e giocatori)

Questa lista serve a te per provare il server con Minecraft Bedrock (client 1.26.6x). Non serve sapere programmare: segui i passi e
annota cosa vedi. Dove una cosa non va, qui sotto trovi cosa significa e cosa mandarmi (il log del server, la finestra nera).

Alcune codifiche dei pacchetti non erano verificabili senza il gioco vero: sono nell'ultima sezione. Se qualcosa si rompe, quasi
sempre è lì.

## Preparazione

1. Avvia il server (`cargo run`) e tieni aperta la finestra del log.
2. Entra con il tuo account. Per sicurezza, prima di cominciare, fai una copia della cartella `worlds`.
3. Se hai un secondo account (o un amico), entra anche con quello: alcune prove sono possibili solo in due.

## A. Con un solo account: il bot

Scrivi in chat `/bot Prova`. Compare un giocatore finto con la tua skin. Controlla:

- [ ] **Lista giocatori**: tieni premuto il tasto della lista (di solito Tab) o apri il menu: c'è "Prova" accanto al tuo nome.
- [ ] **Corpo**: vedi un giocatore intero (testa, corpo, braccia, gambe), con la tua skin e il nome "Prova" sopra la testa.
- [ ] **Movimento fluido**: cammina in cerchio attorno al punto in cui l'hai creato (raggio 3 blocchi, un giro ogni 10 secondi) senza scatti.
- [ ] **Testa**: guarda nella direzione in cui cammina.
- [ ] **Salto**: ogni 3 secondi circa salta e ricade.
- [ ] **Braccio**: ogni 2 secondi circa agita il braccio.
- [ ] **Accovacciarsi**: ogni 5 secondi si accovaccia per 1 secondo (il nome si abbassa e il corpo si piega).
- [ ] **Oggetto in mano**: ogni 5 secondi cambia (pietra, spada di diamante, mela, piccone di ferro).
- [ ] **Skin**: è la tua e non quella di Steve.
- [ ] **Lancio**: ogni 10 secondi lancia una pietra (dal nulla): la vedi volare, cadere e restare a terra.
- [ ] **Raccolta**: avvicinati alla pietra dopo 2 secondi dal lancio: finisce nel tuo inventario con il suono di raccolta.
- [ ] `/list` mostra anche "Prova" e il conto giusto (tu + bot).
- [ ] `/bot rimuovi`: il bot sparisce dal mondo e dalla lista. Riscrivendo `/bot` ne crei uno nuovo (senza nome si chiama "Bot").
- [ ] Esci e rientra con un bot attivo: è ancora lì (i bot restano finché non li rimuovi o spegni il server).

## B. Oggetti a terra (senza bot)

- [ ] **Lancio con Q**: con un oggetto in mano premi Q: ne esce uno solo, davanti a te. Con Ctrl+Q esce tutta la pila.
- [ ] **Trascinare fuori**: apri l'inventario, trascina un oggetto fuori dalla finestra: viene lasciato a terra.
- [ ] **Raccolta**: l'oggetto che hai lanciato non si raccoglie subito (circa 2 secondi di attesa), poi ti torna in tasca.
- [ ] **Inventario pieno**: riempi tutte le caselle, lancia un oggetto diverso e riprova a raccoglierlo: resta a terra, nessun errore.
- [ ] **Sparizione**: lascia un oggetto a terra e aspetta 5 minuti: sparisce da solo.

## C. Con due account

- [ ] Ciascuno vede il corpo dell'altro con la sua skin e il suo nome, e nella lista ci sono entrambi.
- [ ] Movimento fluido, testa che segue lo sguardo, braccio quando uno rompe un blocco o colpisce l'aria.
- [ ] Accovacciarsi, correre, nuotare e planare (elitre) si vedono dall'altro.
- [ ] Oggetto in mano e armatura (da creativo) si vedono dall'altro e cambiano quando li cambi.
- [ ] Cambio skin dal menu del gioco: l'altro vede la nuova skin.
- [ ] Uno esce: sparisce dal mondo dell'altro e dalla sua lista.
- [ ] Uno lancia un oggetto, l'altro lo raccoglie.

## Come riconoscere i guasti

| Cosa vedi | Probabile causa |
| --- | --- |
| Il gioco si disconnette appena crei il bot o entra un secondo giocatore | `PlayerList` o `AddPlayer` scritti male |
| Il bot non ha skin (Steve) o il nome è sbagliato / vuoto | ordine delle due metà dell'UUID, o dettagli di `SerializedSkin` |
| Il gioco si disconnette quando si lancia un oggetto | `AddItemActor` |
| Il bot compare ma resta fermo, o è in aria / sotto terra | posizione del bot (l'altezza è quella di chi lo crea: se sei in volo o su un gradino, il bot è alla tua quota) |
| Il bot non si accovaccia, o fa un'altra cosa | numero del flag di stato (sotto) |
| Il braccio non si agita, o fa un gesto strano | codice dell'azione `Animate` |
| La skin cambiata non arriva all'altro | `PlayerSkin` |

## Codifiche da confermare (e log da mandarmi)

Il server scrive nel log (finestra nera) i pacchetti in esadecimale per le prime volte. Se una di queste cose non funziona, copia
le righe del log vicine al momento del guasto.

1. **`PlayerList`**: ogni voce ha una variante (Add o Remove) e un `Action` (0 = aggiungi, 1 = togli). Guasto: disconnessione alla comparsa di un giocatore o del bot.
2. **Ordine delle metà dell'UUID**: scritto prima la metà alta, poi la bassa (little-endian). Guasto: nome o skin sbagliati, giocatore "fantasma" che non sparisce dalla lista.
3. **`SerializedSkin`**: il numero dei colori prima della lista (`Colors count`), il flag di fiducia (`TrustedSkinFlag`, un byte) e la misura delle braccia (`ArmSize`, un byte). Guasto: disconnessione o skin deformata.
4. **Flag di stato**: accovacciato = 1, in corsa = 3, in planata = 32, a nuoto = 57. Guasto: lo stato non si vede o se ne vede un altro (per esempio il bot nuota invece di accovacciarsi).
5. **`AddPlayer` e `AddItemActor`**: la struttura dei campi (dati sincronizzati con `oneOf`). Guasto: disconnessione quando un giocatore o un oggetto entra nel raggio visivo.
6. **`PlayerSkin`**: nel log compaiono le righe `PlayerSkin (N byte): [..]` (le prime 3 per giocatore, tagliate a 400 byte). Servono a confrontare la struttura se il cambio skin non funziona. Il corpo completo dei primi 3 `PlayerSkin` per giocatore finisce anche in `captures/playerskin-<pid>-<n>.bin` (cartella ignorata da git): allegare quei file se il cambio skin fallisce.
7. **`Animate`**: l'azione è un byte (1 = agita il braccio). Guasto: il braccio non si muove.

Anche le richieste di pila dell'inventario e gli input con azioni (lancio con Q, trascinamento fuori) sono loggati in esadecimale
per le prime volte: se il lancio non funziona, mandami quelle righe.
