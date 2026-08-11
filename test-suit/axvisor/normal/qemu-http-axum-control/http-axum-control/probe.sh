#!/bin/sh
# Host-side probe for the axum lifecycle control API
# (qemu-http-axum-control). Drives the default VM (id 1, linux-smp1.toml, kept
# `Ready` by `no-auto-start`) through the start/stop contract:
#   GET  /api/vms            -> 200   (readiness)
#   poll GET /api/vms/1 until status == "ready"
#   POST /api/vms/1/start    -> 200   (async:false, running)
#   poll GET /api/vms/1 until status == "running"
#   POST /api/vms/1/stop     -> 200   (idempotent; async request)
#   poll GET /api/vms/1 until status == "stopped"
#   POST /api/vms/1/start    -> 409   (restart-after-stop rejected)
# The runner injects AXBUILD_PROBE_PORT (forwarded host port) and
# AXVM_HTTP_TOKEN (the build's baked bearer token). Exit code is the verdict:
# 0 = pass, non-zero = fail. Contract mirrors
# os/axvisor/doc/http-control-plane-quickstart.md §2.3.
set -eu

BASE="http://127.0.0.1:${AXBUILD_PROBE_PORT:?probe port not set}"
TOKEN="${AXVM_HTTP_TOKEN:-}"
AUTH="Authorization: Bearer ${TOKEN}"
DEADLINE="${AXBUILD_PROBE_DEADLINE:-120}"
VM_ID=1

fail() {
    echo "  host http probe: FAIL: $1" >&2
    exit 1
}

# http_code <method> <path> [curl args...]: print the HTTP status (000 on a
# connection failure, e.g. before the guest server is reachable).
http_code() {
    method=$1
    path=$2
    shift 2
    out=$(curl -s -o /dev/null -w '%{http_code}' -X "$method" "${@}" "$BASE$path" 2>/dev/null || true)
    printf '%s' "${out:-000}"
}

# assert_code <label> <actual> <expected>
assert_code() {
    label=$1
    actual=$2
    expected=$3
    echo "  host http probe: $label -> $actual (expect $expected)"
    [ "$actual" = "$expected" ] || fail "$label -> $actual, expected $expected"
}

# vm_state <id>: print the JSON `status` field of VM <id> (empty on a transport
# failure or a non-200 response).
vm_state() {
    id=$1
    body=$(curl -s -X GET "$BASE/api/vms/$id" 2>/dev/null || true)
    printf '%s' "$body" | sed -n 's/.*"status"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'
}

# poll_vm_state <id> <expected> <deadline_secs>: retry GET /api/vms/<id> until
# its status string equals <expected> or the deadline elapses.
poll_vm_state() {
    id=$1
    expected=$2
    deadline=$3
    end=$(( $(date +%s) + deadline ))
    while :; do
        state=$(vm_state "$id")
        [ "$state" = "$expected" ] && { echo "  host http probe: VM[$id] -> $expected"; return 0; }
        [ "$(date +%s)" -ge "$end" ] && fail "VM[$id] never became $expected within ${deadline}s (last: $state)"
        sleep 1
    done
}

# Readiness, and confirm the target VM exists in `Ready` (a `no-auto-start`
# build keeps default VMs un-started).
end=$(( $(date +%s) + DEADLINE ))
while :; do
    code=$(http_code GET /api/vms)
    [ "$code" != "000" ] && break
    [ "$(date +%s)" -ge "$end" ] && fail "guest HTTP server never became reachable within ${DEADLINE}s"
    sleep 1
done
assert_code "GET /api/vms" "$code" 200
poll_vm_state "$VM_ID" ready "$DEADLINE"

# Start, then poll until the vCPU task reports `running`.
assert_code "POST /api/vms/$VM_ID/start" \
    "$(http_code POST /api/vms/$VM_ID/start -H "$AUTH")" 200
poll_vm_state "$VM_ID" running "$DEADLINE"

# Stop is a request: the `Stopped` state arrives asynchronously once the vCPU
# observes it and exits.
assert_code "POST /api/vms/$VM_ID/stop" \
    "$(http_code POST /api/vms/$VM_ID/stop -H "$AUTH")" 200
poll_vm_state "$VM_ID" stopped "$DEADLINE"

# Restart-after-stop is a known scheduling limitation (a stopped VM's new vCPU
# task is not scheduled on its already-idle fixed core); the contract rejects it
# with 409 rather than hanging the VM in `running`.
assert_code "POST /api/vms/$VM_ID/start (restart-after-stop)" \
    "$(http_code POST /api/vms/$VM_ID/start -H "$AUTH")" 409

echo "  host http probe: passed"
