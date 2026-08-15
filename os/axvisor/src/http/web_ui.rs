//! Web management dashboard asset extraction and serving (`web-ui` feature).
//!
//! The static files under `ui/` are embedded into the binary at build time via
//! `include_bytes!` (no `build.rs` step), written to the mounted rootfs at
//! `/web/` on startup ([`init`]), and served back to browsers by the axum routes
//! ([`ui_routes`]). `web-ui` implies `http-axum` and `fs`, so the rootfs is
//! mounted and writable before [`init`] runs.
//!
//! The dashboard calls the management API from the same origin: the VM list and
//! detail are open GET routes, and the lifecycle buttons send the operator's
//! bearer token (entered in the dashboard, held in `sessionStorage`) on the
//! mutating `start`/`stop`/`pause`/`resume` requests.

use axum::{
    body::Body,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};

/// Embedded UI assets: `(asset name, MIME type, bytes)`.
///
/// Kept as a compile-time table so [`init`] and [`serve_asset`] iterate the same
/// set. `include_bytes!` paths are relative to this source file (`src/http/`),
/// so the assets live at `os/axvisor/ui/`.
fn get_web_ui_assets() -> &'static [(&'static str, &'static str, &'static [u8])] {
    &[
        (
            "index.html",
            "text/html; charset=utf-8",
            include_bytes!("../../ui/index.html"),
        ),
        (
            "dashboard.js",
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/dashboard.js"),
        ),
        (
            "style.css",
            "text/css; charset=utf-8",
            include_bytes!("../../ui/style.css"),
        ),
    ]
}

/// Write the embedded UI assets to `/web/` on the mounted rootfs.
///
/// Called once from `http::serve()` before the tokio runtime is built. Files
/// are written unconditionally so a stale `/web/` from a previous boot is always
/// refreshed.
pub fn init() {
    const WEB_ROOT: &str = "/web";
    if let Err(e) = std::fs::create_dir_all(WEB_ROOT) {
        warn!("web-ui: failed to create {WEB_ROOT}: {e}");
        return;
    }
    for (name, _mime, bytes) in get_web_ui_assets() {
        let path = format!("{WEB_ROOT}/{name}");
        match std::fs::write(&path, bytes) {
            Ok(()) => info!("web-ui: wrote {} ({} bytes)", path, bytes.len()),
            Err(e) => warn!("web-ui: failed to write {}: {e}", path),
        }
    }
}

/// Routes for the web dashboard assets.
///
/// Only the exact asset paths are registered (no wildcard), so an unknown path
/// like `/favicon.ico` falls through to 404 instead of probing the filesystem.
pub fn ui_routes() -> axum::Router {
    use axum::routing::get;
    axum::Router::new()
        .route("/", get(serve_asset))
        .route("/style.css", get(serve_asset))
        .route("/dashboard.js", get(serve_asset))
}

/// Serve one UI asset from the filesystem.
async fn serve_asset(uri: Uri) -> Response<Body> {
    let name = uri.path().trim_start_matches('/');
    let name = if name.is_empty() { "index.html" } else { name };
    let path = format!("/web/{name}");
    match std::fs::read(&path) {
        Ok(data) => {
            let mime = get_web_ui_assets()
                .iter()
                .find(|(n, _, _)| *n == name)
                .map(|(_, m, _)| *m)
                .unwrap_or("application/octet-stream");
            Response::builder()
                .header("content-type", mime)
                .body(Body::from(data))
                .unwrap_or_else(|_| {
                    (StatusCode::INTERNAL_SERVER_ERROR, "bad response").into_response()
                })
        }
        Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}
