use axum_server::tls_rustls::RustlsConfig;
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use debounced::debounced;
use eyre::{Context, ContextCompat, Result};
use futures::StreamExt;
use genawaiter::sync::Gen;
use notify::{RecursiveMode, Watcher};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::fs::File;
use tokio::io::copy;
use tokio::sync::mpsc;
use tracing::error;

use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
use crate::webhooks::config::CertConfig;

const TLS_CRT: &str = "tls.crt";
const TLS_KEY: &str = "tls.key";

pub async fn build_reactive_rustls_config(
    config: &CertConfig,
    shutdown: &Shutdown,
) -> Result<RustlsConfig> {
    match config {
        CertConfig::CertDir(cert_dir) => {
            let config = build(cert_dir, shutdown).await?;
            Ok(config)
        }
        CertConfig::Override(cert, key) => {
            let serialized = SerializedCertifiedKey::new_with(&[cert.clone()], key);
            let config = RustlsConfig::from_der(serialized.certs, serialized.key).await?;
            Ok(config)
        }
    }
}

async fn build(cert_dir: &Path, shutdown: &Shutdown) -> Result<RustlsConfig> {
    let config = {
        let cert = load_cert_from(cert_dir).await?;
        RustlsConfig::from_der(cert.certs, cert.key).await?
    };

    let (watcher_tx, mut watcher_rx) = mpsc::channel(1);
    let mut watcher_stream = {
        let mut watcher = notify::recommended_watcher(move |_| {
            let _ = watcher_tx.try_send(());
        })?;
        watcher.watch(&cert_dir.join(TLS_CRT), RecursiveMode::NonRecursive)?;
        watcher.watch(&cert_dir.join(TLS_KEY), RecursiveMode::NonRecursive)?;

        let stream = Gen::new(move |mut co| async move {
            let _watcher = watcher; // move watcher into generator
            while let Some(event) = watcher_rx.recv().await {
                co.yield_(event).await;
            }
        });

        let debounced = debounced(stream, Duration::from_secs(1));
        debounced.take_until(shutdown.wait_shutdown_triggered())
    };

    spawn_service(shutdown, "certwatcher", {
        let config = config.clone();
        let cert_dir = cert_dir.to_path_buf();
        async move {
            while watcher_stream.next().await.is_some() {
                let cert = match load_cert_from(&cert_dir).await {
                    Ok(cert) => cert,
                    Err(err) => {
                        error!(?err, "Reloading cert fail");
                        continue;
                    }
                };

                if let Err(err) = config.reload_from_der(cert.certs, cert.key).await {
                    error!(?err, "Reloading cert fail");
                    continue;
                };
            }
        }
    })?;

    Ok(config)
}

struct SerializedCertifiedKey {
    certs: Vec<Vec<u8>>,
    key: Vec<u8>,
}

impl SerializedCertifiedKey {
    fn new_with(cert_der: &[CertificateDer], key_der: &PrivateKeyDer) -> Self {
        let certs = cert_der
            .iter()
            .map(|cert| Vec::from(cert.as_ref()))
            .collect();
        let key = Vec::from(key_der.secret_der());

        SerializedCertifiedKey { certs, key }
    }
}

async fn load_cert_from(cert_dir: &Path) -> Result<SerializedCertifiedKey> {
    let certs = {
        let path = cert_dir.join(TLS_CRT);
        let mut file = File::open(&path).await.context(format!("File({path:?})"))?;
        let mut crt = Vec::new();
        copy(&mut file, &mut crt).await?;
        rustls_pemfile::certs(&mut Cursor::new(crt))
            .collect::<std::io::Result<Vec<_>>>()
            .context(format!("Cert({path:?})"))?
    };

    let key = {
        let path = cert_dir.join(TLS_KEY);
        let mut file = File::open(&path).await.context(format!("File({path:?}"))?;
        let mut key = Vec::new();
        copy(&mut file, &mut key).await?;
        rustls_pemfile::private_key(&mut Cursor::new(key))
            .context(format!("Key({path:?})"))?
            .context("empty key")?
    };

    Ok(SerializedCertifiedKey::new_with(&certs, &key))
}
