use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use eyre::{ContextCompat, Result};
use k8s_openapi::api::admissionregistration::v1::MutatingWebhookConfiguration;
use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::api::networking::v1::Ingress;
use kube::Api;
use kube::api::{DeleteParams, EvictParams, ListParams, ObjectList, PostParams};
use kube::runtime::events::{Recorder, Reporter};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::labels_and_annotations::DrainingLabelValue;
use crate::tests::utils::context::{TestContext, within_test_cluster, within_test_namespace};
use crate::tests::utils::event_tracker::EventTracker;
use crate::tests::utils::operations::install_test_host_service;
use crate::tests::utils::pod_state::is_pod_patched;
use crate::{
    CONTROLLER_NAME, Config, DownwardAPI, LoadBalancingConfig, ServiceRegistry, WebhookConfig,
    apply_yaml, kubectl, start_reflectors, start_webhook,
};

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
    let downward_api = DownwardAPI::default();
    let loadbalancing = LoadBalancingConfig::with_str("test");
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

    let port = start_webhook(
        &context.api_resolver,
        config,
        WebhookConfig::random_port_for_test(cert, key_pair),
        stores,
        &service_registry,
        &loadbalancing,
        &downward_api,
        &recorder,
        &context.shutdown,
    )
    .await
    .unwrap()
    .port();

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

const DELETE_AFTER: Duration = Duration::from_secs(10);

#[tokio::test]
async fn should_intercept_eviction() {
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

        kubectl!(
            &context,
            [
                "wait",
                "pod/some-pod",
                "--for=condition=Ready",
                "--timeout=1m"
            ]
        );

        let context = Arc::new(context);
        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(5)).await;

        {
            let api: Api<Pod> = context.api_resolver.all();
            _ = api.evict("some-pod", &EvictParams::default()).await;
        }

        assert!(
            event_tracker
                .issued_soon("InterceptEviction", "WaitingForPodDisruptionBudget")
                .await
        );
        assert!(
            is_pod_patched(&context, "some-pod", DrainingLabelValue::Evicting).await,
            "pod should've been patched"
        );

        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            {
                let es_list: ObjectList<EndpointSlice> = context
                    .api_resolver
                    .all()
                    .list(&ListParams::default().labels("kubernetes.io/service-name=some-service"))
                    .await
                    .unwrap();
                !es_list.items.iter().any(|es| es.endpoints.is_empty())
            },
            "pod shouldn't have been removed from the endpointslices"
        );
    })
    .await;
}

#[tokio::test]
async fn should_allow_eviction_when_pod_is_not_ready() {
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

        kubectl!(
            &context,
            [
                "wait",
                "pod/some-pod",
                "--for=jsonpath={.status.phase}=Running'",
                "--timeout=1m"
            ]
        );

        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(1)).await;

        {
            let api: Api<Pod> = context.api_resolver.all();
            _ = api.evict("some-pod", &EvictParams::default()).await;
        }

        assert!(event_tracker.issued_soon("AllowEviction", "NotReady").await);
    })
    .await;
}

#[tokio::test]
async fn should_allow_eviction_when_pod_is_not_exposed() {
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

        kubectl!(
            &context,
            [
                "wait",
                "pod/some-pod",
                "--for=condition=Ready",
                "--timeout=1m"
            ]
        );

        let mut event_tracker = EventTracker::new(&context, Duration::from_secs(1)).await;

        {
            let api: Api<Pod> = context.api_resolver.all();
            _ = api.evict("some-pod", &EvictParams::default()).await;
        }

        assert!(
            event_tracker
                .issued_soon("AllowEviction", "NotExposed")
                .await
        );
    })
    .await;
}

#[tokio::test]
async fn should_allow_eviction_when_dry_run() {
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

        {
            let api: Api<Pod> = context.api_resolver.all();
            let evict_params = EvictParams {
                delete_options: Some(DeleteParams::default().dry_run()),
                post_options: PostParams {
                    dry_run: true,
                    ..PostParams::default()
                },
            };
            _ = api.evict("some-pod", &evict_params).await;
        }

        assert!(event_tracker.issued_soon("Allow", "DryRun").await);
    })
    .await;
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

        tokio::spawn({
            let context = Arc::clone(&context);
            async move {
                kubectl!(
                    &context,
                    [
                        "drain",
                        "--force",
                        "--ignore-daemonsets",
                        &format!("{}-worker", &context.cluster_name)
                    ]
                );
            }
        });
        assert!(
            event_tracker
                .issued_soon("InterceptEviction", "WaitingForPodDisruptionBudget")
                .await
        );

        assert!(
            is_pod_patched(&context, "some-pod", DrainingLabelValue::Evicting).await,
            "pod should've been patched"
        );

        tokio::spawn({
            let context = Arc::clone(&context);
            async move {
                kubectl!(
                    &context,
                    [
                        "drain",
                        "--force",
                        "--ignore-daemonsets",
                        &format!("{}-worker", &context.cluster_name)
                    ]
                );
            }
        });
        assert!(
            event_tracker
                .issued_soon("InterceptEviction", "WaitingForPodDisruptionBudget")
                .await
        );
    })
    .await;
}
