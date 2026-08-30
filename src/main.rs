#[tokio::main]
async fn main() -> anyhow::Result<()> {
    zed_web_server::server::run().await
}
