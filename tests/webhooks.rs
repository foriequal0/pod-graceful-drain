use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use eyre::{ContextCompat, Result};
use k8s_openapi::api::admissionregistration::v1::{
    MutatingWebhookConfiguration, ValidatingWebhookConfiguration,
};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::api::networking::v1::Ingress;
use kube::api::{ListParams, ObjectList};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use uuid::Uuid;

use pod_graceful_drain::{Config, LoadBalancingConfig, ServiceRegistry, WebhookConfig};

use crate::testutils::context::{within_test_namespace, TestContext};
use crate::testutils::event_tracker::EventTracker;
use crate::testutils::operations::install_test_host_service;

mod testutils;

async fn generate_self_signed_cert(
    subject: String,
) -> Result<(String, CertificateDer<'static>, PrivateKeyDer<'static>)> {
    let cert_key = generate_simple_self_signed(vec![subject])?;

    let ca_bundle = base64::engine::general_purpose::STANDARD.encode(cert_key.cert.pem());
    let cert = cert_key.cert.der().clone();
    let private_key = {
        let pem = cert_key.key_pair.serialize_pem();
        let mut cursor = Cursor::new(pem.as_bytes());
        rustls_pemfile::private_key(&mut cursor)?.context("private key")?
    };

    Ok((ca_bundle, cert, private_key))
}

async fn setup(context: &TestContext, config: Config) {
    let namespace = &context.namespace;
    let service_domain = install_test_host_service(context).await;
    let (ca_bundle, cert, key_pair) = generate_self_signed_cert(service_domain).await.unwrap();
    let service_registry = ServiceRegistry::default();
    let loadbalancing = LoadBalancingConfig::new(Uuid::nil());

    pod_graceful_drain::start_controller(
        &context.api_resolver,
        &service_registry,
        &loadbalancing,
        &context.shutdown,
    )
    .unwrap();

    let stores = pod_graceful_drain::start_reflectors(
        &context.api_resolver,
        &config,
        &service_registry,
        &context.shutdown,
    )
    .unwrap();

    let port = pod_graceful_drain::start_webhook(
        &context.api_resolver,
        config,
        WebhookConfig::random_port_for_test(cert, key_pair),
        stores,
        &service_registry,
        &loadbalancing,
        &context.shutdown,
    )
    .await
    .unwrap()
    .port();

    apply_yaml!(
        context,
        ValidatingWebhookConfiguration,
        r#"
metadata:
  name: {namespace}-webhook
webhooks:
  - name: validate.pod-graceful-drain.io
    admissionReviewVersions: [v1beta1, v1]
    clientConfig:
      caBundle: {ca_bundle}
      service:
        namespace: {namespace}
        name: test-host
        path: /webhook/validate
        port: {port}
    rules:
      - apiGroups: [""]
        apiVersions: [v1]
        operations: [DELETE]
        resources: [pods]
    failurePolicy: Fail
    sideEffects: None
    timeoutSeconds: 15
    namespaceSelector:
      matchLabels:
        name: {namespace}"#,
    );

    apply_yaml!(
        context,
        MutatingWebhookConfiguration,
        r#"
metadata:
  name: {namespace}-webhook
webhooks:
  - name: mutate.pod-graceful-drain.io
    admissionReviewVersions: [v1beta1, v1]
    clientConfig:
      caBundle: {ca_bundle}
      service:
        namespace: {namespace}
        name: test-host
        path: /webhook/mutate
        port: {port}
    rules:
      - apiGroups: [""]
        apiVersions: [v1]
        operations: [CREATE]
        resources: [pods/eviction]
    failurePolicy: Fail
    sideEffects: NoneOnDryRun
    namespaceSelector:
      matchLabels:
        name: {namespace}"#,
    );
}

async fn pod_is_alive(context: &TestContext, name: &str) -> bool {
    let pod = context.api_resolver.all::<Pod>().get_metadata(name).await;
    match pod {
        Ok(pod) => pod.metadata.deletion_timestamp.is_none(),
        Err(kube::Error::Api(err)) if err.code == 404 || err.code == 409 => false,
        Err(err) => panic!("error: {err:?}"),
    }
}

