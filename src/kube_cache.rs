use std::{fmt, sync::Arc};

use k8s_openapi::api::{apps::v1::Deployment, core::v1::Service};
use kube::{
    api::{ListParams, ObjectList, Patch, PatchParams},
    runtime::reflector::Lookup,
    Api, Client, ResourceExt,
};
use serde_json::json;
use tokio::{net::TcpStream, sync::Mutex};

use crate::{
    kube_cache,
    packets::{serverbound::handshake::Handshake, Packet, SendPacket},
    OpaqueError,
};

#[derive(Debug)]
pub struct Cache {
    deployments: Api<Deployment>,
    services: Api<Service>,
}
impl Cache {
    pub fn create() -> Option<Cache> {
        let kubeconfig = kube::config::Kubeconfig::read().unwrap();
        let client = Client::try_from(kubeconfig).unwrap();

        let deployments: Api<Deployment> = Api::default_namespaced(client.clone());
        let services: Api<Service> = Api::default_namespaced(client);

        return Some(Cache {
            deployments,
            services,
        });
    }
    pub async fn get_dep(&self, name: &str) -> Result<Deployment, kube::Error> {
        self.deployments.get(name).await
    }
    pub async fn get_srv(&self, name: &str) -> Result<Service, kube::Error> {
        self.services.get(name).await
    }
    pub async fn get_deploys(&self) -> ObjectList<Deployment> {
        // let lp: ListParams = ListParams::default();
        let lp: ListParams = ListParams::default().labels("tami.moe/minecraft");
        self.deployments.list(&lp).await.unwrap()
    }
    pub async fn get_srvs(&self) -> ObjectList<Service> {
        // let lp: ListParams = ListParams::default();
        let lp: ListParams = ListParams::default().labels("tami.moe/minecraft");
        self.services.list(&lp).await.unwrap()
    }

    pub async fn query_dep_addr(&self, addr: &str) -> Option<String> {
        let deploys = self.get_deploys().await;
        let result = deploys.iter().find(|x| filter_label_value(x, addr))?;
        Some(result.name()?.to_string())
    }

    pub async fn query_srv_addr(&self, addr: &str) -> Option<String> {
        let deploys = self.get_srvs().await;
        let result = deploys.iter().find(|x| filter_label_value(x, addr))?;
        Some(result.name()?.to_string())
    }

    pub async fn set_dep_scale(&self, name: &str, num: i32) -> Result<Deployment, kube::Error> {
        let patch = Patch::Merge(json!({"spec":{"replicas": num}}));
        let pp = PatchParams::default();
        self.deployments.patch(name, &pp, &patch).await
    }
}

pub struct KubeServer {
    dep: Deployment,
    srv: Service,
    server_addr: String,
}
impl fmt::Debug for KubeServer {
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
impl KubeServer {
    #[tracing::instrument(name = "KubeServer::create", level = "info", skip(cache))]
    pub async fn create(
        cache: Arc<Mutex<kube_cache::Cache>>,
        server_addr: &str,
    ) -> Result<Self, OpaqueError> {
        let cache_guard = cache.lock().await;
        let dep_name = match cache_guard.query_dep_addr(server_addr).await {
            Some(x) => x,
            None => {
                return Err(OpaqueError::create(&format!(
                    "Failed to find deployment name by addr"
                )))
            }
        };
        let srv_name = match cache_guard.query_srv_addr(server_addr).await {
            Some(x) => x,
            None => {
                return Err(OpaqueError::create(&format!(
                    "Failed to find service name by addr"
                )))
            }
        };

        let deployment = cache_guard.get_dep(&dep_name).await.map_err(|x| {
            format!(
                "Failed to query cache for deployment with dep_name err:{}",
                x.to_string()
            )
        })?;
        let service = cache_guard.get_srv(&srv_name).await.map_err(|x| {
            format!(
                "Failed to query cache for service with dep_name err:{}",
                x.to_string()
            )
        })?;
        drop(cache_guard);
        tracing::debug!("found kubernetes deployment & service");

        return Ok(Self {
            dep: deployment,
            srv: service,
            server_addr: server_addr.to_string(),
        });
    }
    pub fn get_port(&self) -> Option<i32> {
        let a = self.srv.clone().spec.unwrap().ports.unwrap();
        let port = a.iter().find(|x| x.name.clone().unwrap() == "mc-router")?;
        port.node_port
    }
    #[tracing::instrument(level = "info")]
    pub fn get_server_addr(&self) -> String {
        self.server_addr.clone()
    }
    #[tracing::instrument(level = "info")]
    pub async fn get_server_status(&self) -> Result<ServerDeploymentStatus, OpaqueError> {
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
    async fn query_server_connectable(&self) -> Result<TcpStream, OpaqueError> {
        let port = self
            .get_port()
            .ok_or_else(|| "failed to get port from service")?;
        let server_stream = TcpStream::connect(format!("localhost:{port}"))
            .await
            .map_err(|_| "failed to connect to minecraft server")?;

        tracing::trace!(
            "successfully connected to backend server; (connectibility check) {:?}",
            server_stream.peer_addr()
        );
        Ok(server_stream)
    }
    pub async fn proxy_status(
        &self,
        handshake: &Handshake,
        status_request: &Packet,
        client_stream: &mut TcpStream,
        server_stream: &mut TcpStream,
    ) -> Result<(), OpaqueError> {
        handshake
            .send_packet(server_stream)
            .await
            .map_err(|_| "failed to forward handshake packet to minecraft server")?;
        status_request
            .send_packet(server_stream)
            .await
            .map_err(|_| "failed to forward status request packet to minecraft server")?;

        let data_amount = tokio::io::copy_bidirectional(client_stream, server_stream)
            .await
            .map_err(|e| {
                format!(
                    "error during bidirectional copy between server and client; err={:?}",
                    e
                )
            })?;
        tracing::trace!("data exchanged while proxying status: {:?}", data_amount);
        Ok(())
    }
    pub async fn set_scale(
        &self,
        cache: Arc<Mutex<Cache>>,
        num: i32,
    ) -> Result<Deployment, kube::Error> {
        let name = self
            .srv
            .metadata
            .clone()
            .name
            .unwrap_or("#error#".to_string());
        let res = cache.lock().await.set_dep_scale(&name, num).await;
        if res.is_ok() {
            tracing::info!("scaled replicas of {} to {num}", self.server_addr);
        }
        return res;
    }
}

fn filter_label_value<R>(dep: &&R, str: &str) -> bool
where
    R: ResourceExt,
{
    dep.labels().values().filter(|x| x.as_str() == str).count() > 0
}

#[derive(Debug)]
pub enum ServerDeploymentStatus {
    Connectable(TcpStream),
    Starting,
    PodOk,
    Offline,
}
