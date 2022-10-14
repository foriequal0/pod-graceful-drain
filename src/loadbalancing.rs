use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use uuid::Uuid;

use crate::consts;

#[derive(Clone, Debug)]
pub struct LoadBalancingConfig {
    instance_id: Uuid,
}

impl LoadBalancingConfig {
    pub fn new(instance_id: Uuid) -> Self {
        Self { instance_id }
    }

    pub fn get_id(&self) -> String {
        self.instance_id.to_string()
    }

    pub fn controls(&self, pod: &Pod) -> bool {
        let annotation = pod
            .annotations()
            .get(consts::DRAIN_CONTROLLER_ANNOTATION_KEY);

        matches!(
            annotation.map(|controller| Uuid::try_parse(controller)),
            Some(Ok(uuid)) if uuid == self.instance_id)
    }
}
