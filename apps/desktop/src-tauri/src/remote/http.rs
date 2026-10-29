//! The HTTP transport the server sync's clients share ([`super::webdav`],
//! [`super::dropbox`], [`super::gdrive`]): reqwest's async client on a small
//! runtime of its own
//! (`block_on` – the sync engines are blocking and run on threads of their
//! own; never call it from an async task), its TLS from [`super::tls`] (the
//! only place on the phone that builds an HTTP client), no redirects,
//! connect 15 s, read 30 s.
//!
//! Every request has a ceiling: [`REQUEST_BASE`] plus its upload at
//! [`MIN_RATE`] until the answer's head, and a download that stays below
//! [`MIN_RATE`] on average once [`REQUEST_BASE`] has passed since the head
//! came (an upload's time does not count against its answer) is given up –
//! the read timeout alone lets a server that sends a byte every 29 s hold a
//! sync, and the link or setting changes that wait for it, for ever. Both
//! are `sync_offline`. The reason of an answer that is not a success is read
//! for at most [`ERROR_BODY_TIME`] in all; what came by then is the reason.

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
/// Dropbox and Google send it as JSON); the rest is dropped.
pub const ERROR_BODY_MAX: usize = 64 * 1024;
/// How long an error's reason is read in all.
pub const ERROR_BODY_TIME: Duration = Duration::from_secs(10);

/// The ceilings of [`Http::send`] (the unit tests shorten them).
#[derive(Debug, Clone, Copy)]
struct Limits {
    base: Duration,
    min_rate: u64,
    error_time: Duration,
}

const LIMITS: Limits = Limits {
    base: REQUEST_BASE,
    min_rate: MIN_RATE,
    error_time: ERROR_BODY_TIME,
};

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
    limits: Limits,
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
        Ok(Http {
            client,
            runtime,
            limits: LIMITS,
        })
    }

    #[cfg(test)]
    fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
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
        let Limits {
            base,
            min_rate,
            error_time,
        } = self.limits;
        let send_limit = base + Duration::from_secs(upload / min_rate);
        self.runtime.block_on(async move {
            let mut response = tokio::time::timeout(send_limit, request.send())
                .await
                .map_err(|_| AppError::unsupported(OFFLINE))?
                .map_err(|e| network_error(&e))?;
            // The download's clock starts with the head (an upload took its
            // own time before).
            let started = Instant::now();
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
                    if elapsed > base && (body.len() as u64) < min_rate * (elapsed - base).as_secs()
                    {
                        return Err(AppError::unsupported(OFFLINE));
                    }
                }
            } else {
                // The reason, if the server gives one; an error never waits
                // for a long body – nor for a slow one.
                let deadline = tokio::time::Instant::now() + error_time;
                while body.len() < ERROR_BODY_MAX {
                    match tokio::time::timeout_at(deadline, response.chunk()).await {
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

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};

    use super::*;

    /// A server for one request: it reads the request, waits `head_after`,
    /// sends `head` and then `body` in pieces of `piece` bytes, `gap` apart.
    fn slow_server(
        head_after: Duration,
        head: &'static str,
        body: Vec<u8>,
        piece: usize,
        gap: Duration,
    ) -> Url {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let mut seen = Vec::new();
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => seen.extend_from_slice(&buf[..n]),
                }
            }
            std::thread::sleep(head_after);
            let _ = stream.write_all(head.as_bytes());
            for chunk in body.chunks(piece) {
                if stream.write_all(chunk).is_err() {
                    return;
                }
                let _ = stream.flush();
                std::thread::sleep(gap);
            }
        });
        url
    }

    fn http(limits: Limits) -> Http {
        Http::new(Trust::System).unwrap().with_limits(limits)
    }

    #[test]
    fn a_slow_upload_does_not_count_against_its_answer() {
        // The head comes after the base time (as after a long upload); the
        // short answer right after it is fine.
        let limits = Limits {
            base: Duration::from_millis(300),
            min_rate: 1024 * 1024,
            error_time: Duration::from_secs(1),
        };
        let url = slow_server(
            Duration::from_millis(700),
            "HTTP/1.1 201 Created\r\nContent-Length: 7\r\nConnection: close\r\n\r\n",
            b"Created".to_vec(),
            7,
            Duration::ZERO,
        );
        let http = http(limits);
        let request = http.request(Method::PUT, url).body(vec![0u8; 64]);
        let answer = http.send(request, 600_000_000, 100, "too_large").unwrap();
        assert_eq!(answer.status, StatusCode::CREATED);
        assert_eq!(answer.body, b"Created");
    }

    #[test]
    fn a_trickling_error_body_is_cut_off() {
        let limits = Limits {
            base: Duration::from_secs(5),
            min_rate: MIN_RATE,
            error_time: Duration::from_millis(500),
        };
        let url = slow_server(
            Duration::ZERO,
            "HTTP/1.1 500 Oops\r\nContent-Length: 65536\r\nConnection: close\r\n\r\n",
            vec![b'x'; 65536],
            1,
            Duration::from_millis(50),
        );
        let started = Instant::now();
        let http = http(limits);
        let answer = http
            .send(http.request(Method::GET, url), 0, 100, "too_large")
            .unwrap();
        assert_eq!(answer.status.as_u16(), 500);
        assert!(answer.body.len() < 100, "{}", answer.body.len());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_trickling_success_body_is_given_up() {
        let limits = Limits {
            base: Duration::from_millis(200),
            min_rate: 1024 * 1024,
            error_time: Duration::from_secs(1),
        };
        let url = slow_server(
            Duration::ZERO,
            "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n",
            vec![b'x'; 1000],
            10,
            Duration::from_millis(100),
        );
        let http = http(limits);
        let e = http
            .send(http.request(Method::GET, url), 0, 10_000, "too_large")
            .unwrap_err();
        assert_eq!(e.code(), "unsupported:sync_offline");
    }
}
