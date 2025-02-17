use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub struct WebhookConfig {
    pub(crate) bind: BindConfig,
    pub(crate) cert: CertConfig,
}

pub enum BindConfig {
    SocketAddr(SocketAddr),
    RandomForTest,
}

pub enum CertConfig {
    // Find certs `{CertDir}/{tls.crt,tls.key}`
    CertDir(PathBuf),
    // Override cert for test
    Override(CertificateDer<'static>, PrivateKeyDer<'static>),
}

impl WebhookConfig {
    pub fn controller_runtime_default() -> Self {
        // `sigs.k8s.io/controller-runtime` look for `{TempDir}/k8s-webhook-server/serving-certs/{tls.key,tls.crt}` files by default.
        let temp_dir = std::env::temp_dir();
        let default_path = temp_dir.join(Path::new("k8s-webhook-server/serving-certs"));
        Self {
            bind: BindConfig::SocketAddr(SocketAddr::from(([0, 0, 0, 0], 9443))),
            cert: CertConfig::CertDir(default_path),
        }
    }
}

impl WebhookConfig {
    pub fn random_port_for_test(
        cert: CertificateDer<'static>,
        key_pair_der: PrivateKeyDer<'static>,
    ) -> Self {
        Self {
            bind: BindConfig::RandomForTest,
            cert: CertConfig::Override(cert, key_pair_der),
        }
    }
}
