//! versmc-gen: generatore di mondi Java Edition per VersMCBE, basato su SteelMC.
//!
//! Per ora contiene solo la prova:
//! `versmc-gen probe --seed 13579 --x 0 --z 0 [--size N] [--dim overworld|nether|end] [--repeat K]`

mod steel;

use std::collections::HashSet;
use std::process::ExitCode;
use std::time::Instant;

use steel::{Dim, Generator, JavaChunk};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("probe") => match probe(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("errore: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!(
                "uso: versmc-gen probe --seed <u64> --x <i32> --z <i32> \
                 [--size <N>] [--dim overworld|nether|end] [--repeat <K>]"
            );
            ExitCode::FAILURE
        }
    }
}

/// Opzioni del comando `probe`.
struct ProbeArgs {
    seed: u64,
    x: i32,
    z: i32,
    /// Lato della regione quadrata da generare (1 = chunk singolo).
    size: i32,
    dim: Dim,
    /// Quante volte ripetere la generazione (per misurare a caldo).
    repeat: u32,
}

fn parse_probe(args: &[String]) -> Result<ProbeArgs, String> {
    let mut out = ProbeArgs { seed: 13579, x: 0, z: 0, size: 1, dim: Dim::Overworld, repeat: 1 };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("manca il valore di {flag}"))?;
        let bad = |_| format!("valore non valido per {flag}: {value}");
        match flag.as_str() {
            "--seed" => out.seed = value.parse().map_err(bad)?,
            "--x" => out.x = value.parse().map_err(bad)?,
            "--z" => out.z = value.parse().map_err(bad)?,
            "--size" => out.size = value.parse().map_err(bad)?,
            "--repeat" => out.repeat = value.parse().map_err(bad)?,
            "--dim" => {
                out.dim = match value.as_str() {
                    "overworld" => Dim::Overworld,
                    "nether" => Dim::Nether,
                    "end" => Dim::End,
                    _ => return Err(format!("dimensione sconosciuta: {value}")),
                }
            }
            _ => return Err(format!("opzione sconosciuta: {flag}")),
        }
    }
    if out.size < 1 || out.repeat < 1 {
        return Err("--size e --repeat devono essere almeno 1".into());
    }
    Ok(out)
}

fn probe(args: &[String]) -> Result<(), String> {
    let a = parse_probe(args)?;
    println!("prova: seme {} dimensione {:?} chunk ({}, {}) regione {}x{}", a.seed, a.dim, a.x, a.z, a.size, a.size);

    let start = Instant::now();
    let generator = Generator::new(a.seed, a.dim);
    println!("avvio (registri + generatore): {:.0} ms", ms(start));

    for round in 1..=a.repeat {
        let start = Instant::now();
        let chunks = if a.size == 1 { vec![generator.generate(a.x, a.z)] } else { generator.generate_region(a.x, a.z, a.x + a.size - 1, a.z + a.size - 1) };
        let elapsed = ms(start);
        report(round, &chunks, elapsed);
    }
    if let Some(peak) = peak_memory_mib() {
        println!("memoria di picco: {peak:.0} MiB");
    }
    Ok(())
}

/// Stampa sezioni, stati distinti e tempo di un giro di generazione.
fn report(round: u32, chunks: &[JavaChunk], elapsed_ms: f64) {
    let mut states = HashSet::new();
    let mut biomes = HashSet::new();
    for chunk in chunks {
        for section in &chunk.sections {
            states.extend(section.states.iter().copied());
            biomes.extend(section.biomes.iter().copied());
        }
    }
    let first = &chunks[0];
    println!(
        "giro {round}: {} chunk in {elapsed_ms:.0} ms ({:.1} ms/chunk); \
         sezioni per chunk {} (min_y {}), stati distinti {}, biomi distinti {}",
        chunks.len(),
        elapsed_ms / chunks.len() as f64,
        first.sections.len(),
        first.min_y,
        states.len(),
        biomes.len()
    );
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// Picco del working set del processo (solo Windows), in MiB.
#[cfg(windows)]
fn peak_memory_mib() -> Option<f64> {
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, cb: u32) -> i32;
    }
    let mut counters = ProcessMemoryCounters { cb: size_of::<ProcessMemoryCounters>() as u32, ..Default::default() };
    // SAFETY: struttura PROCESS_MEMORY_COUNTERS con dimensione corretta in `cb`.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    (ok != 0).then(|| counters.peak_working_set_size as f64 / (1024.0 * 1024.0))
}

#[cfg(not(windows))]
fn peak_memory_mib() -> Option<f64> {
    None
}
