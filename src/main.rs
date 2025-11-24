//! This is a simple imitation of the basic functionality of kubectl:
//! kubectl {get, delete, apply, watch, edit} <resource> [name]
//! with labels and namespace selectors supported.
use anyhow::{bail, Context, Result};
use futures::{StreamExt, TryStreamExt};
use k8s_openapi::{
    api::apps::v1::Deployment, apimachinery::pkg::apis::meta::v1::Time, chrono::Utc,
};
use kube::{
    api::{Api, DynamicObject, ListParams, Patch, PatchParams, ResourceExt},
    core::GroupVersionKind,
    discovery::{ApiCapabilities, ApiResource, Discovery, Scope},
    runtime::{
        wait::{await_condition, conditions::is_deleted},
        watcher, WatchStreamExt,
    },
    Client,
};
use tracing::*;

mod packets;
mod types;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let kubeconfig = kube::config::Kubeconfig::read()?;
    let client = Client::try_from(kubeconfig)?;

    let deployments: Api<Deployment> = Api::default_namespaced(client);

    let lp: ListParams = ListParams::default();
    println!("{:?}", deployments.list(&lp).await?);

    Ok(())
}
