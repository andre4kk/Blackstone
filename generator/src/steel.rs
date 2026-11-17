//! Ponte verso steel-core: genera chunk Java Edition in un mondo solo in RAM.
//!
//! Ricalca `overworld()` di `steel-core/src/chunk/chunk_map/tests/region_generation.rs`:
//! registri globali, pool rayon con stack ampio, generatore vanilla, mondo `RamOnly`,
//! poi `ChunkMap::generate_region` (feature `region-generation`).

use std::sync::Arc;

use rayon::ThreadPool;
use steel_core::bootstrap::init_globals;
use steel_core::chunk::chunk_holder::ChunkHolder;
use steel_core::chunk::status::ChunkStatus;
use steel_core::entity::damage::DamageHistory;
use steel_core::level_data::{GameTimeSource, WorldGenerationSettings};
use steel_core::world::{World, WorldConfig, WorldStorageConfig};
use steel_core::worldgen::{ChunkGeneratorType, EndGenerator, NetherGenerator, OverworldGenerator};
use steel_registry::dimension_type::DimensionTypeRef;
use steel_registry::vanilla_dimension_types;
use steel_utils::types::{Difficulty, GameType};
use steel_utils::{BlockStateId, ChunkPos, Identifier};
use steel_worldgen::biomes::BiomeSourceKind;
use steel_worldgen::density_functions::end::EndNoiseSettings;
use steel_worldgen::density_functions::nether::NetherNoiseSettings;
use steel_worldgen::density_functions::overworld::OverworldNoiseSettings;
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

/// Stack dei thread di generazione: la ricorsione delle density function è profonda.
const STACK_SIZE: usize = 16 * 1024 * 1024;

/// Dimensione da generare. Provvisorio: sarà sostituito da `vgen::Dim`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dim {
    Overworld,
    Nether,
    End,
}

impl Dim {
    /// Tipo di dimensione vanilla di Steel.
    fn dimension_type(self) -> DimensionTypeRef {
        match self {
            Dim::Overworld => &vanilla_dimension_types::OVERWORLD,
            Dim::Nether => &vanilla_dimension_types::THE_NETHER,
            Dim::End => &vanilla_dimension_types::THE_END,
        }
    }

    /// Nome vanilla senza namespace (`overworld`, `the_nether`, `the_end`).
    fn short_name(self) -> &'static str {
        match self {
            Dim::Overworld => "overworld",
            Dim::Nether => "the_nether",
            Dim::End => "the_end",
        }
    }

    fn sea_level(self) -> i32 {
        match self {
            Dim::Overworld => OverworldNoiseSettings::SEA_LEVEL,
            Dim::Nether => NetherNoiseSettings::SEA_LEVEL,
            Dim::End => EndNoiseSettings::SEA_LEVEL,
        }
    }
}

/// Una sezione 16×16×16 in formato Java.
pub struct JavaSection {
    /// Id globali degli stati di blocco Java; indice `(y*16 + z)*16 + x`.
    pub states: Box<[u16; 4096]>,
    /// Id di registro dei biomi; indice `(y*4 + z)*4 + x`.
    pub biomes: Box<[u16; 64]>,
}

/// Un chunk Java completo (stato Full), sezioni dal basso verso l'alto.
pub struct JavaChunk {
    pub min_y: i32,
    pub sections: Vec<JavaSection>,
}

/// Generatore di chunk per un seme e una dimensione.
pub struct Generator {
    seed: u64,
    dim: Dim,
    pool: Arc<ThreadPool>,
    runtime: Arc<Runtime>,
    generator: Arc<ChunkGeneratorType>,
}

