//! Host-side TCP probe for QEMU hostfwd integration tests (`qemu-http-axum-tcp`).
//!
//! The probe is the reverse of [`super::host_http`]: instead of serving host
//! fixtures to the guest, it acts as a *client* that dials a management API
//! running *inside* the guest through QEMU user-mode networking
//! (`-netdev user,hostfwd=tcp::<host_port>-:<guest_port>`). It makes real HTTP
//! requests, asserts the response statuses, and relays a single PASSED/FAILED
//! verdict to a guest endpoint (`POST /__probe_result`) that the hypervisor
//! mirrors into the serial log. The QEMU runner's stream matcher then picks up
//! the sentinel and terminates the run, exactly as it does for the in-guest
//! self-test sentinels.
//!
//! The probe is currently hardcoded to the read-only contract (the same two
//! endpoints `http-test`'s oneshot self-test covers): `GET /api/vms -> 200` and
//! `GET /api/vms/999 -> 404`.

use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::bail;

use crate::test::case::HostHttpProbeConfig;

/// Per-attempt IO timeout for a single HTTP request/response exchange.
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Sleep between readiness retries.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct HostHttpProbeGuard {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HostHttpProbeGuard {
    pub(crate) fn start(
        config: &HostHttpProbeConfig,
        host_port: u16,
        case_name: &str,
    ) -> anyhow::Result<Self> {
        let addr = format!("127.0.0.1:{host_port}");
        let connect_timeout = Duration::from_secs(config.connect_timeout_secs);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let case_name = case_name.to_string();
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread_addr = addr.clone();
        let thread_case_name = case_name.clone();
        let thread = thread::spawn(move || {
            let _ = ready_tx.send(());
            run_probe(
                &thread_addr,
                &thread_case_name,
                connect_timeout,
                &thread_stop,
            );
        });

        if ready_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            stop.store(true, Ordering::Release);
            bail!("host http probe for `{case_name}` did not become ready");
        }

        println!("  host http probe: {addr} -> guest:{}", config.guest_port);
        Ok(Self {
            stop,
            thread: Some(thread),
        })
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

fn run_probe(addr: &str, case_name: &str, connect_timeout: Duration, stop: &AtomicBool) {
    let started = Instant::now();

    // Wait for the guest HTTP server to accept connections. `GET /api/vms` is
    // retried until it yields a parsed status (readiness), the connect timeout
    // elapses, or a stop is requested. A parsed but wrong status is still
    // "ready"; the assertion below records it as a failure.
    let mut passed = true;
    let list = poll_status(addr, "/api/vms", started, connect_timeout, stop);
    match list {
        Some(status) => {
            println!("  host http probe: {case_name}: GET /api/vms -> {status} (expect 200)");
            passed &= status == 200;
        }
        None => {
            eprintln!(
                "  host http probe: {case_name}: guest HTTP server never became reachable within \
                 {connect_timeout:?}"
            );
            passed = false;
        }
    }

    // The server is up; single attempt for the 404 path.
    if passed {
        match request_status(addr, "GET", "/api/vms/999", None) {
            Some(status) => {
                println!(
                    "  host http probe: {case_name}: GET /api/vms/999 -> {status} (expect 404)"
                );
                passed &= status == 404;
            }
            None => {
                eprintln!("  host http probe: {case_name}: GET /api/vms/999 failed");
                passed = false;
            }
        }
    }

    // Relay the verdict. The hypervisor mirrors it into the serial log, where
    // the QEMU runner's stream matcher sees the sentinel and ends the run. If
    // the relay itself fails (e.g. the server disappeared), no sentinel ever
    // appears in serial and the run ends on the QEMU timeout — still a failure.
    let verdict = if passed { "PASSED" } else { "FAILED" };
    match request_status(addr, "POST", "/__probe_result", Some(verdict)) {
        Some(status) => {
            println!("  host http probe: {case_name}: verdict {verdict} relayed (status {status})")
        }
        None => eprintln!("  host http probe: {case_name}: failed to relay verdict {verdict}"),
    }
}

/// Retry a request until it yields a parsed status, the deadline elapses, or a
/// stop is requested. Used for the first request, which doubles as the
/// readiness probe.
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
        if let Some(status) = request_status(addr, "GET", path, None) {
            return Some(status);
        }
        thread::sleep(CONNECT_RETRY_INTERVAL);
    }
}

/// Send one HTTP/1.1 request over a fresh connection and parse the status code.
/// `body` (when present) is sent as the request body with a JSON content type.
fn request_status(addr: &str, method: &str, path: &str, body: Option<&str>) -> Option<u16> {
    let Ok(mut stream) = TcpStream::connect(addr) else {
        return None;
    };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n");
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
    parse_status(&response)
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

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use super::{parse_status, poll_status, request_status};

    /// Serve canned responses on a background thread: `/api/vms` -> 200,
    /// `/api/vms/999` -> 404, anything else -> 200 with the request body echoed
    /// in a header (so the relay POST can be observed).
    fn start_fake_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(_) => break,
                };
                let mut request = Vec::new();
                let mut buf = [0u8; 512];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            request.extend_from_slice(&buf[..n]);
                            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let head = String::from_utf8_lossy(&request);
                let first_line = head.lines().next().unwrap_or("");
                let status = if first_line.contains("/api/vms/999") {
                    "404 Not Found"
                } else {
                    "200 OK"
                };
                let body =
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(body.as_bytes());
            }
        });
        port
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
        let port = start_fake_server();
        let addr = format!("127.0.0.1:{port}");
        assert_eq!(request_status(&addr, "GET", "/api/vms", None), Some(200));
        assert_eq!(
            request_status(&addr, "GET", "/api/vms/999", None),
            Some(404)
        );
    }

    #[test]
    fn poll_status_returns_once_server_is_up() {
        let port = start_fake_server();
        let addr = format!("127.0.0.1:{port}");
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
        let port = start_fake_server();
        let addr = format!("127.0.0.1:{port}");
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
    fn request_status_handles_body_relay() {
        let port = start_fake_server();
        let addr = format!("127.0.0.1:{port}");
        assert_eq!(
            request_status(&addr, "POST", "/__probe_result", Some("PASSED")),
            Some(200)
        );
    }

    #[test]
    fn connection_refused_returns_none() {
        // Bind and immediately drop to get a guaranteed-free port.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr = format!("127.0.0.1:{port}");
        assert_eq!(request_status(&addr, "GET", "/api/vms", None), None);
    }
}
