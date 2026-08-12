//! Host-side probe script runner for QEMU hostfwd integration tests.
//!
//! The probe is the reverse of [`super::host_http`]: instead of serving host
//! fixtures to the guest, it acts as a *client* that dials a management API
//! running *inside* the guest through QEMU user-mode networking
//! (`-netdev user,hostfwd=tcp::<host_port>-:<guest_port>`). The actual
//! assertions are written as a plain shell script (`curl` + `grep`/`jq`) that
//! lives in the test-case directory — axbuild only provides the generic
//! orchestration: wait for the forwarded port, spawn the script, and treat its
//! exit code as the verdict (0 = pass, non-zero = fail). Nothing in the
//! hypervisor knows a test is running.
//!
//! When the script finishes — pass or fail — the guard quits QEMU over the QMP
//! monitor socket the runner added (`-qmp unix:...,server=on,wait=off`), so the
//! QEMU process exits cleanly and the runner reads the stored verdict from the
//! guard as the test result. The case `timeout` remains the backstop if the
//! script or its QMP quit fails.

use std::{
    collections::BTreeMap,
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};

use crate::test::case::HostHttpProbeConfig;

/// Sleep between readiness retries.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// How long to keep retrying the QMP connect before giving up on quitting QEMU.
const QMP_CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const QMP_CONNECT_RETRIES: usize = 10;
/// How long to wait after QMP `quit` for QEMU to exit on its own before
/// force-killing it. QEMU can ignore `quit` when its main loop is stuck in a
/// busy poll (observed under the axbuild runner), so the guard must not wait
/// forever for a clean exit.
#[cfg(target_os = "linux")]
const QMP_QUIT_GRACE: Duration = Duration::from_secs(3);
/// Interval for polling whether QEMU is still alive after `quit`.
#[cfg(target_os = "linux")]
const QMP_ALIVE_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Env var injected into the probe script carrying the forwarded host port.
const PROBE_PORT_ENV: &str = "AXBUILD_PROBE_PORT";
/// Env var injected into the probe script carrying the bearer token, matching
/// the guest build's `[env] AXVM_HTTP_TOKEN`.
const PROBE_TOKEN_ENV: &str = "AXVM_HTTP_TOKEN";

pub(crate) struct HostHttpProbeGuard {
    stop: Arc<AtomicBool>,
    result: Arc<Mutex<Option<anyhow::Result<()>>>>,
    /// Set when the guard had to SIGKILL QEMU because it ignored QMP `quit`.
    /// The runner uses this to prefer the stored probe verdict over QEMU's
    /// non-zero exit status.
    killed_by_probe: Arc<AtomicBool>,
    /// PID of the running probe-script child, so the guard can kill it on drop
    /// if QEMU died before the script finished. Stored as a plain PID (not the
    /// `Child`) so `Drop` never contends with the probe thread's `wait()` on
    /// the same lock.
    child_pid: Arc<Mutex<Option<i32>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HostHttpProbeGuard {
    /// Spawn the probe-script runner thread and return a guard that owns its
    /// lifecycle.
    ///
    /// `script` is the resolved host-side probe script path. `qmp_socket` is
    /// the path QEMU binds from its `-qmp unix:...` argument; the guard
    /// connects to it after the script finishes to quit QEMU. When `None`, the
    /// guard only stores the verdict and relies on the case timeout to end the
    /// run.
    pub(crate) fn start(
        config: &HostHttpProbeConfig,
        script: PathBuf,
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
        let child_pid = Arc::new(Mutex::new(None));
        let thread_child_pid = child_pid.clone();
        let case_name = case_name.to_string();
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread_addr = addr.clone();
        let thread_case_name = case_name.clone();
        let token = config.token.clone();
        let env = config.env.clone();
        let thread = thread::spawn(move || {
            let _ = ready_tx.send(());
            let verdict = run_probe_script(
                &script,
                &thread_addr,
                connect_timeout,
                token.as_deref(),
                &env,
                &thread_stop,
                &thread_child_pid,
            );
            *thread_result.lock().unwrap() = Some(verdict);
            // Quit QEMU so the run ends on the script verdict instead of the
            // serial-timeout path. `request_qmp_quit` force-kills QEMU when it
            // ignores `quit` (a hang observed under the runner); record that so
            // the runner trusts the stored verdict over QEMU's non-zero exit
            // status.
            if let Some(socket) = qmp_socket {
                match request_qmp_quit(&socket) {
                    Ok(true) => thread_killed.store(true, Ordering::SeqCst),
                    Ok(false) => {}
                    Err(err) => eprintln!(
                        "  host http probe: {thread_case_name}: failed to quit QEMU via QMP: \
                         {err:#}"
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
            child_pid,
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
        // If QEMU died before the script finished, kill the script child so the
        // join below does not wait forever on a probe whose server is gone. The
        // thread reaps the child, so the PID slot is cleared once it exits;
        // killing a reaped/reused PID is avoided because the slot is only
        // non-None while the thread is still waiting.
        #[cfg(target_os = "linux")]
        if let Some(pid) = self.child_pid.lock().unwrap().take() {
            kill_process(pid);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Run the probe script and report the verdict: wait for the forwarded host
/// port to accept connections (guest boot + network init), spawn the script
/// with the port/token/env injected, and map its exit code to a result. The
/// script's stdout/stderr are inherited so its output appears in the runner
/// log.
fn run_probe_script(
    script: &Path,
    addr: &str,
    connect_timeout: Duration,
    token: Option<&str>,
    env: &BTreeMap<String, String>,
    stop: &AtomicBool,
    child_pid_slot: &Mutex<Option<i32>>,
) -> anyhow::Result<()> {
    wait_for_port_ready(addr, connect_timeout, stop).with_context(|| {
        format!("guest HTTP server never became reachable within {connect_timeout:?}")
    })?;

    let port = addr.rsplit_once(':').map(|(_, p)| p).unwrap_or("");
    let mut cmd = Command::new(script);
    cmd.env(PROBE_PORT_ENV, port);
    if let Some(token) = token {
        cmd.env(PROBE_TOKEN_ENV, token);
    }
    cmd.envs(env);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn probe script {}", script.display()))?;
    // Publish the PID so `Drop` can SIGKILL the script if QEMU dies while it is
    // still running. The slot is cleared after `wait` reaps the child, so a
    // dead/reused PID is never killed.
    *child_pid_slot.lock().unwrap() = Some(child.id() as i32);
    let status = child
        .wait()
        .with_context(|| format!("failed to wait for probe script {}", script.display()))?;
    *child_pid_slot.lock().unwrap() = None;
    if status.success() {
        println!("  host http probe: probe passed");
        Ok(())
    } else {
        let code = status.code();
        eprintln!("  host http probe: probe failed (exit status {status})");
        bail!(
            "probe script {} exited with status {status:?} (code {code:?})",
            script.display()
        )
    }
}

/// Poll the forwarded host port until a TCP connection succeeds, the deadline
/// elapses, or a stop is requested. A successful connect means the guest's
/// network stack is up; the in-guest server may still be booting, so the probe
/// script itself should retry its first request.
fn wait_for_port_ready(
    addr: &str,
    connect_timeout: Duration,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            bail!("host http probe stopped");
        }
        if started.elapsed() >= connect_timeout {
            bail!("timed out after {connect_timeout:?}");
        }
        if TcpStream::connect(addr).is_ok() {
            return Ok(());
        }
        thread::sleep(CONNECT_RETRY_INTERVAL);
    }
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
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
    };

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
