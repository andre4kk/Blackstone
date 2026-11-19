# Verifica nel gioco della tappa 3 (sopravvivenza base)

Questa lista serve a te per provare il server con Minecraft Bedrock (client 1.26.6x). Non serve sapere programmare: segui i passi
e annota cosa vedi. Dove una cosa non va, in fondo trovi cosa significa e cosa mandarmi (il log del server, la finestra nera).

Alcune codifiche dei pacchetti non erano verificabili senza il gioco vero: sono nella sezione "Codifiche da confermare". Se
qualcosa si rompe, quasi sempre è lì.

## Preparazione

1. Avvia il server (`cargo run`) e tieni aperta la finestra del log.
2. Prima di cominciare fai una copia della cartella `worlds`.
3. Entra con il tuo account. Si entra in creativo: per le prove di sopravvivenza scrivi `/gamemode s`.
4. Per le prove con un bersaglio scrivi `/bot Prova`: compare un giocatore finto. Un secondo account è comodo ma non obbligatorio.
5. Per cambiare modalità al volo: `/gamemode c` (creativo, per volare e prendere oggetti) e `/gamemode s` (sopravvivenza).

## Prima: correzioni della tappa 2 da riprovare

Sono tre cose che nella tappa 2 non andavano e ho sistemato. Provale per prime.

### 1. Cambio di skin

1. Entra nel mondo.
2. Apri il menu del gioco e cambia skin.
3. Guarda il log: deve esserci la riga `PlayerSkin (N byte): [...]`.
4. Subito dopo NON deve esserci la riga `PlayerSkin con l'UUID di un altro giocatore: ignorato`.
5. La skin nuova compare su di te. Con un secondo account, la vede anche lui.

Cosa ho cambiato: prima il server rifiutava il cambio perché l'identificativo (UUID) non coincideva; ora lo ricava dal tuo XUID.

### 2. Q dalla barra dopo aver piazzato un blocco

1. In creativo piazza un blocco (per esempio pietra).
2. Restando con quel blocco in mano, premi Q.
3. Ne esce uno solo, davanti a te.
4. Premi Ctrl+Q: esce tutta la pila.
5. Nel log non deve comparire una riga `richiesta di pila ... rifiutata`.

### 3. Nuove righe del log "butta …" e "raccoglie …"

1. Lancia un oggetto con Q.
2. Nel log c'è una riga come `[#1] butta <oggetto> x1: oggetto a terra #N in [x, y, z]`.
3. Aspetta circa 2 secondi e cammina sull'oggetto.
4. Nel log c'è `[#1] raccoglie <oggetto> x1 dall'oggetto a terra #N (resto 0)`.
5. Se l'inventario era pieno, "resto" è maggiore di 0 e l'oggetto resta a terra.

## A. Rompere blocchi in sopravvivenza

Scrivi `/gamemode s`. Se ti servono attrezzi, prendili in creativo e poi torna in sopravvivenza.

1. **Pietra a mano**: rompi un blocco di pietra con la mano vuota. Non lascia niente.
2. **Pietra col piccone**: rompila con un piccone di legno. Lascia un ciottolo (non pietra), che si raccoglie dopo mezzo secondo circa.
3. **Erba**: rompi un blocco d'erba con la mano. Lascia terra.
4. **Carbone**: rompi il minerale di carbone col piccone di legno. Lascia carbone, non il blocco.
5. **Ferro, legno**: col piccone di legno il minerale di ferro non lascia niente.
6. **Ferro, pietra**: col piccone di pietra lo stesso minerale lascia ferro grezzo.
7. **Diamante, pietra**: col piccone di pietra il minerale di diamante non lascia niente.
8. **Diamante, ferro**: col piccone di ferro lascia un diamante.
9. **Ghiaia**: rompi una decina di blocchi di ghiaia. Quasi sempre lascia ghiaia, ogni tanto selce.
10. **Foglie**: senza cesoie non lasciano niente; con le cesoie lasciano la foglia.
11. **Niente doppioni**: rompendo un blocco esce un solo oggetto, mai due.
12. **Creativo**: con `/gamemode c` rompi un blocco: non lascia niente.

