use k8s_openapi::api::apps::v1::Deployment;
use kube::{
    api::{ListParams, ObjectList},
    runtime::reflector::Lookup,
    Api, Client, ResourceExt,
};

pub struct Cache {
    deployments: Api<Deployment>,
}
impl Cache {
    pub fn create() -> Option<Cache> {
        let kubeconfig = kube::config::Kubeconfig::read().unwrap();
        let client = Client::try_from(kubeconfig).unwrap();

        let deployments: Api<Deployment> = Api::default_namespaced(client);

        return Some(Cache { deployments });
    }
    pub async fn get_deploys(&self) -> ObjectList<Deployment> {
        // let lp: ListParams = ListParams::default();
        let lp: ListParams = ListParams::default().labels("tami.moe/minecraft");
        self.deployments.list(&lp).await.unwrap()
    }

    pub async fn query_addr(&self, addr: String) -> Option<String> {
        let deploys = self.get_deploys().await;
        let result = deploys
            .iter()
            .find(|x| filter_label_value(x, addr.clone()))?;
        Some(result.name()?.to_string())
    }
}

fn filter_label_value(dep: &Deployment, str: String) -> bool {
    dep.labels()
        .values()
        .filter(|x| x.as_str() == str.as_str())
        .count()
        > 0
}
