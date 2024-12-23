use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use eyre::{Context, Result};
use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ApiResource, DeleteParams, DynamicObject};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Api, Client, Config};
use rand::Rng;
use tempfile::NamedTempFile;
use tokio::task::JoinError;
use tracing::dispatcher::DefaultGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use pod_graceful_drain::{ApiResolver, LoadBalancingConfig, Shutdown};

use crate::testutils::run_command::{get_command_output, run_command, CommandParams};

const DEFAULT_KIND_IMAGE: &str = "kindest/node:v1.32.0";
const DEFAULT_TEST_CLUSTER_NAME: &str = "test-pgd";
const TEST_NAMESPACE_PREFIX: &str = "test";
const TEST_NAMESPACE_LABEL_KEY: &str = "test-pgd-ns";
const TEST_NAMESPACE_NAME_LABEL: &str = "name";

#[derive(Clone)]
pub struct TestContext {
    pub(crate) kubeconfig: Arc<NamedTempFile>,
    pub api_resolver: ApiResolver,
    pub cluster_name: String,
    pub namespace: String,
    pub loadbalancing: LoadBalancingConfig,
    pub shutdown: Shutdown,
    teardown: Arc<Mutex<Vec<Teardown>>>,
    pub(crate) cluster_resources: Arc<Mutex<HashSet<(ApiResource, String)>>>,
}

type Teardown = Box<dyn FnOnce(TestContext) -> Pin<Box<dyn Future<Output = ()>>> + Send + 'static>;
impl TestContext {
    pub fn register_teardown<Fut>(&self, func: impl FnOnce(TestContext) -> Fut + Send + 'static)
    where
        Fut: Future<Output = ()>,
    {
        self.teardown.lock().unwrap().push(Box::new(
            move |context| -> Pin<Box<dyn Future<Output = ()>>> {
                Box::pin(async move {
                    func(context).await;
                })
            },
        ))
    }
}

pub async fn within_test_namespace<F, Fut>(f: F) -> Fut::Output
where
    F: for<'a> FnOnce(TestContext) -> Fut + Send + 'static,
    Fut: Future + Send,
    Fut::Output: Send + 'static,
{
    let _logger = set_default_test_logger();

    let kind_cluster =
        std::env::var("KIND_CLUSTER").unwrap_or(DEFAULT_TEST_CLUSTER_NAME.to_owned());
    let result = within_random_namespace_with_cluster(&kind_cluster, f).await;
    match result {
        Ok(result) => result,
        Err(err) => std::panic::resume_unwind(err.into_panic()),
    }
}

pub async fn within_test_cluster<F, Fut>(f: F) -> Fut::Output
where
    F: for<'a> FnOnce(TestContext) -> Fut + Send + 'static,
    Fut: Future + Send,
    Fut::Output: Send + 'static,
{
    let _logger = set_default_test_logger();

    let random_cluster_name = format!(
        "{DEFAULT_TEST_CLUSTER_NAME}-{}",
        rand::rng().random_range(0..100000)
    );
    let kind_image = std::env::var("KIND_IMAGE").unwrap_or(DEFAULT_KIND_IMAGE.to_owned());
    let dummy_kubeconfig = NamedTempFile::new().unwrap();
    let mut cluster_config = NamedTempFile::new().unwrap();
    write!(
        &mut cluster_config,
        r#"
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
- role: control-plane
- role: worker
- role: worker
"#
    )
    .unwrap();

    run_command(&CommandParams {
        command: "kind",
        config_args: &[],
        args: &[
            "create",
            "cluster",
            "--image",
            &kind_image,
            "--name",
            &random_cluster_name,
            "--config",
            cluster_config.path().to_str().unwrap(),
            // not to messing with global kubeconfig.
            "--kubeconfig",
            dummy_kubeconfig.path().to_str().unwrap(),
        ],
        stdin: None,
    })
    .await
    .unwrap();

    let result = within_random_namespace_with_cluster(&random_cluster_name, f).await;

    run_command(&CommandParams {
        command: "kind",
        config_args: &[],
        args: &["delete", "cluster", "--name", &random_cluster_name],
        stdin: None,
    })
    .await
    .unwrap();

    match result {
        Ok(result) => result,
        Err(err) => std::panic::resume_unwind(err.into_panic()),
    }
}

async fn within_random_namespace_with_cluster<F, Fut>(
    cluster_name: &str,
    f: F,
) -> Result<Fut::Output, JoinError>
where
    F: for<'a> FnOnce(TestContext) -> Fut + Send + 'static,
    Fut: Future + Send,
    Fut::Output: Send + 'static,
{
    let context = match new_test_context(cluster_name, Uuid::nil()).await {
        Ok(context) => context,
        Err(err) => {
            eprintln!("{err:?}");
            panic!(
                "Tests require kind cluster named '{cluster_name}'. \
                Run `kind create cluster --image={DEFAULT_KIND_IMAGE} --name '{cluster_name}'` first."
            );
        }
    };

    let shutdown = context.shutdown.clone();

    let result = tokio::spawn({
        let context = context.clone();
        async move { f(context).await }
    })
    .await;

    shutdown.trigger_shutdown();
    shutdown.wait_shutdown_complete().await;

    let teardowns: Vec<_> = context.teardown.lock().unwrap().drain(..).collect();
    for teardown in teardowns.into_iter().rev() {
        let context = context.clone();
        teardown(context).await;
    }

    result
}

async fn new_test_context(cluster_name: &str, instance_id: Uuid) -> Result<TestContext> {
    let file = get_temp_kubeconfig_file_from_kind(cluster_name).await?;
    let kubeconfig = Kubeconfig::read_from(file.path()).context("valid kubeconfig yaml")?;
    let config = Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default()).await?;
    let shutdown = Shutdown::new();
    let namespace = create_random_namespace(&config).await?;
    let context = TestContext {
        kubeconfig: Arc::new(file),
        api_resolver: ApiResolver::try_new_within(config, &namespace)?,
        cluster_name: cluster_name.to_string(),
        namespace: namespace.clone(),
        loadbalancing: LoadBalancingConfig::new(instance_id),
        shutdown,
        teardown: Arc::new(Mutex::new(Vec::new())),
        cluster_resources: Arc::new(Mutex::new(HashSet::new())),
    };
    context.register_teardown(|context| async move {
        let client = &context.api_resolver.client;
        let namespace = &context.namespace;
        let _ = delete_namespace(client, namespace).await;
    });
    context.register_teardown({
        |context| async move {
            let client = &context.api_resolver.client;
            let cluster_resources = context.cluster_resources.lock().unwrap().clone();
            for (dyntype, name) in cluster_resources.iter() {
                let _ = Api::<DynamicObject>::all_with(client.clone(), dyntype)
                    .delete(name, &DeleteParams::default())
                    .await;
            }
        }
    });

