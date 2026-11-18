# Tappa 3 "sopravvivenza base": design

Data: 19/11/2026 (notte). Ramo `feat/sopravvivenza`, che parte da `feat/entita` (tappa 2, non ancora provata del tutto nel gioco).
Riferimenti: Dragonfly 1.26.60 (`vendor/dragonfly`, MIT) per il comportamento; gophertunnel v1.63.0 (`vendor/gophertunnel`) per i byte.

## Come è nato

Prima di andare a dormire l'utente ha scelto (19/11/2026): **A** sopravvivenza base, **A** sopra `feat/entita`, **A** "decidi tu
e annota". Non c'è stato un giro di domande: tutte le scelte qui sotto sono **mie**, prese seguendo vanilla e Dragonfly, e vanno
riviste dall'utente al risveglio (elenco in fondo, "Scelte mie").

## Perché

Oggi la modalità sopravvivenza esiste solo di nome: si rompono i blocchi senza ottenere niente, non si prende danno, non si
muore e non si ha fame. La tappa 2 ha costruito le entità e gli oggetti a terra; questa tappa li usa per dare al gioco il
ciclo minimo della sopravvivenza: rompi → raccogli → mangia → rischi di farti male → muori → rinasci. È anche la base di cui
avranno bisogno il crafting (tappa dopo) e i mob.

## Obiettivi

1. **Blocchi che lasciano oggetti.** In sopravvivenza un blocco rotto lascia a terra quello che lascerebbe in vanilla
   (pietra → ciottolo, erba → terra, minerali → il loro oggetto), solo se lo strumento in mano è adatto (pietra e minerali
   chiedono il piccone del livello giusto). Tabella ricavata da Dragonfly con uno script, come per oggetti e creativo.
   In creativo nessun oggetto.
2. **Vita.** 20 punti, mostrati dal client; danno con mezzo secondo di invulnerabilità (10 tick), animazione del colpo vista
   da sé e dagli altri; salvata nei dati del giocatore. Creativo e spettatore non prendono danno (tranne il vuoto).
3. **Danno da caduta** (oltre 3 blocchi, come vanilla; niente danno cadendo in acqua o volando) e **danno del vuoto** (sotto
   il fondo del mondo).
4. **Morte e rinascita.** A 0 punti vita il giocatore muore: messaggio in chat, l'inventario cade a terra, gli altri vedono
   la morte; il client mostra la schermata di morte e con "Rinasci" torna al punto di partenza del mondo con vita e fame piene.
5. **Fame.** Punti fame, saturazione ed esaurimento come vanilla/Dragonfly (corsa, salto, rottura, attacco, danno). Con fame
   alta si recupera vita; a fame 0 si perde vita fino a mezzo cuore (difficoltà normale). Salvata nei dati del giocatore.
6. **Mangiare.** Tenendo premuto con un cibo in mano si mangia: dopo 1,6 secondi il cibo si consuma e ridà fame e saturazione
   secondo la tabella di Dragonfly. Non si mangia a fame piena (tranne i cibi che lo permettono in vanilla).
7. **Combattimento corpo a corpo tra giocatori** (e contro i bot): colpire fa danno secondo l'oggetto in mano (spade, asce,
   ecc. dalla tabella di Dragonfly; a mano 1), con spinta all'indietro e portata controllata. I bot hanno vita e muoiono.
8. **`/kill`** per provare morte e rinascita senza cercare un burrone.

## Non obiettivi

- Crafting (tabella 2x2, banco da lavoro, fornace): tappa successiva.
- Usura degli strumenti, incantesimi (tocco di seta, fortuna), esperienza.
- Armature che riducono il danno, effetti (veleno, rigenerazione), mela d'oro con effetti (si mangia come cibo normale).
- Danno da lava, fuoco, annegamento, soffocamento, cactus; mob ostili; difficoltà diverse da "normale".
- Tempo di rottura controllato dal server (ci si fida del client come per il movimento, scelta 4A della tappa 2).
- Letto e punto di rinascita personale: si rinasce al punto di partenza del mondo.

