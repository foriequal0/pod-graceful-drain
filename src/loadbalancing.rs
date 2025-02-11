use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use uuid::Uuid;

use crate::consts;

#[derive(Clone, Debug)]
pub struct LoadBalancingConfig {
    instance_id: String,
}

impl LoadBalancingConfig {
    pub fn with_pod_uid(pod_uid: Option<String>) -> Self {
        let instance_id = if let Some(id) = pod_uid {
            id
        } else {
            Uuid::new_v4().to_string()
        };

        Self { instance_id }
    }

    pub fn with_str(instance_id: &str) -> Self {
        Self {
            instance_id: instance_id.to_owned(),
        }
    }

    pub fn get_id(&self) -> &str {
        &self.instance_id
    }

    pub fn controls(&self, pod: &Pod) -> bool {
        let annotation = pod
            .annotations()
            .get(consts::DRAIN_CONTROLLER_ANNOTATION_KEY);

        matches!(annotation, Some(uuid) if uuid == &self.instance_id)
    }
}
