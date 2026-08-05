//! axum-based management HTTP server (`http-axum` feature).
//!
//! Runs an axum `Router` on a tokio current-thread runtime and serves the
//! management API. Routes and JSON fields mirror the hand-rolled pilot's API,
//! but dispatch and JSON construction are delegated to axum + serde_json.
//!
//! ```text
//! GET  /api/vms      → 200, JSON array (summary form)
//! GET  /api/vms/{id} → 200, JSON detail (with vcpu_states) | 404
//! ```
//!
//! PR B is the first in-hypervisor axum runtime validation. Two things are
//! deliberately exercised:
//!
//! 1. The tokio reactor initializes with `enable_io()` only (no time driver),
//!    so no `timerfd` syscall is required. If axum serve turns out to need the
//!    time driver, PR D (timerfd syscall) becomes mandatory.
//! 2. `tower::ServiceExt::oneshot` calls the router without TCP, so the
//!    `http-test` self-test is deterministic and free of task-scheduling timing.

use axum::{Router, routing::get};

use crate::http::vm;

/// Assemble the management routes.
pub fn router() -> Router {
    Router::new()
        .route("/api/vms", get(vm::list_vms))
        .route("/api/vms/{id}", get(vm::vm_detail))
}

/// Blocking serve: build a tokio current-thread runtime and hand it to axum.
///
/// `main` spawns this on its own task via `std::thread::spawn(|| http::serve())`;
/// the runtime is built here. Only the IO driver is enabled — the epoll
/// reactor suffices for `axum::serve`; a time driver would need `timerfd`.
pub fn serve() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("failed to build tokio runtime");
    rt.block_on(async {
        #[cfg(feature = "http-test")]
        self_test().await;

        let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
            .await
            .expect("failed to bind management HTTP server");
        info!("management HTTP server (axum) listening on 0.0.0.0:8080");
        axum::serve(listener, router()).await.expect("server error");
    });
}

/// `http-test` built-in self-test: drive the router with
/// `tower::ServiceExt::oneshot` (no TCP loopback) and print the actual status
/// codes for QEMU smoke-test regex assertion. Asserts the same contract as the
/// pilot: `GET /api/vms -> 200` and `GET /api/vms/999 -> 404` (no specific VM
/// id is bound).
#[cfg(feature = "http-test")]
async fn self_test() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let router = router();

    let list = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/vms")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("GET /api/vms failed");
    info!("HTTP self-test: GET /api/vms -> {}", list.status());

    let detail = router
        .oneshot(
            Request::builder()
                .uri("/api/vms/999")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("GET /api/vms/999 failed");
    info!("HTTP self-test: GET /api/vms/999 -> {}", detail.status());
}
