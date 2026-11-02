//! HTTP agents: one for the web (verified TLS), one for League's local APIs
//! (self-signed certificate on 127.0.0.1, so verification is skipped there only).

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[derive(Debug)]
struct LocalhostVerifier(Arc<CryptoProvider>);

impl ServerCertVerifier for LocalhostVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// TLS config for 127.0.0.1 only (League client + live game API).
pub fn local_tls() -> Arc<rustls::ClientConfig> {
    static CFG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("tls versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(LocalhostVerifier(provider)))
            .with_no_client_auth();
        Arc::new(cfg)
    })
    .clone()
}

pub fn local() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .tls_config(local_tls())
            .timeout_connect(Duration::from_millis(500))
            .timeout(Duration::from_secs(3))
            .build()
    })
}

pub fn web() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent("aram-mayhem-overlay/0.2")
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
    })
}

pub fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = web().get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut out = Vec::with_capacity(256 * 1024);
    std::io::Read::read_to_end(&mut resp.into_reader().take(64 << 20), &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

use std::io::Read as _;
