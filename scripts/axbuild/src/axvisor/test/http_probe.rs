//! Typed host-side probe for the AxVisor management HTTP control plane.
//!
//! Direction is host -> guest: the probe acts as a *client* that dials the
//! axum management API running *inside* the AxVisor guest through QEMU
//! user-mode networking hostfwd, and asserts the responses entirely host-side.
//! All assertions live here as typed `reqwest` requests with `serde_json`
//! parsing — there is no guest-side test relay endpoint and no shell script.
//! The runner's [`HostHttpProbeGuard`](super::host_probe::HostHttpProbeGuard)
//! only orchestrates: wait for the forwarded port, invoke this probe, store its
//! verdict, and quit QEMU over QMP.
//!
//! The probe drives the single converged `http-control-plane` test case
//! (`test-suit/axvisor/normal/qemu-http-control-plane/`). Its default VM is
//! `http-control-plane/vm-memory.toml`, a build-time embedded (`memory`)
//! guest image registered with `base.id = 1` and kept `Ready` by the
//! `no-auto-start` feature. The probe exercises the whole lifecycle — including
//! the destroy-then-recreate resource re-acquire regression — in one boot,
//! mirroring `os/axvisor/doc/http-control-plane-quickstart.md`:
//!
//! ```text
//! GET    /api/vms            -> 200            (list; id=1 present)
//! GET    /api/vms/1          -> 200 ready      (detail; id/name/cpu_num/vcpu_states)
//! GET    /api/vms/not-an-id  -> 404            (non-numeric id)
//! GET    /api/vms/999        -> 404            (unknown VM)
//! POST   /api/vms/create     -> 401            (no token)
//! POST   /api/vms/1/start    -> 401            (no token)
//! POST   /api/vms/1/stop     -> 401            (no token)
//! DELETE /api/vms/1          -> 401            (no token)
//! POST   /api/vms/create {}  -> 400            (missing toml)
//! POST   /api/vms/create <bad toml> -> 400     (invalid TOML)
//! POST   /api/vms/999/start  -> 404            (auth'd unknown VM)
//! POST   /api/vms/999/stop   -> 404            (auth'd unknown VM)
//! DELETE /api/vms/999        -> 404            (auth'd unknown VM)
//! POST   /api/vms/create     -> 409            (id=1 already registered)
//! POST   /api/vms/1/start    -> 200 -> running (async=false)
//! POST   /api/vms/1/start    -> 409            (already running)
//! POST   /api/vms/1/stop     -> 200 -> stopped (async=true)
//! POST   /api/vms/1/start    -> 409            (restart-after-stop)
//! DELETE /api/vms/1          -> 204 -> 404     (gone)
//! POST   /api/vms/create     -> 200 {id:1}     (recreate after delete)
//! POST   /api/vms/create     -> 409            (id=1 re-registered)
//! POST   /api/vms/1/start    -> 200 -> running (recreated VM usable)
//! POST   /api/vms/1/stop     -> 200 -> stopped
//! DELETE /api/vms/1          -> 204 -> 404     (cleanup)
//! ```
//!
//! The last recreate -> start -> stop -> delete block is the resource
//! re-acquire regression: it proves destroy freed guest memory, vCPUs, devices,
//! and the registry entry so a fresh VM can be rebuilt from the same embedded
//! image. `vm-memory.toml` is matched by `base.id` against the build-time
//! embedded images, so the create body carries that file verbatim (the
//! `kernel_path` / `ramdisk_path` `${workspace}` placeholders are unused at
//! runtime for memory images).

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use reqwest::Method;
use serde_json::{Value, json};

use super::types::AxvisorHttpProbeConfig;

