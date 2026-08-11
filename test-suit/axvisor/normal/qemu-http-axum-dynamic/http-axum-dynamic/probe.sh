#!/bin/sh
# Host-side probe for the axum dynamic create/delete API
# (qemu-http-axum-dynamic). Creates a VM from a host-side TOML config, verifies
# the duplicate-create conflict, then deletes it and polls it gone:
#   GET  /api/vms              -> 200   (readiness)
#   POST /api/vms/create       -> 200   {"id":N}   body: {"toml": "<config>"}
#   poll GET /api/vms/N until status == "ready"
#   POST /api/vms/create (dup) -> 409   (id already registered)
#   DELETE /api/vms/N          -> 204
#   poll GET /api/vms/N until 404       (gone)
# create only reproduces a guest image baked into the build (linux-smp1.toml,
# fs-backed kernel on the rootfs); a new id with no embedded image fails 500.
# CONFIG_TOML is workspace-relative; the runner's CWD is the workspace root.
# The runner injects AXBUILD_PROBE_PORT (forwarded host port) and AXVM_HTTP_TOKEN
# (the build's baked bearer token). Exit code is the verdict: 0 = pass, non-zero
# = fail. Contract mirrors os/axvisor/doc/http-control-plane-quickstart.md §2.3.
set -eu

BASE="http://127.0.0.1:${AXBUILD_PROBE_PORT:?probe port not set}"
TOKEN="${AXVM_HTTP_TOKEN:-}"
AUTH="Authorization: Bearer ${TOKEN}"
DEADLINE="${AXBUILD_PROBE_DEADLINE:-120}"
CONFIG_TOML="${CONFIG_TOML:-os/axvisor/configs/vms/qemu/aarch64/linux-smp1.toml}"

fail() {
    echo "  host http probe: FAIL: $1" >&2
    exit 1
}

[ -f "$CONFIG_TOML" ] || fail "VM config TOML not found: $CONFIG_TOML (run from the workspace root)"

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

# _call <method> <path> [curl args...]: one request storing the status in
# CURL_CODE and the response body in CURL_BODY.
_call() {
    method=$1
    path=$2
    shift 2
    tmp=$(mktemp)
    code=$(curl -s -o "$tmp" -w '%{http_code}' -X "$method" "${@}" "$BASE$path" 2>/dev/null || true)
    CURL_CODE=${code:-000}
    CURL_BODY=$(cat "$tmp" 2>/dev/null || true)
    rm -f "$tmp"
}

# json_field <body> <field>: print a top-level string/number field of a
# single-line JSON object, without jq.
json_field() {
    body=$1
    field=$2
    printf '%s' "$body" | sed -nE "s/.*\"$field\"[[:space:]]*:[[:space:]]*\"?([^\",}]*)\"?.*/\1/p"
}

# vm_state <id>: print the JSON `status` field of VM <id> (empty on a transport
# failure or a non-200 response).
vm_state() {
    id=$1
    body=$(curl -s -X GET "$BASE/api/vms/$id" 2>/dev/null || true)
    printf '%s' "$body" | sed -n 's/.*"status"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'
}

# poll_vm_state <id> <expected> <deadline_secs>
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

# poll_vm_gone <id> <deadline_secs>: retry GET /api/vms/<id> until it 404s.
poll_vm_gone() {
    id=$1
    deadline=$2
    end=$(( $(date +%s) + deadline ))
    while :; do
        code=$(http_code GET /api/vms/$id)
        [ "$code" = "404" ] && { echo "  host http probe: VM[$id] -> gone"; return 0; }
        [ "$(date +%s)" -ge "$end" ] && fail "VM[$id] never disappeared within ${deadline}s (last: $code)"
        sleep 1
    done
}

# Readiness.
end=$(( $(date +%s) + DEADLINE ))
while :; do
    code=$(http_code GET /api/vms)
    [ "$code" != "000" ] && break
    [ "$(date +%s)" -ge "$end" ] && fail "guest HTTP server never became reachable within ${DEADLINE}s"
    sleep 1
done
assert_code "GET /api/vms" "$code" 200

# Create body: the config TOML sent verbatim, escaped for a JSON string.
# The TOML is multi-line, so after escaping backslashes/quotes each line is
# terminated with a literal `\n` (JSON cannot hold a raw newline). awk output
# reassembles the whole file into a single logical line.
toml_escaped=$(
    sed 's/\\/\\\\/g; s/"/\\"/g' "$CONFIG_TOML" |
    awk '{ gsub("\r", "\\r"); gsub("\t", "\\t"); printf "%s\\n", $0 }'
)
create_body="{\"toml\": \"$toml_escaped\"}"

_call POST /api/vms/create -H "$AUTH" -H 'Content-Type: application/json' -d "$create_body"
assert_code "POST /api/vms/create" "$CURL_CODE" 200
id=$(json_field "$CURL_BODY" id)
[ -n "$id" ] || fail "create response missing id: $CURL_BODY"
echo "  host http probe: created VM[$id]"

# Poll the new VM into `Ready`.
poll_vm_state "$id" ready "$DEADLINE"

# Re-creating the same config must conflict (409): the id is registered.
_call POST /api/vms/create -H "$AUTH" -H 'Content-Type: application/json' -d "$create_body"
assert_code "POST /api/vms/create (duplicate)" "$CURL_CODE" 409

# Delete, then poll until the VM is gone (404).
_call DELETE /api/vms/$id -H "$AUTH"
assert_code "DELETE /api/vms/$id" "$CURL_CODE" 204
poll_vm_gone "$id" "$DEADLINE"

echo "  host http probe: passed"
