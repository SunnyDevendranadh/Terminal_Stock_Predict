#[path = "../client.rs"]
#[allow(dead_code)]
mod client;

use anyhow::Result;
use client::GrpcSignalClient;
use tokio::time::{timeout, Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let config = client::ClientConfig::from_env()?;
    let mut grpc = GrpcSignalClient::connect(&config).await?;

    let mut stream = grpc.subscribe_signals().await?;
    let snapshot = timeout(Duration::from_secs(20), stream.message()).await??;

    if let Some(data) = snapshot {
        let parsed = GrpcSignalClient::map_stream_snapshot(data)?;
        println!(
            "ok symbols={} kill_switch={} chain={} captured_at={}",
            parsed.signals.len(),
            parsed.threat.kill_switch_level,
            parsed.threat.chain_verification_status,
            parsed.captured_at.to_rfc3339()
        );
    } else {
        println!("stream ended with no data");
    }

    Ok(())
}