/// Poll deadline for VM state transitions (boot, stop, delete), mirroring the
/// case `timeout` headroom requirement: must stay well below the QEMU case
/// timeout (600s) so a stuck transition fails on the probe, not on the timeout.
const POLL_DEADLINE: Duration = Duration::from_secs(120);
/// Extra retry window for the very first request: the guard's TCP port wait
/// proves the guest is listening, but the axum router may still be wiring up.
const INITIAL_READY_DEADLINE: Duration = Duration::from_secs(30);
/// Poll interval for state transitions.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Run the full management-control-plane contract against one boot.
///
/// `addr` is the forwarded host address (`127.0.0.1:<port>`). `config` carries
/// the bearer token and timeouts; `case_dir` locates the `vm-memory.toml`
/// create body. `stop` is the shared abort flag: the poll loops check it so a
/// run whose QEMU already failed aborts on the next poll instead of waiting
/// out the deadline.
pub(crate) fn run(
    addr: &str,
    config: &AxvisorHttpProbeConfig,
    case_dir: &Path,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    // axbuild already runs on tokio; a nested current-thread runtime in the
    // guard's std thread keeps this probe sequential and reuses the existing
    // async reqwest client (no `blocking` feature needed).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build HTTP probe tokio runtime")?;
    runtime.block_on(run_async(addr, config, case_dir, stop))
}

