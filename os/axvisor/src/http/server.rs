//! axum-based management HTTP server (`http-axum` feature).
//!
//! Runs an axum `Router` on a tokio current-thread runtime and serves the
//! management API. Routes and JSON fields mirror the hand-rolled pilot's API,
//! but dispatch and JSON construction are delegated to axum + serde_json.
//!
//! ```text
//! GET    /api/vms            → 200, JSON array (summary form)
//! GET    /api/vms/{id}       → 200, JSON detail (with vcpu_states) | 404
//! POST   /api/vms/create     → 200 {"id":N} | 400 | 409 | 500 (body {"toml": "..."})
//! DELETE /api/vms/{id}       → 204 | 404 | 500
//! POST   /api/vms/{id}/start → 200 {"ok":true,"status":...} | 404 | 409 | 503
//! POST   /api/vms/{id}/stop  → 200 {"ok":true,"status":...} | 404 | 409 | 503
//! ```
//!
//! The tokio reactor is initialized with `enable_io()` only (no time driver),
//! which needs only epoll, so no `timerfd` syscall is required.
//!
//! `http-test` drives the router with `tower::ServiceExt::oneshot` (no TCP
//! loopback), so the assertions are deterministic and free of task-scheduling
//! timing. Under `no-auto-start` the default VMs stay in `Ready` and the
//! self-test additionally drives one VM through a full
//! start/409/stop/stopped/404 lifecycle. `http-dynamic-test` then drives the
//! same VM through remove -> create -> ready -> 409 -> remove -> 404.

use axum::{Router, routing::get, routing::post};

use crate::http::vm;

/// Assemble the management routes.
pub fn router() -> Router {
    Router::new()
        .route("/api/vms", get(vm::list_vms))
        .route("/api/vms/{id}", get(vm::vm_detail).delete(vm::vm_delete))
        .route("/api/vms/create", post(vm::vm_create))
        .route("/api/vms/{id}/start", post(vm::vm_start))
        .route("/api/vms/{id}/stop", post(vm::vm_stop))
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
        #[cfg(feature = "http-dynamic-test")]
        dynamic_test::self_test_dynamic().await;

        let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
            .await
            .expect("failed to bind management HTTP server");
        info!("management HTTP server (axum) listening on 0.0.0.0:8080");
        axum::serve(listener, router()).await.expect("server error");
    });
}

/// `http-test` built-in self-test: drive the router with
/// `tower::ServiceExt::oneshot` (no TCP loopback) and assert the read-only
/// endpoints: `GET /api/vms -> 200` and `GET /api/vms/999 -> 404` (no specific
/// VM id is bound). The per-request status lines are diagnostics only; the QEMU
/// regex matcher stops at the FIRST marker it sees, so two independent success
/// lines could not assert both endpoints. The test therefore prints a single
/// `readonly PASSED`/`readonly FAILED` sentinel that reflects both assertions.
#[cfg(feature = "http-test")]
async fn self_test() {
    let router = router();

    let list = send_status(&router, "GET", "/api/vms").await;
    info!("HTTP self-test: GET /api/vms -> {}", list);

    let detail = send_status(&router, "GET", "/api/vms/999").await;
    info!("HTTP self-test: GET /api/vms/999 -> {}", detail);

    let passed = list == axum::http::StatusCode::OK && detail == axum::http::StatusCode::NOT_FOUND;
    if passed {
        info!("HTTP self-test: readonly PASSED");
    } else {
        error!("HTTP self-test: readonly FAILED");
    }

    // With `no-auto-start` the default VMs are created but left in `Ready`, so
    // the control API can be exercised over a full start/stop cycle.
    #[cfg(feature = "no-auto-start")]
    if let Some(id) = lifecycle_test::first_vm_id() {
        lifecycle_test::self_test_lifecycle(router, id).await;
    }
}

/// Send a single request to the router and return its status code.
#[cfg(feature = "http-test")]
async fn send_status(router: &Router, method: &str, uri: &str) -> axum::http::StatusCode {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("request failed")
        .status()
}