## B. Danno da caduta

Per salire in alto: in creativo vola, poi `/gamemode s` e lasciati cadere (il conteggio parte da quando smetti di volare).

1. **Da 5 blocchi su terra**: perdi 1 cuore (2 punti). Vedi un lampo rosso.
2. **Da 20 blocchi su terra**: perdi quasi tutto, restano 3 punti (un cuore e mezzo).
3. **Da 3 blocchi**: nessun danno.
4. **In acqua**: da 20 blocchi in un lago, nessun danno.
5. **Scala**: scendi una scala lunga, nessun danno.
6. **In creativo**: da 20 blocchi, nessun danno.
7. **Volo, poi sopravvivenza**: vola a 100 blocchi, passa a sopravvivenza a mezz'aria e atterra: il danno conta solo dal punto del cambio in giù, non da 100.
8. **Vuoto**: vola sotto il fondo del mondo (quota -64). Perdi 2 cuori ogni mezzo secondo e a un certo punto muori. Succede anche in creativo.

## C. Fame

1. **Corri a lungo**: corri per un paio di minuti e salta ogni tanto. La barra della fame cala piano.
2. **Rompere blocchi**: fa calare la barra, ma molto più lentamente.
3. **Sotto 6 punti di fame** non si può correre: la corsa si ferma da sola.
4. **Rigenerazione**: con la fame alta (almeno 18) e un po' ferito, la vita torna da sola, mezzo cuore ogni pochi secondi.
5. **Fame a zero**: lascia calare la fame fino a 0. La vita scende fino a mezzo cuore e si ferma: non si muore di fame (difficoltà normale).

## D. Mangiare

1. Prendi del pane in creativo e passa a sopravvivenza, con la fame non piena.
2. Tieni premuto il tasto destro con il pane in mano. Si vedono le briciole ogni mezzo secondo circa.
3. Dopo circa 1,6 secondi il pane si consuma e la fame sale di 5 punti.
4. Rifallo rilasciando il tasto prima: non si mangia e il pane resta.
5. Cambia casella mentre mangi: si interrompe.
6. A fame piena il pane non si mangia. La mela d'oro sì.
7. In creativo si mangia ma il cibo non finisce.
8. Nel log, solo le prime 3 volte, cerca `InventoryTransaction di uso nell'aria (N byte): [...]` e `InventoryTransaction di rilascio (N byte): [...]`. Mi dicono quale pacchetto manda il gioco alla fine del pasto.

## E. Morte e rinascita

