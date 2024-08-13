use std::env;

#[derive(Clone, Default)]
pub struct DownwardAPI {
    pub pod_name: Option<String>,
    pub pod_namespace: Option<String>,
    pub pod_uid: Option<String>,
    pub pod_service_account_name: Option<String>,
}

impl DownwardAPI {
    pub fn from_env() -> Self {
        let pod_name = get_env_var("POD_NAME");
        let pod_namespace = get_env_var("POD_NAMESPACE");
        let pod_uid = get_env_var("POD_UID");
        let pod_service_account_name = get_env_var("POD_SERVICE_ACCOUNT_NAME");
        Self {
            pod_name,
            pod_namespace,
            pod_uid,
            pod_service_account_name,
        }
    }
}

fn get_env_var(key: &str) -> Option<String> {
    let var = env::var(key).ok()?;
    if var.is_empty() {
        return None;
    }

    Some(var)
}
