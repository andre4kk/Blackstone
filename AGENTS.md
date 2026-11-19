# AGENTS.md — Blackstone

Istruzioni per chi (persona o agente) lavora su questo repository.

## Cos'è
Blackstone è un server Minecraft **Bedrock** scritto in Rust (stile PocketMine-MP, più veloce). Il mondo si genera come in
Minecraft Java tramite un processo separato. Protocollo **2225** (client 1.26.6x), connessione NetherNet.
Presentazione completa: `PRESENTAZIONE.md`; sito: `site/index.html` (pubblicato da `.github/workflows/pages.yml`).

## Struttura
| Percorso | Cosa |
|---|---|
| `crates/protocol` (`blackstone-protocol`) | byte dei pacchetti; `generated.rs` si rigenera con `tools/protocolgen.py`, non a mano |
| `crates/net` (`blackstone-net`) | rete NetherNet/WebRTC, `log!`/`warn!`/`debug!`, `identity.key` |
| `crates/server` (`blackstone-server`, eseguibile `blackstone`) | logica di gioco, ciclo unico a 20 Hz, test in `src/tests.rs` (`Harness`) e `tests/join.rs` |
| `crates/vgen` | tipi condivisi col generatore (chunk, dimensioni) |
| `generator/` | generatore del mondo, workspace a parte, **AGPL**, Rust nightly; eseguibile `blackstone-gen` (ripiego `versmc-gen`) |
| `crates/server/data/` | tabelle generate dagli script in `tools/` (oggetti, blocchi, drop, cibi, ricette, fusione) |
| `docs/superpowers/` | design (`specs/`), piani (`plans/`), note e liste di prova nel gioco (`notes/`) per ogni tappa |
| `vendor/` | riferimenti clonati in sola lettura, ignorati da git (gophertunnel, dragonfly, bedrock-protocol-docs) |

## Comandi
```bash
cargo run                                   # server (ricompila il codice nostro in debug, le dipendenze ottimizzate)
cargo test --workspace                      # tutti i test
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                             # rustfmt larghezza 160
```
Se `target/debug/blackstone.exe` è bloccato (server acceso), usare `cargo test -p <crate> --lib`.
Config: `server.toml` (modello `server.example.toml`); log con `log_level` o `BLACKSTONE_LOG` (info/warn/debug).

## Regole
- **Byte dei pacchetti:** l'autorità è **gophertunnel v1.63.0** (`vendor/gophertunnel/minecraft/protocol` e `/packet`),
  non gli schemi Mojang (hanno sbagliato flag e skin). Clonare con
  `git clone --depth 1 --branch v1.63.0 https://github.com/Sandertv/gophertunnel vendor/gophertunnel`.
- **Comportamento e numeri:** **Dragonfly 1.26.60** (`vendor/dragonfly`, MIT), citato nei commenti. Riusare prima di inventare.
- **Mai un valore di enum fuori dal suo insieme**: il client si disconnette (è successo con `BuildPlatform` 0).
- Flag delle entità dopo CAN_DASH(46) spostati di uno (gravità 49, collisioni 48, nuoto 57).
- Codifiche non verificabili coi test: log esadecimale a livello debug per le prime `HEX_LOGS` volte, e voce nella lista di prova nel gioco.
- Ogni azione su mondo o giocatore passa da `Server::allow(&WorldAction)` (ganci per i futuri plugin).
- Inventario: tutto passa dalle richieste di pila (`inventory.rs`) con ID di pila; nessuna duplicazione o perdita di oggetti.
- Ciclo unico a 20 Hz, nessun thread nuovo per la logica di gioco.
- Commenti e log **in italiano** (contributi esterni: inglese accettato). Test per ogni comportamento.
- **Licenze:** server MIT; mai copiare codice del generatore AGPL nel server. Sul sito non dire da dove viene la generazione:
  solo "come Minecraft Java".
- Mai committare `identity.key` (è in `.gitignore`); non fare push senza il consenso del proprietario.

## Stato (20/11/2026)
Fatte e unite in `main`: mondo infinito, creativo, giocatori ed entità (`/bot`), sopravvivenza (drop, vita, fame, morte,
combattimento), crafting, forzieri, fornace, suggerimenti dei comandi, console con livelli.
Da provare nel gioco: caduta/morte/rinascita/fame/combattimento (`docs/superpowers/notes/2026-11-20-verifica-tappa3.md`), fornace.
Manca: armature, usura, effetti, mob, Nether/End, API dei plugin, verifica del login Xbox. Idee per iniziare:
`docs/good-first-issues.md`.