1. **`/kill`**: scrivilo in sopravvivenza. Compare la schermata di morte con il pulsante "Rinasci" e in chat "<nome> è morto".
2. **Rinascita**: premi il pulsante. Torni al punto di partenza del mondo con vita 20, fame 20 e inventario vuoto. Non resti bloccato.
3. **Inventario pieno**: riempi barra e inventario (e l'armatura, da creativo), passa a sopravvivenza e scrivi `/kill`.
4. Tutto cade a terra dove sei morto. Rinasci, torna lì e raccogli (dopo mezzo secondo): niente doppioni, e nulla resta nell'inventario alla morte.
5. **Morte da caduta**: cadi da 30 blocchi. Messaggio "<nome> è caduto da troppo in alto".
6. **Morte nel vuoto**: messaggio "<nome> è caduto fuori dal mondo".
7. **Con un secondo account**: l'altro vede il corpo cadere e sparire dopo circa un secondo, legge il messaggio in chat, e ti rivede quando rinasci.
8. **Nel log**: `[#1] morto: Kill` (o `Fall`, `Void`, `Starvation`) e poi `[#1] rinasce in [x, y, z]`.
9. Se non riesci a rinascere, mandami le righe `Respawn (N byte): [...]` (al massimo 3).
10. Da morto non puoi muoverti, rompere né raccogliere.

## F. Uscire e rientrare ferito

1. Ferisciti (caduta da 12 blocchi circa) e lascia calare un po' la fame.
2. Esci dal gioco (non dal server) e rientra.
3. Vita e fame sono come le avevi lasciate, non piene.
4. Ripeti spegnendo e riaccendendo il server (Ctrl+C e riavvio): stesso risultato.
5. **Morto e uscito**: muori ed esci dalla schermata di morte. Rientri vivo al punto di partenza, con vita e fame piene e inventario vuoto (era già caduto a terra).

## G. Colpire il bot

Scrivi `/bot Prova`. Il bot gira in cerchio attorno a te.

1. Colpiscilo a mani nude: ogni colpo gli toglie 1 punto di vita. Vedi il lampo rosso.
2. Colpiscilo con una spada di diamante (da creativo, poi sopravvivenza): ogni colpo toglie 7 punti, ne bastano 3.
3. A vita 0 il bot sparisce dal mondo e dalla lista giocatori. Non lascia oggetti.
4. Il bot non viene spinto all'indietro: è normale, cammina su un percorso fisso.
5. Ogni tuo colpo a segno ti toglie un po' di fame.
6. Portata: da molto lontano (oltre 8 blocchi) il colpo non arriva.
7. Nel log, solo le prime 3 volte: `InventoryTransaction di attacco (N byte): [...]`.
8. **Con due account**: colpisci l'altro. Perde vita, vede il lampo rosso e viene spinto all'indietro. Se muore, in chat compare "<nome> è stato ucciso da <nome>".

## H. PvP spento e inventario conservato

1. Chiudi il server e apri `server.toml` (se non c'è, si crea al primo avvio con i valori di `server.example.toml`).
2. Metti `pvp = false`, riavvia ed entra con due account in sopravvivenza.
3. Colpire l'altro giocatore non fa niente (né danno né spinta). Colpire il bot funziona ancora.
4. Rimetti `pvp = true`.
5. Prova anche `keep_inventory = true`: dopo `/kill` l'inventario resta con te e non cade niente a terra.
6. Rimetti `keep_inventory = false`.

## Come riconoscere i guasti

| Cosa vedi | Probabile causa |
| --- | --- |
| Il gioco si disconnette quando muori o rinasci | `Respawn` o `MovePlayer` di rinascita scritti male |
| La schermata di morte non lascia rinascere | il gioco manda un pacchetto di rinascita diverso da quello atteso (righe `Respawn (N byte)`) |
| La barra della vita o della fame non si muove o è sbagliata | `UpdateAttributes` |
| Niente lampo rosso quando ti fai male o colpisci | `ActorEvent` (HURT = 2) |
| Per l'altro giocatore il corpo di un morto non cade o non sparisce | `ActorEvent` (DEATH = 3) o scomparsa del corpo |
| Mangi ma la fame non sale, o il cibo non si consuma | pasto: il pacchetto che chiude il pasto (righe `uso nell'aria` / `rilascio`) |
| Colpisci ma non succede niente | transazione di attacco (righe `di attacco`) |
| La caduta toglie troppo o troppo poco | calcolo della distanza di caduta dal movimento |
| Muori a ogni caduta anche in acqua o su una scala | stato dei piedi (acqua, scala) |
| Rientri e vita o fame sono sempre piene | salvataggio dei dati del giocatore |
| Rompi un blocco e non esce niente (e dovrebbe) | tabella dei blocchi: mandami il nome del blocco e lo strumento in mano |

## Codifiche da confermare (e log da mandarmi)

Il server scrive nel log (finestra nera) i pacchetti in esadecimale per le prime 3 volte per giocatore. Se una di queste cose non
funziona, copia le righe del log vicine al momento del guasto.

1. **`Respawn`**: righe `[#N] Respawn (N byte): [...]`. Il gioco manda lo stato "pronto a rinascere" con il tuo ID; il server risponde con un teletrasporto (`MovePlayer`) e un `Respawn`. Guasto: la schermata di morte non finisce.
2. **`MovePlayer` di rinascita**: modo teletrasporto, con il campo `TeleportData` (due numeri a 32 bit). Guasto: disconnessione alla rinascita.
3. **`ActorEvent`**: HURT = 2, DEATH = 3, mangiare = 57. Guasto: niente lampo rosso, corpo che non cade, niente briciole.
4. **`UpdateAttributes`**: vita, fame, saturazione. La vita arriva al gioco arrotondata per eccesso. Guasto: barre ferme o sbagliate.
5. **Uso nell'aria e rilascio** (`InventoryTransaction`): righe `di uso nell'aria` e `di rilascio`. Servono a capire quale pacchetto chiude il pasto; il server li accetta entrambi.
6. **Attacco** (`InventoryTransaction` UseItemOnEntity): riga `di attacco`. Contiene l'ID del bersaglio, l'azione (1 = colpo) e la casella in mano. Guasto: i colpi non arrivano.
7. **Flag di salto** nell'input (`PlayerAuthInput`): serve a contare l'esaurimento del salto. Se saltando la fame non cala mai, è qui.
8. **Dati del giocatore**: `Health`, `foodLevel`, `foodSaturationLevel`, `foodExhaustionLevel` (nomi di PocketMine, Dragonfly non li salva). Guasto: vita o fame piene a ogni rientro.

## "Scelte mie" da confermare

Non potevi rispondermi di notte, quindi ho deciso io. Per ognuna dimmi se va bene o la cambio.

1. **L'inventario cade a terra alla morte.** Si cambia con `keep_inventory = true` nella config.
2. **PvP attivo per default.** Si spegne con `pvp = false`.
3. **Difficoltà fissa "normale"**: la fame toglie vita fino a mezzo cuore e non uccide.
4. **Creativo immune a tutto tranne il vuoto; spettatore immune a tutto.**
5. **Si rinasce al punto di partenza del mondo**, con inventario vuoto, vita 20, fame 20 e saturazione 5 (niente letto).
6. **I bot hanno 20 punti vita e muoiono**: spariscono senza rinascere e senza lasciare oggetti.
7. **Gli oggetti dei blocchi** compaiono al centro del blocco con una piccola spinta e si raccolgono dopo mezzo secondo.
8. **Messaggi di morte in italiano**, scritti dal server (non quelli tradotti dal gioco).

Altre decisioni prese lavorando (dal registro):

9. Livelli degli strumenti come Dragonfly: diamante e netherite al livello 4. La spada di diamante fa 7, non 8.
10. I blocchi senza voce in tabella lasciano se stessi solo se esiste un oggetto con quel nome, altrimenti niente.
11. Il vuoto fa danno sotto la quota -64 (come Dragonfly), non -128.
12. La fame si ferma a mezzo cuore (1 punto), come nel progetto; Dragonfly si ferma a 2.
13. Il pasto finisce con un tick di tolleranza (come Dragonfly), per non perdere pasti per i ritardi di rete.
14. L'azione di rinascita dentro `PlayerAction` è ignorata, come in Dragonfly: si rinasce solo col pacchetto `Respawn`. Se il tuo gioco non rinasce, parti da qui.
15. Il bot non viene spinto all'indietro: cammina su un percorso fisso.

## Cosa NON c'è ancora (per non cercarlo)

Crafting, armature che riducono il danno, usura degli strumenti, incantesimi, esperienza, effetti (veleno, rigenerazione), mob, lava,
fuoco, annegamento, soffocamento, letto e punto di rinascita personale, colture, difficoltà diverse da normale.
