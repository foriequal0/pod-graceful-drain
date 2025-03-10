use uuid::Uuid;

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
}