    Ok(context)
}

async fn get_temp_kubeconfig_file_from_kind(context: &str) -> Result<NamedTempFile> {
    let mut file = NamedTempFile::new()?;

    let params = CommandParams {
        command: "kind",
        config_args: &[],
        args: &["get", "kubeconfig", "--name", context],
        stdin: None,
    };
    let output = get_command_output(&params).await?;
    file.as_file_mut().write_all(&output)?;

    Ok(file)
}

async fn create_random_namespace(config: &Config) -> Result<String> {
    let client = Client::try_from(config.clone())?;
    let api: Api<Namespace> = Api::all(client);
    let random_id = rand::rng().random_range(1..1000000);
    let random_name = format!("{TEST_NAMESPACE_PREFIX}-{random_id}");
    let namespace = Namespace {
        metadata: ObjectMeta {
            name: Some(random_name.clone()),
            labels: Some({
                let mut labels = BTreeMap::new();
                labels.insert(String::from(TEST_NAMESPACE_LABEL_KEY), String::new());
                labels.insert(String::from(TEST_NAMESPACE_NAME_LABEL), random_name.clone());
                labels
            }),
            ..ObjectMeta::default()
        },
        ..Namespace::default()
    };

    api.create(&Default::default(), &namespace).await?;
    Ok(random_name)
}

async fn delete_namespace(client: &Client, ns: &str) -> Result<()> {
    let api: Api<Namespace> = Api::all(client.clone());
    let _ns = api.delete(ns, &DeleteParams::default()).await?;
    Ok(())
}

fn set_default_test_logger() -> DefaultGuard {
    tracing::subscriber::set_default({
        let filter = EnvFilter::new("pod_graceful_drain=trace,test=trace");

        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_test_writer())
    })
}
