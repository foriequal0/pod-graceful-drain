use std::time::Duration;

use crate::pdb::fixup::force_trigger_sync;
use crate::tests::utils::context::within_test_namespace;
use crate::{apply_yaml, eventually, kubectl};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::api::policy::v1::PodDisruptionBudget;
use kube::ResourceExt;

#[tokio::test]
async fn check_stock_kubernetes_delete_updates_current_healthy_on_delete() {
    within_test_namespace(|context| async move {
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

        assert!(
            {
                let pdb: PodDisruptionBudget =
                    context.api_resolver.all().get("some-pod").await.unwrap();
                pdb.status.unwrap().current_healthy == 1
            },
            "pdb.status.currentHealthy is correctly set to 1"
        );

        kubectl!(&context, ["delete", "pod", "some-pod"]);

        assert!(
            eventually!({
                let pdb: PodDisruptionBudget =
                    context.api_resolver.all().get("some-pod").await.unwrap();
                pdb.status.unwrap().current_healthy == 0
            }),
            "pdb.status.currentHealthy is correctly set to 0"
        );
    })
    .await;
}

#[tokio::test]
async fn check_stock_kubernetes_delete_does_not_updates_current_healthy_on_label_remove() {
    within_test_namespace(|context| async move {
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

        assert!(
            {
                let pdb: PodDisruptionBudget =
                    context.api_resolver.all().get("some-pod").await.unwrap();
                pdb.status.unwrap().current_healthy == 1
            },
            "pdb.status.currentHealthy is correctly set to 1"
        );

        kubectl!(&context, ["label", "pod/some-pod", "app-",]);

        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            {
                let pdb: PodDisruptionBudget =
                    context.api_resolver.all().get("some-pod").await.unwrap();
                pdb.status.unwrap().current_healthy == 1
            },
            "pdb.status.currentHealthy is still incorrectly 1"
        );
    })
    .await;
}

#[tokio::test]
async fn test_fixup_current_healthy() {
    within_test_namespace(|context| async move {
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

        assert!(
            {
                let pdb: PodDisruptionBudget =
                    context.api_resolver.all().get("some-pod").await.unwrap();
                pdb.status.unwrap().current_healthy == 1
            },
            "pdb.status.currentHealthy is correctly set to 1"
        );

        kubectl!(&context, ["label", "pod/some-pod", "app-",]);
        {
            let pdb = context.api_resolver.all().get("some-pod").await.unwrap();

            force_trigger_sync(&pdb, &context.api_resolver)
                .await
                .unwrap();
        }

        assert!(
            eventually!({
                let pdb: PodDisruptionBudget =
                    context.api_resolver.all().get("some-pod").await.unwrap();
                pdb.annotations()
                    .get("pod-graceful-drain/force-sync-nonce")
                    .is_some()
            }),
            "pdb should have force-sync nonce annotation"
        );

        assert!(
            eventually!({
                let pdb: PodDisruptionBudget =
                    context.api_resolver.all().get("some-pod").await.unwrap();
                pdb.status.unwrap().current_healthy == 0
            }),
            "pdb.status.currentHealthy is correctly set to 0"
        );
    })
    .await;
}
