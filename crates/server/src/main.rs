#[tokio::main]
async fn main() -> std::io::Result<()> {
    bedrock_server::run(bedrock_server::config(19132)).await
}
