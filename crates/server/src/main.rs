#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cfg = bedrock_server::config::ServerConfig::load(std::path::Path::new("server.toml"));
    bedrock_server::run_with(bedrock_server::config(cfg.port), &cfg).await
}
