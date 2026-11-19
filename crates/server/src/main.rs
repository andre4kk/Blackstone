#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cfg = bedrock_server::config::ServerConfig::load(std::path::Path::new("server.toml"));
    let level = bedrock_net::Level::parse(&cfg.log_level);
    bedrock_net::set_level(level.unwrap_or(bedrock_net::Level::Info));
    if level.is_none() {
        bedrock_net::warn!("[config] log_level \"{}\" non valido (debug, info, warn): uso info", cfg.log_level);
    }
    bedrock_server::run_with(bedrock_server::config(cfg.port), &cfg).await
}
