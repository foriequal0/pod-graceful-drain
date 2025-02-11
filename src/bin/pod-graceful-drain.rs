use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use color_eyre::config::Frame;
use eyre::Result;
use tokio::select;
use tracing::{debug, error, info, Level};
use tracing_error::ErrorLayer;
use tracing_subscriber::filter::FromEnvError;
use tracing_subscriber::fmt::Layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{filter::Directive, EnvFilter};

use pod_graceful_drain::{
    start_controller, start_reflectors, start_webhook, ApiResolver, Config, DownwardAPI,
    LoadBalancingConfig, ServiceRegistry, Shutdown, WebhookConfig,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<ExitCode> {
    let config = Config::parse();

    init_tracing_subscriber()?;
    install_color_eyre()?;

    print_build_info();

    let shutdown = Shutdown::new();
    if let Err(err) = try_main(config, &shutdown).await {
        error!(?err, "Failed to start server");
        shutdown.trigger_shutdown();
    }

    shutdown.wait_shutdown_triggered().await;

    select! {
        _ = shutdown.wait_shutdown_complete() => {},
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
            info!("Waiting for graceful shutdown");
            shutdown.wait_shutdown_complete().await;
        }
    }

    info!("Bye!");
    Ok(ExitCode::from(1))
}

async fn try_main(config: Config, shutdown: &Shutdown) -> Result<()> {
    let downward_api = DownwardAPI::from_env();
    let api_resolver = ApiResolver::try_new(kube::Config::infer().await?)?;
    let service_registry = ServiceRegistry::default();
    let loadbalancing = LoadBalancingConfig::with_pod_uid(downward_api.pod_uid.clone());

    info!("Starting");

    start_controller(&api_resolver, &service_registry, &loadbalancing, shutdown)?;
    let reflectors = start_reflectors(&api_resolver, &config, &service_registry, shutdown)?;
    start_webhook(
        &api_resolver,
        config,
        WebhookConfig::controller_runtime_default(),
        reflectors,
        &service_registry,
        &loadbalancing,
        &downward_api,
        shutdown,
    )
    .await?;

    info!("Services started");
    loop {
        let not_ready = service_registry.get_not_ready_services();
        if not_ready.is_empty() {
            info!("Service ready");
            break;
        }

        select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            _ = shutdown.wait_shutdown_triggered() => {
                break
            },
        }
    }

    Ok(())
}

fn selfish_frame_filter(frames: &mut Vec<&Frame>) {
    frames.retain(|frame| {
        matches!(frame.name.as_ref(),
            Some(name) if name == "pod_graceful_drain"
            || name.starts_with("pod_graceful_drain::"))
    });
}

fn init_tracing_subscriber() -> Result<()> {
    tracing_subscriber::registry()
        .with({
            let layer = Layer::default();
            let filter = env_filter()?;
            layer.with_filter(filter)
        })
        .with({
            let layer = ErrorLayer::default();
            let filter = env_filter()?;
            layer.with_filter(filter)
        })
        .try_init()?;

    return Ok(());

    fn env_filter() -> Result<EnvFilter, FromEnvError> {
        EnvFilter::builder()
            .with_default_directive(Directive::from(Level::INFO))
            .from_env()
    }
}

fn install_color_eyre() -> Result<()> {
    color_eyre::config::HookBuilder::new()
        .capture_span_trace_by_default(true)
        .add_frame_filter(Box::new(selfish_frame_filter))
        .install()?;
    Ok(())
}

fn print_build_info() {
    info!("tag: {}", env!("VERGEN_GIT_DESCRIBE"));
    debug!("branch: {}", env!("VERGEN_GIT_BRANCH"));
    debug!("commit: {}", env!("VERGEN_GIT_SHA"));
    debug!("commit date: {}", env!("VERGEN_GIT_COMMIT_DATE"));

    debug!("rustc: {}", env!("VERGEN_RUSTC_SEMVER"));
    debug!("build date: {}", env!("VERGEN_BUILD_TIMESTAMP"));
}
