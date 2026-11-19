# Stato del brainstorming: generazione del mondo (2026-11-17)

Pagina con tutte le opzioni: https://claude.ai/artifact/3TX7kDFL7MJBSXRVfKp7Lf (copia locale: `catalogo-worldgen.html`).

## Obiettivo
Server Bedrock in stile PocketMine-MP, ma migliore. Il mondo è generato con l'algoritmo Java Edition (SteelMC):
entrando si vede un mondo Minecraft "vero". In futuro arriverà un'API per i plugin.

## Decisioni prese
1. **Integrazione: D, processo separato.** Il server resta MIT e su Rust stable. Il generatore AGPL è isolato: i plugin non sono toccati dall'AGPL.
2. **Generatore: D1.** `blackstone-gen` è un nostro binario (AGPL, nightly) che usa `steel-core` come libreria (`ChunkMap::generate_region`).
   Il server lo avvia da solo e gli chiede i chunk su TCP 127.0.0.1. La traduzione Java → Bedrock (mappature GeyserMC per Java 26.3 / Bedrock 1.26.60) avviene nel generatore.
3. **Salvataggio: S1, LevelDB Bedrock** (`rusty-leveldb` con compressore deflate id 4). Salva il server.
4. **Traguardo: T1 + T2 + T3.**
   - T1: Overworld infinito, chunk mandati mentre il giocatore si muove, spawn calcolato come in Java, mondo salvato.
   - T2: rompere e posare blocchi.
   - T3: Nether ed End.

## Scelte mie (correggibili)
- Riavvio automatico del generatore se cade; nel frattempo il server manda chunk vuoti.
- Seed nel file di configurazione.
- `BiomeDefinitionList` presa da pmmp/BedrockData (CC0).
- Fuori per ora: entità generate, bottino delle casse, luce lato server.

## Prossimi passi
- Domanda 5: creativa o sopravvivenza per T2.
- Design doc, poi audit del progetto (bug, ottimizzazioni, `.gitignore`, `identity.key` pubblico), poi il piano.
