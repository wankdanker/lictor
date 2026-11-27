//! Running individual TCP and HTTP checks.

use lictor_types::HealthStatus;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::time::Instant;

/// Longest output kept from a check.
pub(crate) const MAX_OUTPUT: usize = 4096;

/// Result of one check run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub status: HealthStatus,
    pub output: String,
    pub duration: Duration,
}

pub(crate) fn truncate(mut s: String) -> String {
    if s.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str(" ...(truncated)");
    }
    s
}

/// Passing if a TCP connection to `address` succeeds within `timeout`.
pub(crate) async fn tcp(address: &str, timeout: Duration) -> Outcome {
    let start = Instant::now();
    let (status, output) = match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(address)).await {
        Ok(Ok(_)) => (HealthStatus::Passing, format!("TCP connect {address}: Success")),
        Ok(Err(e)) => (HealthStatus::Critical, format!("TCP connect {address}: {e}")),
        Err(_) => (HealthStatus::Critical, format!("TCP connect {address}: timed out after {timeout:?}")),
    };
    Outcome { status, output, duration: start.elapsed() }
}

fn client(skip_verify: bool) -> &'static reqwest::Client {
    static VERIFY: OnceLock<reqwest::Client> = OnceLock::new();
    static SKIP: OnceLock<reqwest::Client> = OnceLock::new();
    let cell = if skip_verify { &SKIP } else { &VERIFY };
    cell.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
            .user_agent(concat!("lictor-health-check/", env!("CARGO_PKG_VERSION")))
            .tls_danger_accept_invalid_certs(skip_verify)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("HTTP client")
    })
}

/// Passing on 2xx, warning on 429, critical on anything else or on error.
pub(crate) async fn http(
    url: &str,
    method: &str,
    headers: &BTreeMap<String, String>,
    skip_verify: bool,
    timeout: Duration,
) -> Outcome {
    let start = Instant::now();
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = client(skip_verify).request(method.clone(), url).timeout(timeout);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let (status, output) = match req.send().await {
        Ok(resp) => {
            let code = resp.status();
            let mut body = resp.text().await.unwrap_or_default();
            body.truncate(body.char_indices().nth(MAX_OUTPUT).map_or(body.len(), |(i, _)| i));
            let status = if code.is_success() {
                HealthStatus::Passing
            } else if code.as_u16() == 429 {
                HealthStatus::Warning
            } else {
                HealthStatus::Critical
            };
            (status, format!("HTTP {method} {url}: {code} Output: {body}"))
        }
        Err(e) if e.is_timeout() => {
            (HealthStatus::Critical, format!("HTTP {method} {url}: timed out after {timeout:?}"))
        }
        Err(e) => (HealthStatus::Critical, format!("HTTP {method} {url}: {}", error_chain(&e))),
    };
    Outcome { status, output: truncate(output), duration: start.elapsed() }
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push_str(": ");
        out.push_str(&s.to_string());
        cur = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, http::StatusCode, routing::get};

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn tcp_pass_and_fail() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let o = tcp(&addr, Duration::from_secs(1)).await;
        assert_eq!(o.status, HealthStatus::Passing, "{}", o.output);
        drop(l);
        let o = tcp(&addr, Duration::from_secs(1)).await;
        assert_eq!(o.status, HealthStatus::Critical);
        assert!(o.output.starts_with(&format!("TCP connect {addr}: ")));
    }

    #[tokio::test]
    async fn http_status_mapping() {
        let base = serve(
            Router::new()
                .route("/ok", get(|| async { "fine" }))
                .route("/busy", get(|| async { (StatusCode::TOO_MANY_REQUESTS, "slow down") }))
                .route("/bad", get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }))
                .route(
                    "/slow",
                    get(|| async {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        "late"
                    }),
                )
                .route(
                    "/hdr",
                    get(|h: axum::http::HeaderMap| async move {
                        if h.get("x-token").is_some_and(|v| v == "s3cret") {
                            StatusCode::OK
                        } else {
                            StatusCode::FORBIDDEN
                        }
                    }),
                ),
        )
        .await;
        let t = Duration::from_secs(2);
        let none = BTreeMap::new();
        let o = http(&format!("{base}/ok"), "GET", &none, false, t).await;
        assert_eq!(o.status, HealthStatus::Passing);
        assert!(o.output.contains("200 OK Output: fine"), "{}", o.output);
        assert_eq!(http(&format!("{base}/busy"), "GET", &none, false, t).await.status, HealthStatus::Warning);
        assert_eq!(http(&format!("{base}/bad"), "GET", &none, false, t).await.status, HealthStatus::Critical);
        let o = http(&format!("{base}/slow"), "GET", &none, false, Duration::from_millis(200)).await;
        assert_eq!(o.status, HealthStatus::Critical);
        assert!(o.output.contains("timed out"), "{}", o.output);
        assert_eq!(http(&format!("{base}/hdr"), "GET", &none, false, t).await.status, HealthStatus::Critical);
        let hdr = BTreeMap::from([("x-token".to_string(), "s3cret".to_string())]);
        assert_eq!(http(&format!("{base}/hdr"), "GET", &hdr, false, t).await.status, HealthStatus::Passing);
        let o = http("http://127.0.0.1:1/", "GET", &none, false, t).await;
        assert_eq!(o.status, HealthStatus::Critical);
    }

    #[test]
    fn truncates_long_output() {
        let s = truncate("é".repeat(MAX_OUTPUT));
        assert!(s.len() < MAX_OUTPUT + 20);
        assert!(s.ends_with("...(truncated)"));
        assert_eq!(truncate("short".into()), "short");
    }
}
