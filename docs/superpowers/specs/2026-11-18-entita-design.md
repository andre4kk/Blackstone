# Tappa 2 "entità e giocatori": design

Data: 18/11/2026. Catalogo e risposte: `docs/brainstorm/catalogo-entita.html`, `docs/brainstorm/stato-entita.md`.
Riferimenti: Dragonfly 1.26.60 (`vendor/dragonfly`, MIT), schemi Mojang `vendor/bedrock-protocol-docs` (tag v1.26.65, protocollo 2225).

## Perché

VersMCBE oggi è un mondo per giocatore singolo anche con più persone collegate: nessuno vede gli altri. La sopravvivenza
(tappa successiva) ha bisogno degli oggetti a terra, che sono entità. Questa tappa costruisce la base comune: un nucleo
delle entità, gli altri giocatori visibili e il primo tipo di entità non giocatore (oggetto a terra), in modo che la
sopravvivenza trovi tutto pronto e provato e che l'API dei plugin, più avanti, abbia un modello di entità semplice da esporre.

## Obiettivi

1. **Nucleo delle entità** (registro semplice, scelta 3A): ogni cosa che esiste nel mondo ha un ID unico, un tipo
   (giocatore, oggetto a terra), posizione, rotazione, dimensione, e l'insieme dei giocatori che la vedono.
2. **Lista giocatori**: ogni giocatore riceve la lista di tutti i collegati (nome, UUID, XUID, skin) all'ingresso;
   aggiunte e rimozioni arrivano a tutti.
3. **Comparire e sparire per distanza** (scelta 5A): un'entità è visibile a un giocatore quando il suo chunk è tra
   quelli che il giocatore ha ricevuto. Quando il chunk arriva o esce, o l'entità cambia chunk, il giocatore riceve la
   comparsa o la rimozione.
4. **Movimento degli altri** (scelta 4A): la posizione e la rotazione (corpo e testa) dichiarate dal client nel
   `PlayerAuthInput` si ritrasmettono a chi lo vede, solo se cambiate, al massimo una volta per tick (20 Hz).
5. **Animazioni e stato**: braccio che colpisce (`Animate`), accovacciarsi, corsa, nuoto, planata, volo (flag delle
   entità), oggetto in mano (`MobEquipment`) e armatura (`MobArmorEquipment`) visibili agli altri.
6. **Skin vere** (scelta 2A): la skin che il client manda nel login (dati del client nel JWT) arriva agli altri nella
   lista giocatori; se il giocatore cambia skin nel gioco (`PlayerSkin`) la nuova arriva a tutti. La skin si manda una
   sola volta per giocatore e per destinatario (nella lista), mai a ogni comparsa.