async fn run_async(
    addr: &str,
    config: &AxvisorHttpProbeConfig,
    case_dir: &Path,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let base = format!("http://{addr}");
    let token = config.token.as_deref();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .build()
        .context("failed to build HTTP probe client")?;

    let vm_config =
        std::fs::read_to_string(case_dir.join("vm-memory.toml")).with_context(|| {
            format!(
                "failed to read VM config fixture {}",
                case_dir.join("vm-memory.toml").display()
            )
        })?;
    let create_body = json!({ "toml": vm_config }).to_string();

    // 1. Readiness: the guard already waited for the TCP port; retry the first
    //    request briefly in case the axum router is still binding.
    wait_for_status(
        &client,
        &format!("{base}/api/vms"),
        200,
        INITIAL_READY_DEADLINE,
        &stop,
    )
    .await
    .with_context(|| "guest management HTTP server never became reachable")?;
    println!("  http probe: guest management server reachable");

    // 2. List: the default VM (id 1) is registered and `Ready`.
    let (code, body) =
        request(&client, Method::GET, &format!("{base}/api/vms"), None, None).await?;
    assert_status("GET /api/vms", code, 200)?;
    ensure!(
        list_has_vm(&body, 1),
        "GET /api/vms did not list the default VM id=1"
    );

    // 3. Detail of the default VM: identity, shape, and ready status.
    let (code, body) = request(
        &client,
        Method::GET,
        &format!("{base}/api/vms/1"),
        None,
        None,
    )
    .await?;
    assert_status("GET /api/vms/1", code, 200)?;
    assert_vm_status("GET /api/vms/1", &body, "ready")?;
    let detail = body
        .as_ref()
        .with_context(|| "GET /api/vms/1 had no JSON body")?;
    ensure!(
        detail.get("id").and_then(Value::as_u64) == Some(1),
        "GET /api/vms/1 did not report id=1"
    );
    ensure!(
        detail.get("name").and_then(Value::as_str) == Some("linux-http-control-plane"),
        "GET /api/vms/1 did not report the fixture name"
    );
    ensure!(
        detail.get("cpu_num").and_then(Value::as_u64) == Some(1),
        "GET /api/vms/1 did not report cpu_num=1"
    );
    let vcpus = detail
        .get("vcpu_states")
        .and_then(Value::as_array)
        .with_context(|| "GET /api/vms/1 had no vcpu_states array")?;
    ensure!(
        !vcpus.is_empty(),
        "GET /api/vms/1 reported an empty vcpu_states array"
    );

    // 4-5. Error path: non-numeric and unknown ids are 404.
    let (code, _) = request(
        &client,
        Method::GET,
        &format!("{base}/api/vms/not-an-id"),
        None,
        None,
    )
    .await?;
    assert_status("GET /api/vms/not-an-id", code, 404)?;
    let (code, _) = request(
        &client,
        Method::GET,
        &format!("{base}/api/vms/999"),
        None,
        None,
    )
    .await?;
    assert_status("GET /api/vms/999", code, 404)?;

    // 6-9. Auth: every mutating route rejects an unauthenticated write with
    //      401, before any VM lookup or body parse.
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/create"),
        None,
        None,
    )
    .await?;
    assert_status("POST /api/vms/create (no auth)", code, 401)?;
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/1/start"),
        None,
        None,
    )
    .await?;
    assert_status("POST /api/vms/1/start (no auth)", code, 401)?;
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/1/stop"),
        None,
        None,
    )
    .await?;
    assert_status("POST /api/vms/1/stop (no auth)", code, 401)?;
    let (code, _) = request(
        &client,
        Method::DELETE,
        &format!("{base}/api/vms/1"),
        None,
        None,
    )
    .await?;
    assert_status("DELETE /api/vms/1 (no auth)", code, 401)?;

    // 10-11. Create validates its body: a missing `toml` and an invalid TOML
    //        document both reject with 400.
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/create"),
        token,
        Some("{}"),
    )
    .await?;
    assert_status("POST /api/vms/create (missing toml)", code, 400)?;
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/create"),
        token,
        Some(&json!({ "toml": "this is not [[ valid toml {{{" }).to_string()),
    )
    .await?;
    assert_status("POST /api/vms/create (invalid toml)", code, 400)?;

    // 12-14. Authenticated writes to an unknown VM are 404.
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/999/start"),
        token,
        None,
    )
    .await?;
    assert_status("POST /api/vms/999/start (auth'd)", code, 404)?;
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/999/stop"),
        token,
        None,
    )
    .await?;
    assert_status("POST /api/vms/999/stop (auth'd)", code, 404)?;
    let (code, _) = request(
        &client,
        Method::DELETE,
        &format!("{base}/api/vms/999"),
        token,
        None,
    )
    .await?;
    assert_status("DELETE /api/vms/999 (auth'd)", code, 404)?;

    // 15. Duplicate create while id=1 is registered conflicts.
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/create"),
        token,
        Some(&create_body),
    )
    .await?;
    assert_status("POST /api/vms/create (duplicate id=1)", code, 409)?;

    // 16. Start the default VM: accepted synchronously (`async=false`), then
    //     poll the detail into `running`.
    let (code, body) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/1/start"),
        token,
        None,
    )
    .await?;
    assert_status("POST /api/vms/1/start", code, 200)?;
    assert_action("POST /api/vms/1/start", &body, true, false)?;
    poll_vm_status(&client, &base, 1, "running", &stop).await?;

    // 17. Re-starting an already-running VM conflicts.
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/1/start"),
        token,
        None,
    )
    .await?;
    assert_status("POST /api/vms/1/start (already running)", code, 409)?;

    // 18. Stop is a request (`async=true`): the `stopped` state arrives
    //     asynchronously once the vCPU observes it and exits.
    let (code, body) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/1/stop"),
        token,
        None,
    )
    .await?;
    assert_status("POST /api/vms/1/stop", code, 200)?;
    assert_action("POST /api/vms/1/stop", &body, true, true)?;
    poll_vm_status(&client, &base, 1, "stopped", &stop).await?;

    // 19. Restart-after-stop is a known scheduling limitation; the contract
    //     rejects it with 409 rather than hanging the VM in `running`.
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/1/start"),
        token,
        None,
    )
    .await?;
    assert_status("POST /api/vms/1/start (restart-after-stop)", code, 409)?;

    // 20. Delete the stopped VM, then poll until it is gone.
    let (code, _) = request(
        &client,
        Method::DELETE,
        &format!("{base}/api/vms/1"),
        token,
        None,
    )
    .await?;
    assert_status("DELETE /api/vms/1", code, 204)?;
    poll_vm_gone(&client, &base, 1, &stop).await?;

    // 21. Recreate after delete: the embedded image is matched by id, so a
    //     fresh create with the same config succeeds and registers id 1 again.
    let (code, body) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/create"),
        token,
        Some(&create_body),
    )
    .await?;
    assert_status("POST /api/vms/create (recreate)", code, 200)?;
    ensure!(
        body.and_then(|v| v.get("id").and_then(Value::as_u64)) == Some(1),
        "recreate did not return id=1"
    );
    poll_vm_status(&client, &base, 1, "ready", &stop).await?;

    // 22. The re-registered id conflicts with a second create.
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/create"),
        token,
        Some(&create_body),
    )
    .await?;
    assert_status("POST /api/vms/create (recreate duplicate)", code, 409)?;

    // 23-25. The recreated VM must be fully usable, not merely re-registered:
    //        destroy must have freed guest memory, vCPUs, devices, and the
    //        registry entry so a fresh VM can be rebuilt and run from the same
    //        embedded image. This is the resource re-acquire regression.
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/1/start"),
        token,
        None,
    )
    .await?;
    assert_status("POST /api/vms/1/start (recreated)", code, 200)?;
    poll_vm_status(&client, &base, 1, "running", &stop).await?;
    let (code, _) = request(
        &client,
        Method::POST,
        &format!("{base}/api/vms/1/stop"),
        token,
        None,
    )
    .await?;
    assert_status("POST /api/vms/1/stop (recreated)", code, 200)?;
    poll_vm_status(&client, &base, 1, "stopped", &stop).await?;

    // 26. Cleanup: leave the hypervisor without a registered VM.
    let (code, _) = request(
        &client,
        Method::DELETE,
        &format!("{base}/api/vms/1"),
        token,
        None,
    )
    .await?;
    assert_status("DELETE /api/vms/1 (cleanup)", code, 204)?;
    poll_vm_gone(&client, &base, 1, &stop).await?;

    println!("  http probe: full control-plane contract passed");
    Ok(())
}

