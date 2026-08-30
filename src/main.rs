#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Some(output) = zed_web_server::flags::process_control().map_err(anyhow::Error::msg)? {
        print!("{output}");
        return Ok(());
    }
    zed_web_server::server::run().await
}
