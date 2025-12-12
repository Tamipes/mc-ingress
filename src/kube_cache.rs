use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use k8s_openapi::api::{apps::v1::Deployment, core::v1::Service};
use kube::{
    api::{ListParams, ObjectList, Patch, PatchParams},
    runtime::reflector::Lookup,
    Api, Client, ResourceExt,
};
use serde_json::json;
use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::{
    mc_server::{MinecraftAPI, MinecraftServerHandle, ServerDeploymentStatus},
    packets::{
        clientbound::status::StatusTrait,
        serverbound::handshake::{self},
        SendPacket,
    },
    OpaqueError,
};

/// This is the layer who is respinsible for caching requests.
///
/// TODO:
/// It should be also clone-able freely, because it deals with
/// the underlying async data access.
#[derive(Debug, Clone)]
pub struct KubeCache {
    deployments: Api<Deployment>,
    services: Api<Service>,
}
impl KubeCache {
    /// This initializes the creation of a "kubernetes client"
    /// and if it is not possible returns a None.
    pub fn create() -> Option<KubeCache> {
        let kubeconfig = kube::config::Kubeconfig::read().unwrap();
        let client = Client::try_from(kubeconfig).unwrap();

        let deployments: Api<Deployment> = Api::default_namespaced(client.clone());
        let services: Api<Service> = Api::default_namespaced(client);

        return Some(KubeCache {
            deployments,
            services,
        });
    }
    async fn get_dep(&self, name: &str) -> Result<Deployment, kube::Error> {
        self.deployments.get(name).await
    }
    async fn get_srv(&self, name: &str) -> Result<Service, kube::Error> {
        self.services.get(name).await
    }
    async fn get_deploys(&self) -> ObjectList<Deployment> {
        // let lp: ListParams = ListParams::default();
        let lp: ListParams = ListParams::default().labels("tami.moe/minecraft");
        self.deployments.list(&lp).await.unwrap()
    }
    async fn get_srvs(&self) -> ObjectList<Service> {
        // let lp: ListParams = ListParams::default();
        let lp: ListParams = ListParams::default().labels("tami.moe/minecraft");
        self.services.list(&lp).await.unwrap()
    }

    pub async fn query_dep_addr(&self, addr: &str, port: &str) -> Option<String> {
        let deploys = self.get_deploys().await;
        let result = deploys.iter().find(|x| filter_label_value(x, addr, port))?;
        Some(result.name()?.to_string())
    }

    pub async fn query_srv_addr(&self, addr: &str, port: &str) -> Option<String> {
        let deploys = self.get_srvs().await;
        let result = deploys.iter().find(|x| filter_label_value(x, addr, port))?;
        Some(result.name()?.to_string())
    }

    async fn set_dep_scale(&self, name: &str, num: i32) -> Result<Deployment, kube::Error> {
        let patch = Patch::Merge(json!({"spec":{"replicas": num}}));
        let pp = PatchParams::default();
        self.deployments.patch(name, &pp, &patch).await
    }
}

#[derive(Clone)]
pub struct McApi {
    cache: KubeCache,
    map: Arc<tokio::sync::Mutex<HashMap<String, JoinHandle<()>>>>,
}