/// One HTTP request; returns the status code and JSON body (if any). A
/// transport error (connection refused/reset while the guest server is still
/// coming up) is converted to a `reqwest::Error` for the caller to retry.
async fn request(
    client: &reqwest::Client,
    method: Method,
    url: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> anyhow::Result<(reqwest::StatusCode, Option<Value>)> {
    let mut builder = client.request(method, url);
    if let Some(token) = token {
        builder = builder.bearer_auth(token);
    }
    if let Some(body) = body {
        builder = builder
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
    }
    let response = builder
        .send()
        .await
        .with_context(|| format!("HTTP request {url} failed"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("HTTP response body from {url} failed to read"))?;
    let json = if bytes.is_empty() {
        None
    } else {
        serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "HTTP response from {url} was not valid JSON: {:?}",
                &bytes[..bytes.len().min(256)]
            )
        })?
    };
    Ok((status, json))
}

/// Assert that `actual` equals `expected`, printing a progress line.
fn assert_status(label: &str, actual: reqwest::StatusCode, expected: u16) -> anyhow::Result<()> {
    let actual_u16 = actual.as_u16();
    println!("  http probe: {label} -> {actual_u16} (expect {expected})");
    ensure!(
        actual_u16 == expected,
        "{label} returned {actual_u16}, expected {expected}"
    );
    Ok(())
}

/// Whether a `GET /api/vms` body lists a VM with the given id.
fn list_has_vm(body: &Option<Value>, id: u64) -> bool {
    body.as_ref()
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("id").and_then(Value::as_u64) == Some(id))
        })
}

/// Extract the top-level `status` string of a VM detail body.
fn vm_status(body: &Option<Value>) -> anyhow::Result<&str> {
    let value = body
        .as_ref()
        .with_context(|| "VM detail response had no JSON body")?;
    value
        .get("status")
        .and_then(Value::as_str)
        .with_context(|| format!("VM detail response had no status string: {value}"))
}

/// Assert the VM detail body reports a specific status.
fn assert_vm_status(label: &str, body: &Option<Value>, expected: &str) -> anyhow::Result<()> {
    let status = vm_status(body)?;
    println!("  http probe: {label} -> status {status} (expect {expected})");
    ensure!(
        status == expected,
        "{label} reported status {status}, expected {expected}"
    );
    Ok(())
}

/// Assert a lifecycle action response reports the expected `ok` and `async`
/// markers. `async` distinguishes a synchronous start from a request-style
/// stop whose `Stopped` state arrives later.
fn assert_action(
    label: &str,
    body: &Option<Value>,
    ok_expected: bool,
    async_expected: bool,
) -> anyhow::Result<()> {
    let value = body
        .as_ref()
        .with_context(|| format!("{label} had no JSON body"))?;
    let ok = value.get("ok").and_then(Value::as_bool);
    let is_async = value.get("async").and_then(Value::as_bool);
    println!(
        "  http probe: {label} -> ok={ok:?} async={is_async:?} (expect ok={ok_expected} \
         async={async_expected})"
    );
    ensure!(
        ok == Some(ok_expected),
        "{label} reported ok={ok:?}, expected {ok_expected}"
    );
    ensure!(
        is_async == Some(async_expected),
        "{label} reported async={is_async:?}, expected {async_expected}"
    );
    Ok(())
}

