use std::io::Cursor;
use std::time::Duration;

use axum_server::tls_rustls::RustlsConfig;
use eyre::{Context, ContextCompat, Result, eyre};
use futures::StreamExt;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret;
use kube::Api;
use kube::runtime::{WatchStreamExt, watcher};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tracing::{Level, debug, error, info, span};

use crate::ApiResolver;
use crate::error_codes::is_410_expired_error_response;
use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
use crate::webhooks::config::{CertConfig, SecretCertConfig};

const TLS_CRT: &str = "tls.crt";
const TLS_KEY: &str = "tls.key";

pub async fn build_reactive_rustls_config(
    config: &CertConfig,
    api_resolver: &ApiResolver,
    shutdown: &Shutdown,
) -> Result<RustlsConfig> {
    match config {
        CertConfig::Secret(cert_config) => {
            let rustls_config = build(api_resolver, cert_config, shutdown).await?;
            Ok(rustls_config)
        }
        CertConfig::Override(cert, key) => {
            let serialized = Der::new_with(&[cert.clone()], key);
            let config = RustlsConfig::from_der(serialized.certs, serialized.key).await?;
            Ok(config)
        }
    }
}

enum State {
    Initial {
        config_tx: tokio::sync::oneshot::Sender<RustlsConfig>,
    },
    Running {
        last_der: Der,
        config: RustlsConfig,
    },
}

async fn build(
    api_resolver: &ApiResolver,
    cert_config: &SecretCertConfig,
    shutdown: &Shutdown,
) -> Result<RustlsConfig> {
    let (config_tx, config_rx) = tokio::sync::oneshot::channel();

    spawn_service(shutdown, span!(Level::INFO, "certwatcher"), {
        let shutdown = shutdown.clone();
        let api: Api<Secret> = api_resolver.default_namespaced();
        let config = watcher::Config::default()
            .fields(&format!("metadata.name={}", cert_config.cert_secret_name));

        async move {
            let mut stream = Box::pin(
                watcher(api, config)
                    .default_backoff()
                    .take_until(shutdown.wait_shutdown_triggered())
                    .applied_objects(),
            );

            let mut state = State::Initial { config_tx };

            while let Some(result) = stream.next().await {
                let secret = match result {
                    Ok(secret) => secret,
                    Err(err) => {
                        match err {
                            watcher::Error::WatchFailed(err)
                                if matches!(&state, &State::Running { .. }) =>
                            {
                                debug!(?err, "certwatcher failed. stream will restart soon");
                            }
                            watcher::Error::WatchError(resp)
                                if matches!(&state, &State::Running { .. })
                                    && is_410_expired_error_response(&resp) =>
                            {
                                debug!(?resp, "certwatcher error. stream will restart");
                            }
                            _ => {
                                error!(?err, "certwatcher error");
                            }
                        };

                        continue;
                    }
                };

                let new_der = match load_cert_from_secret(&secret) {
                    Ok(der) => der,
                    Err(err) => {
                        error!(%err, "cert reload err");
                        continue;
                    }
                };

                match state {
                    State::Initial { config_tx } => {
                        let Der { certs, key } = new_der.clone();
                        let config = match RustlsConfig::from_der(certs, key).await {
                            Ok(config) => config,
                            Err(err) => {
                                error!(%err, "cert reload err");
                                // reset the state
                                state = State::Initial { config_tx };
                                continue;
                            }
                        };

                        if config_tx.send(config.clone()).is_err() {
                            error!("certwatcher rx dropped");
                            break;
                        }

                        info!("cert loaded");
                        state = State::Running {
                            last_der: new_der,
                            config,
                        }
                    }
                    State::Running { config, last_der } => {
                        if last_der == new_der {
                            // reset the state
                            state = State::Running { config, last_der };
                            continue;
                        }

                        let Der { certs, key } = new_der.clone();
                        if let Err(err) = config.reload_from_der(certs, key).await {
                            error!(%err, "cert reload err");
                            // reset the state
                            state = State::Running { config, last_der };
                            continue;
                        };

                        info!("cert reloaded");
                        state = State::Running {
                            last_der: new_der,
                            config,
                        }
                    }
                }
            }
        }
    })?;

    let config = tokio::time::timeout(Duration::from_secs(10), config_rx).await??;
    Ok(config)
}

#[derive(PartialEq, Eq, Clone)]
struct Der {
    certs: Vec<Vec<u8>>,
    key: Vec<u8>,
}

impl Der {
    fn new_with(cert_der: &[CertificateDer], key_der: &PrivateKeyDer) -> Self {
        let certs = cert_der
            .iter()
            .map(|cert| Vec::from(cert.as_ref()))
            .collect();
        let key = Vec::from(key_der.secret_der());

        Der { certs, key }
    }
}

fn load_cert_from_secret(secret: &Secret) -> Result<Der> {
    let data = secret
        .data
        .as_ref()
        .ok_or(eyre!("secret doesn't have data"))?;

    let certs = {
        let ByteString(crt) = data
            .get(TLS_CRT)
            .ok_or(eyre!("secret doesn't have '{TLS_CRT}'"))?;
        rustls_pemfile::certs(&mut Cursor::new(crt))
            .collect::<std::io::Result<Vec<_>>>()
            .context(format!("Key({TLS_CRT})"))?
    };

    let key = {
        let ByteString(key) = data
            .get(TLS_KEY)
            .ok_or(eyre!("secret doesn't have '{TLS_KEY}'"))?;
        rustls_pemfile::private_key(&mut Cursor::new(key))
            .context(format!("Key({TLS_KEY})"))?
            .context("empty key")?
    };

    Ok(Der::new_with(&certs, &key))
}
