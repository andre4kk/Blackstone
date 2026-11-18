# Brainstorming "gioco normale di Minecraft" — stato al 18/11/2026

Catalogo: `docs/brainstorm/catalogo-gameplay.html` (pubblicato: https://claude.ai/artifact/VVcMThPpASs1WbRN1Kivuo).

## Richiesta

Dopo il mondo generato (T1, funzionante nel gioco), l'utente vuole i comportamenti normali di Minecraft: rompere,
piazzare, inventario, salto, ecc. Obiettivo di fondo invariato: server stile PocketMine-MP ma migliore, con API dei
plugin in futuro.

## Risposte

1. **Logica di gioco:** A — tradotta in Rust da Dragonfly (Go, MIT).
2. **Autorità:** A — il server decide e controlla rotture, piazzamenti e interazioni.
3. **Portata di questo giro:** A — solo la tappa 1 "creativo vero" (menu creativo pieno, rompere, piazzare con
   orientamento, porte/botole/leve/pulsanti, salvataggio, `/gamemode`). La sopravvivenza (tappa 2) è un piano a parte.
4. **Stato dei blocchi piazzati:** A — regole per tipo di stato (`pillar_axis`, direzioni, metà sopra/sotto…) più casi
   speciali presi da Dragonfly (porte, letti, mezzi blocchi doppi, torce/pulsanti su muro, muretti, piante).
5. **Dati del giocatore:** A — posizione, modalità e inventario salvati nel LevelDB in formato Bedrock.

## Decisioni mie (dichiarate all'utente)

- Rotture, piazzamenti e interazioni passano da un punto unico a "evento annullabile", aggancio per la futura API plugin.

## Trappole segnalate

- Schemi del protocollo Mojang da riscaricare (EULA, fuori dal repository).
- `ItemStackRequest` serve già nella tappa 1 (dal menu creativo alla barra).
- Login non verificato: i dati del giocatore sono legati a un'identità dichiarata dal client.
- Correzioni del 18/11 (lista biomi spenta, blocco del generatore) non ancora committate: l'utente non ha risposto.
