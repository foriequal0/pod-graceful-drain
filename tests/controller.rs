use std::ops::Add;

use chrono::TimeDelta;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use tokio::time::Duration;

use pod_graceful_drain::{ServiceRegistry, patch_pod_isolate};

use crate::testutils::context::{TestContext, within_test_namespace};
use crate::testutils::operations::install_test_host_service;

mod testutils;

async fn setup(context: &TestContext) {
    install_test_host_service(context).await;
    let service_registry = ServiceRegistry::default();

    pod_graceful_drain::start_controller(
        &context.api_resolver,
        &service_registry,
        &context.loadbalancing,
        &context.shutdown,
    )
    .unwrap();
}

#[tokio::test]
async fn controller_shouldnt_delete_too_early() {
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

        patch_drain_until(&context, "some-pod", TimeDelta::seconds(10), None).await;

        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            !pod_has_been_deleted(&context, "some-pod").await,
            "pod shouldn't be deleted yet"
        );
    })
    .await;
}

#[tokio::test]
async fn controller_shouldnt_evict_too_early() {
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

        patch_drain_until(
            &context,
            "some-pod",
            TimeDelta::seconds(10),
            Some(&DeleteOptions::default()),
        )
        .await;

        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            !pod_has_been_deleted(&context, "some-pod").await,
            "pod shouldn't be deleted yet"
        );
    })
    .await;
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

        patch_drain_until(&context, "some-pod", TimeDelta::seconds(5), None).await;

        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            pod_has_been_deleted(&context, "some-pod").await,
            "pod should've been deleted"
        );
    })
    .await;
}

#[tokio::test]
async fn controller_should_evict_expired_pod() {
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

        patch_drain_until(
            &context,
            "some-pod",
            TimeDelta::seconds(5),
            Some(&DeleteOptions::default()),
        )
        .await;

        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            pod_has_been_deleted(&context, "some-pod").await,
            "pod should've been deleted"
        );
    })
    .await;
}

async fn patch_drain_until(
    context: &TestContext,
    name: &str,
    delta: TimeDelta,
    delete_options: Option<&DeleteOptions>,
) {
    let now = chrono::Utc::now();
    let drain_until = now.add(delta);
    let pod: Pod = context.api_resolver.all().get(name).await.unwrap();
    patch_pod_isolate(
        &context.api_resolver,
        &pod,
        drain_until,
        delete_options,
        &context.loadbalancing,
    )
    .await
    .unwrap();
}

async fn pod_has_been_deleted(context: &TestContext, name: &str) -> bool {
    let result = context.api_resolver.all::<Pod>().get(name).await;
    match result {
        Err(_) => true,
        Ok(pod) => pod.metadata.deletion_timestamp.is_some(),
    }
}