/// Poll `GET /api/vms/{id}` until its status equals `expected`, the deadline
/// elapses, or a stop is requested. Retries transport errors (the server may be
/// mid-transition).
async fn poll_vm_status(
    client: &reqwest::Client,
    base: &str,
    id: u64,
    expected: &str,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let url = format!("{base}/api/vms/{id}");
    let started = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            anyhow::bail!("host http probe stopped");
        }
        match request(client, Method::GET, &url, None, None).await {
            Ok((code, body)) if code.as_u16() == 200 => {
                if vm_status(&body)? == expected {
                    println!("  http probe: VM[{id}] -> {expected}");
                    return Ok(());
                }
            }
            Ok((code, _)) => {
                // A non-200 during a transition is transient (e.g. the VM is
                // being torn down); keep polling until the deadline.
                eprintln!("  http probe: VM[{id}] poll saw HTTP {code}");
            }
            Err(error) => {
                eprintln!("  http probe: VM[{id}] poll transport error: {error:#}");
            }
        }
        ensure!(
            started.elapsed() < POLL_DEADLINE,
            "VM[{id}] never became {expected} within {POLL_DEADLINE:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GET /api/vms/{id}` until it returns 404 (the VM was deleted).
async fn poll_vm_gone(
    client: &reqwest::Client,
    base: &str,
    id: u64,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let url = format!("{base}/api/vms/{id}");
    let started = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            anyhow::bail!("host http probe stopped");
        }
        match request(client, Method::GET, &url, None, None).await {
            Ok((code, _)) if code.as_u16() == 404 => {
                println!("  http probe: VM[{id}] -> gone");
                return Ok(());
            }
            Ok((code, _)) => {
                eprintln!("  http probe: VM[{id}] gone-poll saw HTTP {code}");
            }
            Err(error) => {
                eprintln!("  http probe: VM[{id}] gone-poll transport error: {error:#}");
            }
        }
        ensure!(
            started.elapsed() < POLL_DEADLINE,
            "VM[{id}] never disappeared within {POLL_DEADLINE:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll one unauthenticated `GET url` until it returns `expected`, the deadline
/// elapses, or a stop is requested. Used only for the initial readiness check,
/// whose request needs no token or body.
async fn wait_for_status(
    client: &reqwest::Client,
    url: &str,
    expected: u16,
    deadline: Duration,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            anyhow::bail!("host http probe stopped");
        }
        if let Ok((code, _)) = request(client, Method::GET, url, None, None).await
            && code.as_u16() == expected
        {
            return Ok(());
        }
        ensure!(
            started.elapsed() < deadline,
            "request {url} never returned HTTP {expected} within {deadline:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_body_round_trips_the_fixture_toml() {
        let toml = "name = \"linux\"\npath = \"a\\b\"";
        let body = json!({ "toml": toml }).to_string();
        // A raw newline would break JSON; serde_json must escape it as `\n`.
        assert!(!body.contains('\n'));
        // Round-trips back to the original TOML.
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["toml"], Value::String(toml.to_string()));
    }

    #[test]
    fn assert_status_reports_mismatch() {
        assert!(assert_status("x", reqwest::StatusCode::OK, 200).is_ok());
        assert!(assert_status("x", reqwest::StatusCode::NOT_FOUND, 200).is_err());
    }

    #[test]
    fn vm_status_parses_the_detail_body() {
        let body = Some(json!({ "id": 1, "status": "running" }));
        assert_eq!(vm_status(&body).unwrap(), "running");
        assert!(vm_status(&None).is_err());
        assert!(vm_status(&Some(json!({ "id": 1 }))).is_err());
    }

    #[test]
    fn list_has_vm_matches_by_id() {
        let body = Some(json!([{ "id": 2 }, { "id": 1 }]));
        assert!(list_has_vm(&body, 1));
        assert!(!list_has_vm(&body, 3));
        assert!(!list_has_vm(&None, 1));
        assert!(!list_has_vm(&Some(json!({ "id": 1 })), 1)); // object, not array
    }

    #[test]
    fn assert_action_checks_ok_and_async() {
        let start = Some(json!({ "ok": true, "status": "running", "async": false }));
        assert!(assert_action("start", &start, true, false).is_ok());
        assert!(assert_action("start", &start, true, true).is_err());
        assert!(assert_action("start", &start, false, false).is_err());
        assert!(
            assert_action(
                "stop",
                &Some(json!({ "ok": true, "async": true })),
                true,
                true
            )
            .is_ok()
        );
        assert!(assert_action("start", &None, true, false).is_err());
    }
}
