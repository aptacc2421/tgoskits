#!/usr/bin/env python3
"""Host-side probe asset for the AxVisor control plane over the VM pool.

Case asset for the `qemu-vm-pool-http` test case
(`test-suit/axvisor/normal/qemu-vm-pool-http/`). It owns the test content — the
concrete requests and assertions — and can evolve independently of the axbuild
runner.

The generic axbuild probe runner
(`scripts/axbuild/src/axvisor/test/http_probe.rs`) executes this script after
the QEMU hostfwd port is reachable, then treats the exit code as the verdict:
0 = all assertions passed, nonzero = a step failed. The script dials the axum
management API running *inside* the AxVisor guest through QEMU user-mode
networking hostfwd. Nothing in the hypervisor knows a test is running.

Environment (set by the generic runner):

    AXVISOR_HTTP_BASE            http://127.0.0.1:<host_port> (forwarded)
    AXVISOR_HTTP_CASE_DIR        case directory holding the `sh/` fixtures
    AXVISOR_HTTP_CONNECT_TIMEOUT seconds for the initial reachability wait
    AXVISOR_HTTP_REQUEST_TIMEOUT seconds per HTTP request

The case boots with no default guest and a pool directory holding three complete
guest configs plus one unusable file, all injected into the guest filesystem by
the `sh/` asset pipeline. The probe drives the whole pool → lifecycle contract in
one boot:

    GET    /api/vms                -> 200 []          (nothing created at startup)
    GET    /api/vms/pool           -> 200             (3 entries + 1 issue, TOML verbatim)
    POST   /api/vms/99/start       -> 404             (neither registered nor pooled)
    POST   /api/vms/1/start        -> 200 running     (created on demand from its entry)
    POST   /api/vms/1/start        -> 409             (already running)
    POST   /api/vms/2/start        -> 200 running     (second guest starts beside the first)
    POST   /api/vms/3/start        -> 200 running
    GET    /api/vms                -> 200             (exactly the three, all running)
    GET    /api/vms/pool           -> 200             (entries survive being started)
    DELETE /api/vms/2              -> 204             (close)
    GET    /api/vms/2              -> 404             (gone from the registry)
    GET    /api/vms/pool           -> 200             (its entry is still a candidate)
    POST   /api/vms/2/start        -> 200 running     (restart from the same entry)
    DELETE /api/vms/{1,2,3}        -> 204             (close the rest)
    GET    /api/vms                -> 200 []          (registry empty again)
    GET    /api/vms/pool           -> 200             (pool unchanged throughout)

Every start is additionally checked against `guest_entry_count` from the VM
detail: the hypervisor's vCPU run loop increments it only after the guest has
actually (re-)entered, so a start that merely flips a status without running the
guest cannot pass. The pool listing is compared against the fixture files in
`sh/` byte for byte, so a listing that invents or mangles configs cannot pass,
and entries are matched by id rather than by position so the assertions do not
depend on the directory's enumeration order.
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request

BASE = os.environ.get("AXVISOR_HTTP_BASE", "http://127.0.0.1:8080").rstrip("/")
CASE_DIR = os.environ.get(
    "AXVISOR_HTTP_CASE_DIR", os.path.dirname(os.path.abspath(__file__))
)
CONNECT_TIMEOUT = float(os.environ.get("AXVISOR_HTTP_CONNECT_TIMEOUT", "120"))
REQUEST_TIMEOUT = float(os.environ.get("AXVISOR_HTTP_REQUEST_TIMEOUT", "5"))
# Deadline for VM state transitions (boot, delete): stays well below the case
# `timeout` (600s) so a stuck transition fails on the probe, not on QEMU.
POLL_DEADLINE = 120.0
POLL_INTERVAL = 1.0

# The pool entries the `sh/` pipeline injected, keyed by VM id.
POOL_ENTRIES = {
    1: "pool-guest-1.toml",
    2: "pool-guest-2.toml",
    3: "pool-guest-3.toml",
}
BROKEN_ENTRY = "pool-broken.toml"


def request(method, path, body=None):
    """One HTTP request; returns (status, parsed JSON or None).

    The control plane has no authentication, so no request carries an
    Authorization header. A non-2xx response is not an error here — the caller
    asserts the status. A transport error (connection refused/reset/timeout
    while the guest server is coming up, or while it is busy creating a guest)
    raises RuntimeError for the caller to retry or verify.
    """
    headers = {}
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
        # `resp.read()` raises a bare socket timeout (an OSError) that the
        # URLError handler does not wrap. QEMU's user-mode hostfwd accepts the
        # host-side connection before the in-guest server binds, so a first
        # request can stall; retry instead of crashing in the boot window.
        raise RuntimeError("request %s %s failed: %s" % (method, path, err))
    if not raw:
        return status, None
    return status, json.loads(raw.decode("utf-8"))


def get(path, label):
    """One GET, retried until the deadline.

    The management API runs on a single-threaded runtime *inside* the guest and
    a start request creates the VM synchronously, so the server stops answering
    while it reads a guest kernel image and builds the VM. A read that lands in
    that window times out at the transport level; retrying keeps the probe
    deterministic without turning a busy server into a failure.
    """
    start = time.monotonic()
    while True:
        try:
            return request("GET", path)
        except RuntimeError as error:
            if time.monotonic() - start > POLL_DEADLINE:
                raise AssertionError("%s: %s (and it never recovered)" % (label, error))
            time.sleep(POLL_INTERVAL)


def check(label, actual, expected):
    """Assert an observed value, printing a progress line."""
    if actual != expected:
        raise AssertionError("%s was %r, expected %r" % (label, actual, expected))
    print("  pool http probe: %s -> %r" % (label, actual))


def expect_status(label, actual, expected):
    """Assert a status code, printing a progress line."""
    if actual != expected:
        raise AssertionError("%s returned %s, expected %s" % (label, actual, expected))
    print("  pool http probe: %s -> %s" % (label, actual))


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
                print("  pool http probe: guest management HTTP server reachable")
                return
        except RuntimeError:
            pass
        time.sleep(POLL_INTERVAL)


def fixture_text(name):
    """Read one injected fixture from the case directory on the host."""
    path = os.path.join(CASE_DIR, "sh", name)
    with open(path, "r", encoding="utf-8") as handle:
        return handle.read()


def pool_body(label):
    """Fetch the pool listing, asserting 200 and the expected shape."""
    status, body = get("/api/vms/pool", label)
    expect_status(label, status, 200)
    if not isinstance(body, dict):
        raise AssertionError("%s did not return a JSON object: %r" % (label, body))
    for field in ("directory", "entries", "issues"):
        if field not in body:
            raise AssertionError("%s had no %s field: %r" % (label, field, body))
    if not isinstance(body["entries"], list) or not isinstance(body["issues"], list):
        raise AssertionError("%s had non-list entries/issues: %r" % (label, body))
    return body


def check_pool_matches_fixtures(label, body):
    """Assert the listing holds exactly the injected valid entries, verbatim.

    Entries are matched by id, so the check does not depend on the directory's
    enumeration order. Each reported `toml` must equal the fixture file byte for
    byte: the pool is the authority for the config, so a listing that drops,
    reorders into the wrong ids, or rewrites configs cannot pass.
    """
    entries = {entry.get("id"): entry for entry in body["entries"]}
    if sorted(entries) != sorted(POOL_ENTRIES):
        raise AssertionError(
            "%s listed ids %r, expected %r"
            % (label, sorted(entries), sorted(POOL_ENTRIES))
        )
    for vm_id, fixture in POOL_ENTRIES.items():
        entry = entries[vm_id]
        for field in ("id", "name", "path", "toml"):
            if field not in entry:
                raise AssertionError("%s entry %d had no %s: %r" % (label, vm_id, field, entry))
        expected_toml = fixture_text(fixture)
        if entry["toml"] != expected_toml:
            raise AssertionError(
                "%s entry %d returned a TOML body that differs from %s"
                % (label, vm_id, fixture)
            )
        if not isinstance(entry["name"], str) or not entry["name"]:
            raise AssertionError("%s entry %d had no name: %r" % (label, vm_id, entry))
    print(
        "  pool http probe: %s -> ids %r with TOML verbatim from sh/"
        % (label, sorted(entries))
    )


def check_pool_issue(label, body):
    """Assert the unusable fixture is reported with its kind and path."""
    issues = [
        issue
        for issue in body["issues"]
        if isinstance(issue, dict) and issue.get("path", "").endswith(BROKEN_ENTRY)
    ]
    if len(issues) != 1:
        raise AssertionError(
            "%s reported %d issues for %s, expected 1: %r"
            % (label, len(issues), BROKEN_ENTRY, body["issues"])
        )
    issue = issues[0]
    check("%s issue kind" % label, issue.get("kind"), "invalid-toml")
    if not isinstance(issue.get("detail"), str) or not issue["detail"]:
        raise AssertionError("%s issue had no detail text: %r" % (label, issue))
    print("  pool http probe: %s -> %s reported as %r" % (label, BROKEN_ENTRY, issue["kind"]))


def vm_status(body):
    if not isinstance(body, dict) or not isinstance(body.get("status"), str):
        raise AssertionError("VM detail response had no status: %r" % (body,))
    return body["status"]


def poll_vm_status(vm_id, expected):
    """Poll `GET /api/vms/{id}` until it reports `expected`.

    `start` returns once the request is accepted, so the running state is
    observed through the detail endpoint. A transition that never arrives fails
    on the poll deadline.
    """
    start = time.monotonic()
    last = None
    while True:
        if time.monotonic() - start > POLL_DEADLINE:
            raise AssertionError(
                "VM[%d] never reached %s within %.0fs (last %r)"
                % (vm_id, expected, POLL_DEADLINE, last)
            )
        status, body = get("/api/vms/%d" % vm_id, "VM[%d] detail" % vm_id)
        if status == 200:
            last = vm_status(body)
            if last == expected:
                print("  pool http probe: VM[%d] -> status %s" % (vm_id, expected))
                return body
        time.sleep(POLL_INTERVAL)


def check_guest_ran(vm_id):
    """Poll until the guest has entered the vCPU run loop at least once.

    `guest_entry_count` is incremented by the hypervisor only after a successful
    guest entry, so this distinguishes a started guest from a start that only
    flipped a status. It must be polled, not read once: `start` is accepted and
    the status flips to `running` before the vCPU task has entered the guest, so
    a single read can observe 0. A guest that never enters fails on the poll
    deadline.
    """
    start = time.monotonic()
    last = None
    while True:
        if time.monotonic() - start > POLL_DEADLINE:
            raise AssertionError(
                "VM[%d] guest_entry_count never reached 1 within %.0fs (last report %r)"
                % (vm_id, POLL_DEADLINE, last)
            )
        status, body = get("/api/vms/%d" % vm_id, "VM[%d] detail" % vm_id)
        if status == 200 and isinstance(body, dict):
            last = body.get("guest_entry_count")
            if isinstance(last, int) and last >= 1:
                print("  pool http probe: VM[%d] -> guest_entry_count %d" % (vm_id, last))
                return
        time.sleep(POLL_INTERVAL)


def start_vm(vm_id, expected_body_status="running"):
    """Start one pool entry through the control API and verify it runs."""
    try:
        status, body = request("POST", "/api/vms/%d/start" % vm_id)
        expect_status("POST /api/vms/%d/start" % vm_id, status, 200)
        check("POST /api/vms/%d/start status" % vm_id, vm_status(body), expected_body_status)
    except RuntimeError as error:
        # Creating the guest synchronously (kernel image read from the host
        # filesystem, VM build, vCPU start) can outlast the request timeout, so
        # a lost response does not mean the request was refused. Decide by the
        # resulting state instead: the VM has to appear and run, or the poll
        # below fails on its deadline.
        print("  pool http probe: POST /api/vms/%d/start -> no response (%s); checking state" % (vm_id, error))
    poll_vm_status(vm_id, "running")
    check_guest_ran(vm_id)


def close_vm(vm_id):
    """Close one guest: `delete` is the close action, not `stop`."""
    try:
        status, _ = request("DELETE", "/api/vms/%d" % vm_id)
        expect_status("DELETE /api/vms/%d" % vm_id, status, 204)
    except RuntimeError as error:
        print(
            "  pool http probe: DELETE /api/vms/%d -> no response (%s); checking state"
            % (vm_id, error)
        )
    # A lost response also covers the case where the delete was accepted: the
    # registry has to show the VM gone either way.
    poll_vm_gone(vm_id)


def poll_vm_gone(vm_id):
    """Poll `GET /api/vms/{id}` until it returns 404."""
    start = time.monotonic()
    last = None
    while True:
        if time.monotonic() - start > POLL_DEADLINE:
            raise AssertionError(
                "VM[%d] was still registered within %.0fs (last status %r)"
                % (vm_id, POLL_DEADLINE, last)
            )
        status, _ = get("/api/vms/%d" % vm_id, "VM[%d] detail" % vm_id)
        if status == 404:
            print("  pool http probe: GET /api/vms/%d after delete -> 404" % vm_id)
            return
        last = status
        time.sleep(POLL_INTERVAL)


def list_report(label):
    """Fetch `GET /api/vms` and return {id: status} for the registered VMs."""
    status, body = get("/api/vms", label)
    expect_status(label, status, 200)
    if not isinstance(body, list):
        raise AssertionError("%s did not return a JSON array: %r" % (label, body))
    report = {}
    for item in body:
        if not isinstance(item, dict):
            raise AssertionError("%s listed a non-object entry: %r" % (label, item))
        report[item.get("id")] = item.get("status")
    print("  pool http probe: %s -> %r" % (label, report))
    return report


def main():
    poll_ready()

    # Nothing in the pool is created at startup: the registry is empty and the
    # pool is only a list of candidates.
    check("VM registry at startup", list_report("GET /api/vms"), {})

    pool = pool_body("GET /api/vms/pool")
    check("pool directory", pool["directory"], "/usr/bin")
    check_pool_matches_fixtures("GET /api/vms/pool", pool)
    check_pool_issue("GET /api/vms/pool", pool)

    # An id that is neither registered nor pooled is unknown, not created.
    status, _ = request("POST", "/api/vms/99/start")
    expect_status("POST /api/vms/99/start", status, 404)

    # Starting a pooled id creates it from its entry and boots it. The second
    # call hits the running VM and reports the lifecycle conflict.
    start_vm(1)
    status, _ = request("POST", "/api/vms/1/start")
    expect_status("POST /api/vms/1/start (running)", status, 409)

    # A second and third candidate start beside the first one.
    start_vm(2)
    start_vm(3)
    check(
        "three pooled guests running",
        list_report("GET /api/vms"),
        {1: "running", 2: "running", 3: "running"},
    )

    # Starting is not consuming: every entry is still a candidate.
    body = pool_body("GET /api/vms/pool after starts")
    check_pool_matches_fixtures("GET /api/vms/pool after starts", body)

    # Close one guest and start it again from the same entry: this is the
    # repeated start/close cycle the pool exists for.
    close_vm(2)
    check(
        "registry after closing one guest",
        list_report("GET /api/vms after close"),
        {1: "running", 3: "running"},
    )
    body = pool_body("GET /api/vms/pool after close")
    check_pool_matches_fixtures("GET /api/vms/pool after close", body)
    start_vm(2)

    # Close the rest and confirm the registry empties while the pool survives.
    close_vm(1)
    close_vm(3)
    close_vm(2)
    check("VM registry after closing all guests", list_report("GET /api/vms after close all"), {})
    body = pool_body("GET /api/vms/pool at the end")
    check_pool_matches_fixtures("GET /api/vms/pool at the end", body)

    print("  pool http probe: PASS")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as exc:
        print("  pool http probe: FAILED: %s" % exc, file=sys.stderr)
        sys.exit(1)
    except Exception as exc:
        print("  pool http probe: ERROR: %s" % exc, file=sys.stderr)
        sys.exit(2)
