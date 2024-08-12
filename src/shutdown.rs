use std::future::Future;

use async_shutdown::{
    DelayShutdownToken, ShutdownAlreadyCompleted, ShutdownComplete, ShutdownManager,
    ShutdownSignal, WrapDelayShutdown,
};
use eyre::Result;
use tokio::signal;
use tracing::info;

#[derive(Clone)]
pub struct Shutdown {
    drain: ShutdownManager<()>,
    shutdown: ShutdownManager<()>,
}

impl Shutdown {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Shutdown {
        Self::new_with_drain_signal(shutdown_signal())
    }

    pub fn new_with_drain_signal<F>(signal: F) -> Shutdown
    where
        F: Future + Send + Sync + 'static,
    {
        let drain = ShutdownManager::new();
        let shutdown = ShutdownManager::new();

        tokio::spawn({
            let drain = drain.clone();
            let shutdown = shutdown.clone();

            async move {
                signal.await;

                info!("Drain start");
                _ = drain.trigger_shutdown(());
                drain.wait_shutdown_complete().await;

                info!("Shutdown start");
                _ = shutdown.trigger_shutdown(());
            }
        });

        Shutdown { drain, shutdown }
    }

    pub fn is_drain_triggered(&self) -> bool {
        self.drain.is_shutdown_triggered()
    }

    pub fn wait_drain_triggered(&self) -> ShutdownSignal<()> {
        self.drain.wait_shutdown_triggered()
    }

    pub fn wait_drain_complete(&self) -> ShutdownComplete<()> {
        self.drain.wait_shutdown_complete()
    }

    pub fn delay_drain_token(
        &self,
    ) -> Result<DelayShutdownToken<()>, ShutdownAlreadyCompleted<()>> {
        self.drain.delay_shutdown_token()
    }

    pub fn wrap_delay_shutdown<F: Future>(
        &self,
        future: F,
    ) -> Result<WrapDelayShutdown<(), F>, ShutdownAlreadyCompleted<()>> {
        self.shutdown.wrap_delay_shutdown(future)
    }

    pub fn trigger_shutdown(&self) {
        _ = self.drain.trigger_shutdown(());
        _ = self.shutdown.trigger_shutdown(());
    }

    pub fn is_shutdown_triggered(&self) -> bool {
        self.shutdown.is_shutdown_triggered()
    }

    pub fn wait_shutdown_triggered(&self) -> ShutdownSignal<()> {
        self.shutdown.wait_shutdown_triggered()
    }

    pub fn wait_shutdown_complete(&self) -> ShutdownComplete<()> {
        self.shutdown.wait_shutdown_complete()
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler")
    };

    #[cfg(not(unix))]
    ctrl_c.await;

    #[cfg(unix)]
    {
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("failed to install signal handler")
                .recv()
                .await;
        };

        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        };
    }
}
