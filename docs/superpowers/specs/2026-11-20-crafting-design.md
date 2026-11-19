# Tappa 4 "crafting e forzieri": abbozzo

Data: 20/11/2026. Ramo `feat/crafting` da `feat/worldgen`. Catalogo: `docs/brainstorm/catalogo-crafting.html`.
L'utente (pochi token) ha detto: "abbozzalo e basta, fai le ricette e le casse e inizia crafting". Scelte mie, da rivedere:

- **Ampiezza (risposta A dell'utente):** ricette + tavolo 2x2 + banco da lavoro + forziere (anche doppio) + fornace.
- **Ricette:** tutte quelle vanilla dal file di Dragonfly (`vendor/dragonfly/server/item/recipe/crafting_data.nbt`, MIT) convertite
  da uno script; tutte sbloccate da subito.
- **Forzieri:** contenuto salvato nel mondo come block entity (come vanilla, il server sa già farlo); due giocatori possono
  aprirlo insieme e vedono le modifiche dell'altro; rotto lascia cadere il contenuto.
- **Riferimenti:** byte da gophertunnel v1.63.0, comportamento da Dragonfly (session/handler_item_stack_request.go per il
  crafting, block/chest.go e session per i contenitori).

Non obiettivi: barile, fornace a fusione, affumicatore, incudine, incantesimi, tramogge, sblocco graduale delle ricette, crafting automatico dei villager.
Anti-obiettivi: oggetti duplicati o persi (risultato preso con Shift, banco chiuso con oggetti nella griglia, forziere rotto
mentre è aperto), client disconnesso per un `CraftingData` o un `ContainerOpen` malformati.
