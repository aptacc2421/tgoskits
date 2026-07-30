#!/usr/bin/env bash
# Regression tests for quick-start.sh x86_64 image name migration.
#
# Verifies that the x86_64/NimbOS/UEFI setup functions use the correct
# image names (qemu-x86_64) and paths (tmp/images/qemu-x86_64/) from
# the current registry, rather than the old underscore-based layout.
#
# Usage:
#   bash os/axvisor/scripts/test_quick_start_x86_64.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
QUICK_START="${SCRIPT_DIR}/quick-start.sh"

if [ ! -f "${QUICK_START}" ]; then
  echo "ERROR: quick-start.sh not found at ${QUICK_START}" >&2
  exit 1
fi

PASS=0
FAIL=0

assert_no_old_image_names() {
  local desc="$1" func_name="$2"
  local func_body

  # Extract the function body from quick-start.sh
  func_body="$(sed -n "/^${func_name}() {/,/^}/p" "${QUICK_START}")"

  if echo "${func_body}" | grep -q 'qemu_x86_64_nimbos'; then
    echo "  FAIL: ${desc} — old image name 'qemu_x86_64_nimbos' found"
    FAIL=$((FAIL + 1))
  else
    echo "  PASS: ${desc} — no old image names"
    PASS=$((PASS + 1))
  fi

  if echo "${func_body}" | grep -q 'cargo axvisor image pull'; then
    echo "  FAIL: ${desc} — old command 'cargo axvisor image pull' found"
    FAIL=$((FAIL + 1))
  else
    echo "  PASS: ${desc} — no old pull commands"
    PASS=$((PASS + 1))
  fi
}

assert_uses_new_image_name() {
  local desc="$1" func_name="$2"
  local func_body

  func_body="$(sed -n "/^${func_name}() {/,/^}/p" "${QUICK_START}")"

  if echo "${func_body}" | grep -q 'cargo xtask image pull qemu-x86_64'; then
    echo "  PASS: ${desc} — uses 'cargo xtask image pull qemu-x86_64'"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: ${desc} — missing expected pull command"
    FAIL=$((FAIL + 1))
  fi
}

# ---------------------------------------------------------------------------
# Test sourcing works (proves source guard is in place)
# ---------------------------------------------------------------------------
test_sourcing_loads_functions() {
  echo ""
  echo "=== Test: sourcing quick-start.sh loads functions ==="
  local rc=0
  (
    _saved_opts="$(set +o)"
    set +euo pipefail
    source "${QUICK_START}"
    eval "${_saved_opts}"
    declare -F setup_qemu_x86_64 > /dev/null 2>&1 || exit 1
  ) || rc=$?
  if [ "${rc}" -eq 0 ]; then
    echo "  PASS: functions loadable via source"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: functions not loadable via source (guard may be broken)"
    FAIL=$((FAIL + 1))
  fi
}

# ---------------------------------------------------------------------------
# Test: setup_qemu_x86_64 uses new image names and paths
# ---------------------------------------------------------------------------
test_setup_qemu_x86_64_image_names() {
  echo ""
  echo "=== Test: setup_qemu_x86_64 uses correct image names ==="
  assert_no_old_image_names "setup_qemu_x86_64" "setup_qemu_x86_64"
  assert_uses_new_image_name "setup_qemu_x86_64" "setup_qemu_x86_64"
}

# ---------------------------------------------------------------------------
# Test: setup_qemu_x86_64_uefi uses new image names and paths
# ---------------------------------------------------------------------------
test_setup_qemu_x86_64_uefi_image_names() {
  echo ""
  echo "=== Test: setup_qemu_x86_64_uefi uses correct image names ==="
  assert_no_old_image_names "setup_qemu_x86_64_uefi" "setup_qemu_x86_64_uefi"
}

# ---------------------------------------------------------------------------
# Run all tests
# ---------------------------------------------------------------------------
echo "=== quick-start.sh x86_64 image name regression tests ==="
echo "Source: ${QUICK_START}"

test_sourcing_loads_functions
test_setup_qemu_x86_64_image_names
test_setup_qemu_x86_64_uefi_image_names

echo ""
echo "=== Results: ${PASS} passed, ${FAIL} failed ==="

if [ "${FAIL}" -gt 0 ]; then
  echo "FAILURE: ${FAIL} test(s) failed." >&2
  exit 1
fi

echo "All tests passed."
exit 0
