//! Host-side TCP probe for QEMU hostfwd integration tests.
//!
//! The probe is the reverse of [`super::host_http`]: instead of serving host
//! fixtures to the guest, it acts as a *client* that dials a management API
//! running *inside* the guest through QEMU user-mode networking
//! (`-netdev user,hostfwd=tcp::<host_port>-:<guest_port>`). It makes real HTTP
//! requests and asserts the responses entirely host-side — there is no
//! guest-side relay endpoint, so nothing in the hypervisor knows a test is
//! running.
//!
//! When the probe finishes — pass or fail — it quits QEMU over the QMP monitor
//! socket the runner added (`-qmp unix:...,server=on,wait=off`), so the QEMU
//! process exits cleanly and the runner reads the stored verdict from the guard
//! as the test result. The case `timeout` remains the backstop if the probe or
//! its QMP quit fails.
//!
//! Scenarios (see [`HostHttpProbeScenario`]):
//! - `ReadOnly`: the security-boundary + read contract. An unauthenticated
//!   write (`POST /api/vms/999/start` with no `Authorization` header) must be
//!   rejected with 401; then the authenticated contract (`GET /api/vms -> 200`,
//!   `GET /api/vms/999 -> 404`, authenticated write to an unknown VM -> 404).
//! - `Lifecycle { vm_id }`: drive one registered VM through
//!   start -> running -> stop -> stopped via the mutating routes.
//! - `Dynamic { config_toml }`: create a VM from a host-side TOML config, poll
//!   it `Ready`, verify a duplicate create conflicts (409), then delete it and
//!   poll until it is gone.
//!
//! All authenticated requests carry the `token` from the case config, which must
//! match the guest build's `[env] AXVM_HTTP_TOKEN`.

use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, bail, ensure};
use serde_json::Value;

use crate::test::case::{HostHttpProbeConfig, HostHttpProbeScenario};

/// Per-attempt IO timeout for a single HTTP request/response exchange. The
/// dynamic `create` handler reads the guest kernel from the rootfs inside the
/// request (fs loading), which is the slowest single exchange; 30s covers it
/// without making a stuck server hold the probe for too long.
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// Sleep between readiness retries.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// How long to keep retrying the QMP connect before giving up on quitting QEMU.
const QMP_CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const QMP_CONNECT_RETRIES: usize = 10;
/// How long to wait after QMP `quit` for QEMU to exit on its own before
/// force-killing it. QEMU can ignore `quit` when its main loop is stuck in a
/// busy poll (observed under the axbuild runner), so the probe must not wait
/// forever for a clean exit.
#[cfg(target_os = "linux")]
const QMP_QUIT_GRACE: Duration = Duration::from_secs(3);
/// Interval for polling whether QEMU is still alive after `quit`.
#[cfg(target_os = "linux")]
const QMP_ALIVE_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct HostHttpProbeGuard {
    stop: Arc<AtomicBool>,
    result: Arc<Mutex<Option<anyhow::Result<()>>>>,
    /// Set when the probe had to SIGKILL QEMU because it ignored QMP `quit`.
    /// The runner uses this to prefer the stored probe verdict over QEMU's
    /// non-zero exit status.
    killed_by_probe: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HostHttpProbeGuard {
    /// Spawn the probe thread and return a guard that owns its lifecycle.
    ///
    /// `qmp_socket` is the path QEMU binds from its `-qmp unix:...` argument;
    /// the probe connects to it after its assertions finish to quit QEMU. When
    /// `None`, the probe only stores its verdict and relies on the case timeout
    /// to end the run.
    pub(crate) fn start(
        config: &HostHttpProbeConfig,
        host_port: u16,
        case_name: &str,
        qmp_socket: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let addr = format!("127.0.0.1:{host_port}");
        let connect_timeout = Duration::from_secs(config.connect_timeout_secs);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let result = Arc::new(Mutex::new(None));
        let thread_result = result.clone();
        let killed_by_probe = Arc::new(AtomicBool::new(false));
        let thread_killed = killed_by_probe.clone();
        let case_name = case_name.to_string();
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread_addr = addr.clone();
        let thread_case_name = case_name.clone();
        let token = config.token.clone();
        let scenario = config.scenario.clone();
        let thread = thread::spawn(move || {
            let _ = ready_tx.send(());
            let verdict = run_probe(
                &thread_addr,
                &thread_case_name,
                connect_timeout,
                token.as_deref(),
                &scenario,
                &thread_stop,
            );
            *thread_result.lock().unwrap() = Some(verdict);
            // Quit QEMU so the run ends on the probe verdict instead of the
            // serial-timeout path. `request_qmp_quit` force-kills QEMU when it
            // ignores `quit` (a hang observed under the runner); record that so
            // the runner trusts the stored probe verdict over QEMU's non-zero
            // exit status.
            if let Some(socket) = qmp_socket {
                match request_qmp_quit(&socket) {
                    Ok(true) => thread_killed.store(true, Ordering::SeqCst),
                    Ok(false) => {}
                    Err(err) => eprintln!(
                        "  host http probe: {thread_case_name}: failed to quit QEMU via QMP: {err:#}"
                    ),
                }
            }
        });

        if ready_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            stop.store(true, Ordering::Release);
            bail!("host http probe for `{case_name}` did not become ready");
        }

        println!("  host http probe: {addr} -> guest:{}", config.guest_port);
        Ok(Self {
            stop,
            result,
            killed_by_probe,
            thread: Some(thread),
        })
    }

    /// Take the probe's stored verdict, if the thread produced one.
    ///
    /// Called once, after QEMU has exited. The probe always stores a verdict
    /// *before* it quits QEMU, so a clean QEMU exit implies a verdict exists.
    pub(crate) fn take_result(&self) -> Option<anyhow::Result<()>> {
        self.result.lock().unwrap().take()
    }

    /// Whether the probe had to SIGKILL QEMU (it ignored QMP `quit`). When
    /// true, the runner prefers the stored probe verdict over QEMU's exit code.
    pub(crate) fn killed_by_probe(&self) -> bool {
        self.killed_by_probe.load(Ordering::SeqCst)
    }
}

