use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::api::policy::v1::PodDisruptionBudget;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use kube::runtime::events::{Recorder, Reporter};
use tokio::time::Duration;

use crate::controllers::evict::start_evict_controller;
use crate::labels_and_annotations::DrainingLabelValue;
use crate::patch::evict::patch_to_evict;
use crate::tests::utils::context::{TestContext, within_test_namespace};
use crate::tests::utils::event_tracker::EventTracker;
use crate::tests::utils::operations::install_test_host_service;
use crate::tests::utils::pod_state::{is_pod_patched, is_pod_patched_in};
use crate::{CONTROLLER_NAME, Config, ServiceRegistry, apply_yaml, kubectl, start_reflectors};

async fn setup(context: &TestContext) {
    install_test_host_service(context).await;
    let service_registry = ServiceRegistry::default();
    let config = Config {
        delete_after: Duration::from_secs(10),
        experimental_general_ingress: true,
    };
    let recorder = Recorder::new(
        context.api_resolver.client.clone(),
        Reporter {
            controller: String::from(CONTROLLER_NAME),
            instance: None,
        },
    );

    let stores = start_reflectors(
        &context.api_resolver,
        &config,
        &service_registry,
        &context.shutdown,
    )
    .unwrap();

    start_evict_controller(
        &context.api_resolver,
        &service_registry,
        &context.loadbalancing,
        &stores,
        &recorder,
        &context.shutdown,
    )
    .unwrap();
}

#[tokio::test]
async fn controller_should_patch() {
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

        patch_evict(&context, "some-pod").await;

        assert!(
            is_pod_patched_in(&context, "some-pod", 3, DrainingLabelValue::Draining).await,
            "pod should be patched to drain"
        );
    })
    .await;
}

#[tokio::test]
async fn controller_should_not_patch_until_pod_disruption_budget() {
    within_test_namespace(|context| async move {
        setup(&context).await;

        apply_yaml!(
            &context,
            PodDisruptionBudget,
            r#"
metadata:
  name: some-pod
spec:
  selector:
    matchLabels:
      app: test
  minAvailable: 1"# // maxUnavailable works when there is controller for the pod
        );

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

        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(5)).await;
        patch_evict(&context, "some-pod").await;
        assert!(
            event_tracker
                .issued_soon("WaitForPodDisruptionBudget", "PodDisruptionBudget")
                .await
        );

        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            !is_pod_patched(&context, "some-pod", DrainingLabelValue::Draining).await,
            "pod shouldn't have been patched yet to draining state"
        );
        assert!(
            is_pod_patched(&context, "some-pod", DrainingLabelValue::Evicting).await,
            "pod should be in evicting state"
        );

        apply_yaml!(
            &context,
            PodDisruptionBudget,
            r#"
metadata:
  name: some-pod
spec:
  selector:
    matchLabels:
      app: test
  minAvailable: 0"#
        );

        assert!(
            is_pod_patched_in(&context, "some-pod", 5, DrainingLabelValue::Draining).await,
            "pod should've been patched"
        );
    })
    .await;
}

async fn patch_evict(context: &TestContext, name: &str) {
    let pod: Pod = context.api_resolver.all().get(name).await.unwrap();
    patch_to_evict(
        &pod,
        &context.api_resolver,
        &context.loadbalancing,
        &DeleteOptions::default(),
    )
    .await
    .unwrap();
}
