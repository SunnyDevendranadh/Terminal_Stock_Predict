use std::collections::HashSet;
use std::sync::Arc;

use tonic::service::Interceptor;
use tonic::transport::server::{TcpConnectInfo, TlsConnectInfo};
use tonic::{Request, Status};
use x509_parser::parse_x509_certificate;

#[derive(Clone)]
pub struct ClientCertificateInterceptor {
    allowlist: Arc<HashSet<String>>,
}

impl ClientCertificateInterceptor {
    pub fn new(allowlist: HashSet<String>) -> Self {
        Self {
            allowlist: Arc::new(allowlist),
        }
    }
}

impl Interceptor for ClientCertificateInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let common_name = peer_common_name(&request)?;
        if !is_cn_allowed(&common_name, &self.allowlist) {
            return Err(Status::permission_denied(format!(
                "client certificate common name not allowed: {common_name}"
            )));
        }
        Ok(request)
    }
}

pub fn peer_common_name(request: &Request<()>) -> Result<String, Status> {
    let tls_info = request
        .extensions()
        .get::<TlsConnectInfo<TcpConnectInfo>>()
        .ok_or_else(|| Status::unauthenticated("missing TLS connection info"))?;

    let certs = tls_info
        .peer_certs()
        .ok_or_else(|| Status::unauthenticated("missing peer certificate chain"))?;

    let cert = certs
        .first()
        .ok_or_else(|| Status::unauthenticated("empty peer certificate chain"))?;

    extract_common_name_from_der(cert.as_ref())
        .ok_or_else(|| Status::unauthenticated("unable to extract certificate common name"))
}

pub fn extract_common_name_from_der(der: &[u8]) -> Option<String> {
    let (_, cert) = parse_x509_certificate(der).ok()?;
    let name = cert
        .subject()
        .iter_common_name()
        .find_map(|attr| attr.as_str().ok())
        .map(ToString::to_string);
    name
}

pub fn is_cn_allowed(common_name: &str, allowlist: &HashSet<String>) -> bool {
    allowlist.is_empty() || allowlist.contains(common_name)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{extract_common_name_from_der, is_cn_allowed};

    #[test]
    fn unknown_der_returns_none() {
        assert!(extract_common_name_from_der(&[0x00, 0x01, 0x02]).is_none());
    }

    #[test]
    fn allowlist_validation_works() {
        let mut allowlist = HashSet::new();
        allowlist.insert("ops-client-1".to_string());

        assert!(is_cn_allowed("ops-client-1", &allowlist));
        assert!(!is_cn_allowed("unknown-client", &allowlist));
        assert!(is_cn_allowed("any-client", &HashSet::new()));
    }
}
