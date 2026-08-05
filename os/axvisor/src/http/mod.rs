//! Management HTTP control plane.
//!
//! Served by an axum `Router` running on a tokio current-thread runtime
//! (see [`axum`]). The read-only routes live in [`vm`]; control routes
//! (start/stop) are added by the next PR. JSON is built with `serde_json`.
//!
//! This whole module is only compiled under the `http-axum` feature, which is
//! off by default. The hand-rolled HTTP/1.0 pilot was intentionally not
//! carried forward.

#[cfg(feature = "http-axum")]
pub mod axum;
#[cfg(feature = "http-axum")]
pub mod vm;

/// Blocking entry point for the management HTTP server.
///
/// Spawned on its own task (see `crate::main`); builds the tokio runtime and
/// serves until the hypervisor shuts down. Under `http-test` the built-in
/// self-test runs first and prints deterministic handler results.
#[cfg(feature = "http-axum")]
pub fn serve() {
    axum::serve();
}
