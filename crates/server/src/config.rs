//! Configurazione del server (`server.toml`).
use blackstone_net::{log, warn};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// File di esempio, scritto come `server.toml` quando manca.
const EXAMPLE: &str = include_str!("../../../server.example.toml");

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Nome del mondo: i dati stanno in `worlds/<world>`.
    pub world: String,
    /// Seme; se manca ne viene scelto uno a caso alla creazione del mondo.
    pub seed: Option<u64>,
    /// Programma del generatore.
    pub generator: PathBuf,
    /// Raggio di chunk inviato a ogni giocatore.
    pub view_distance: i32,
    pub port: u16,
    /// Se vero, chi muore tiene l'inventario (niente oggetti a terra).
    pub keep_inventory: bool,
    /// Se falso, i giocatori non si fanno danno tra loro (i bot si colpiscono sempre).
    pub pvp: bool,
    /// Livello minimo dei messaggi in console: "debug", "info" o "warn". La variabile d'ambiente `BLACKSTONE_LOG` ha la precedenza.
    pub log_level: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            world: "Mondo".into(),
            seed: None,
            generator: PathBuf::from(format!("generator/target/release/blackstone-gen{}", std::env::consts::EXE_SUFFIX)),
            view_distance: 6,
            port: 19132,
            keep_inventory: false,
            pvp: true,
            log_level: "info".into(),
        }
    }
}

impl ServerConfig {
    /// Sulle piattaforme con un suffisso per gli eseguibili (`.exe`) lo aggiunge al generatore se manca l'estensione.
    fn with_exe_suffix(mut self) -> Self {
        let suffix = std::env::consts::EXE_SUFFIX;
        if !suffix.is_empty() && self.generator.extension().is_none() {
            let mut name = self.generator.into_os_string();
            name.push(suffix);
            self.generator = name.into();
        }
        // Vecchio nome del generatore (`versmc-gen`): se il nuovo eseguibile non esiste ma il vecchio sì, si usa il vecchio.
        if !self.generator.exists() {
            let old = self.generator.with_file_name(format!("versmc-gen{suffix}"));
            if self.generator.file_name().and_then(|n| n.to_str()) == Some(&format!("blackstone-gen{suffix}")) && old.exists() {
                self.generator = old;
            }
        }
        self
    }

    /// Legge `path`. Se manca usa i valori di default e ci scrive l'esempio; se è illeggibile o non valido
    /// segnala l'errore e usa i default (senza toccare il file).
    pub fn load(path: &Path) -> ServerConfig {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str::<ServerConfig>(&text).map(ServerConfig::with_exe_suffix).unwrap_or_else(|e| {
                warn!("[config] {} non valido, uso i valori di default: {}", path.display(), e.message());
                ServerConfig::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fs::write(path, EXAMPLE) {
                    Ok(()) => log!("[config] {} non trovato: scritto l'esempio, uso i valori di default", path.display()),
                    Err(e) => warn!("[config] impossibile scrivere {}: {e}", path.display()),
                }
                ServerConfig::default()
            }
            Err(e) => {
                warn!("[config] impossibile leggere {}, uso i valori di default: {e}", path.display());
                ServerConfig::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cartella temporanea di un test, rimossa alla fine.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("blackstone-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn config_defaults_when_file_missing() {
        let dir = TempDir::new("config-missing");
        let path = dir.0.join("server.toml");
        let cfg = ServerConfig::load(&path);
        assert_eq!(cfg, ServerConfig::default());
        assert_eq!((cfg.world.as_str(), cfg.seed, cfg.view_distance, cfg.port), ("Mondo", None, 6, 19132));
        assert!(!cfg.keep_inventory);
        assert!(cfg.pvp);
        assert!(cfg.generator.to_string_lossy().starts_with("generator/target/release/blackstone-gen"));
        assert_eq!(fs::read_to_string(&path).unwrap(), EXAMPLE, "scritto l'esempio");
        assert_eq!(ServerConfig::load(&path), cfg, "l'esempio riletto dà gli stessi valori");
    }

    #[test]
    fn config_reads_values_and_keeps_defaults_for_the_rest() {
        let dir = TempDir::new("config-values");
        let path = dir.0.join("server.toml");
        fs::write(&path, "world = \"Prova\"\nseed = 777\nport = 25565\n").unwrap();
        let cfg = ServerConfig::load(&path);
        assert_eq!((cfg.world.as_str(), cfg.seed, cfg.port), ("Prova", Some(777), 25565));
        assert_eq!(cfg.view_distance, 6);
    }

    #[test]
    fn config_reads_keep_inventory() {
        let dir = TempDir::new("config-keep");
        let path = dir.0.join("server.toml");
        fs::write(
            &path,
            "keep_inventory = true
",
        )
        .unwrap();
        assert!(ServerConfig::load(&path).keep_inventory);
    }

    #[test]
    fn config_reads_pvp() {
        let dir = TempDir::new("config-pvp");
        let path = dir.0.join("server.toml");
        fs::write(
            &path,
            "pvp = false
",
        )
        .unwrap();
        assert!(!ServerConfig::load(&path).pvp);
    }

    #[test]
    fn invalid_config_falls_back_to_defaults_without_touching_the_file() {
        let dir = TempDir::new("config-invalid");
        let path = dir.0.join("server.toml");
        fs::write(&path, "port = \"non un numero\"").unwrap();
        assert_eq!(ServerConfig::load(&path), ServerConfig::default());
        assert_eq!(fs::read_to_string(&path).unwrap(), "port = \"non un numero\"");
    }
}
