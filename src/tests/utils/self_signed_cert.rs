use std::io::Cursor;

use base64::Engine;
use eyre::{ContextCompat, Result};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub struct SelfSignedCert {
    pub ca_bundle: String,
    pub cert: CertificateDer<'static>,
    pub key_pair: PrivateKeyDer<'static>,
}

pub async fn generate(subject: String) -> Result<SelfSignedCert> {
    let cert_key = generate_simple_self_signed(vec![subject])?;

    let ca_bundle = base64::engine::general_purpose::STANDARD.encode(cert_key.cert.pem());
    let cert = cert_key.cert.der().clone();
    let private_key = {
        let pem = cert_key.key_pair.serialize_pem();
        let mut cursor = Cursor::new(pem.as_bytes());
        rustls_pemfile::private_key(&mut cursor)?.context("private key")?
    };

    Ok(SelfSignedCert {
        ca_bundle,
        cert,
        key_pair: private_key,
    })
}
