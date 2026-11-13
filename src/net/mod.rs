//! The launcher's own outbound networking: TLS 1.3 only, system CA store, optional pins,
//! a fixed host allowlist, no redirects, no proxies, no cookies, no compression.

pub mod http;
pub mod tls;