impl Drop for HostHttpProbeGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Dispatch the probe to the scenario-specific assertion flow and report the
/// verdict. The result is also stored in the guard and drives the test result.
fn run_probe(
    addr: &str,
    case_name: &str,
    connect_timeout: Duration,
    token: Option<&str>,
    scenario: &HostHttpProbeScenario,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let result = match scenario {
        HostHttpProbeScenario::ReadOnly => {
            run_readonly_probe(addr, case_name, connect_timeout, token, stop)
        }
        HostHttpProbeScenario::Lifecycle { vm_id } => {
            run_lifecycle_probe(addr, case_name, connect_timeout, token, *vm_id, stop)
        }
        HostHttpProbeScenario::Dynamic { config_toml } => {
            run_dynamic_probe(addr, case_name, connect_timeout, token, config_toml, stop)
        }
    };
    match &result {
        Ok(()) => println!("  host http probe: {case_name}: probe passed"),
        Err(err) => eprintln!("  host http probe: {case_name}: probe failed: {err:#}"),
    }
    result
}

/// Assert a single request's status against the contract, with a visible log
/// line for the runner's transcript.
fn check_status(case_name: &str, label: &str, actual: u16, expected: u16) -> anyhow::Result<()> {
    println!("  host http probe: {case_name}: {label} -> {actual} (expect {expected})");
    ensure!(
        actual == expected,
        "{label} -> {actual}, expected {expected}"
    );
    Ok(())
}

/// Poll `GET /api/vms` until it yields a parsed status, the deadline elapses,
/// or a stop is requested. This doubles as the readiness probe: the route is
/// open, so no token is needed.
fn poll_status(
    addr: &str,
    path: &str,
    started: Instant,
    connect_timeout: Duration,
    stop: &AtomicBool,
) -> Option<u16> {
    loop {
        if stop.load(Ordering::Acquire) {
            return None;
        }
        if started.elapsed() >= connect_timeout {
            return None;
        }
        if let Some(status) = request_status(addr, "GET", path, None, None) {
            return Some(status);
        }
        thread::sleep(CONNECT_RETRY_INTERVAL);
    }
}

