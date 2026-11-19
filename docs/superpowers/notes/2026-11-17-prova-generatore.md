# Prova del generatore (Task 1, cancello) — risultati

Data: 2026-11-17. Esito: **cancello superato** — compila su Windows e un chunk singolo
(con tutte le dipendenze) richiede circa 55 ms in release, ben sotto i 2 s richiesti.

## Configurazione

- SteelMC rev `62fd936e352c375bde611f419260397df0096bf8` (git), crate `steel-core` con feature
  `region-generation`; `steel-registry`, `steel-utils`, `steel-worldgen` alla stessa rev.
- Toolchain `nightly-2026-10-14` (rustc 1.101.0-nightly 822182db4), in `generator/rust-toolchain.toml`.
- Workspace separato `generator/` (AGPL-3.0-or-later), non membro del workspace principale.
  Nessun `exclude` necessario: `generator/Cargo.toml` ha il proprio `[workspace]` e
  `cargo test --workspace` alla radice continua a passare.
- `Cargo.lock` di Steel copiato come punto di partenza, così le dipendenze hanno le stesse versioni.
- Profili copiati da Steel: `release` (lto, codegen-units 1, panic abort), `dev` con
  `opt-level = 3` per `steel-worldgen` e `steel-math`. Nessuna sezione `[patch]` in Steel.
- Macchina: AMD Ryzen 5 3600 (6 core / 12 thread), Windows 11. Pool rayon con 12 thread.

## Visibilità dell'API di Steel

Non è servito alcun fork. Tutto il necessario è pubblico con la sola feature `region-generation`
(`ChunkMap::generate_region` è dietro `#[cfg(feature = "region-generation")]`,
`steel-core/src/chunk/chunk_map/mod.rs:84`). `test-framework` non serve.

Unico ostacolo: `ChunkSection::states()` è `pub(crate)` (`steel-core/src/chunk/section.rs:758`),
quindi `ChunkContents::of` non si può ricopiare alla lettera. Si usa l'API pubblica
`Sections::read_column_into(x, z, &mut buf)` (`section.rs:505`), che legge una colonna intera
prendendo il lock di ogni sezione una sola volta. I biomi si leggono con il campo pubblico
`ChunkSection::biomes.get(x, y, z)`.

Prima di `generate_region` la mappa dei chunk deve essere vuota e ferma: si chiama
`stop_generation_refill_loop()`, poi `task_tracker.close()` e `task_tracker.wait()`
(come `stop_chunk_tasks` nei test). Per questo ogni chiamata crea un mondo nuovo in RAM;
il generatore vanilla (`Arc<ChunkGeneratorType>`) è condiviso tra i mondi.

## Misure

| Misura | Valore |
|---|---|
| `cargo build --release` da zero (dopo `cargo clean`) | **12 min 19 s** |
| `cargo check` iniziale (include download sorgenti git e jar Mojang 26.3) | 4 min 58 s |
| Avvio (registri globali + generatore overworld) | 200–260 ms |
| Chunk singolo overworld (0,0), con dipendenze | **57 ms** a freddo, 52–53 ms ripetuto |
| Regione 8×8 overworld (0,0)–(7,7) | **338–358 ms** (5,3–5,6 ms/chunk) |
| Regione 32×32 overworld (1000,−2000) | 4173 ms (4,1 ms/chunk) |
| Chunk singolo Nether (0,0) | 30 ms a freddo, 23 ms ripetuto |
| Chunk singolo End (0,0) | 17 ms a freddo, 13 ms ripetuto |
| Memoria di picco (working set), chunk singolo overworld ×3 | 57 MiB |
| Memoria di picco, regione 8×8 ×2 | 89 MiB |
| Memoria di picco, regione 32×32 | 449 MiB |

Il jar del server Java viene scaricato ed estratto dallo script di build di `steel-utils` dentro
il checkout git di cargo (`src/generated`); dopo il primo download una build da zero non lo
riscarica, quindi i 12 min 19 s sono solo compilazione (dominata da LTO con codegen-units 1).

Output della prova (`blackstone-gen probe --seed 13579 --x 0 --z 0 --repeat 3`):

```
prova: seme 13579 dimensione Overworld chunk (0, 0) regione 1x1
avvio (registri + generatore): 256 ms
giro 1: 1 chunk in 57 ms (56.9 ms/chunk); sezioni per chunk 24 (min_y -64), stati distinti 27, biomi distinti 2
giro 2: 1 chunk in 52 ms (52.1 ms/chunk); sezioni per chunk 24 (min_y -64), stati distinti 27, biomi distinti 2
giro 3: 1 chunk in 53 ms (53.4 ms/chunk); sezioni per chunk 24 (min_y -64), stati distinti 27, biomi distinti 2
memoria di picco: 57 MiB
```

## Note per i task successivi

- Il contenuto non è ancora confrontato con Java vanilla: la prova verifica solo che la
  generazione funzioni e i tempi. La correttezza dei dati è compito dei task successivi.
- Block entity: `Chunk::get_block_entities()` è pubblico (`steel-core/src/chunk/data.rs:811`),
  raggiungibile da `holder.try_chunk(ChunkStatus::Full)`.
- Ricerca dello spawn: `World::initialize_spawn_if_needed` (pubblico, `world/spawn.rs:25`);
  le funzioni di supporto in `world/spawn.rs` e `world/player_spawn_finder` sono `pub(super)` /
  `pub(crate)`. Pubblici: il metodo di trait `ChunkGenerator::initial_spawn_search_origin`
  (`worldgen/generator/mod.rs:48`) e `OverworldClimateSampler::find_spawn_position` in `steel-worldgen`
  (`biomes/climate_sampler.rs:108`).
- Con `lto = true` e `codegen-units = 1` anche una ricompilazione del solo crate `blackstone-gen`
  in release richiede circa 10 min (rilevato: 10 min 09 s), perché LTO rielabora tutto
  steel-core. Per iterare conviene un profilo senza LTO (come `cicd` di Steel) o il profilo dev.
- Il tempo per chunk scende molto con regioni grandi (le dipendenze condivise si generano una
  volta): conviene generare a regioni, non chunk per chunk.
