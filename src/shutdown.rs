use staged_shutdown::StagedShutdown;
use strum_macros::EnumIter;
use tokio::signal;
use tracing::info;

#[derive(Eq, PartialEq, Hash, Copy, Clone, EnumIter, Debug)]
pub enum ShutdownStage {
    PreStop,
    Drain,
    Final,
}

pub type Shutdown = StagedShutdown<ShutdownStage>;

pub fn create_shutdown() -> Shutdown {
    let shutdown = StagedShutdown::new();

    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            sigint().await;
            info!("SIGINT");

            shutdown.trigger(ShutdownStage::PreStop);
        }
    });

    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown.wait_triggered(ShutdownStage::PreStop).await;
            info!("Shutdown stage 'PreStop' started");
            shutdown.wait_complete(ShutdownStage::PreStop).await;
            info!("Shutdown stage 'PreStop' finished");
            shutdown.trigger(ShutdownStage::Drain);
        }
    });

    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown.wait_triggered(ShutdownStage::Drain).await;
            info!("Shutdown stage 'Drain' started");
            shutdown.wait_complete(ShutdownStage::Drain).await;
            info!("Shutdown stage 'Drain' finished");
            shutdown.trigger(ShutdownStage::Final);
        }
    });

    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown.wait_triggered(ShutdownStage::Final).await;
            info!("Shutdown stage 'Cleanup' started");
            shutdown.wait_complete(ShutdownStage::Final).await;
            info!("Shutdown stage 'Cleanup' finished");
        }
    });

    shutdown
}

async fn sigint() {
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