/// Poll `GET /api/vms/{id}` until its reported status equals `expected`, the
/// deadline elapses, or a stop is requested.
fn poll_vm_status(
    addr: &str,
    vm_id: u64,
    expected: &str,
    started: Instant,
    connect_timeout: Duration,
    stop: &AtomicBool,
) -> Option<()> {
    loop {
        if stop.load(Ordering::Acquire) {
            return None;
        }
        if started.elapsed() >= connect_timeout {
            return None;
        }
        if let Some(status) = vm_status(addr, vm_id, stop)
            && status == expected
        {
            return Some(());
        }
        thread::sleep(CONNECT_RETRY_INTERVAL);
    }
}

/// Poll `GET /api/vms/{id}` until it 404s (the VM is gone), the deadline
/// elapses, or a stop is requested.
fn poll_vm_gone(
    addr: &str,
    vm_id: u64,
    started: Instant,
    connect_timeout: Duration,
    stop: &AtomicBool,
) -> Option<()> {
    loop {
        if stop.load(Ordering::Acquire) {
            return None;
        }
        if started.elapsed() >= connect_timeout {
            return None;
        }
        match request_status(addr, "GET", &format!("/api/vms/{vm_id}"), None, None) {
            Some(404) => return Some(()),
            _ => thread::sleep(CONNECT_RETRY_INTERVAL),
        }
    }
}

/// Fetch the current status string of one VM, or `None` on a transport failure
/// or a non-200 response. The detail route is open (read-only), so no token is
/// needed.
fn vm_status(addr: &str, vm_id: u64, stop: &AtomicBool) -> Option<String> {
    if stop.load(Ordering::Acquire) {
        return None;
    }
    let (status, json) = request_json(addr, "GET", &format!("/api/vms/{vm_id}"), None, None)?;
    if status != 200 {
        return None;
    }
    json.get("status")?.as_str().map(String::from)
}

/// Read-only scenario: security boundary + read contract.
fn run_readonly_probe(
    addr: &str,
    case_name: &str,
    connect_timeout: Duration,
    token: Option<&str>,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let started = Instant::now();

    // Readiness: `GET /api/vms` is retried until it yields a parsed status.
    let list =
        poll_status(addr, "/api/vms", started, connect_timeout, stop).with_context(|| {
            format!("guest HTTP server never became reachable within {connect_timeout:?}")
        })?;
    check_status(case_name, "GET /api/vms", list, 200)?;

    // Access-denied regression (security review): an unauthenticated write to a
    // mutating route must be rejected with 401. The auth gate runs before any VM
    // lookup, so this holds regardless of whether VM 999 exists.
    let denied = request_status(addr, "POST", "/api/vms/999/start", None, None)
        .with_context(|| "unauthenticated write request failed")?;
    check_status(case_name, "POST /api/vms/999/start (no token)", denied, 401)?;

    // Unknown VM -> 404 on the read path.
    let missing = request_status(addr, "GET", "/api/vms/999", None, token)
        .with_context(|| "GET /api/vms/999 failed")?;
    check_status(case_name, "GET /api/vms/999", missing, 404)?;

    // Authenticated write to an unknown VM -> 404: writes are reachable with
    // valid credentials rather than silently open or always denied.
    let authed_write = request_status(addr, "POST", "/api/vms/999/start", None, token)
        .with_context(|| "authenticated write request failed")?;
    check_status(
        case_name,
        "POST /api/vms/999/start (with token)",
        authed_write,
        404,
    )?;

    Ok(())
}

