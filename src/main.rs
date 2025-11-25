//! This is a simple imitation of the basic functionality of kubectl:
//! kubectl {get, delete, apply, watch, edit} <resource> [name]
//! with labels and namespace selectors supported.
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};

mod KubeCache;
mod packets;
mod types;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let commit_hash: &'static str = env!("COMMIT_HASH");
    tracing::info!("COMMIT_HASH: {}", commit_hash);

    let cache = KubeCache::Cache::create().unwrap();
    let arcCache = Arc::new(cache);
    tracing::info!("kube api initialized");

    let mut listener = TcpListener::bind("0.0.0.0:25565").await.unwrap();
    tracing::info!("tcp server started");

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let acc = arcCache.clone();

        tokio::spawn(async move {
            if let Err(e) = process_socket(socket, acc).await {
                tracing::error!("socket error: {e:?}");
            }
        });
    }
}

async fn process_socket(stream: TcpStream, cache: Arc<KubeCache::Cache>) -> Result<(), ()> {
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