/// The control-lifecycle self-test, only built when both `http-test` and
/// `no-auto-start` are enabled (the default VMs stay in `Ready`).
///
/// All polling here parks the task on a timer alarm (`std::thread::sleep` →
/// ArceOS `run_queue::sleep_until`), which reschedules and lets other Core-0
/// tasks (shell, VM manager) run while waiting — it does not busy-wait.
#[cfg(all(feature = "http-test", feature = "no-auto-start"))]
mod lifecycle_test {
    use ax_std::time::{Duration, Instant};
    use axum::Router;

    use super::send_status;

    /// The id of the first registered VM, if any.
    pub(super) fn first_vm_id() -> Option<usize> {
        crate::manager::AxvmManager::vm_list()
            .first()
            .map(|vm| vm.id())
    }

    /// Drive one VM through `start -> 409 -> stop -> stopped` and the 404 path,
    /// verifying each expected outcome internally and printing a single
    /// deterministic PASSED/FAILED sentinel for the QEMU regex matcher.
    ///
    /// `stop` is a request: the `Stopped` state only arrives once the vCPU
    /// (running on another CPU) observes the request and exits, so the self-test
    /// polls with explicit sleeps instead of blocking. `start` flips the VM status
    /// to `Running` synchronously while the vCPU task is still being queued on its
    /// target CPU, so before issuing a stop the self-test must wait until the vCPU
    /// task has actually entered the guest (`running_vcpu_count`); otherwise a stop
    /// issued in that window would strand the vCPU task waiting forever for a
    /// `Running` state it already missed.
    ///
    /// Restarting a stopped VM spawns a fresh vCPU task that the scheduler never
    /// runs on its pinned CPU once that CPU has idled (no IPI wake source in the
    /// current build). The API contract rejects `start` on a `Stopped` VM with
    /// 409 (see `vm_action`) instead of letting it hang in `Running`, and the
    /// self-test asserts that rejection below.
    pub(super) async fn self_test_lifecycle(router: Router, id: usize) {
        let mut passed = true;

        let start = send_status(&router, "POST", &format!("/api/vms/{id}/start")).await;
        info!("HTTP self-test: POST /api/vms/{id}/start -> {}", start);
        passed &= start == axum::http::StatusCode::OK;
        passed &= poll_vcpu_running(id);

        // A `Ready`/`Stopped`/`Running`-incompatible transition is a 409.
        let invalid = send_status(&router, "POST", &format!("/api/vms/{id}/start")).await;
        info!("HTTP self-test: POST start on running VM -> {}", invalid);
        passed &= invalid == axum::http::StatusCode::CONFLICT;

        let stop = send_status(&router, "POST", &format!("/api/vms/{id}/stop")).await;
        info!("HTTP self-test: POST /api/vms/{id}/stop -> {}", stop);
        passed &= stop == axum::http::StatusCode::OK;
        passed &= poll_status(id, "stopped");

        // Restart-after-stop is unsupported (scheduler limitation); the contract
        // rejects it with 409 rather than hanging the VM in `Running`.
        let restart = send_status(&router, "POST", &format!("/api/vms/{id}/start")).await;
        info!(
            "HTTP self-test: POST /api/vms/{id}/start on stopped VM -> {}",
            restart
        );
        passed &= restart == axum::http::StatusCode::CONFLICT;

        let bad = send_status(&router, "POST", "/api/vms/999/start").await;
        info!("HTTP self-test: POST /api/vms/999/start -> {}", bad);
        passed &= bad == axum::http::StatusCode::NOT_FOUND;

        if passed {
            info!("HTTP self-test: control lifecycle PASSED");
        } else {
            error!("HTTP self-test: control lifecycle FAILED");
        }
    }

    /// Wait until a vCPU of the VM has actually entered the guest run loop.
    ///
    /// `start_vm()` returns as soon as the VM status is `Running`; the vCPU task is
    /// spawned on the calling CPU and migrated to its pinned CPU asynchronously. A
    /// stop issued before that migration completes is observed by a vCPU task still
    /// waiting for the `Running` state and never becomes effective. Polling
    /// `running_vcpu_count` closes that window deterministically. Returns whether
    /// the vCPU entered within the poll bound, for the self-test's pass/fail
    /// accounting.
    pub(super) fn poll_vcpu_running(id: usize) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let entered = crate::manager::AxvmManager::vm_by_id(id)
                .map(|vm| vm.running_vcpu_count() > 0)
                .unwrap_or(false);
            if entered {
                info!("HTTP self-test: VM[{id}] vCPU entered guest");
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        warn!("HTTP self-test: VM[{id}] vCPU did not enter guest within the poll bound");
        false
    }