7. **Oggetti a terra** (scelta 1A): lanciare un oggetto (tasto Q, azione `Drop` delle richieste di pila, e il lancio
   trascinando fuori dall'inventario) crea un'entità oggetto davanti al giocatore con una spinta nella direzione dello
   sguardo; l'oggetto cade, si ferma sui blocchi, dopo un ritardo può essere raccolto da chi ci passa vicino
   (`TakeItemActor` e l'oggetto entra nell'inventario), e sparisce da solo dopo 5 minuti.
8. **Giocatore finto per le prove** (scelta 6A): il comando `/bot [nome]` crea un giocatore lato server accanto a chi
   lo lancia, con la stessa skin di chi lo crea, che cammina in cerchio, salta, agita il braccio e ogni tanto lancia un
   oggetto; `/bot rimuovi` lo toglie. Serve a provare tutto con un solo account.
9. **Eventi annullabili**: lancio e raccolta di oggetti passano dal meccanismo `allow(WorldAction)` già esistente.

## Non obiettivi

- Mob, intelligenza, generazione dei mob, combattimento e danni tra giocatori (tappa sopravvivenza o dopo).
- Fisica dei giocatori lato server e anticheat (scelta 4A: ci si fida del client; un controllo anti-teletrasporto è
  rimandato).
- Salvataggio delle entità oggetto: al riavvio del server gli oggetti a terra spariscono.
- Unione di oggetti a terra vicini uguali, oggetti trascinati dall'acqua, hopper, esperienza.
- Nether ed End: le entità esistono solo nell'Overworld come il resto del server.
- Verifica del login (resta un limite noto della tappa 1).

## Anti-obiettivi (sarebbe un fallimento anche se "funziona")

- Il client si disconnette o si blocca per un pacchetto malformato (AddPlayer, PlayerList, dati delle entità).
- Le skin rimandate a ogni avvicinamento: centinaia di KB ripetuti.
- Corpi che restano visibili dopo che il giocatore è uscito o si è allontanato (fantasmi), o giocatori invisibili
  dentro chunk caricati.
- Oggetti duplicati: un oggetto raccolto da due giocatori nello stesso tick, o lanciato e anche rimasto nell'inventario.
- Il proprio corpo mandato a se stessi (il client lo disegna doppio).

## Vincoli

- Protocollo 2225 (client 1.26.6x). Dal 2225 `AddPlayer`, `PlayerList` e i dati delle entità usano le varianti con
  etichetta (indice di variante varuint32 + tipo u8 + valore, come `actor.rs`); i flag delle entità hanno la numerazione
  dopo CAN_DASH(46) (collisioni 48, gravità 49). Gli schemi Mojang sono l'autorità sui byte; Dragonfly sull'ordine dei
  pacchetti e sul comportamento.
- Le codifiche che i test non possono confermare sul client vero si loggano in esadecimale per le prime volte (come
  `HEX_LOGS` della tappa 1) e sono nella lista di verifica nel gioco.
- Ciclo unico a 20 Hz, come oggi; nessun nuovo thread per le entità.
- Stile del codice: commenti e log in italiano, rustfmt 160, clippy `-D warnings`, test per ogni comportamento.
- Licenze: server MIT; dati da Dragonfly (MIT) con attribuzione; niente codice dal generatore AGPL.

## Approcci considerati

- **Nucleo delle entità.** Scelto **registro semplice** (mappa ID → entità con tipo enum e dati comuni), nello stile
  del codice attuale e facile da esporre ai plugin. Scartati: ECS (`hecs`/`bevy_ecs`), che cambia lo stile di tutto il
  server per un vantaggio che serve solo con migliaia di entità; oggetti con tratti come Dragonfly, più rigidi in Rust
  con lo stato condiviso.
- **Movimento.** Scelto **ritrasmettere** ciò che dichiara il client. Scartati: controllo anti-teletrasporto (rimandato),
  fisica completa lato server (anticheat, troppo lavoro).
- **Visibilità.** Scelta **per chunk caricati**, come vanilla e Dragonfly. Scartato "tutti vedono tutti" (traffico
  inutile, corpi fuori dai chunk).
- **Skin.** Scelta **skin vera dal login** subito, invece di Steve/Alex provvisori: il pezzo più pesante si fa una volta.
- **Prove.** Scelto il **giocatore finto `/bot`**: con un solo account oggi non si possono avere due giocatori (il
  secondo ingresso dello stesso account butta fuori il primo).

## Decisioni già prese (dall'utente)

Ampiezza 1A, skin 2A, nucleo 3A, movimento 4A, visibilità 5A, prove con `/bot` 6A (vedi `stato-entita.md`).

## Lasciato a chi costruisce

- Struttura precisa del registro (moduli, nomi), purché: un solo spazio di ID runtime per tutte le entità, unico e
  stabile per la vita dell'entità; il proprio ID runtime del giocatore resta quello di `StartGame`.
- Quale pacchetto di movimento usare per i giocatori e per gli oggetti (`MovePlayer`, `MoveActorDelta`,
  `MoveActorAbsolute`), seguendo Dragonfly.
- Fisica dell'oggetto a terra: gravità, attrito e appoggio sui blocchi con valori vanilla (riferimento Dragonfly
  `entity/item_behaviour.go` e il movimento delle entità); ritardo di raccolta e raggio di raccolta vanilla.
- Comportamento del bot (percorso, frequenza dei salti e dei lanci), purché esercizi movimento, salto, braccio,
  accovacciarsi, oggetto in mano e lancio.
- Quali stati di entità mandare nei dati (`SetActorData`) oltre a flag e riquadro, seguendo Dragonfly.

## Scelte mie (dopo la lettura del revisore, da controllare)

- In creativa si raccolgono gli oggetti a terra, come in vanilla. Con l'inventario pieno l'oggetto resta a terra; se
  entra solo in parte, la parte che non entra resta a terra.
- Gli oggetti che il bot lancia nascono dal nulla (non ha un inventario vero): servono solo a provare lancio e raccolta.
- Il bot è un'entità di tipo giocatore nel registro, senza sessione di rete, non un `Player` collegato: non salva dati,
  non riceve pacchetti e non conta nel limite dei giocatori; appare in lista giocatori e in `/list` con il suo nome.
- Se il login non porta UUID o XUID, il server ricava un UUID stabile dall'identità del giocatore (stessa identità usata
  per i dati salvati), così la lista giocatori ha sempre un UUID.
