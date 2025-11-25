//! This is a simple imitation of the basic functionality of kubectl:
//! kubectl {get, delete, apply, watch, edit} <resource> [name]
//! with labels and namespace selectors supported.
use std::{net::SocketAddr, sync::Arc};

use tokio::net::{TcpListener, TcpStream};
use tracing::instrument;
use tracing_subscriber::{prelude::*, EnvFilter};

use crate::packets::Packet;

mod KubeCache;
mod packets;
mod types;

#[tokio::main]
async fn main() {
    // tracing_subscriber::fmt::init();
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_level(true);
    let filter_layer = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")); // default to INFO
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(filter_layer)
        .init();

    let commit_hash: &'static str = env!("COMMIT_HASH");
    tracing::info!("COMMIT_HASH: {}", commit_hash);

    let cache = KubeCache::Cache::create().unwrap();
    let arc_cache = Arc::new(cache);
    tracing::info!("kube api initialized");

    let listener = TcpListener::bind("0.0.0.0:25565").await.unwrap();
    tracing::info!("tcp server started");

    loop {
        let (socket, addr) = listener.accept().await.unwrap();
        let acc = arc_cache.clone();

        tokio::spawn(async move {
            if let Err(e) = process_socket(socket, addr, acc).await {
                tracing::error!("socket error: {e:?}");
            }
        });
    }
}

#[instrument(level = "trace", skip(cache, stream))]
async fn process_socket(
    mut stream: TcpStream,
    addr: SocketAddr,
    cache: Arc<KubeCache::Cache>,
) -> Result<(), ()> {
    tracing::info!(
        addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
        "Client connected"
    );
    let client_packet = match Packet::parse(&mut stream).await {
        Some(x) => x,
        None => {
            tracing::trace!("Client HANDSHAKE -> bad packet; Disconnecting...");
            return Ok(());
        }
    };
    tracing::info!(
        addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
        "Client disconnected"
    );
    todo!()
}
// ----- Debug tools -----
// let mc_deployments = cache.get_deploys().await;
// for dep in mc_deployments.iter() {
//     println!("{:?}", dep.labels().values())
// }
// // println!("{:?}", mc_deployments);
// println!("count: {}", mc_deployments.iter().count());

// println!(
//     "{:?}",
//     cache.query_addr("ferret.tami.moe".to_string()).await
// );
