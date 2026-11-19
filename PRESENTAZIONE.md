# VersMCBE: presentazione del progetto

## 1. Cos'è e perché esiste

VersMCBE è un server per **Minecraft Bedrock** (la versione per Windows, console e telefoni) scritto in **Rust**. Segue l'idea di
PocketMine-MP, il server Bedrock più noto, ma punta a essere più veloce e più ordinato.

Il mondo è **infinito** e viene creato con le **stesse regole di Minecraft Java 26.3** (montagne, grotte, biomi), anche se
giochi con Bedrock. In futuro avrà un'**API per i plugin**, così chiunque potrà aggiungere funzioni senza toccare il server.

## 2. Come è fatto (in parole semplici)

| Pezzo | Cosa fa | Dove sta |
|---|---|---|
| Rete | Collega il gioco al server. Usa **NetherNet** (WebRTC), il sistema delle versioni 26.60 in poi: si entra dalla scheda Amici/LAN. Prima c'è una "stretta di mano" via HTTP (signaling). | `crates/net` |
| Protocollo | Traduce i pacchetti del gioco (versione di protocollo 2225, client 1.26.6x) da byte a dati e viceversa. Non fa input/output. | `crates/protocol` |
| Server | La logica di gioco: giocatori, blocchi, inventario, chat, comandi, 20 aggiornamenti al secondo. | `crates/server` |
| Generatore | Programma **separato** (`versmc-gen`) basato su **SteelMC** che crea i chunk. Il server lo avvia da solo e gli parla via TCP in locale; se cade, lo riavvia. | `generator/` + `crates/vgen` |
| Salvataggio | Il mondo sta in `worlds/<nome>` nel formato **LevelDB di Bedrock**, con `level.dat`. Si salvano anche inventario, modalità, vita e fame di ogni giocatore. | dentro il server |

Perché il generatore è separato: SteelMC ha licenza AGPL e richiede una versione sperimentale di Rust. Tenendolo a parte, il
server resta con licenza MIT, usa Rust stabile e i futuri plugin non sono vincolati all'AGPL. Il tick di gioco inoltre non
aspetta mai il generatore.

## 3. Cosa c'è già

Il lavoro è diviso in tappe.

