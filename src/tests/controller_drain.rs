use k8s_openapi::api::core::v1::Pod;
use tokio::time::Duration;

use crate::controllers::drain::start_drain_controller;
use crate::error_codes::is_404_not_found_error;
use crate::patch::drain::{PatchToDrainCaller, patch_to_drain};
use crate::tests::utils::context::{TestContext, within_test_namespace};
use crate::tests::utils::operations::install_test_host_service;
use crate::{Config, ServiceRegistry, apply_yaml, kubectl};

async fn setup(context: &TestContext) {
    install_test_host_service(context).await;
    let service_registry = ServiceRegistry::default();
    let config = Config {
        delete_after: Duration::from_secs(10),
        experimental_general_ingress: true,
    };

    start_drain_controller(
        &context.api_resolver,
        &service_registry,
        &context.loadbalancing,
        &config,
        &context.shutdown,
    )
    .unwrap();
}

#[tokio::test]
async fn controller_should_delete_expired_pod() {
    within_test_namespace(|context| async move {
        setup(&context).await;
        apply_yaml!(
            &context,
            Pod,
            r#"
metadata:
  name: some-pod
  labels:
    app: test
spec:
  containers:
  - name: app
    image: public.ecr.aws/docker/library/busybox
    command: ["sleep", "9999"]"#
        );
        kubectl!(
            &context,
            [
                "wait",
                "pod/some-pod",
                "--for=condition=Ready",
                "--timeout=1m"
            ]
        );

        patch_drain(&context, "some-pod").await;

        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            is_pod_deleted_in(&context, "some-pod", 5).await,
            "pod should've been deleted"
        );
    })
    .await;
}

#[tokio::test]
async fn controller_should_not_delete_too_early() {
    within_test_namespace(|context| async move {
        setup(&context).await;
        apply_yaml!(
            &context,
            Pod,
            r#"
metadata:
  name: some-pod
  labels:
    app: test
spec:
  containers:
  - name: app
    image: public.ecr.aws/docker/library/busybox
    command: ["sleep", "9999"]"#
        );
        kubectl!(
            &context,
            [
                "wait",
                "pod/some-pod",
                "--for=condition=Ready",
                "--timeout=1m"
            ]
        );

        patch_drain(&context, "some-pod").await;

        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            !is_pod_deleted(&context, "some-pod").await,
            "pod shouldn't be deleted yet"
        );
    })
    .await;
}

async fn patch_drain(context: &TestContext, name: &str) {
    let pod: Pod = context.api_resolver.all().get(name).await.unwrap();
    patch_to_drain(
        &pod,
        &context.api_resolver,
        &context.loadbalancing,
        PatchToDrainCaller::Webhook,
    )
    .await
    .unwrap();
}

async fn is_pod_deleted(context: &TestContext, name: &str) -> bool {
    let result = context.api_resolver.all::<Pod>().get(name).await;
    match result {
        Err(err) => is_404_not_found_error(&err),
        Ok(pod) => pod.metadata.deletion_timestamp.is_some(),
    }
}

async fn is_pod_deleted_in(context: &TestContext, name: &str, secs: u64) -> bool {
    for _ in 0..secs {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if is_pod_deleted(context, name).await {
            return true;
        }
    }

    false
}