## Anti-obiettivi (fallimenti anche se "funziona")

- Il client si disconnette per un pacchetto malformato (Respawn, ActorEvent, UpdateAttributes, transazioni di attacco).
- Oggetti duplicati: blocco rotto che lascia due volte l'oggetto, inventario che cade a terra e resta anche nell'inventario.
- Morte che lascia il giocatore bloccato (schermata di morte senza poter rinascere) o "fantasmi" per gli altri.
- Danno in creativo, o danno da caduta dopo aver volato in creativo e cambiato modalità a mezz'aria.
- Vita o fame che tornano piene a ogni rientro (devono essere salvate).

## Vincoli

- Come le tappe 1 e 2: protocollo 2225; per i byte vale gophertunnel della stessa versione; Dragonfly per comportamento e
  ordine dei pacchetti; codifiche non confermate loggate in esadecimale per le prime volte e messe nella lista di verifica nel gioco.
- Ciclo unico a 20 Hz; commenti e log in italiano; rustfmt 160; clippy `-D warnings`; test per ogni comportamento.
- Licenze: server MIT; tabelle da Dragonfly (MIT) con attribuzione; niente codice dal generatore AGPL.
- Ogni azione nuova che cambia il mondo o un giocatore passa da `allow(WorldAction)` (ganci per i futuri plugin): danno,
  morte, rottura con oggetti, mangiare, attacco.

## Approcci considerati

- **Tabella degli oggetti lasciati.** Scelto: script Python che legge i file `server/block/*.go` di Dragonfly e produce un
  JSON (blocco → strumento richiesto e livello, oggetti lasciati, probabilità semplici), con una tabella di eccezioni a mano
  per quel che lo script non capisce. Scartati: scriverla tutta a mano (centinaia di blocchi, errori certi); far girare
  Dragonfly in Go (Go non è installato su questa macchina); le loot table Java del generatore (codice AGPL accanto).
- **Danno da caduta.** Scelto: il server calcola la distanza di caduta dalle posizioni dichiarate dal client (come Dragonfly,
  che non rifà la fisica), azzerata in acqua, volando o planando. Scartato: fidarsi del pacchetto di danno del client
  (non esiste per la caduta) o rifare la fisica (anticheat, fuori tappa).
- **Morte.** Scelto: il flusso di Dragonfly (`Respawn` con stati, vita a 0 negli attributi, `ActorEvent` di morte agli
  altri). Scartato: rimandare al punto di partenza senza schermata di morte (non è il gioco vero).

## Scelte mie (da rivedere)

1. L'inventario cade a terra alla morte (niente "keepInventory"); si può cambiare con `keep_inventory = true` nella config.
2. PvP attivo per default; si spegne con `pvp = false` nella config.
3. Difficoltà fissa "normale": la fame toglie vita fino a mezzo cuore, non uccide.
4. Creativo immune a tutto tranne il vuoto; spettatore immune a tutto.
5. Si rinasce al punto di partenza del mondo (quello di `StartGame`), con inventario vuoto, vita 20, fame 20, saturazione 5.
6. I bot hanno 20 punti vita, muoiono e spariscono (niente rinascita, niente oggetti lasciati).
7. Oggetti dei blocchi: compaiono al centro del blocco con una piccola spinta casuale e si raccolgono dopo 10 tick (vanilla).
8. Messaggi di morte in italiano dal server (es. "X è caduto da troppo in alto", "X è stato ucciso da Y"), non quelli tradotti
   dal client.

## Lasciato a chi costruisce

- I numeri esatti (esaurimento, rigenerazione, spinta, invulnerabilità, soglia del vuoto) presi da Dragonfly e citati.
- La forma del modulo della vita/fame (es. `crates/server/src/survival.rs`), purché vita e fame vivano nel `Player` e si
  salvino in `playerdata`.
- I pacchetti e il loro ordine per morte, rinascita, mangiare e attacco, seguendo Dragonfly e i byte di gophertunnel.
