#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => desk_agent::interfaces::http::server::serve().await,
        ["setup"] => desk_agent::interfaces::cli::setup::run(false).await,
        ["setup", "--revoke"] => desk_agent::interfaces::cli::setup::run(true).await,
        _ => anyhow::bail!("usage: desk-agent [setup [--revoke]]"),
    }
}
