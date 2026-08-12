#!/bin/sh
# Host-side probe for the axum read-only management API
# (qemu-http-axum-readonly). Verifies the security boundary + read contract:
#   GET  /api/vms           -> 200  (readiness; server reachable)
#   POST /api/vms/999/start -> 401  (no token: access-denied regression)
#   GET  /api/vms/999       -> 404  (unknown VM on the read path)
#   POST /api/vms/999/start -> 404  (with token: auth passes, VM unknown)
# The runner injects AXBUILD_PROBE_PORT (forwarded host port) and
# AXVM_HTTP_TOKEN (the build's baked bearer token). Exit code is the verdict:
# 0 = pass, non-zero = fail. Contract mirrors
# os/axvisor/doc/http-control-plane-quickstart.md §2.3.
set -eu

BASE="http://127.0.0.1:${AXBUILD_PROBE_PORT:?probe port not set}"
TOKEN="${AXVM_HTTP_TOKEN:-}"
AUTH="Authorization: Bearer ${TOKEN}"
DEADLINE="${AXBUILD_PROBE_DEADLINE:-120}"

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

# Readiness: retry GET /api/vms until the server answers with a real status.
end=$(( $(date +%s) + DEADLINE ))
while :; do
    code=$(http_code GET /api/vms)
    [ "$code" != "000" ] && break
    [ "$(date +%s)" -ge "$end" ] && fail "guest HTTP server never became reachable within ${DEADLINE}s"
    sleep 1
done
assert_code "GET /api/vms" "$code" 200

# Access-denied regression (security review): an unauthenticated write to a
# mutating route is rejected with 401. The auth gate runs before any VM lookup,
# so this holds regardless of whether VM 999 exists.
assert_code "POST /api/vms/999/start (no token)" \
    "$(http_code POST /api/vms/999/start)" 401

# Unknown VM -> 404 on the read path.
assert_code "GET /api/vms/999" \
    "$(http_code GET /api/vms/999)" 404

# Authenticated write to an unknown VM -> 404: writes are reachable with valid
# credentials rather than silently open or always denied.
assert_code "POST /api/vms/999/start (with token)" \
    "$(http_code POST /api/vms/999/start -H "$AUTH")" 404

echo "  host http probe: passed"