/// Lifecycle scenario: drive one registered VM through the start/stop contract.
fn run_lifecycle_probe(
    addr: &str,
    case_name: &str,
    connect_timeout: Duration,
    token: Option<&str>,
    vm_id: u64,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let started = Instant::now();

    // Readiness, and confirm the target VM exists in `Ready` (a `no-auto-start`
    // build keeps default VMs un-started).
    let list =
        poll_status(addr, "/api/vms", started, connect_timeout, stop).with_context(|| {
            format!("guest HTTP server never became reachable within {connect_timeout:?}")
        })?;
    check_status(case_name, "GET /api/vms", list, 200)?;
    poll_vm_status(addr, vm_id, "ready", started, connect_timeout, stop)
        .with_context(|| format!("VM[{vm_id}] never became ready"))?;

    // Start, then poll until the vCPU task reports `running`.
    let action = Instant::now();
    let start = request_status(
        addr,
        "POST",
        &format!("/api/vms/{vm_id}/start"),
        None,
        token,
    )
    .with_context(|| format!("POST /api/vms/{vm_id}/start failed"))?;
    check_status(
        case_name,
        &format!("POST /api/vms/{vm_id}/start"),
        start,
        200,
    )?;
    poll_vm_status(addr, vm_id, "running", action, connect_timeout, stop)
        .with_context(|| format!("VM[{vm_id}] never became running after start"))?;

    // Stop is a request: the `Stopped` state arrives asynchronously once the
    // vCPU observes the request and exits.
    let action = Instant::now();
    let stop_status = request_status(addr, "POST", &format!("/api/vms/{vm_id}/stop"), None, token)
        .with_context(|| format!("POST /api/vms/{vm_id}/stop failed"))?;
    check_status(
        case_name,
        &format!("POST /api/vms/{vm_id}/stop"),
        stop_status,
        200,
    )?;
    poll_vm_status(addr, vm_id, "stopped", action, connect_timeout, stop)
        .with_context(|| format!("VM[{vm_id}] never became stopped after stop"))?;

    Ok(())
}

/// Dynamic scenario: create a VM from a host-side TOML config, verify the
/// duplicate-create conflict, then delete it and poll it gone.
fn run_dynamic_probe(
    addr: &str,
    case_name: &str,
    connect_timeout: Duration,
    token: Option<&str>,
    config_toml: &str,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let started = Instant::now();

    // Readiness.
    let list =
        poll_status(addr, "/api/vms", started, connect_timeout, stop).with_context(|| {
            format!("guest HTTP server never became reachable within {connect_timeout:?}")
        })?;
    check_status(case_name, "GET /api/vms", list, 200)?;

    // The create body is the host-side VM config TOML, sent verbatim. The
    // config must reference a guest image the runtime can load (an fs-backed
    // kernel on the rootfs, or an embedded image).
    let toml_text = fs::read_to_string(config_toml)
        .with_context(|| format!("failed to read VM config TOML `{config_toml}`"))?;
    let create_body = serde_json::json!({ "toml": toml_text }).to_string();
    let (created, created_json) =
        request_json(addr, "POST", "/api/vms/create", Some(&create_body), token)
            .with_context(|| "POST /api/vms/create failed")?;
    check_status(case_name, "POST /api/vms/create", created, 200)?;
    let created_id = created_json
        .get("id")
        .and_then(Value::as_u64)
        .with_context(|| "POST /api/vms/create response missing `id`")?;

    // Poll the new VM into `Ready`.
    let created_at = Instant::now();
    poll_vm_status(addr, created_id, "ready", created_at, connect_timeout, stop)
        .with_context(|| format!("VM[{created_id}] never became ready after create"))?;

    // Re-creating the same config must conflict (409): the id is registered.
    let dup = request_status(addr, "POST", "/api/vms/create", Some(&create_body), token)
        .with_context(|| "duplicate POST /api/vms/create failed")?;
    check_status(case_name, "POST /api/vms/create (duplicate)", dup, 409)?;

    // Delete, then poll until the VM is gone (404).
    let deleted = request_status(
        addr,
        "DELETE",
        &format!("/api/vms/{created_id}"),
        None,
        token,
    )
    .with_context(|| format!("DELETE /api/vms/{created_id} failed"))?;
    check_status(
        case_name,
        &format!("DELETE /api/vms/{created_id}"),
        deleted,
        204,
    )?;
    let gone_at = Instant::now();
    poll_vm_gone(addr, created_id, gone_at, connect_timeout, stop)
        .with_context(|| format!("VM[{created_id}] never disappeared after delete"))?;

    Ok(())
}

