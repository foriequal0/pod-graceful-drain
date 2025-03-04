use std::net::SocketAddr;

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
    // Find certs from secret
    Secret(SecretCertConfig),
    // Override cert for test
    Override(CertificateDer<'static>, PrivateKeyDer<'static>),
}

pub struct SecretCertConfig {
    pub cert_secret_name: String,
}

impl SecretCertConfig {
    fn new(release_fullname: &str) -> Self {
        let cert_secret_name = format!("{release_fullname}-cert");
        Self { cert_secret_name }
    }
}

impl WebhookConfig {
    pub fn controller_runtime_default(release_fullname: &str) -> Self {
        let config = SecretCertConfig::new(release_fullname);

        Self {
            bind: BindConfig::SocketAddr(SocketAddr::from(([0, 0, 0, 0], 9443))),
            cert: CertConfig::Secret(config),
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
