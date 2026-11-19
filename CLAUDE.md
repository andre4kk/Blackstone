# CLAUDE.md — Blackstone

Leggi prima `AGENTS.md`: struttura, comandi, regole e stato valgono anche qui. Sotto, solo ciò che riguarda Claude.

## Il proprietario
- Scrive in italiano e non è un programmatore: rispondi in italiano semplice, frasi brevi, niente gergo.
- Prova lui nel gioco (client Bedrock vero) e incolla i log del server: sono la verifica che conta.
- Ha pochi token: sii efficiente, delega il lavoro pesante a subagenti Sonnet/Haiku (brief corti che rimandano a file e a
  Dragonfly/gophertunnel), fai tu solo coordinamento e controlli piccoli.

## Come decidere
- Per scelte nuove: prima una pagina catalogo (HTML leggibile da telefono, in `docs/brainstorm/`) con opzioni, trappole,
  tempi; poi una domanda alla volta, A/B/C con la consigliata per prima. Salva lo stato delle risposte in
  `docs/brainstorm/stato-*.md`. Se dice "decidi tu" o "abbozzalo", scegli la consigliata e scrivilo nel design.
- Ogni tappa: design in `docs/superpowers/specs/`, piano in `docs/superpowers/plans/`, lista di prova nel gioco in
  `docs/superpowers/notes/`. Lavoro su un ramo `feat/*`, unione in `main` solo a tappa revisionata.
- Push solo con il suo consenso esplicito, ogni volta.
- Commit e pull request **senza** `Co-Authored-By`, "Generated with Claude Code" o altre firme di Claude (lo ha chiesto il proprietario). Vale anche per i subagenti: diglielo nel brief.

## Lezioni imparate
- I bug veri li trova il client: un pacchetto con un byte sbagliato non dà errori nei test ma disconnette o fa sparire oggetti.
  Confronta sempre con gophertunnel riga per riga; per i flussi (quali pacchetti, in che ordine) segui Dragonfly.
- Quando un'azione nel gioco "non fa niente" e il log è vuoto, il client sta usando un pacchetto che il server scarta
  in silenzio (es. Q con inventario chiuso = `InventoryTransaction` normale).
- `cargo run` lento = dipendenze in debug: `[profile.dev.package."*"] opt-level = 2` è già nel `Cargo.toml`.