async fn pod_is_alive_for(context: &TestContext, name: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    while Instant::now() - start < timeout {
        if !pod_is_alive(context, name).await {
            return false;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    true
}

async fn pod_is_deleted_within(context: &TestContext, name: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    while Instant::now() - start < timeout {
        if !pod_is_alive(context, name).await {
            return true;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    false
}

const DELETE_AFTER_SECS: u64 = 10;
const DELETE_AFTER: Duration = Duration::from_secs(DELETE_AFTER_SECS);
const DELETE_DELAY_APPROX_SECS: u64 = DELETE_AFTER_SECS * 60 / 100;
const DELETE_DELAY_APPROX: Duration = Duration::from_secs(DELETE_DELAY_APPROX_SECS);

#[tokio::test]
async fn should_delay_deletion_by_kubectl_delete() {
    within_test_namespace(|context| async move {
        let config = Config {
            delete_after: DELETE_AFTER,
            experimental_general_ingress: true,
        };
        setup(&context, config).await;

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

        apply_yaml!(
            &context,
            Service,
            r#"
metadata:
  name: some-service
spec:
  ports:
  - name: http
    port: 80
  selector:
    app: test"#
        );

        apply_yaml!(
            &context,
            Ingress,
            r#"
metadata:
  name: some-ingress
spec:
  rules:
  - http:
      paths:
      - backend:
          service:
            name: some-service
            port:
              name: http
        pathType: Exact
        path: /"#
        );

        kubectl!(&context, ["wait", "pod/some-pod", "--for=condition=Ready"]);

        let context = Arc::new(context);
        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(5)).await;

        let first = tokio::spawn({
            let context = Arc::clone(&context);
            async move {
                let start = Instant::now();
                kubectl!(&context, ["delete", "pod", "some-pod"]);
                let duration = Instant::now() - start;

                assert!(
                    duration > DELETE_DELAY_APPROX,
                    "should be delayed approx. 10s"
                );
            }
        });

        assert!(event_tracker.issued_soon("DelayDeletion", "Drain").await);

        assert_eq!(
            Some(true),
            {
                let pod: Pod = context.api_resolver.all().get("some-pod").await.unwrap();
                let labels = pod.metadata.labels.as_ref();
                let annotations = pod.metadata.annotations.as_ref();
                labels
                    .map(|l| l.contains_key("pod-graceful-drain/draining"))
                    .and(annotations.map(|a| a.contains_key("pod-graceful-drain/drain-until")))
            },
            "pod should've been patched"
        );
        assert!(
            {
                let es_list: ObjectList<EndpointSlice> = context
                    .api_resolver
                    .all()
                    .list(&ListParams::default().labels("kubernetes.io/service-name=some-service"))
                    .await
                    .unwrap();
                es_list.items.iter().all(|es| es.endpoints.is_empty())
            },
            "pod should've been removed from the endpointslices"
        );

        let second = tokio::spawn({
            let context = Arc::clone(&context);
            async move {
                let start = Instant::now();
                kubectl!(&context, ["delete", "pod", "some-pod"]);
                let duration = Instant::now() - start;
                assert!(
                    duration > DELETE_DELAY_APPROX,
                    "should still wait approx. 10s"
                );
            }
        });
        assert!(event_tracker.issued_soon("DelayDeletion", "Draining").await);

        assert!(
            pod_is_alive_for(&context, "some-pod", DELETE_DELAY_APPROX).await,
            "pod is alive for approx. 10s"
        );

        first.await.unwrap();
        second.await.unwrap();

        assert!(
            pod_is_deleted_within(&context, "some-pod", Duration::from_secs(20)).await,
            "pod is eventually deleted"
        );
    })
    .await;
}

#[tokio::test]
async fn should_allow_deletion_when_pod_is_not_ready() {
    within_test_namespace(|context| async move {
        let config = Config {
            delete_after: DELETE_AFTER,
            experimental_general_ingress: true,
        };
        setup(&context, config).await;

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
    command: ["sleep", "9999"]
    readinessProbe:
      httpGet:
        path: /no-existing
        port: 8080"#
        );

        apply_yaml!(
            &context,
            Service,
            r#"
metadata:
  name: some-service
spec:
  ports:
  - name: http
    port: 80
  selector:
    app: test"#
        );

        apply_yaml!(
            &context,
            Ingress,
            r#"
metadata:
  name: some-ingress
spec:
  rules:
  - http:
      paths:
      - backend:
          service:
            name: some-service
            port:
              name: http
        pathType: Exact
        path: /"#
        );

        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(1)).await;
        kubectl!(&context, ["delete", "pod", "some-pod", "--wait=false"]);
        assert!(event_tracker.issued_soon("AllowDeletion", "NotReady").await);
    })
    .await;
}

#[tokio::test]
async fn should_allow_deletion_when_pod_is_not_exposed() {
    within_test_namespace(|context| async move {
        let config = Config {
            delete_after: DELETE_AFTER,
            experimental_general_ingress: true,
        };
        setup(&context, config).await;

        kubectl!(
            &context,
            [
                "run",
                "some-pod",
                "--image=public.ecr.aws/docker/library/busybox",
                "--",
                "sleep",
                "9999"
            ]
        );

        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(1)).await;
        kubectl!(&context, ["delete", "pod", "some-pod"]);
        assert!(
            event_tracker
                .issued_soon("AllowDeletion", "NotExposed")
                .await
        );
    })
    .await;
}

