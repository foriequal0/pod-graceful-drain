use std::future::Future;
use std::time::Duration;

use eyre::{Context, Result};
use tokio::task::{JoinError, JoinHandle};
use tokio::{select, spawn};
use tracing::{Instrument, Level, debug, error, span, warn};

use crate::shutdown::Shutdown;

#[derive(Debug)]
pub enum ServiceExit {
    GracefulShutdown,
    EarlyStop,
    Panic(JoinError),
}

pub fn spawn_service(
    shutdown: &Shutdown,
    name: impl Into<String>,
    future: impl Future<Output = ()> + Send + 'static,
) -> Result<JoinHandle<ServiceExit>> {
    let shutdown = shutdown.clone();
    let service_name = name.into();

    let wrapped = {
        let shutdown = shutdown.clone();
        async move {
            match spawn(future).await {
                Ok(_) if shutdown.is_shutdown_triggered() => ServiceExit::GracefulShutdown,
                Ok(_) => {
                    shutdown.trigger_shutdown();
                    ServiceExit::EarlyStop
                }
                Err(err) => {
                    shutdown.trigger_shutdown();
                    ServiceExit::Panic(err)
                }
            }
        }
    };

    let logged = {
        let shutdown = shutdown.clone();
        async move {
            let mut wrapped = Box::pin(wrapped);
            let shutdown_log = async move {
                shutdown.wait_shutdown_triggered().await;
                tokio::time::sleep(Duration::from_secs(3)).await;
            };

            debug!("Service starting");
            select! {
                exit = &mut wrapped => {
                    match &exit {
                        ServiceExit::GracefulShutdown => {
                            debug!("Service gracefully shutdown")
                        }
                        ServiceExit::EarlyStop => error!("Service stopped early"),
                        ServiceExit::Panic(err) => error!(%err, "Service panicked"),
                    }
                    exit
                },
                _ = shutdown_log => {
                    warn!("Service shutdown is taking some time");
                    wrapped.await
                },
            }
        }
    };

    let instrumented = logged.instrument(span!(Level::ERROR, "service", "{}", service_name));

    let waited = shutdown
        .wrap_delay_shutdown(instrumented)
        .context(service_name)?;

    Ok(spawn(waited))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn graceful_shutdown_on_shutdown_request() {
        let shutdown = Shutdown::new();
        let handle = spawn_service(&shutdown, "test", {
            let shutdown = shutdown.clone();
            async move {
                shutdown.wait_shutdown_triggered().await;
                tokio::time::sleep(Duration::from_micros(500)).await;
            }
        })
        .unwrap();

        shutdown.trigger_shutdown();

        assert_matches!(handle.await, Ok(ServiceExit::GracefulShutdown));
    }

    #[tokio::test]
    async fn should_capture_early_shutdown() {
        let shutdown = Shutdown::new();
        let handle = spawn_service(&shutdown, "test", async move {
            tokio::time::sleep(Duration::from_micros(500)).await;
        })
        .unwrap();

        assert_matches!(handle.await, Ok(ServiceExit::EarlyStop));
    }

    #[tokio::test]
    async fn should_capture_panic() {
        let shutdown = Shutdown::new();
        let handle = spawn_service(&shutdown, "test", async move {
            tokio::time::sleep(Duration::from_micros(500)).await;
            panic!();
        })
        .unwrap();

        assert_matches!(handle.await, Ok(ServiceExit::Panic(_)));
    }

    #[tokio::test]
    async fn should_early_shutdown_trigger_others_graceful_shutdown() {
        let shutdown = Shutdown::new();
        let handle = spawn_service(&shutdown, "test", async move {
            tokio::time::sleep(Duration::from_micros(500)).await;
        })
        .unwrap();

        let other_handle = spawn_service(&shutdown, "other", {
            let shutdown = shutdown.clone();
            async move {
                shutdown.wait_shutdown_triggered().await;
                tokio::time::sleep(Duration::from_micros(500)).await;
            }
        })
        .unwrap();

        assert_matches!(handle.await, Ok(ServiceExit::EarlyStop));
        assert!(shutdown.is_shutdown_triggered());
        assert_matches!(other_handle.await, Ok(ServiceExit::GracefulShutdown));
    }

    #[tokio::test]
    async fn should_panic_trigger_others_graceful_shutdown() {
        let shutdown = Shutdown::new();
        let handle = spawn_service(&shutdown, "test", async move {
            tokio::time::sleep(Duration::from_micros(500)).await;
            panic!();
        })
        .unwrap();

        let other_handle = spawn_service(&shutdown, "other", {
            let shutdown = shutdown.clone();
            async move {
                shutdown.wait_shutdown_triggered().await;
                tokio::time::sleep(Duration::from_micros(500)).await;
            }
        })
        .unwrap();

        assert_matches!(handle.await, Ok(ServiceExit::Panic(_)));
        assert!(shutdown.is_shutdown_triggered());
        shutdown.wait_shutdown_complete().await;
        assert_matches!(other_handle.await, Ok(ServiceExit::GracefulShutdown));
    }
}