impl MinecraftAPI<Server> for McApi {
    #[tracing::instrument(
        name = "MinecraftAPI::query_server",
        level = "info",
        skip(self, addr, port)
    )]
    async fn query_server(&self, addr: &str, port: &str) -> Result<Server, OpaqueError> {
        let addr = sanitize_addr(&addr);

        let dep_name = match self.cache.query_dep_addr(&addr, &port).await {
            Some(x) => x,
            None => {
                return Err(OpaqueError::create(&format!(
                    "Failed to find deployment name by addr"
                )))
            }
        };
        let srv_name = match self.cache.query_srv_addr(&addr, &port).await {
            Some(x) => x,
            None => {
                return Err(OpaqueError::create(&format!(
                    "Failed to find service name by addr"
                )))
            }
        };

        let deployment = self.cache.get_dep(&dep_name).await.map_err(|x| {
            format!(
                "Failed to query cache for deployment with dep_name err:{}",
                x.to_string()
            )
        })?;
        let service = self.cache.get_srv(&srv_name).await.map_err(|x| {
            format!(
                "Failed to query cache for service with dep_name err:{}",
                x.to_string()
            )
        })?;
        tracing::debug!("found kubernetes deployment & service");

        return Ok(Server {
            dep: deployment,
            srv: service,
            server_addr: addr.to_string(),
            cache: self.cache.clone(),
        });
    }

    async fn start_watch(
        self,
        server: impl MinecraftServerHandle,
        frequency: Duration,
    ) -> Result<(), OpaqueError> {
        let addr = server.get_addr().ok_or("could not get addr of server")?;
        let port = server.get_port().ok_or("could not get port of server")?;
        let full_addr = format!("{addr}:{port}");

        if let Some(handle) = self.map.lock().await.get(&full_addr) {
            if !handle.is_finished() {
                return Ok(());
            }
        }
        let span = tracing::span!(parent: None,tracing::Level::INFO, "server_watcher", addr, port);

        let full_addr_clone = full_addr.clone();
        let api = self.clone();
        let handle = tokio::spawn(
            async move {
                tracing::info!("starting watcher");
                loop {
                    tokio::time::sleep(frequency).await;
                    let server = api.query_server(&addr, &port).await.unwrap();
                    let status_json = match server.query_description().await {
                        Ok(x) => x,
                        Err(e) => {
                            tracing::error!(
                                err = format!("{}", e.context),
                                "could not query description"
                            );
                            return;
                        }
                    };
                    if status_json.get_players_online() == 0 {
                        // With this I don't need to specify that StatusTrait
                        // should be send as well.

                        // Otherwise I would need to have it be defined as:
                        // trait StatusTrait: Send { ... }
                        drop(status_json);
                        let mut guard = api.map.lock().await;
                        guard.remove(&full_addr_clone);
                        drop(guard);
                        if let Err(err) = server.stop().await {
                            tracing::error!(
                                trace = err.get_span_trace(),
                                err = err.context,
                                msg = "failed to stop server"
                            );
                        }
                        return;
                    }
                }
            }
            .instrument(span),
        );
        let mut guard = self.map.lock().await;
        guard.insert(full_addr.clone(), handle);
        drop(guard);

        Ok(())
    }
}

impl McApi {
    pub fn create() -> Option<Self> {
        Some(Self {
            cache: KubeCache::create()?,
            map: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        })
    }
}

#[derive(Clone)]
pub struct Server {
    dep: Deployment,
    srv: Service,
    server_addr: String,
    cache: KubeCache,
}
impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KubeServer")
            .field(
                "dep",
                &self
                    .dep
                    .metadata
                    .clone()
                    .name
                    .unwrap_or("#error#".to_string()),
            )
            .field(
                "srv",
                &self
                    .srv
                    .metadata
                    .clone()
                    .name
                    .unwrap_or("#error#".to_string()),
            )
            .field("server_addr", &self.server_addr)
            .finish()
    }
}
impl MinecraftServerHandle for Server {
    async fn start(&self) -> Result<(), OpaqueError> {
        self.set_scale(1).await.map_err(|e| {
            OpaqueError::create(&format!("failed to set deployment scale: err = {:?}", e))
        })
    }

    async fn stop(&self) -> Result<(), OpaqueError> {
        self.set_scale(0).await.map_err(|e| {
            OpaqueError::create(&format!("failed to set deployment scale: err = {:?}", e))
        })
    }

    #[tracing::instrument(level = "info")]
    async fn query_status(&self) -> Result<crate::mc_server::ServerDeploymentStatus, OpaqueError> {
        let mut status = match self.dep.clone().status {
            Some(x) => x,
            None => {
                return Err(OpaqueError::create(
                    "failed to get status of deployment for checking replicas",
                ))
            }
        };
        let total_replicas = status
            .replicas
            .get_or_insert_with(|| {
                tracing::trace!("total_replicas failed to get");
                -1
            })
            .clone();
        let available_replicas = status
            .available_replicas
            .get_or_insert_with(|| {
                tracing::trace!("available_replicas failed to get");
                -1
            })
            .clone();
        let ready_replicas = status
            .ready_replicas
            .get_or_insert_with(|| {
                tracing::trace!("ready_replicas failed to get");
                -1
            })
            .clone();
        tracing::debug!("total_replicas: {total_replicas} available_replicas: {available_replicas} ready_replicas : {ready_replicas }");

        if total_replicas > 0 {
            if ready_replicas > 0 {
                return match self.query_server_connectable().await {
                    Ok(x) => Ok(ServerDeploymentStatus::Connectable(x)),
                    Err(_) => Ok(ServerDeploymentStatus::PodOk),
                };
            }
            return Ok(ServerDeploymentStatus::Starting);
        } else {
            return Ok(ServerDeploymentStatus::Offline);
        }
    }

    fn get_internal_address(&self) -> Option<String> {
        Some(format!("localhost:{}", self.get_port()?))
    }

    fn get_addr(&self) -> Option<String> {
        Some(self.server_addr.clone())
    }