/// Send one HTTP/1.1 request over a fresh connection and parse the status code.
/// `body` (when present) is sent as the request body with a JSON content type.
/// `token` (when present) adds an `Authorization: Bearer <token>` header for
/// protected (mutating) routes.
fn request_status(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> Option<u16> {
    request_response(addr, method, path, body, token).map(|(status, _)| status)
}

/// Send one HTTP/1.1 request and parse the status code plus the JSON body.
/// Returns `None` on a transport failure or a non-JSON response.
fn request_json(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> Option<(u16, Value)> {
    let (status, response) = request_response(addr, method, path, body, token)?;
    let json = serde_json::from_slice(response_body(&response)?).ok()?;
    Some((status, json))
}

/// Extract the HTTP response body (everything after the header/body separator).
///
/// The response is read whole (`Connection: close`, read-to-EOF), so the headers
/// prefix must be stripped before the body can be parsed as JSON.
fn response_body(response: &[u8]) -> Option<&[u8]> {
    const SEPARATOR: &[u8] = b"\r\n\r\n";
    let pos = response
        .windows(SEPARATOR.len())
        .position(|window| window == SEPARATOR)?;
    Some(&response[pos + SEPARATOR.len()..])
}

/// Send one HTTP/1.1 request and return the status code plus the raw response
/// body.
fn request_response(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> Option<(u16, Vec<u8>)> {
    let Ok(mut stream) = TcpStream::connect(addr) else {
        return None;
    };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n");
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    request.push_str("Connection: close\r\n\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }

    if stream.write_all(request.as_bytes()).is_err() {
        return None;
    }
    let mut response = Vec::new();
    if stream.read_to_end(&mut response).is_err() {
        return None;
    }
    Some((parse_status(&response)?, response))
}

/// Extract the numeric HTTP status code from a response.
fn parse_status(response: &[u8]) -> Option<u16> {
    let head = String::from_utf8_lossy(response);
    let status_line = head.lines().next()?;
    let mut parts = status_line.split_whitespace();
    let _protocol = parts.next()?;
    let status = parts.next()?;
    status.parse().ok()
}

/// Quit QEMU by connecting to its QMP monitor socket and issuing `quit`, then
/// wait for it to exit. The socket path comes from the `-qmp
/// unix:...,server=on,wait=off` argument the runner added.
///
/// Returns `true` when QEMU had to be SIGKILL'd because it did not exit after
/// `quit` (a main-loop hang observed under the axbuild runner). In that case
/// the caller records the kill so the runner trusts the stored probe verdict
/// over QEMU's non-zero exit status.
#[cfg(unix)]
fn request_qmp_quit(socket: &Path) -> anyhow::Result<bool> {
    use std::os::unix::net::UnixStream;

    let mut stream = None;
    for _ in 0..QMP_CONNECT_RETRIES {
        match UnixStream::connect(socket) {
            Ok(stream_ok) => {
                stream = Some(stream_ok);
                break;
            }
            Err(_) => thread::sleep(QMP_CONNECT_RETRY_INTERVAL),
        }
    }
    let mut stream =
        stream.with_context(|| format!("failed to connect QMP socket {}", socket.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_millis(200)))
        .ok();
    #[cfg(target_os = "linux")]
    let peer_pid = peer_pid(&stream);
    let mut buf = [0_u8; 512];
    let _ = stream.read(&mut buf); // QMP greeting
    stream.write_all(b"{\"execute\":\"qmp_capabilities\"}\r\n")?;
    buf.fill(0);
    let _ = stream.read(&mut buf); // capabilities response
    stream.write_all(b"{\"execute\":\"quit\"}\r\n")?;
    stream.flush()?;

    // Give QEMU a short window to honor `quit` and exit on its own. When it is
    // still alive afterwards (the hang seen under the runner), SIGKILL it so
    // the run ends promptly on the probe verdict instead of the serial
    // timeout.
    #[cfg(target_os = "linux")]
    {
        let Some(pid) = peer_pid else {
            // Could not learn QEMU's PID; fall back to the case timeout.
            return Ok(false);
        };
        let deadline = Instant::now() + QMP_QUIT_GRACE;
        while Instant::now() < deadline {
            if !process_alive(pid) {
                return Ok(false); // exited cleanly
            }
            thread::sleep(QMP_ALIVE_POLL_INTERVAL);
        }
        if process_alive(pid) {
            kill_process(pid);
            return Ok(true); // force-killed: probe verdict is authoritative
        }
    }
    Ok(false)
}

#[cfg(not(unix))]
fn request_qmp_quit(_socket: &Path) -> anyhow::Result<bool> {
    bail!("QMP unix sockets are not supported on this host")
}

/// Peer PID of a connected unix socket via `SO_PEERCRED` (Linux). Returns
/// `None` when the credential lookup fails.
#[cfg(target_os = "linux")]
fn peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;

    let mut creds: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `stream` is an open socket fd owned by the caller, and `creds`
    // is a valid `ucred` buffer of the correct size.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut creds as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc == 0 && creds.pid > 0 {
        Some(creds.pid)
    } else {
        None
    }
}

/// Whether the process identified by `pid` is still alive.
#[cfg(target_os = "linux")]
fn process_alive(pid: i32) -> bool {
    // SAFETY: `kill` with signal 0 only performs an existence check and never
    // delivers a signal.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Force-kill the process identified by `pid`.
#[cfg(target_os = "linux")]
fn kill_process(pid: i32) {
    // SAFETY: `pid` came from SO_PEERCRED, i.e. it is the QEMU we connected to.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::atomic::AtomicBool,
        thread,
        time::Duration,
    };

    use super::{
        parse_status, poll_status, poll_vm_gone, poll_vm_status, request_json, request_status,
        run_dynamic_probe, run_lifecycle_probe, run_readonly_probe, vm_status,
    };
    use crate::test::case::HostHttpProbeConfig;

    /// Bearer token the fake server accepts, mirroring the guest build's
    /// `[env] AXVM_HTTP_TOKEN` for the control-plane auth gate.
    const TEST_TOKEN: &str = "test-token";
    /// Fixed VM id the fake server registers (mirrors `linux-smp1.toml`).
    const TEST_VM_ID: u64 = 1;

    /// Stateful fake of the in-guest control plane. Emulates the auth boundary
    /// (mutating routes need the bearer token) and the VM registry lifecycle
    /// (start -> running, stop -> stopped, create -> ready, delete -> gone).
    #[derive(Default)]
    struct FakeVmState {
        vms: HashMap<u64, String>,
    }

    impl FakeVmState {
        fn serve(&mut self, stream: &mut TcpStream) {
            let mut request = Vec::new();
            let mut buf = [0u8; 512];
            let headers_end = loop {
                match stream.read(&mut buf) {
                    Ok(0) => break None,
                    Ok(n) => {
                        request.extend_from_slice(&buf[..n]);
                        if request.windows(4).any(|w| w == b"\r\n\r\n") {
                            break Some(request.len());
                        }
                    }
                    Err(_) => break None,
                }
            };
            let Some(_headers_end) = headers_end else {
                return;
            };
            let head = String::from_utf8_lossy(&request);
            let first_line = head.lines().next().unwrap_or("").to_string();
            let mut parts = first_line.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("/").to_string();
            let authorized = head
                .lines()
                .any(|line| line == format!("Authorization: Bearer {TEST_TOKEN}"));
            let response = self.route(&method, &path, authorized);
            let _ = stream.write_all(response.as_bytes());
        }

        fn route(&mut self, method: &str, path: &str, authorized: bool) -> String {
            let is_write = method == "POST" || method == "DELETE";
            if is_write && !authorized {
                return status_body("401 Unauthorized", "");
            }
            match (method, path) {
                ("GET", "/api/vms") => status_body("200 OK", "[]"),
                ("GET", path) if path.starts_with(&format!("/api/vms/{TEST_VM_ID}")) => {
                    match self.vms.get(&TEST_VM_ID) {
                        Some(status) => {
                            status_body("200 OK", &format!(r#"{{"status":"{status}"}}"#))
                        }
                        None => status_body("404 Not Found", ""),
                    }
                }
                ("GET", _) => status_body("404 Not Found", ""),
                ("POST", path) if path == format!("/api/vms/{TEST_VM_ID}/start") => {
                    if self.vms.contains_key(&TEST_VM_ID) {
                        self.vms.insert(TEST_VM_ID, "running".to_string());
                        status_body("200 OK", r#"{"ok":true,"status":"running"}"#)
                    } else {
                        status_body("404 Not Found", "")
                    }
                }
                ("POST", path) if path == format!("/api/vms/{TEST_VM_ID}/stop") => {
                    if self.vms.contains_key(&TEST_VM_ID) {
                        self.vms.insert(TEST_VM_ID, "stopped".to_string());
                        status_body("200 OK", r#"{"ok":true,"status":"stopped"}"#)
                    } else {
                        status_body("404 Not Found", "")
                    }
                }
                ("POST", "/api/vms/create") => {
                    if self.vms.contains_key(&TEST_VM_ID) {
                        status_body("409 Conflict", "")
                    } else {
                        self.vms.insert(TEST_VM_ID, "ready".to_string());
                        status_body("200 OK", &format!(r#"{{"id":{TEST_VM_ID}}}"#))
                    }
                }
                ("DELETE", path) if path == format!("/api/vms/{TEST_VM_ID}") => {
                    self.vms.remove(&TEST_VM_ID);
                    status_body("204 No Content", "")
                }
                ("POST", _) => status_body("404 Not Found", ""),
                _ => status_body("404 Not Found", ""),
            }
        }
    }

    fn status_body(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn start_fake_server() -> u16 {
        start_fake_server_seeded(true)
    }

    /// Start a fake server whose VM registry is empty, so the create route
    /// succeeds instead of hitting the seeded-id conflict. Mirrors the real
    /// dynamic test build, whose `vm_configs = []` leaves id 1 free at boot.
    fn start_fake_server_empty() -> u16 {
        start_fake_server_seeded(false)
    }

    fn start_fake_server_seeded(seed: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let mut state = FakeVmState::default();
            if seed {
                state.vms.insert(TEST_VM_ID, "ready".to_string());
            }
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(_) => break,
                };
                state.serve(&mut stream);
            }
        });
        port
    }

    fn addr_for(port: u16) -> String {
        format!("127.0.0.1:{port}")
    }

    fn scenario_config(scenario: crate::test::case::HostHttpProbeScenario) -> HostHttpProbeConfig {
        HostHttpProbeConfig {
            guest_port: 8080,
            connect_timeout_secs: 5,
            token: Some(TEST_TOKEN.to_string()),
            scenario,
        }
    }

    #[test]
    fn parse_status_extracts_numeric_code() {
        assert_eq!(
            parse_status(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"),
            Some(200)
        );
        assert_eq!(
            parse_status(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"),
            Some(404)
        );
        assert_eq!(parse_status(b"garbage"), None);
        assert_eq!(parse_status(b""), None);
    }

    #[test]
    fn request_status_returns_expected_codes_from_fake_server() {
        let addr = addr_for(start_fake_server());
        assert_eq!(
            request_status(&addr, "GET", "/api/vms", None, None),
            Some(200)
        );
        assert_eq!(
            request_status(&addr, "GET", "/api/vms/999", None, None),
            Some(404)
        );
    }

    #[test]
    fn unauthenticated_write_is_denied_with_401() {
        let addr = addr_for(start_fake_server());
        assert_eq!(
            request_status(&addr, "POST", "/api/vms/999/start", None, None),
            Some(401)
        );
    }

    #[test]
    fn authenticated_write_reaches_the_route() {
        let addr = addr_for(start_fake_server());
        // With the token the gate admits the write; the unknown VM still yields
        // the contract's 404.
        assert_eq!(
            request_status(&addr, "POST", "/api/vms/999/start", None, Some(TEST_TOKEN)),
            Some(404)
        );
    }

    #[test]
    fn poll_status_returns_once_server_is_up() {
        let addr = addr_for(start_fake_server());
        let stop = AtomicBool::new(false);
        let status = poll_status(
            &addr,
            "/api/vms",
            std::time::Instant::now(),
            Duration::from_secs(5),
            &stop,
        );
        assert_eq!(status, Some(200));
    }

    #[test]
    fn poll_status_gives_up_on_stop() {
        let addr = addr_for(start_fake_server());
        let stop = AtomicBool::new(true);
        let status = poll_status(
            &addr,
            "/api/vms",
            std::time::Instant::now(),
            Duration::from_secs(10),
            &stop,
        );
        assert_eq!(status, None);
    }

    #[test]
    fn vm_status_parses_the_status_field() {
        let addr = addr_for(start_fake_server());
        let stop = AtomicBool::new(false);
        assert_eq!(
            vm_status(&addr, TEST_VM_ID, &stop).as_deref(),
            Some("ready")
        );
    }

    #[test]
    fn poll_vm_status_observes_lifecycle_transitions() {
        let addr = addr_for(start_fake_server());
        let stop = AtomicBool::new(false);
        let started = std::time::Instant::now();
        assert_eq!(
            poll_vm_status(
                &addr,
                TEST_VM_ID,
                "ready",
                started,
                Duration::from_secs(5),
                &stop
            ),
            Some(())
        );
        assert_eq!(
            request_status(
                &addr,
                "POST",
                &format!("/api/vms/{TEST_VM_ID}/start"),
                None,
                Some(TEST_TOKEN)
            ),
            Some(200)
        );
        assert_eq!(
            poll_vm_status(
                &addr,
                TEST_VM_ID,
                "running",
                started,
                Duration::from_secs(5),
                &stop
            ),
            Some(())
        );
    }

    #[test]
    fn request_json_parses_the_create_response() {
        let addr = addr_for(start_fake_server_empty());
        let (status, json) = request_json(
            &addr,
            "POST",
            "/api/vms/create",
            Some(r#"{"toml":"[base]\nid = 1"}"#),
            Some(TEST_TOKEN),
        )
        .expect("create request failed");
        assert_eq!(status, 200);
        assert_eq!(
            json.get("id").and_then(serde_json::Value::as_u64),
            Some(TEST_VM_ID)
        );
    }

    #[test]
    fn poll_vm_gone_observes_delete() {
        let addr = addr_for(start_fake_server());
        assert_eq!(
            request_status(
                &addr,
                "DELETE",
                &format!("/api/vms/{TEST_VM_ID}"),
                None,
                Some(TEST_TOKEN)
            ),
            Some(204)
        );
        let stop = AtomicBool::new(false);
        assert_eq!(
            poll_vm_gone(
                &addr,
                TEST_VM_ID,
                std::time::Instant::now(),
                Duration::from_secs(5),
                &stop
            ),
            Some(())
        );
    }

    #[test]
    fn run_readonly_probe_passes() {
        let addr = addr_for(start_fake_server());
        let stop = AtomicBool::new(false);
        run_readonly_probe(
            &addr,
            "readonly",
            Duration::from_secs(5),
            Some(TEST_TOKEN),
            &stop,
        )
        .expect("readonly probe should pass");
    }

    #[test]
    fn run_lifecycle_probe_passes() {
        let addr = addr_for(start_fake_server());
        let stop = AtomicBool::new(false);
        run_lifecycle_probe(
            &addr,
            "lifecycle",
            Duration::from_secs(5),
            Some(TEST_TOKEN),
            TEST_VM_ID,
            &stop,
        )
        .expect("lifecycle probe should pass");
    }

    #[test]
    fn run_dynamic_probe_passes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("vm.toml");
        std::fs::write(&config, "[base]\nid = 1\n").expect("write config");
        let addr = addr_for(start_fake_server_empty());
        let stop = AtomicBool::new(false);
        run_dynamic_probe(
            &addr,
            "dynamic",
            Duration::from_secs(5),
            Some(TEST_TOKEN),
            config.to_str().expect("utf8 path"),
            &stop,
        )
        .expect("dynamic probe should pass");
    }

    #[test]
    fn connection_refused_returns_none() {
        // Bind and immediately drop to get a guaranteed-free port.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr = addr_for(port);
        assert_eq!(request_status(&addr, "GET", "/api/vms", None, None), None);
    }
}
