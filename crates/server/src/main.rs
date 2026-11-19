#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cfg = blackstone_server::config::ServerConfig::load(std::path::Path::new("server.toml"));
    let level = blackstone_net::Level::parse(&cfg.log_level);
    blackstone_net::set_level(level.unwrap_or(blackstone_net::Level::Info));
    if level.is_none() {
        blackstone_net::warn!("[config] log_level \"{}\" non valido (debug, info, warn): uso info", cfg.log_level);
    }
    blackstone_server::run_with(blackstone_server::config(cfg.port), &cfg).await
}
