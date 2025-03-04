use std::env;

use eyre::{Result, eyre};

#[derive(Clone, Default)]
pub struct DownwardAPI {
    pub pod_name: Option<String>,
    pub pod_namespace: Option<String>,
    pub pod_uid: Option<String>,
    pub pod_service_account_name: Option<String>,

    pub(crate) release_fullname: Option<String>,
}

impl DownwardAPI {
    pub fn from_env() -> Self {
        let pod_name = get_env_var("POD_NAME");
        let pod_namespace = get_env_var("POD_NAMESPACE");
        let pod_uid = get_env_var("POD_UID");
        let pod_service_account_name = get_env_var("POD_SERVICE_ACCOUNT_NAME");

        let release_fullname = get_env_var("RELEASE_FULLNAME");
        Self {
            pod_name,
            pod_namespace,
            pod_uid,
            pod_service_account_name,

            release_fullname,
        }
    }

    pub fn get_release_fullname(&self) -> Result<&str> {
        if let Some(release_fullname) = &self.release_fullname {
            Ok(release_fullname.as_str())
        } else {
            Err(eyre!("environment variable 'RELEASE_FULLNAME' is missing"))
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
