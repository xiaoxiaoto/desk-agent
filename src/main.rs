#[tokio::main]
async fn main() -> anyhow::Result<()> {
    desk_agent::interfaces::http::server::serve().await
}
