#[tokio::main]
async fn main() -> anyhow::Result<()> {
    desk_agent::interfaces::cli::commands::run().await
}
