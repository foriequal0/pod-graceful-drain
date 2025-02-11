use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use eyre::{ContextCompat, Result};
use k8s_openapi::api::admissionregistration::v1::{
    MutatingWebhookConfiguration, ValidatingWebhookConfiguration,
};
use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::api::networking::v1::Ingress;
use kube::api::{ListParams, ObjectList};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use uuid::Uuid;

use pod_graceful_drain::{Config, LoadBalancingConfig, ServiceRegistry, WebhookConfig};

use crate::testutils::context::{within_test_cluster, TestContext};
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
    failurePolicy: Ignore
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
    failurePolicy: Ignore
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

#[tokio::test]
async fn should_intercept_eviction_by_kubectl_drain() {
    within_test_cluster(|context| async move {
        let config = Config {
            delete_after: Duration::from_secs(10),
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
  nodeName: {}-worker
  containers:
  - name: app
    image: public.ecr.aws/docker/library/busybox
    command: ["sleep", "9999"]"#,
            &context.cluster_name
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
        let context = Arc::new(context);

        let first = tokio::spawn({
            let context = Arc::clone(&context);
            async move {
                let start = Instant::now();
                kubectl!(
                    &context,
                    [
                        "drain",
                        "--force",
                        "--ignore-daemonsets",
                        &format!("{}-worker", &context.cluster_name)
                    ]
                );
                let duration = Instant::now() - start;
                assert!(
                    duration > Duration::from_secs(10 - 2),
                    "should be delayed approx 10s"
                );
            }
        });
        assert!(
            event_tracker
                .issued_soon("InterceptEviction", "Drain")
                .await
        );

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
                kubectl!(
                    &context,
                    [
                        "drain",
                        "--force",
                        "--ignore-daemonsets",
                        &format!("{}-worker", &context.cluster_name)
                    ]
                );
                let duration = Instant::now() - start;
                assert!(
                    duration > Duration::from_secs(10 - 2),
                    "should wait approx 10s"
                );
            }
        });
        assert!(
            event_tracker
                .issued_soon("InterceptEviction", "Draining")
                .await
        );

        assert!(
            pod_is_alive_for(&context, "some-pod", Duration::from_secs(10 - 2)).await,
            "pod is alive for approx 10s"
        );

        first.await.unwrap();
        second.await.unwrap();

        assert!(pod_is_deleted_within(&context, "some-pod", Duration::from_secs(5)).await);
    })
    .await;
}

#[tokio::test]
async fn should_intercept_deletion_by_kubectl_drain_disable_eviction() {
    within_test_cluster(|context| async move {
        let config = Config {
            delete_after: Duration::from_secs(30),
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
  nodeName: {}-worker
  containers:
  - name: app
    image: public.ecr.aws/docker/library/busybox
    command: ["sleep", "9999"]"#,
            &context.cluster_name
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
        let context = Arc::new(context);

        let first = tokio::spawn({
            let context = Arc::clone(&context);
            async move {
                let start = Instant::now();
                kubectl!(
                    &context,
                    [
                        "drain",
                        "--force",
                        "--ignore-daemonsets",
                        "--disable-eviction=true",
                        &format!("{}-worker", &context.cluster_name)
                    ]
                );
                let duration = Instant::now() - start;
                assert!(
                    duration > Duration::from_secs(30 - 5),
                    "should be delayed approx 30s again"
                );
            }
        });
        assert!(event_tracker.issued_soon("DelayDeletion", "Drain").await);

        assert_eq!(
            Some(true),
            {
                let pod = context
                    .api_resolver
                    .all::<Pod>()
                    .get_metadata("some-pod")
                    .await
                    .unwrap();
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
                kubectl!(
                    &context,
                    [
                        "drain",
                        "--force",
                        "--ignore-daemonsets",
                        "--disable-eviction=true",
                        &format!("{}-worker", &context.cluster_name)
                    ]
                );
                let duration = Instant::now() - start;
                assert!(
                    duration > Duration::from_secs(10 - 2),
                    "should wait approx 10s"
                );
            }
        });
        assert!(event_tracker.issued_soon("DelayDeletion", "Draining").await);

        assert!(
            pod_is_alive_for(&context, "some-pod", Duration::from_secs(10 - 2)).await,
            "pod is alive for approx 10s"
        );

        first.await.unwrap();
        second.await.unwrap();

        assert!(!pod_is_alive(&context, "some-pod").await);
    })
    .await;
}
