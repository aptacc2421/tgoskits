#!/usr/bin/env python3
"""Host-side probe asset for the AxVisor pause/resume/reset HTTP control plane.

Case asset for the `http-pause-resume` test case
(`test-suit/axvisor/normal/qemu-http-pause-resume/`). It owns the *test
content* — the concrete requests, the `vm-memory.toml` fixture, and the
assertions — and can evolve independently of the axbuild runner.

The generic axbuild probe runner
(`scripts/axbuild/src/axvisor/test/http_probe.rs`) executes this script after
the QEMU hostfwd port is reachable, then treats the exit code as the verdict:
0 = all assertions passed, nonzero = a step failed. The script dials the axum
management API running *inside* the AxVisor guest through QEMU user-mode
networking hostfwd. Nothing in the hypervisor knows a test is running.

Environment (set by the generic runner):

    AXVISOR_HTTP_BASE            http://127.0.0.1:<host_port> (forwarded)
    AXVISOR_HTTP_TOKEN           bearer token for authenticated requests
    AXVISOR_HTTP_CASE_DIR        case directory holding `vm-memory.toml`
                                 (default: this file's directory)
    AXVISOR_HTTP_CONNECT_TIMEOUT seconds for the initial reachability wait
    AXVISOR_HTTP_REQUEST_TIMEOUT seconds per HTTP request

The probe drives the pause/resume/reset lifecycle contract in one boot,
covering auth, error mapping, the `async` markers, and repeated suspend/wake
cycles:

    GET    /api/vms              -> 200            (list; id=1 present)
    GET    /api/vms/1            -> 200 ready      (detail; id/name/cpu_num/vcpu_states)
    POST   /api/vms/1/pause      -> 401            (no token)
    POST   /api/vms/1/resume     -> 401            (no token)
    POST   /api/vms/1/reset      -> 401            (no token)
    POST   /api/vms/999/pause    -> 404            (auth'd unknown VM)
    POST   /api/vms/999/resume   -> 404            (auth'd unknown VM)
    POST   /api/vms/999/reset    -> 404            (auth'd unknown VM)
    POST   /api/vms/1/pause      -> 409            (pause from Ready)
    POST   /api/vms/1/resume     -> 409            (resume from Ready)
    POST   /api/vms/1/start      -> 200 -> running (async=false)
    POST   /api/vms/1/start      -> 409            (already running)
    POST   /api/vms/1/resume     -> 409            (resume from Running)
    POST   /api/vms/1/pause      -> 200 -> paused  (async=true)
    POST   /api/vms/1/pause      -> 409            (already paused)
    POST   /api/vms/1/resume     -> 200 -> running (async=false)
    POST   /api/vms/1/resume     -> 409            (already running)
    POST   /api/vms/1/pause      -> 200 -> paused  (second suspend/wake cycle)
    POST   /api/vms/1/resume     -> 200 -> running
    POST   /api/vms/1/reset      -> 200 -> running (async=false, restart)
    POST   /api/vms/1/resume     -> 409            (reset rebuilt to Running)
    POST   /api/vms/1/pause      -> 200 -> paused  (reset VM fully usable)
    POST   /api/vms/1/resume     -> 200 -> running
    POST   /api/vms/1/stop       -> 200 -> stopped (async=true)
    DELETE /api/vms/1            -> 204 -> 404     (cleanup)

The fixture pins the vCPU to Core 1 (`phys_cpu_ids = [1]`) with the management
console on Core 0, so resume must wake a vCPU parked on a *non-primary* pinned
CPU. The generic axbuild runner only lets the probe observe VM status and
per-vCPU snapshot state over HTTP; neither distinguishes a genuinely
re-executing guest from a status flip, so this asset asserts the full
state-machine contract (transitions, error mapping, async markers, and that
each suspend/wake cycle converges within its poll deadline). Guest execution
itself is validated by the QEMU serial capture — the hypervisor logs
`VCpu[x] resumed from suspend` on each wake and a resumed guest reaches the
`stopped` poll below — rather than by an HTTP-visible guest counter.
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request

BASE = os.environ.get("AXVISOR_HTTP_BASE", "http://127.0.0.1:8080").rstrip("/")
TOKEN = os.environ.get("AXVISOR_HTTP_TOKEN", "")
CASE_DIR = os.environ.get(
    "AXVISOR_HTTP_CASE_DIR", os.path.dirname(os.path.abspath(__file__))
)
CONNECT_TIMEOUT = float(os.environ.get("AXVISOR_HTTP_CONNECT_TIMEOUT", "120"))
REQUEST_TIMEOUT = float(os.environ.get("AXVISOR_HTTP_REQUEST_TIMEOUT", "5"))
# Deadline for VM state transitions (boot, pause, resume, reset, stop, delete):
# must stay well below the case `timeout` (600s) so a stuck transition fails on
# the probe, not on the QEMU timeout.
POLL_DEADLINE = 120.0
POLL_INTERVAL = 1.0


def request(method, path, token=None, body=None):
    """One HTTP request; returns (status, parsed JSON or None)."""
    headers = {}
    if token:
        headers["Authorization"] = "Bearer " + token
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = body.encode("utf-8")
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT) as resp:
            status = resp.status
            raw = resp.read()
    except urllib.error.HTTPError as err:
        status = err.code
        raw = err.read()
    except urllib.error.URLError as err:
        raise RuntimeError("request %s %s failed: %s" % (method, path, err.reason))
    except OSError as err:
        # QEMU's user-mode hostfwd accepts the host-side connection as soon as
        # QEMU starts, before the in-guest management server binds, so a first
        # request can stall to the request timeout. Converting it to a retryable
        # RuntimeError here lets the poll loops retry in the boot window.
        raise RuntimeError("request %s %s failed: %s" % (method, path, err))
    if not raw:
        return status, None
    return status, json.loads(raw.decode("utf-8"))


def check(label, actual, expected):
    """Assert a status code, printing a progress line."""
    if actual != expected:
        raise AssertionError("%s returned %s, expected %s" % (label, actual, expected))
    print("  http probe: %s -> %s (expect %s)" % (label, actual, expected))


def vm_status(body):
    """Extract the top-level `status` string of a VM detail body."""
    if not isinstance(body, dict):
        raise AssertionError("VM detail response was not a JSON object")
    status = body.get("status")
    if not isinstance(status, str):
        raise AssertionError("VM detail response had no status string: %r" % (body,))
    return status


def check_vm_status(label, body, expected):
    status = vm_status(body)
    print("  http probe: %s -> status %s (expect %s)" % (label, status, expected))
    if status != expected:
        raise AssertionError(
            "%s reported status %s, expected %s" % (label, status, expected)
        )


def check_action(label, body, ok_expected, async_expected):
    """Assert a lifecycle action response's `ok` and `async` markers."""
    if not isinstance(body, dict):
        raise AssertionError("%s had no JSON body" % (label,))
    ok = body.get("ok")
    is_async = body.get("async")
    print(
        "  http probe: %s -> ok=%r async=%r (expect ok=%r async=%r)"
        % (label, ok, is_async, ok_expected, async_expected)
    )
    if ok != ok_expected:
        raise AssertionError(
            "%s reported ok=%r, expected %r" % (label, ok, ok_expected)
        )
    if is_async != async_expected:
        raise AssertionError(
            "%s reported async=%r, expected %r" % (label, is_async, async_expected)
        )


