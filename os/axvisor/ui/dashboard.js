"use strict";

const tbody = document.getElementById("vm-tbody");
const errorBar = document.getElementById("error-bar");
const refreshBtn = document.getElementById("refresh-btn");
const tokenInput = document.getElementById("token-input");

// The mutating lifecycle routes require `Authorization: Bearer <token>`; the
// operator pastes the build-time token here once and it is held in
// sessionStorage (not persisted across browser sessions). GET routes stay open.
tokenInput.value = sessionStorage.getItem("axvisor.token") || "";
tokenInput.addEventListener("input", () => {
  sessionStorage.setItem("axvisor.token", tokenInput.value.trim());
});

function getToken() {
  const token = tokenInput.value.trim();
  return token ? `Bearer ${token}` : "";
}

// A VM can only be started from `ready`; restart-after-stop is unsupported by
// the control API (it rejects with 409), so the button stays disabled on a
// stopped VM rather than surfacing a confusing conflict.
function canStart(status) {
  return status === "ready";
}

// Stop is a request on running/paused VMs; from `ready` it transitions straight
// to `stopped` (both accepted). Idempotent on an already-stopped VM, but the
// button is disabled there so the dashboard does not suggest a no-op.
function canStop(status) {
  return ["ready", "running", "pausing", "paused"].includes(status);
}

// Pause is only valid from `running`; resume only from `paused`.
function canPause(status) {
  return status === "running";
}

function canResume(status) {
  return status === "paused";
}

function showError(msg) {
  errorBar.textContent = msg;
  errorBar.hidden = false;
}

function clearError() {
  errorBar.hidden = true;
  errorBar.textContent = "";
}

function esc(text) {
  const div = document.createElement("div");
  div.textContent = String(text);
  return div.innerHTML;
}

function statusCell(status) {
  const safe = /^[a-z-]+$/.test(status) ? status : "unknown";
  return `<span class="status-dot status-${safe}"></span>${esc(status)}`;
}

function renderTable(vms) {
  tbody.innerHTML = "";

  if (!Array.isArray(vms) || vms.length === 0) {
    const tr = document.createElement("tr");
    tr.className = "empty-row";
    tr.innerHTML = '<td colspan="6">没有 VM</td>';
    tbody.appendChild(tr);
    return;
  }

  for (const vm of vms) {
    const tr = document.createElement("tr");

    const idTd = document.createElement("td");
    idTd.textContent = vm.id;

    const nameTd = document.createElement("td");
    nameTd.textContent = vm.name;

    const statusTd = document.createElement("td");
    statusTd.className = "status-cell";
    statusTd.innerHTML = statusCell(vm.status);

    const cpuTd = document.createElement("td");
    cpuTd.textContent = vm.cpu_num;

    const memTd = document.createElement("td");
    memTd.textContent = vm.memory_mb;

    const actionsTd = document.createElement("td");

    const startBtn = document.createElement("button");
    startBtn.className = "btn btn-start";
    startBtn.textContent = "Start";
    startBtn.disabled = !canStart(vm.status);
    startBtn.addEventListener("click", () => postAction(vm.id, "start"));

    const stopBtn = document.createElement("button");
    stopBtn.className = "btn btn-stop";
    stopBtn.textContent = "Stop";
    stopBtn.disabled = !canStop(vm.status);
    stopBtn.addEventListener("click", () => postAction(vm.id, "stop"));

    const pauseBtn = document.createElement("button");
    pauseBtn.className = "btn btn-pause";
    pauseBtn.textContent = "Pause";
    pauseBtn.disabled = !canPause(vm.status);
    pauseBtn.addEventListener("click", () => postAction(vm.id, "pause"));

    const resumeBtn = document.createElement("button");
    resumeBtn.className = "btn btn-resume";
    resumeBtn.textContent = "Resume";
    resumeBtn.disabled = !canResume(vm.status);
    resumeBtn.addEventListener("click", () => postAction(vm.id, "resume"));

    actionsTd.appendChild(startBtn);
    actionsTd.appendChild(stopBtn);
    actionsTd.appendChild(pauseBtn);
    actionsTd.appendChild(resumeBtn);

    tr.appendChild(idTd);
    tr.appendChild(nameTd);
    tr.appendChild(statusTd);
    tr.appendChild(cpuTd);
    tr.appendChild(memTd);
    tr.appendChild(actionsTd);

    tbody.appendChild(tr);
  }
}

async function loadVms() {
  clearError();
  try {
    const res = await fetch("/api/vms");
    if (!res.ok) {
      throw new Error(`GET /api/vms -> ${res.status}`);
    }
    renderTable(await res.json());
  } catch (err) {
    showError("加载 VM 列表失败: " + err.message);
  }
}

async function postAction(id, action) {
  clearError();
  const auth = getToken();
  if (!auth) {
    showError("请先在顶栏输入 bearer token（构建时 [env] AXVM_HTTP_TOKEN）");
    return;
  }
  try {
    const res = await fetch(`/api/vms/${id}/${action}`, {
      method: "POST",
      headers: { Authorization: auth },
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      const statusText = data.status ? ` (${data.status})` : "";
      throw new Error(`${action} VM ${id} -> ${res.status}${statusText}`);
    }
    // stop/pause are requests: the VM reaches the target state asynchronously,
    // so re-poll the table after the action is accepted.
    await loadVms();
  } catch (err) {
    showError(err.message);
    await loadVms();
  }
}

refreshBtn.addEventListener("click", loadVms);
document.addEventListener("DOMContentLoaded", loadVms);