| Tappa | Contenuto | Come è stata provata |
|---|---|---|
| **Mondo** | Overworld infinito generato con le regole Java, chunk inviati attorno al giocatore, scaricati quando sono lontani, salvati su disco. Se il generatore manca c'è un mondo piatto di ripiego. | Con il gioco vero: il giocatore entra e vede il mondo. Più test automatici con un generatore finto. |
| **1 – Creativo** | Menu creativo completo, barra e inventario, rottura e posa di blocchi con orientamento (scale, lastre, porte, letti, torce...), aggiornamento dei blocchi vicini, porte/botole/leve/bottoni, `/gamemode`, salvataggio. | Con il gioco vero, con alcune correzioni fatte dopo le prove. Alcuni dettagli (icone dei gruppi, codifica di certe richieste dell'inventario) erano in lista di controllo. |
| **2 – Entità e giocatori** | I giocatori si vedono tra loro (movimento, testa, braccio, skin, oggetto in mano), oggetti a terra che si lanciano e si raccolgono, `/bot` per un giocatore finto utile a provare con un solo account. | Test automatici e lista di prova nel gioco; il cambio di skin e il lancio con Q sono stati corretti dopo. |
| **3 – Sopravvivenza base** | Blocchi che lasciano oggetti (dati da Dragonfly), vita, danno da caduta e dal vuoto, fame, mangiare, morte e rinascita, `/kill`, combattimento corpo a corpo contro giocatori e bot, `keep_inventory` e `pvp` configurabili. | Test automatici. Nel gioco **mangiare funziona**. |

### Cosa è verificato nel gioco e cosa solo con i test

- **Verificato con il gioco vero:** ingresso nel mondo, mondo generato visibile, menu creativo e posa/rottura in creativo, mangiare in sopravvivenza.
- **Solo test automatici (da confermare nel gioco):** gran parte della tappa 3 (danni, fame, morte, combattimento) e dei bot.
- **Problemi trovati nelle prove di oggi:**
  - le **crepe di rottura** del blocco non si vedono in sopravvivenza (in correzione);
  - i blocchi piazzati in sopravvivenza **non vengono consumati** dalla pila (in correzione);
  - il tasto **Q dalla barra con l'inventario chiuso** ancora non butta l'oggetto.
- La lista di prova passo per passo è in `docs/superpowers/notes/2026-11-20-verifica-tappa3.md`.

## 4. Cosa manca

| Area | Stato |
|---|---|
| Crafting (banco, fornace, ricette) | non fatto |
| Armature che riducono il danno, usura degli strumenti, incantesimi | non fatto |
| Effetti (pozioni, veleno...), lava, fuoco, annegamento | non fatto |
| Mob e animali (anche quelli generati col mondo, villaggi) | non fatto |
| Nether, End, altre dimensioni | non fatto (solo Overworld) |
| Contenuto di casse e blocchi con dati (bottino) | non fatto |
| **API per i plugin** | pianificata; l'architettura (eventi annullabili per rottura, posa, interazione) è già pensata per questo |
| Sistema di permessi (oggi chiunque può usare `/bot`) | non fatto |
| **Autenticazione Xbox verificata** | non fatta: le firme del login non sono controllate, chiunque può dichiararsi chiunque; il signaling è HTTP senza TLS |
| Anticheat | non fatto (il server però decide da solo cosa si rompe e si piazza) |
| Importare mondi Java esistenti | non fatto |
| Lista dei biomi inviata al client | spenta di default: il client l'ha rifiutata, da riprovare |

## 5. Come si avvia e si configura

1. Generatore, una volta sola (serve Internet, circa 12 minuti la prima volta): `cd generator` poi `cargo build --release`.
2. Server, dalla cartella principale: `cargo run`. Crea da solo `server.toml` se manca; ascolta sulla porta 19132 (TCP e UDP).
3. Nel gioco si entra dalla scheda Amici/LAN. Si parte in creativo; `/gamemode s` passa alla sopravvivenza.
4. Per giocare da Internet: impostare `PUBLIC_IP` e aprire la porta 19132 (TCP e UDP) sul router.
5. Prove automatiche: `cargo test --workspace` (il generatore ha i suoi test dentro `generator/`).

Impostazioni di `server.toml`:

| Campo | Default | Significato |
|---|---|---|
| `world` | `Mondo` | nome del mondo (cartella in `worlds/`) |
| `seed` | casuale | seme, vale solo alla creazione del mondo |
| `generator` | `generator/target/release/versmc-gen` | dove si trova il generatore |
| `view_distance` | 6 | raggio di chunk per giocatore (1-32) |
| `port` | 19132 | porta TCP e UDP |
| `keep_inventory` | false | chi muore tiene l'inventario |
| `pvp` | true | i giocatori si fanno danno tra loro |

Comandi nel gioco: `/help`, `/list`, `/tps`, `/say`, `/gamemode` (`/gm`), `/bot [nome]`, `/bot rimuovi`, `/kill`.

Non cancellare né condividere `identity.key`: è l'identità del server.

## 6. Riferimenti e licenze

| Cosa | Uso | Licenza |
|---|---|---|
| Server e crate principali (`crates/*`) | il progetto | MIT |
| **Dragonfly** (server Bedrock in Go) | riferimento di comportamento; da lì vengono le tabelle di drop, strumenti e cibi | MIT |
| **gophertunnel** | riferimento per i byte dei pacchetti (la sequenza di ingresso segue quella) | MIT, solo consultato |
| **PocketMine-MP** | modello di riferimento per lo stile del server | LGPL-3.0, solo come ispirazione (nessun codice copiato) |
| **SteelMC** | motore di generazione del mondo, dentro `generator/` | AGPL-3.0 |
| Generatore `versmc-gen` | processo separato, parla col server solo via TCP | AGPL-3.0-or-later |
| Mappature **GeyserMC** (blocchi e biomi) | traduzione Java -> Bedrock | MIT |
| **pmmp/BedrockData** | dati dei biomi | CC0 |
| Dati e schemi **Mojang** (jar Java, bedrock-protocol-docs, bedrock-samples) | pacchetti e dati | EULA di Minecraft |

Prima di distribuire il server va verificato cosa dell'EULA permette di includere e cosa invece l'utente deve scaricare.
Se si distribuisce il binario del generatore valgono le condizioni dell'AGPL.

## 7. Prossimi passi possibili

1. Chiudere i problemi aperti della sopravvivenza: crepe di rottura, consumo dei blocchi piazzati, Q con inventario chiuso.
2. Finire di provare nel gioco la lista della tappa 3 (danni, fame, morte, combattimento).
3. Crafting e usura degli strumenti, poi armature ed effetti.
4. Primi mob, con comportamento semplice.
5. Autenticazione Xbox e permessi, indispensabili prima di aprire il server al pubblico.
6. API per i plugin.
7. Nether e End, e l'importazione di mondi Java.
