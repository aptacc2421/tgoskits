//! Optional HTTP services hosted by Axvisor.
//!
//! Served by an axum `Router` running on a tokio current-thread runtime
//! (see [`server`]). The browser console and VM management API are independent
//! features that may share this listener when both are enabled.
//!
//! The server binds `127.0.0.1:8080` by default and only binds wider when
//! `[env] AXVM_HTTP_BIND` opts in. The control plane runs under a local-host
//! trust model: it has no authentication, so every caller that can reach the
//! listener may create, start, and delete VMs.
//!
//! This module is compiled when either optional HTTP feature is selected. Both
//! features are off by default.

#[cfg(feature = "browser-console")]
pub mod browser_console;
pub mod server;
#[cfg(feature = "http-axum")]
pub mod vm;

/// Blocking entry point for the configured HTTP services.
///
/// Spawned on its own task (see `crate::main`); builds the tokio runtime and
/// serves until the hypervisor shuts down.
pub fn serve() -> anyhow::Result<()> {
    server::serve()
}

/// Configured HTTP listener address used by the startup access banner.
#[cfg(feature = "browser-console")]
pub(crate) fn bind_addr() -> &'static str {
    server::bind_addr()
}

/// Whether the HTTP listener has successfully bound its configured address.
#[cfg(feature = "browser-console")]
pub(crate) fn is_listening() -> bool {
    server::is_listening()
}