def list_has_vm(body, vm_id):
    """Whether a `GET /api/vms` body lists a VM with the given id."""
    return isinstance(body, list) and any(
        isinstance(item, dict) and item.get("id") == vm_id for item in body
    )


def poll_ready():
    """Poll `GET /api/vms` until it returns 200 or the connect deadline passes."""
    start = time.monotonic()
    while True:
        if time.monotonic() - start > CONNECT_TIMEOUT:
            raise AssertionError(
                "guest management HTTP server never became reachable within %.0fs"
                % CONNECT_TIMEOUT
            )
        try:
            status, _ = request("GET", "/api/vms")
            if status == 200:
                return
        except RuntimeError:
            pass
        time.sleep(POLL_INTERVAL)


def poll_vm_status(vm_id, expected):
    """Poll `GET /api/vms/{id}` until its status equals `expected`."""
    start = time.monotonic()
    while True:
        if time.monotonic() - start > POLL_DEADLINE:
            raise AssertionError(
                "VM[%d] never became %s within %.0fs" % (vm_id, expected, POLL_DEADLINE)
            )
        try:
            status, body = request("GET", "/api/vms/%d" % vm_id)
            if status == 200 and vm_status(body) == expected:
                print("  http probe: VM[%d] -> %s" % (vm_id, expected))
                return
        except (RuntimeError, AssertionError):
            # A non-200 or transport error during a transition is transient;
            # keep polling until the deadline.
            pass
        time.sleep(POLL_INTERVAL)


