use solana_poh::PoHServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let poh_server = PoHServer::build("127.0.0.1:5000".to_string())?;
    poh_server.run().await?;
    Ok(())
}