    async fn query_description(&self) -> Result<Box<dyn StatusTrait>, OpaqueError> {
        let status = self.query_status().await?;
        match status {
            ServerDeploymentStatus::Connectable(mut tcp_stream) => {
                let handshake = crate::packets::serverbound::handshake::Handshake::create(
                    crate::types::VarInt::from(746).ok_or("could not create VarInt WTF?")?,
                    crate::types::VarString::from(
                        self.get_addr().ok_or("failed to get addr of server")?,
                    ),
                    crate::types::UShort::from(1234),
                    crate::types::VarInt::from(1).ok_or("could not create VarInt WTF?")?,
                )
                .ok_or("failed to create handshake packet from scratch... WTF?")?;
                handshake
                    .send_packet(&mut tcp_stream)
                    .await
                    .map_err(|_e| "failed to send handshake packet to server")?;
                let status_rq = crate::packets::Packet::from_bytes(0, Vec::new())
                    .ok_or("Failed to create status request packet from scratch")?;
                status_rq
                    .send_packet(&mut tcp_stream)
                    .await
                    .map_err(|_e| "failed to send status request packet to server")?;
                let return_packet = crate::packets::Packet::parse(&mut tcp_stream).await?;
                let status_response =
                    crate::packets::clientbound::status::StatusResponse::parse(return_packet)
                        .await
                        .unwrap();

                return status_response.get_json().ok_or(OpaqueError::create(
                    "failed to parse status response from server",
                ));
            }
            _ => {
                return Err(OpaqueError::create(&format!(
                    "server is not running; status={:?}",
                    status
                )))
            }
        }
    }

    fn get_port(&self) -> Option<String> {
        let a = self.srv.clone().spec.unwrap().ports.unwrap();
        let port = a.iter().find(|x| x.name.clone().unwrap() == "mc-router")?;
        port.node_port.map(|x| x.to_string())
    }

    fn get_motd(&self) -> Option<String> {
        let all_container_motds = self
            .dep
            .spec
            .clone()?
            .template
            .spec?
            .containers
            .iter()
            .map(|cont| match cont.env.clone() {
                Some(es) => es
                    .iter()
                    .filter(|e| e.name.as_str() == "MOTD")
                    .map(|x| x.value.clone())
                    .collect::<Vec<Option<String>>>()
                    .first()?
                    .clone(),
                None => None,
            })
            .collect::<Vec<Option<String>>>();
        all_container_motds.first()?.clone()
    }
}

impl Server {
    async fn set_scale(&self, num: i32) -> Result<(), kube::Error> {
        let name = self
            .srv
            .metadata
            .clone()
            .name
            .unwrap_or("#error#".to_string());
        let res = self.cache.set_dep_scale(&name, num).await;
        if res.is_ok() {
            tracing::info!("scaled replicas of {} to {num}", self.server_addr);
        }
        Ok(())
    }
}

fn filter_label_value<R>(res: &&R, addr: &str, port: &str) -> bool
where
    R: ResourceExt,
{
    let mut found_port = false;
    res.labels()
        .iter()
        .filter(|(key, value)| match key.as_str() {
            "tami.moe/minecraft" => value.as_str() == addr,
            "tami.moe/minecraft-port" => {
                found_port = true;
                value.as_str() == port
            }
            _ => false,
        })
        .count()
        > 0
}

impl fmt::Debug for ServerDeploymentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connectable(_) => write!(f, "Connectable"),
            Self::Starting => write!(f, "Starting"),
            Self::PodOk => write!(f, "PodOk"),
            Self::Offline => write!(f, "Offline"),
        }
    }
}
impl From<kube::Error> for OpaqueError {
    fn from(value: kube::Error) -> Self {
        OpaqueError::create(value.to_string().as_str())
    }
}

fn terminate_at_null(str: &str) -> &str {
    match str.split('\0').next() {
        Some(x) => x,
        None => str,
    }
}

fn sanitize_addr(addr: &str) -> &str {
    // Thanks to a buggy minecraft, when the client sends a join
    // from a SRV DNS record, it will not use the address typed
    // in the game, but use the address redicted *to* by the
    // DNS record as the address for joining, plus a trailing "."
    //
    // For example:
    // server.example.com (_minecraft._tcp.server.example.com)
    // (the typed address)     I (the DNS SRV record which gets read)
    //                         V
    //            5 25565 server.example.com
    //                         I (the response for the DNS SRV query)
    //                         V
    //                server.example.com.
    //         (the address used in the protocol)
    let addr = addr.trim_end_matches(".");

    // Modded minecraft clients send null terminated strings,
    // after which they have extra data. This just removes them
    // from the addr lookup
    let addr = terminate_at_null(addr);
    addr
}