#[tokio::test]
async fn should_allow_deletion_when_dry_run() {
    within_test_namespace(|context| async move {
        let config = Config {
            delete_after: DELETE_AFTER,
            experimental_general_ingress: true,
        };
        setup(&context, config).await;

        kubectl!(
            &context,
            [
                "run",
                "some-pod",
                "--image=public.ecr.aws/docker/library/busybox",
                "--",
                "sleep",
                "9999"
            ]
        );

        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(1)).await;
        kubectl!(&context, ["delete", "pod", "some-pod", "--dry-run=server"]);
        assert!(event_tracker.issued_soon("Allow", "DryRun").await);
    })
    .await;
}

#[tokio::test]
async fn should_delay_deletion_by_deployment_rollout() {
    within_test_namespace(|context| async move {
        let config = Config {
            delete_after: DELETE_AFTER,
            experimental_general_ingress: true,
        };
        setup(&context, config).await;

        apply_yaml!(
            &context,
            Deployment,
            r#"
metadata:
  name: some-deploy
spec:
  selector:
    matchLabels:
      app: some-deploy
  template:
    metadata:
      labels:
        app: some-deploy
    spec:
      containers:
      - name: app
        image: public.ecr.aws/docker/library/busybox
        command: ["sleep", "9999"]"#
        );

        apply_yaml!(
            &context,
            Service,
            r#"
metadata:
  name: some-service
spec:
  ports:
  - name: http
    port: 80
  selector:
    app: some-deploy"#
        );

        apply_yaml!(
            &context,
            Ingress,
            r#"
metadata:
  name: some-ingress
spec:
  rules:
  - http:
      paths:
      - backend:
          service:
            name: some-service
            port:
              name: http
        pathType: Exact
        path: /"#
        );

        kubectl!(
            &context,
            [
                "wait",
                "deployment/some-deploy",
                "--for=condition=Available"
            ]
        );

        let pod_name = context
            .api_resolver
            .all::<Pod>()
            .list_metadata(&ListParams::default())
            .await
            .expect("list success")
            .iter()
            .next()
            .expect("there's a pod")
            .metadata
            .name
            .clone()
            .expect("there's a pod name");

        kubectl!(&context, ["rollout", "restart", "deployment/some-deploy"]);

        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(5)).await;
        assert!(event_tracker.issued_soon("DelayDeletion", "Drain").await);

        assert!(
            pod_is_alive_for(&context, &pod_name, DELETE_DELAY_APPROX).await,
            "pod is alive for approx. 10s"
        );

        kubectl!(
            &context,
            ["rollout", "status", "deployment/some-deploy", "--watch"]
        );

        assert!(
            pod_is_deleted_within(&context, &pod_name, Duration::from_secs(20)).await,
            "pod is eventually deleted"
        );
    })
    .await;
}
