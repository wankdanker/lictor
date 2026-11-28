//! The web UI at `/ui`: a single-page app built from `ui/` and embedded at compile time
//! (see `build.rs`). It talks to the agent through the same `/v1` API as the CLI.
//!
//! The UI's files hold no data, so they are served without the API token; every API call the
//! page makes still needs it.

use axum::Router;
use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));
}

/// Embedded files: path relative to the UI build directory, and contents.
pub type Assets = &'static [(&'static str, &'static [u8])];

/// Where the UI is served. Must match `paths.base` in `ui/vite.config.ts`.
pub const BASE: &str = "/ui";

/// The page SvelteKit generates for routes that are rendered in the browser.
const FALLBACK: &str = "200.html";

/// True when this binary was built with the UI.
pub fn embedded() -> bool {
    !embedded::ASSETS.is_empty()
}

/// Routes for `/` (redirects to the UI) and `/ui/...`, serving the embedded build.
pub fn router() -> Router {
    router_with(embedded::ASSETS)
}

/// Routes serving `assets`.
pub fn router_with(assets: Assets) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        .route(BASE, get(move || async move { serve(assets, "") }))
        .route("/ui/", get(move || async move { serve(assets, "") }))
        .route("/ui/{*path}", get(move |Path(path): Path<String>| async move { serve(assets, &path) }))
}

fn serve(assets: Assets, path: &str) -> Response {
    if assets.is_empty() {
        return with_headers(
            (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], NOT_BUILT).into_response(),
        );
    }
    if let Some((name, body)) = assets.iter().find(|(name, _)| *name == path) {
        let cache =
            if name.starts_with("_app/immutable/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        return with_headers(
            ([(header::CONTENT_TYPE, content_type(name)), (header::CACHE_CONTROL, cache)], *body).into_response(),
        );
    }
    // A missing file (anything under `_app/`, or with an extension) is a 404. Anything else is a
    // page of the app: service, check and node ids cannot contain dots, so `/ui/services/web` is
    // never mistaken for a file.
    let last = path.rsplit('/').next().unwrap_or("");
    if path.starts_with("_app/") || last.contains('.') {
        return with_headers((StatusCode::NOT_FOUND, "not found").into_response());
    }
    match assets.iter().find(|(name, _)| *name == FALLBACK) {
        Some((_, body)) => with_headers(
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], *body)
                .into_response(),
        ),
        None => with_headers((StatusCode::NOT_FOUND, "not found").into_response()),
    }
}

/// Security headers for every UI response. The UI manages the agent, so it must not be framed
/// by another site (clickjacking).
fn with_headers(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("frame-ancestors 'none'"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    r
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

const NOT_BUILT: &str = "<!doctype html>
<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Lictor UI not built</title></head>
<body style=\"font-family: system-ui, sans-serif; max-width: 40rem; margin: 4rem auto; padding: 0 1rem\">
<h1>The web UI is not built into this binary</h1>
<p>This <code>lictor</code> was compiled without the UI. Build the UI, then the binary:</p>
<pre>npm --prefix ui ci
npm --prefix ui run build
cargo build --release</pre>
<p>The HTTP API under <code>/v1</code> works as usual.</p>
</body></html>
";

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    const FILES: Assets = &[
        ("200.html", b"<!doctype html>app"),
        ("_app/immutable/entry/start.abc123.js", b"export{}"),
        ("_app/version.json", b"{\"version\":\"1\"}"),
        ("favicon.svg", b"<svg/>"),
    ];

    async fn get(assets: Assets, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        let r = router_with(assets).oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
        let (parts, body) = r.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (parts.status, parts.headers, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn serves_files_with_types_and_caching() {
        let (s, h, body) = get(FILES, "/ui/_app/immutable/entry/start.abc123.js").await;
        assert_eq!((s, body.as_str()), (StatusCode::OK, "export{}"));
        assert_eq!(h[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert!(h[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
        let (s, h, _) = get(FILES, "/ui/favicon.svg").await;
        assert_eq!((s, &h[header::CONTENT_TYPE]), (StatusCode::OK, &HeaderValue::from_static("image/svg+xml")));
        assert_eq!(h[header::CACHE_CONTROL], "no-cache");
    }

    #[tokio::test]
    async fn pages_get_the_app_and_missing_files_get_404() {
        for uri in ["/ui", "/ui/", "/ui/services/web", "/ui/agent/services/web/edit"] {
            let (s, h, body) = get(FILES, uri).await;
            assert_eq!((s, body.as_str()), (StatusCode::OK, "<!doctype html>app"), "{uri}");
            assert_eq!(h[header::CACHE_CONTROL], "no-cache");
        }
        for uri in ["/ui/_app/immutable/missing.js", "/ui/robots.txt", "/ui/_app/nope"] {
            assert_eq!(get(FILES, uri).await.0, StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn root_redirects_and_headers_forbid_framing() {
        let (s, h, _) = get(FILES, "/").await;
        assert_eq!((s, &h[header::LOCATION]), (StatusCode::TEMPORARY_REDIRECT, &HeaderValue::from_static("/ui/")));
        let (_, h, _) = get(FILES, "/ui/services").await;
        assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(h[header::CONTENT_SECURITY_POLICY], "frame-ancestors 'none'");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }

    #[tokio::test]
    async fn explains_a_missing_build() {
        let (s, h, body) = get(&[], "/ui/services").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert!(body.contains("npm --prefix ui run build"), "{body}");
    }
}
