# Verifica tappa 1 (creativo vero) — risultati e lista di controllo

Data: 2026-11-19. Ramo `feat/creativo`.

## Test automatici

| Comando | Esito |
|---|---|
| `cargo test --workspace` | tutto PASS: net 5, protocol 53, server 227 (1 ignorato), end-to-end `join` 2, vgen 10 |
| `cargo clippy --workspace --all-targets -- -D warnings` | nessun avviso |
| `cargo fmt --all -- --check` | pulito |

Il test end-to-end `join` (client WebRTC finto) controlla che all'ingresso arrivino abilità, contenuto dell'inventario e menu
creativo (`CreativeContent`), nell'ordine giusto.

## Prova a vuoto del server (senza client)

Avviato `bedrock-server` in una cartella temporanea (seme 13579, porta 19141, generatore reale in release), 25 s, poi fermato
(nessun processo rimasto). Il log mostra: mondo aperto, server in ascolto, `[gen] generatore pronto`, palette di 17793 voci,
spawn calcolato -96, 66, 32. Alla chiusura forzata il generatore segnala la connessione chiusa: atteso.

## Lista di controllo da fare con il vero Minecraft

Preparazione: `cd generator; cargo build --release` (una volta), poi `cargo run` dalla radice (`server.toml` si crea da solo).
Per partire puliti cancella `worlds/Mondo`. Il log è l'output a terminale.

1. **Ingresso in creativo.** Entra dalla scheda Amici/LAN. Devi essere in modalità creativa, con la barra rapida vuota e senza
   disconnessioni.
2. **Menu creativo.** Apri l'inventario: devono esserci le schede (costruzione, natura, oggetti, ecc.) con le voci in ordine.
   Le 36 icone dei gruppi usano il primo oggetto del gruppo (non verificato): segnala se mancano o sono sbagliate.
3. **Prendere oggetti.** Clicca un blocco nel menu: deve finire in barra. Prova stack (shift-clic o clic ripetuto), spostare
   tra barra e inventario, buttare nel cestino. Nel log cerca le righe `ItemStackRequest (N byte): [...]` (esadecimali delle
   prime 3 richieste per giocatore): copiale, servono per confermare la codifica (non verificata). Se compare
   `ItemStackRequest illeggibile` il menu non funziona del tutto.
4. **Barra.** Cambia slot con rotella e tasti 1-9: l'oggetto in mano deve cambiare (log `MobEquipment` se illeggibile).
5. **Rompere.** Rompi terra, pietra, erba alta, foglie: in creativo si rompe subito. Il blocco sparisce e non ricompare.
   Non ci sono drop (tappa 2).
6. **Piazzare.** Prova e guarda l'orientamento:
   - scale (anche angoli tra scale vicine), lastre (sopra/sotto, doppia lastra), tronchi (asse secondo la faccia cliccata);
   - porte (solo dalla faccia superiore di un blocco, doppie non collegate), letti (due metà), muri (si collegano ai vicini),
     torce, leve, bottoni, lanterne (appese/appoggiate), scale a pioli, rampicanti, piante;
   - regole NON verificate sul client: `end_rod`, blocchi multi-faccia, `block_face`, osservatore, repeater/comparator.
     Segnala quelli girati al contrario.
   Piazzando un blocco accanto a uno già piazzato i vicini devono aggiornarsi (muri, recinti, scale).
7. **Interazioni.** Porte, botole, cancelli: si aprono/chiudono con clic (la porta cambia anche l'altra metà). Leve e bottoni:
   scattano; il bottone torna da solo. Suoni: place, door.open ecc. (i nomi non sono verificati; manca il suono = nome sbagliato).
8. **Accovacciarsi.** Con Shift piazza un blocco su una porta/leva: deve piazzare e non interagire (flag sneaking non verificato).
9. **Uscita e rientro.** Piazza qualche blocco, metti oggetti in barra, esci e rientra: stessi oggetti, stessa posizione, stessa
   modalità. Ferma il server con Ctrl+C (`[mondo] chiusura: N chunk salvati`), riavvia, rientra: blocchi modificati ancora lì.
   Il salvataggio automatico è ogni 60 s.
9b. **Doppio ingresso.** Entra con lo stesso account da un secondo client: il primo deve essere espulso.
10. **/gamemode.** `/gamemode survival`, `/gamemode creative`, `/gm 1`: cambia modalità senza uscire; in survival le abilità di
    volo spariscono e il menu creativo non è disponibile. Dopo il riavvio la modalità resta.
11. **Mondo su disco (facoltativo).** Il mondo si apre con Amulet; i dati giocatore sono in formato BDS (non verificato).

## Righe di log da controllare

- `PlayerAuthInput illeggibile (N byte): ...`: layout non verificato; se compare le azioni (rompere/piazzare) si perdono ma il
  movimento funziona. Copia la riga.
- `ItemStackRequest (N byte): [...]`: esadecimali delle prime 3 richieste (vedi punto 3); `... illeggibile`: decodifica fallita.
- `InventoryTransaction illeggibile`, `PlayerAction illeggibile`, `MobEquipment illeggibile`, `SetPlayerGameType illeggibile`,
  `comando illeggibile`.
- `!! IL CLIENT SEGNALA UN PACCHETTO MALFORMATO` / `violazione segnalata dal client`: il client rifiuta un pacchetto (id nel testo).
- `blocco sconosciuto (hash ...)`: blocco fuori palette.
- `[gen] ...`: generatore caduto o assente (mondo piatto di ripiego).

## Interruttori di diagnostica

- `VERSMC_DEBUG=nozero,nowater,plains,nobe` (separati da virgola): parti dei chunk (storage a 0 bit, acqua, biomi, block entity).
- `SEND=biomes`: invia la lista biomi (spenta di default: il client la rifiutava). `SEND=actors` come in README.
- `SKIP=jigsaw,voxel,items,creative,publisher`, `NO_CHUNKS=1`, `CHUNKS_EARLY=1`.
- PowerShell: `$env:VERSMC_DEBUG="nozero"; cargo run`, poi `Remove-Item Env:VERSMC_DEBUG`.

## Limiti noti

- Niente drop né regole di sopravvivenza (tappa 2).
- Porte solo dalla faccia superiore; porte doppie non collegate.
- Bottoni premuti: dopo un riavvio restano bloccati finché non si clicca.
- Il login non è verificato: chi usa il tuo XUID può espellerti.
- Le modifiche al mondo piatto di ripiego (generatore assente) non si salvano.

## Non verificato da un agente

Tutto il punto "Lista di controllo" richiede il client vero: codifiche ItemStackRequest, PlayerAuthInput, user data a 10 byte,
icone dei gruppi, nomi dei suoni, flag sneaking, regole di orientamento non verificate, formato NBT del giocatore.
