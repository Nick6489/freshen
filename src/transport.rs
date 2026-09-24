use crate::{Error, Result};
use reqwest::{blocking::Client, redirect::Policy};
use rustls_platform_verifier::BuilderVerifierExt;
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use url::Url;

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    Checking,
    Downloading { received: u64, total: Option<u64> },
    Verifying,
    Extracting { path: String },
    Ready,
}

#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Implement this trait for custom authentication, offline storage, or tests.
/// Implementations must enforce `limit` before writing excess bytes.
pub trait Transport: Send + Sync {
    fn download(
        &self,
        url: &Url,
        output: &mut dyn Write,
        limit: u64,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<u64>;
}

pub struct HttpTransport {
    client: Client,
}
impl HttpTransport {
    /// HTTPS only, including redirects, with bounded connect and total timeouts.
    pub fn new(timeout: Duration) -> Result<Self> {
        // Configure this client explicitly instead of changing the host's global
        // crypto provider. Certificate verification still uses the platform roots.
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Invalid(e.to_string()))?
        .with_platform_verifier()
        .map_err(|e| Error::Invalid(e.to_string()))?
        .with_no_client_auth();
        let client = Client::builder()
            .tls_backend_preconfigured(tls)
            .https_only(true)
            .connect_timeout(Duration::from_secs(15))
            .timeout(timeout)
            .user_agent(concat!("Freshen/", env!("CARGO_PKG_VERSION")))
            .redirect(Policy::limited(5))
            .build()?;
        Ok(Self { client })
    }
}

impl Transport for HttpTransport {
    fn download(
        &self,
        url: &Url,
        output: &mut dyn Write,
        limit: u64,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<u64> {
        cancel.check()?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(Error::Invalid(
                "download URLs must use HTTPS without embedded credentials".into(),
            ));
        }
        let mut response = self.client.get(url.clone()).send()?.error_for_status()?;
        let total = response.content_length();
        if total.is_some_and(|size| size > limit) {
            return Err(Error::SizeLimit);
        }
        let mut received = 0u64;
        let mut buffer = [0; 64 * 1024];
        loop {
            cancel.check()?;
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            received = received.checked_add(count as u64).ok_or(Error::SizeLimit)?;
            if received > limit {
                return Err(Error::SizeLimit);
            }
            output.write_all(&buffer[..count])?;
            events(Event::Downloading { received, total });
        }
        Ok(received)
    }
}

pub(crate) fn fetch(
    transport: &dyn Transport,
    url: &Url,
    limit: u64,
    cancel: &Cancellation,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    transport.download(url, &mut bytes, limit, cancel, &mut |_| {})?;
    if bytes.len() as u64 > limit {
        return Err(Error::SizeLimit);
    }
    Ok(bytes)
}
