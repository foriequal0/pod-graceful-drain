use crate::labels_and_annotations::{DrainingLabelValue, get_pod_draining_label_value};
use crate::tests::utils::context::TestContext;
use k8s_openapi::api::core::v1::Pod;
use std::time::{Duration, Instant};

pub async fn is_pod_patched(context: &TestContext, name: &str, target: DrainingLabelValue) -> bool {
    let result = context.api_resolver.all::<Pod>().get(name).await;
    match result {
        Err(_) => false,
        Ok(pod) => get_pod_draining_label_value(&pod) == Ok(Some(target)),
    }
}

pub async fn is_pod_patched_in(
    context: &TestContext,
    name: &str,
    secs: u64,
    target: DrainingLabelValue,
) -> bool {
    for _ in 0..secs {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if is_pod_patched(context, name, target).await {
            return true;
        }
    }

    false
}

pub async fn pod_is_alive(context: &TestContext, name: &str) -> bool {
    let pod = context.api_resolver.all::<Pod>().get_metadata(name).await;
    match pod {
        Ok(pod) => pod.metadata.deletion_timestamp.is_none(),
        Err(kube::Error::Api(err)) if err.code == 404 || err.code == 409 => false,
        Err(err) => panic!("error: {err:?}"),
    }
}

pub async fn pod_is_alive_for(context: &TestContext, name: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    while Instant::now() - start < timeout {
        if !pod_is_alive(context, name).await {
            return false;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    true
}

pub async fn pod_is_deleted_within(context: &TestContext, name: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    while Instant::now() - start < timeout {
        if !pod_is_alive(context, name).await {
            return true;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    false
}