impl Generator {
    /// Prepara registri, thread e generatore vanilla (operazione costosa, da fare una volta).
    pub fn new(seed: u64, dim: Dim) -> Generator {
        init_globals();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .stack_size(STACK_SIZE)
                .thread_name(|i| format!("versmc-gen-{i}"))
                .build()
                .expect("il pool di generazione deve avviarsi"),
        );
        let runtime =
            Arc::new(RuntimeBuilder::new_multi_thread().thread_stack_size(STACK_SIZE).enable_all().build().expect("il runtime di generazione deve avviarsi"));
        let generator = Arc::new(match dim {
            Dim::Overworld => ChunkGeneratorType::Overworld(OverworldGenerator::new(None, BiomeSourceKind::overworld(seed), seed, &pool)),
            Dim::Nether => ChunkGeneratorType::Nether(NetherGenerator::new(None, BiomeSourceKind::nether(seed), seed, &pool)),
            Dim::End => ChunkGeneratorType::End(EndGenerator::new(None, BiomeSourceKind::end(seed), seed, &pool)),
        });
        Generator { seed, dim, pool, runtime, generator }
    }

    /// Genera il chunk `(x, z)` con tutte le sue dipendenze.
    pub fn generate(&self, x: i32, z: i32) -> JavaChunk {
        self.generate_region(x, z, x, z).pop().expect("la regione contiene esattamente un chunk")
    }

    /// Genera i chunk da `(min_x, min_z)` a `(max_x, max_z)` inclusi, in ordine X poi Z.
    ///
    /// Ogni chiamata usa un mondo nuovo: `generate_region` esige una mappa dei chunk vuota.
    pub fn generate_region(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> Vec<JavaChunk> {
        // Il mondo tiene solo un riferimento debole a damage_history: va tenuto vivo qui.
        let (world, _damage_history) = self.new_world();
        let chunk_map = &world.chunk_map;
        // Ferma il ciclo di generazione a ticket: generate_region vuole la mappa ferma.
        chunk_map.stop_generation_refill_loop();
        chunk_map.task_tracker.close();
        self.runtime.block_on(chunk_map.task_tracker.wait());

        let holders = self
            .runtime
            .block_on(chunk_map.generate_region(ChunkPos::new(min_x, min_z), ChunkPos::new(max_x, max_z)))
            .unwrap_or_else(|e| panic!("generazione della regione fallita: {e}"));
        holders.iter().map(|h| java_chunk(h)).collect()
    }

    /// Crea un mondo solo in RAM che usa il generatore condiviso.
    fn new_world(&self) -> (Arc<World>, Arc<DamageHistory>) {
        let dim_type = self.dim.dimension_type();
        let name = self.dim.short_name();
        let damage_history = Arc::new(DamageHistory::default());
        let config = WorldConfig {
            damage_history: Arc::clone(&damage_history),
            game_time_source: GameTimeSource::Primary,
            storage: WorldStorageConfig::RamOnly,
            level_data_path: None,
            generator: Arc::clone(&self.generator),
            generation_settings: WorldGenerationSettings::from_generator_config(
                Identifier::vanilla_static(name),
                &toml::Value::Table(toml::map::Map::new()),
                dim_type.key.clone(),
                dim_type.min_y,
                dim_type.height,
            ),
            view_distance: 2,
            simulation_distance: 2,
            max_chained_neighbor_updates: 1_000_000,
            compression: None,
            is_flat: false,
            sea_level: self.dim.sea_level(),
            default_gamemode: GameType::Survival,
            difficulty: Difficulty::Normal,
        };
        let world = self
            .runtime
            .block_on(World::new_with_config(
                Arc::clone(&self.runtime),
                Identifier::new_static("versmc_gen", name),
                dim_type,
                self.seed as i64,
                config,
                Arc::clone(&self.pool),
            ))
            .expect("il mondo in RAM deve crearsi");
        (world, damage_history)
    }
}

/// Copia stati e biomi di un chunk Full, come `ChunkContents::of` nei test di Steel.
fn java_chunk(holder: &ChunkHolder) -> JavaChunk {
    let Some(chunk) = holder.try_chunk(ChunkStatus::Full) else {
        panic!("{:?} dovrebbe essere Full", holder.get_pos());
    };
    let sections = &chunk.sections.sections;
    let mut out: Vec<JavaSection> = sections
        .iter()
        .map(|section| {
            let section = section.read();
            let mut biomes = Box::new([0u16; 64]);
            for y in 0..4 {
                for z in 0..4 {
                    for x in 0..4 {
                        biomes[(y * 4 + z) * 4 + x] = section.biomes.get(x, y, z);
                    }
                }
            }
            JavaSection { states: Box::new([0u16; 4096]), biomes }
        })
        .collect();
    // `ChunkSection::states()` è pub(crate): si legge per colonne con l'API pubblica.
    let mut column: Vec<BlockStateId> = Vec::new();
    for z in 0..16 {
        for x in 0..16 {
            chunk.sections.read_column_into(x, z, &mut column);
            for (ry, state) in column.iter().enumerate() {
                let (s, y) = (ry / 16, ry % 16);
                out[s].states[(y * 16 + z) * 16 + x] = state.0;
            }
        }
    }
    JavaChunk { min_y: holder.min_y(), sections: out }
}