    /// Poll the VM status until it reports `want`, sleeping between checks so other
    /// primary-CPU tasks are not starved. A wall-clock deadline is used rather than
    /// an iteration count because the guest boot + stop completion latency is
    /// timing-dependent (~100 ms in QEMU). Returns whether the status was reached,
    /// for the self-test's internal pass/fail accounting.
    pub(super) fn poll_status(id: usize, want: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let status = crate::manager::AxvmManager::vm_by_id(id)
                .map(|vm| vm.status().as_str().to_owned())
                .unwrap_or_default();
            if status == want {
                info!("HTTP self-test: VM[{id}] reached status '{want}'");
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        warn!("HTTP self-test: VM[{id}] did not reach '{want}' within the poll bound");
        false
    }
}

/// The create/delete self-test, only built under `http-dynamic-test`.
///
/// Runs after the base control self-test, so the default VM has already been
/// started and stopped by [`super::lifecycle_test::self_test_lifecycle`] and
/// sits in `Stopped`. The test removes that VM, recreates it from its own
/// build-time config (the create body reuses `static_vm_configs().first()`,
/// whose id owns an embedded guest image), checks it is `Ready`, verifies a
/// duplicate create is rejected with 409, then removes it again and confirms a
/// 404.
#[cfg(feature = "http-dynamic-test")]
mod dynamic_test {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;
    use tower::ServiceExt;

    use super::{lifecycle_test, send_status};

    /// Drive one VM through remove -> create -> ready -> 409 -> remove -> 404,
    /// printing a single deterministic PASSED/FAILED sentinel.
    pub(super) async fn self_test_dynamic() {
        let router = super::router();
        let mut passed = true;

        // The create body is the VM's own build-time config: its id owns an
        // embedded guest image, which is the only way the runtime boot-image
        // resolver (`memory_images_for_vm`) can satisfy the load.
        let toml = crate::config::vmcfg::static_vm_configs()
            .first()
            .copied()
            .expect("dynamic self-test requires a static VM config");
        let id = lifecycle_test::first_vm_id().expect("dynamic self-test requires a default VM");

        // 1. Remove the default VM (registered at boot, left `Stopped` by the
        //    base control self-test). destroy() + remove_vm() are synchronous.
        let removed = send_status(&router, "DELETE", &format!("/api/vms/{id}")).await;
        info!("HTTP self-test: DELETE /api/vms/{id} -> {removed}");
        passed &= removed == axum::http::StatusCode::NO_CONTENT;

        // 2. Recreate it from the same TOML.
        let created = send_create(&router, toml).await;
        info!("HTTP self-test: POST /api/vms/create -> {created}");
        passed &= created == axum::http::StatusCode::OK;

        // 3. The recreated VM is registered and `Ready`.
        passed &= lifecycle_test::poll_status(id, "ready");

        // 4. A duplicate id is a contract error (409), not an opaque 500.
        let dup = send_create(&router, toml).await;
        info!("HTTP self-test: POST /api/vms/create duplicate -> {dup}");
        passed &= dup == axum::http::StatusCode::CONFLICT;

        // 5. Remove it again, then confirm it is gone.
        let removed = send_status(&router, "DELETE", &format!("/api/vms/{id}")).await;
        info!("HTTP self-test: DELETE /api/vms/{id} -> {removed}");
        passed &= removed == axum::http::StatusCode::NO_CONTENT;

        let gone = send_status(&router, "GET", &format!("/api/vms/{id}")).await;
        info!("HTTP self-test: GET /api/vms/{id} -> {gone}");
        passed &= gone == axum::http::StatusCode::NOT_FOUND;

        if passed {
            info!("HTTP self-test: dynamic create/delete PASSED");
        } else {
            error!("HTTP self-test: dynamic create/delete FAILED");
        }
    }

    /// POST a create request with the given TOML body.
    async fn send_create(router: &axum::Router, toml: &str) -> axum::http::StatusCode {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/vms/create")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "toml": toml }).to_string()))
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed")
            .status()
    }
}
