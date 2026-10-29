//! The HTTP transport the server sync's clients share ([`super::webdav`],
//! [`super::dropbox`]): reqwest's async client on a small runtime of its own
//! (`block_on` – the sync engines are blocking and run on threads of their
//! own; never call it from an async task), its TLS from [`super::tls`] (the
//! only place on the phone that builds an HTTP client), no redirects,
//! connect 15 s, read 30 s.
//!
//! Every request has a ceiling: [`REQUEST_BASE`] plus its upload at
//! [`MIN_RATE`] until the answer's head, and a download that stays below
//! [`MIN_RATE`] on average once [`REQUEST_BASE`] has passed is given up –
//! the read timeout alone lets a server that sends a byte every 29 s hold a
//! sync, and the link or setting changes that wait for it, for ever. Both
//! are `sync_offline`.

use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use reqwest::{Method, RequestBuilder, StatusCode, Url};

use super::tls::{self, Trust};
use super::{CERTIFICATE, OFFLINE, SERVER_ERROR};
use crate::error::{AppError, AppResult};

/// A request gets this long, plus [`MIN_RATE`] for its body, before it is
/// given up.
pub const REQUEST_BASE: Duration = Duration::from_secs(120);
/// Bytes per second a transfer must reach on average once [`REQUEST_BASE`]
/// has passed.
pub const MIN_RATE: u64 = 16 * 1024;
/// How much of an answer that is not a success is read (an error's reason:
/// Dropbox sends it as JSON); the rest is dropped.
pub const ERROR_BODY_MAX: usize = 64 * 1024;

/// An answer: its status, headers and body (a success's in full, at most
/// the caller's limit; another one's first [`ERROR_BODY_MAX`] bytes).
#[derive(Debug)]
pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// The client and its runtime.
pub struct Http {
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http").finish_non_exhaustive()
    }
}

/// The code of a failed request (no answer): a TLS error anywhere in its
/// chain (`io::Error`s wrap it, nested) decides; otherwise the server could
/// not be reached.
pub fn network_error(e: &reqwest::Error) -> AppError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = source {
        match err.downcast_ref::<rustls::Error>() {
            Some(rustls::Error::InvalidCertificate(_)) => {
                return AppError::unsupported(CERTIFICATE)
            }
            Some(_) => return AppError::unsupported(SERVER_ERROR),
            None => {}
        }
        source = match err
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
        {
            Some(inner) => Some(inner as &(dyn std::error::Error + 'static)),
            None => err.source(),
        };
    }
    AppError::unsupported(OFFLINE)
}

impl Http {
    /// A client that accepts the server certificates of `trust`.
    pub fn new(trust: Trust) -> AppResult<Self> {
        let client = tls::client(trust)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(AppError::from)?;
        Ok(Http { client, runtime })
    }

    /// A request to build on.
    pub fn request(&self, method: Method, url: Url) -> RequestBuilder {
        self.client.request(method, url)
    }

    /// Sends `request` (`upload`: the length of its body, for the time
    /// limit). A success's body is read up to `max` bytes (`too_large`
    /// beyond), another answer's up to [`ERROR_BODY_MAX`].
    pub fn send(
        &self,
        request: RequestBuilder,
        upload: u64,
        max: u64,
        too_large: &str,
    ) -> AppResult<Answer> {
        let send_limit = REQUEST_BASE + Duration::from_secs(upload / MIN_RATE);
        let started = Instant::now();
        self.runtime.block_on(async move {
            let mut response = tokio::time::timeout(send_limit, request.send())
                .await
                .map_err(|_| AppError::unsupported(OFFLINE))?
                .map_err(|e| network_error(&e))?;
            let status = response.status();
            let headers = response.headers().clone();
            let mut body = Vec::new();
            if status.is_success() {
                if response.content_length().is_some_and(|n| n > max) {
                    return Err(AppError::unsupported(too_large));
                }
                while let Some(chunk) = response.chunk().await.map_err(|e| network_error(&e))? {
                    if (body.len() + chunk.len()) as u64 > max {
                        return Err(AppError::unsupported(too_large));
                    }
                    body.extend_from_slice(&chunk);
                    // A trickle: below the minimum rate once the base time
                    // has passed.
                    let elapsed = started.elapsed();
                    if elapsed > REQUEST_BASE
                        && (body.len() as u64) < MIN_RATE * (elapsed - REQUEST_BASE).as_secs()
                    {
                        return Err(AppError::unsupported(OFFLINE));
                    }
                }
            } else {
                // The reason, if the server gives one; an error never waits
                // for a long body.
                while body.len() < ERROR_BODY_MAX {
                    let next = tokio::time::timeout(tls::READ_TIMEOUT, response.chunk()).await;
                    match next {
                        Ok(Ok(Some(chunk))) => body.extend_from_slice(&chunk),
                        _ => break,
                    }
                }
                body.truncate(ERROR_BODY_MAX);
            }
            Ok(Answer {
                status,
                headers,
                body,
            })
        })
    }
}
