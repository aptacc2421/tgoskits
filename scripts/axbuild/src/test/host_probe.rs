//! Host-side probe runner for QEMU hostfwd integration tests.
//!
//! The probe is the reverse of [`super::host_http`]: instead of serving host
//! fixtures to the guest, it acts as a *client* that dials a management API
//! running *inside* the guest through QEMU user-mode networking
//! (`-netdev user,hostfwd=tcp::<host_port>-:<guest_port>`). The actual HTTP
//! assertions live in a typed probe module (see
//! [`crate::axvisor::test::http_probe`]) that the runner wires in as a
//! callback; this module only provides the generic orchestration: wait for the
//! forwarded port, invoke the probe, and store its result as the verdict.
//! Nothing in the hypervisor knows a test is running.
//!
//! When the probe finishes — pass or fail — the guard quits QEMU over the QMP
//! monitor socket the runner added (`-qmp unix:...,server=on,wait=off`), so the
//! QEMU process exits cleanly and the runner reads the stored verdict from the
//! guard as the test result. The runner owns the QEMU child, so the case
//! `timeout` remains the backstop if the probe or its QMP quit fails.

use std::{
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

use anyhow::{Context, bail};

use crate::test::case::HostHttpProbeConfig;

/// The probe callback invoked by the guard once the forwarded port accepts
/// connections. Returns the verdict (`Ok` = pass, `Err` = fail). The probe is a
/// `FnOnce` so it may own everything it needs (base address, token, config
/// paths) and runs on the guard's worker thread.
pub(crate) type HostHttpProbeFn = Box<dyn FnOnce() -> anyhow::Result<()> + Send + 'static>;

/// Sleep between readiness retries.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// How long to keep retrying the QMP connect before giving up on quitting QEMU.
const QMP_CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const QMP_CONNECT_RETRIES: usize = 10;

pub(crate) struct HostHttpProbeGuard {
    stop: Arc<AtomicBool>,
    result: Arc<Mutex<Option<anyhow::Result<()>>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HostHttpProbeGuard {
    /// Spawn the probe runner thread and return a guard that owns its
    /// lifecycle.
    ///
    /// `probe` is the host-side probe callback (the typed HTTP assertions);
    /// the guard waits for the forwarded port to accept connections, invokes
    /// it, and stores its result as the verdict. `qmp_socket` is the path QEMU
    /// binds from its `-qmp unix:...` argument; the guard connects to it after
    /// the probe finishes to quit QEMU. When `None`, the guard only stores the
    /// verdict and relies on the case timeout to end the run.
    pub(crate) fn start(
        config: &HostHttpProbeConfig,
        host_port: u16,
        case_name: &str,
        qmp_socket: Option<PathBuf>,
        probe: HostHttpProbeFn,
    ) -> anyhow::Result<Self> {
        let addr = format!("127.0.0.1:{host_port}");
        let connect_timeout = Duration::from_secs(config.connect_timeout_secs);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let result = Arc::new(Mutex::new(None));
        let thread_result = result.clone();
        let case_name = case_name.to_string();
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread_addr = addr.clone();
        let thread_case_name = case_name.clone();
        let thread = thread::spawn(move || {
            let _ = ready_tx.send(());
            // The guard waits for the forwarded port (guest boot + network
            // init); the probe then runs the HTTP assertions. The probe is
            // consumed exactly once.
            let verdict = (|| -> anyhow::Result<()> {
                wait_for_port_ready(&thread_addr, connect_timeout, &thread_stop).with_context(
                    || {
                        format!(
                            "guest HTTP server never became reachable within {connect_timeout:?}"
                        )
                    },
                )?;
                probe()
            })();
            *thread_result.lock().unwrap() = Some(verdict);
            // Quit QEMU so a successful run ends promptly on the probe verdict
            // instead of the serial-timeout path. The runner owns the QEMU
            // child, so it decides whether QEMU actually exits: a `quit` that
            // is ignored degrades to the case timeout, which then fails the
            // run — a stuck QEMU must not be reported as a probe success.
            if let Some(socket) = qmp_socket
                && let Err(err) = request_qmp_quit(&socket)
            {
                eprintln!(
                    "  host http probe: {thread_case_name}: failed to quit QEMU via QMP: {err:#}"
                );
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
}

impl Drop for HostHttpProbeGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Poll the forwarded host port until a TCP connection succeeds, the deadline
/// elapses, or a stop is requested. A successful connect means the guest's
/// network stack is up; the in-guest server may still be booting, so the probe
/// itself should retry its first request.
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

/// Quit QEMU by connecting to its QMP monitor socket and issuing `quit`. The
/// socket path comes from the `-qmp unix:...,server=on,wait=off` argument the
/// runner added. Returns once `quit` has been sent; whether QEMU actually exits
/// is the runner's job — it owns the QEMU child, and an ignored `quit` degrades
/// to the case timeout, which then fails the run (a stuck QEMU must not be
/// reported as a probe success).
#[cfg(unix)]
fn request_qmp_quit(socket: &Path) -> anyhow::Result<()> {
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
    let mut buf = [0_u8; 512];
    let _ = stream.read(&mut buf); // QMP greeting
    stream.write_all(b"{\"execute\":\"qmp_capabilities\"}\r\n")?;
    buf.fill(0);
    let _ = stream.read(&mut buf); // capabilities response
    stream.write_all(b"{\"execute\":\"quit\"}\r\n")?;
    stream.flush()?;
    Ok(())
}

#[cfg(not(unix))]
fn request_qmp_quit(_socket: &Path) -> anyhow::Result<()> {
    bail!("QMP unix sockets are not supported on this host")
}
