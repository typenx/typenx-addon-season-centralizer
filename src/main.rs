mod centralization;
mod models;
mod server;
mod source_client;
mod tvmaze;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::serve().await
}