def poll_vm_gone(vm_id):
    """Poll `GET /api/vms/{id}` until it returns 404 (the VM was deleted)."""
    start = time.monotonic()
    while True:
        if time.monotonic() - start > POLL_DEADLINE:
            raise AssertionError(
                "VM[%d] never disappeared within %.0fs" % (vm_id, POLL_DEADLINE)
            )
        try:
            status, _ = request("GET", "/api/vms/%d" % vm_id)
            if status == 404:
                print("  http probe: VM[%d] -> gone" % vm_id)
                return
        except RuntimeError:
            pass
        time.sleep(POLL_INTERVAL)


def main():
    # 1. Readiness: the runner already waited for the TCP port; retry the first
    #    request briefly in case the axum router is still binding.
    poll_ready()
    print("  http probe: guest management server reachable")

    # 2. List: the default VM (id 1) is registered and `Ready`.
    status, body = request("GET", "/api/vms")
    check("GET /api/vms", status, 200)
    if not list_has_vm(body, 1):
        raise AssertionError("GET /api/vms did not list the default VM id=1")

    # 3. Detail of the default VM: identity, shape, and ready status.
    status, body = request("GET", "/api/vms/1")
    check("GET /api/vms/1", status, 200)
    check_vm_status("GET /api/vms/1", body, "ready")
    if body.get("id") != 1:
        raise AssertionError("GET /api/vms/1 did not report id=1")
    if body.get("name") != "linux-http-pause-resume":
        raise AssertionError("GET /api/vms/1 did not report the fixture name")
    if body.get("cpu_num") != 1:
        raise AssertionError("GET /api/vms/1 did not report cpu_num=1")
    if not isinstance(body.get("vcpu_states"), list) or not body["vcpu_states"]:
        raise AssertionError("GET /api/vms/1 reported an empty vcpu_states array")

    # 4-6. Auth: every mutating lifecycle route rejects an unauthenticated write
    #       with 401, before any VM lookup or state check.
    status, _ = request("POST", "/api/vms/1/pause")
    check("POST /api/vms/1/pause (no auth)", status, 401)
    status, _ = request("POST", "/api/vms/1/resume")
    check("POST /api/vms/1/resume (no auth)", status, 401)
    status, _ = request("POST", "/api/vms/1/reset")
    check("POST /api/vms/1/reset (no auth)", status, 401)

    # 7-9. Authenticated lifecycle writes to an unknown VM are 404.
    status, _ = request("POST", "/api/vms/999/pause", token=TOKEN)
    check("POST /api/vms/999/pause (auth'd)", status, 404)
    status, _ = request("POST", "/api/vms/999/resume", token=TOKEN)
    check("POST /api/vms/999/resume (auth'd)", status, 404)
    status, _ = request("POST", "/api/vms/999/reset", token=TOKEN)
    check("POST /api/vms/999/reset (auth'd)", status, 404)

    # 10-11. Pause/resume are only valid from Running/Paused respectively; a
    #        `Ready` (not started) VM rejects both with 409.
    status, _ = request("POST", "/api/vms/1/pause", token=TOKEN)
    check("POST /api/vms/1/pause (from Ready)", status, 409)
    status, _ = request("POST", "/api/vms/1/resume", token=TOKEN)
    check("POST /api/vms/1/resume (from Ready)", status, 409)

    # 12. Start the default VM: accepted synchronously (`async=false`), then
    #     poll the detail into `running`.
    status, body = request("POST", "/api/vms/1/start", token=TOKEN)
    check("POST /api/vms/1/start", status, 200)
    check_action("POST /api/vms/1/start", body, True, False)
    poll_vm_status(1, "running")

    # 13. Re-starting an already-running VM conflicts.
    status, _ = request("POST", "/api/vms/1/start", token=TOKEN)
    check("POST /api/vms/1/start (already running)", status, 409)

    # 14. Resume is only valid from Paused; a running VM rejects it.
    status, _ = request("POST", "/api/vms/1/resume", token=TOKEN)
    check("POST /api/vms/1/resume (from Running)", status, 409)

    # 15. Pause is a request (`async=true`): the status flips to `Paused`
    #     synchronously while the vCPU parks at its next run-loop iteration.
    status, body = request("POST", "/api/vms/1/pause", token=TOKEN)
    check("POST /api/vms/1/pause", status, 200)
    check_action("POST /api/vms/1/pause", body, True, True)
    poll_vm_status(1, "paused")

    # 16. Pausing an already-paused VM conflicts.
    status, _ = request("POST", "/api/vms/1/pause", token=TOKEN)
    check("POST /api/vms/1/pause (already paused)", status, 409)

    # 17. Resume is synchronous (`async=false`): the status flips back to
    #     `Running` and the parked vCPU is woken to re-enter the guest.
    status, body = request("POST", "/api/vms/1/resume", token=TOKEN)
    check("POST /api/vms/1/resume", status, 200)
    check_action("POST /api/vms/1/resume", body, True, False)
    poll_vm_status(1, "running")

    # 18. Resuming an already-running VM conflicts.
    status, _ = request("POST", "/api/vms/1/resume", token=TOKEN)
    check("POST /api/vms/1/resume (already running)", status, 409)

    # 19-20. Second suspend/wake cycle: a parked vCPU is woken and re-parked
    #        repeatedly, so the resume wake path must converge every time.
    status, body = request("POST", "/api/vms/1/pause", token=TOKEN)
    check("POST /api/vms/1/pause (cycle 2)", status, 200)
    check_action("POST /api/vms/1/pause (cycle 2)", body, True, True)
    poll_vm_status(1, "paused")
    status, body = request("POST", "/api/vms/1/resume", token=TOKEN)
    check("POST /api/vms/1/resume (cycle 2)", status, 200)
    check_action("POST /api/vms/1/resume (cycle 2)", body, True, False)
    poll_vm_status(1, "running")

    # 21. Reset discards and rebuilds the runtime synchronously, restarting the
    #     VM in `Running` from any state (here: Running).
    status, body = request("POST", "/api/vms/1/reset", token=TOKEN)
    check("POST /api/vms/1/reset", status, 200)
    check_action("POST /api/vms/1/reset", body, True, False)
    poll_vm_status(1, "running")

    # 22. The rebuilt VM is `Running` again, so resume is still invalid and the
    #     pause/resume lifecycle works on the fresh runtime (not merely a
    #     state flip of the old one).
    status, _ = request("POST", "/api/vms/1/resume", token=TOKEN)
    check("POST /api/vms/1/resume (after reset)", status, 409)
    status, body = request("POST", "/api/vms/1/pause", token=TOKEN)
    check("POST /api/vms/1/pause (after reset)", status, 200)
    check_action("POST /api/vms/1/pause (after reset)", body, True, True)
    poll_vm_status(1, "paused")
    status, body = request("POST", "/api/vms/1/resume", token=TOKEN)
    check("POST /api/vms/1/resume (after reset)", status, 200)
    check_action("POST /api/vms/1/resume (after reset)", body, True, False)
    poll_vm_status(1, "running")

    # 23. Stop is a request (`async=true`): the `stopped` state arrives
    #     asynchronously once the vCPU observes it and exits.
    status, body = request("POST", "/api/vms/1/stop", token=TOKEN)
    check("POST /api/vms/1/stop", status, 200)
    check_action("POST /api/vms/1/stop", body, True, True)
    poll_vm_status(1, "stopped")

    # 24. Cleanup: leave the hypervisor without a registered VM.
    status, _ = request("DELETE", "/api/vms/1", token=TOKEN)
    check("DELETE /api/vms/1 (cleanup)", status, 204)
    poll_vm_gone(1)

    print("  http probe: full pause/resume/reset contract passed")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as exc:
        print("  http probe: FAILED: %s" % exc, file=sys.stderr)
        sys.exit(1)
    except Exception as exc:
        print("  http probe: ERROR: %s" % exc, file=sys.stderr)
        sys.exit(2)
