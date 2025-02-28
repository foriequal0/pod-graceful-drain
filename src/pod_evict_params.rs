use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use kube::api::{DeleteParams, EvictParams, Preconditions};

use crate::consts::DELETE_OPTIONS_ANNOTATION_KEY;
use crate::utils::to_delete_params;

pub fn get_pod_evict_params(pod: &Pod) -> Option<EvictParams> {
    let annotation = pod.annotations().get(DELETE_OPTIONS_ANNOTATION_KEY)?;

    let Ok(delete_options) = serde_json::from_str(annotation) else {
        // TODO : propagate error
        return None;
    };

    let Ok(delete_params) = to_delete_params(delete_options, false) else {
        // TODO : propagate error
        return None;
    };

    Some(EvictParams {
        delete_options: Some(DeleteParams {
            dry_run: false,
            preconditions: Some(Preconditions {
                uid: pod.uid(),
                // it'll cause conflict
                resource_version: None,
            }),
            grace_period_seconds: delete_params.grace_period_seconds,
            propagation_policy: delete_params.propagation_policy,
        }),
        ..EvictParams::default()
    })
}
