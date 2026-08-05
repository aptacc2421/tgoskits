//! VM status and lifecycle axum handlers.
//!
//! JSON is built with `serde_json::json!()` (no hand-written escaping). These
//! handlers are shared by the TCP serving path in [`super::server`] and the
//! `http-test` built-in self-test, so the self-test exercises exactly the same
//! logic the network path dispatches to.

use axum::{Json, extract::Path, http::StatusCode};
use axvm::{AxVMRef, AxVmError, VmVcpuState};
use serde_json::{Value, json};

use crate::manager::AxvmManager;

/// `GET /api/vms` — list all known VMs (summary form).
pub async fn list_vms() -> Json<Vec<Value>> {
    let items: Vec<Value> = AxvmManager::vm_list().iter().map(vm_json_summary).collect();
    Json(items)
}

/// `GET /api/vms/{id}` — detail for one VM, or 404 if unknown.
pub async fn vm_detail(Path(id_str): Path<String>) -> Result<Json<Value>, StatusCode> {
    let Ok(id) = id_str.parse::<usize>() else {
        return Err(StatusCode::NOT_FOUND);
    };
    match AxvmManager::vm_by_id(id) {
        Some(vm) => Ok(Json(vm_json(&vm, true))),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// `POST /api/vms/{id}/start` — start a VM.
pub async fn vm_start(Path(id_str): Path<String>) -> Result<Json<Value>, StatusCode> {
    vm_action(&id_str, VmAction::Start)
}

/// `POST /api/vms/{id}/stop` — request a VM stop.
///
/// `stop` has request semantics: it returns as soon as the request is accepted,
/// while the vCPU exits and the VM reaches `Stopped` asynchronously.
pub async fn vm_stop(Path(id_str): Path<String>) -> Result<Json<Value>, StatusCode> {
    vm_action(&id_str, VmAction::Stop)
}

/// A lifecycle action on a VM.
enum VmAction {
    Start,
    Stop,
}

/// Drive one lifecycle action, mapping host errors to HTTP status codes.
///
/// Unknown VMs yield 404, invalid lifecycle transitions yield 409, and host
/// resource exhaustion yields 503.
fn vm_action(id_str: &str, action: VmAction) -> Result<Json<Value>, StatusCode> {
    let Ok(id) = id_str.parse::<usize>() else {
        return Err(StatusCode::NOT_FOUND);
    };
    // No existence pre-check: an unknown VM surfaces as `VmNotFound` from the
    // action and maps to 404 below, keeping the check-then-act window closed.
    let result = match action {
        VmAction::Start => AxvmManager::start_vm(id),
        VmAction::Stop => AxvmManager::stop_vm(id),
    };
    match result {
        Ok(()) => Ok(Json(vm_action_json(id))),
        Err(error) => Err(map_axvm_error(error)),
    }
}

/// Report the VM status right after a lifecycle action was accepted.
fn vm_action_json(id: usize) -> Value {
    let status = AxvmManager::vm_by_id(id)
        .map(|vm| vm.status().as_str())
        .unwrap_or("unknown");
    json!({ "ok": true, "status": status })
}

/// Map an AxVM runtime error to an HTTP status code.
fn map_axvm_error(error: anyhow::Error) -> StatusCode {
    let cause = error.root_cause();
    match cause.downcast_ref::<AxVmError>() {
        // A lifecycle transition that the current state does not allow.
        Some(AxVmError::InvalidTransition { .. } | AxVmError::InvalidState { .. }) => {
            StatusCode::CONFLICT
        }
        // Host resources (memory, vCPU list, devices, ...) were unavailable.
        Some(AxVmError::OutOfMemory { .. } | AxVmError::ResourceUnavailable { .. }) => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        // Unknown VMs surface as `VmNotFound` from the action (there is no
        // existence pre-check), mapping to 404. Anything else is a host-side
        // fault.
        Some(AxVmError::VmNotFound { .. }) => StatusCode::NOT_FOUND,
        _ => {
            error!("management HTTP action failed: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn vm_json_summary(vm: &AxVMRef) -> Value {
    vm_json(vm, false)
}

fn vm_json(vm: &AxVMRef, with_vcpus: bool) -> Value {
    let memory_mb = vm
        .memory_regions()
        .iter()
        .fold(0usize, |acc, region| acc.saturating_add(region.size()))
        / (1024 * 1024);
    let mut json = json!({
        "id": vm.id(),
        "name": vm.name(),
        "status": vm.status().as_str(),
        "cpu_num": vm.vcpu_num(),
        "memory_mb": memory_mb,
    });
    if with_vcpus {
        let vcpus: Vec<Value> = vm
            .vcpu_snapshots()
            .iter()
            .map(|vcpu| {
                json!({
                    "id": vcpu.id,
                    "state": vcpu_state_str(vcpu.state),
                    "phys_cpu_set": vcpu.phys_cpu_set,
                })
            })
            .collect();
        json["vcpu_states"] = json!(vcpus);
    }
    json
}

fn vcpu_state_str(state: VmVcpuState) -> &'static str {
    match state {
        VmVcpuState::Invalid => "invalid",
        VmVcpuState::Created => "created",
        VmVcpuState::Free => "free",
        VmVcpuState::Ready => "ready",
        VmVcpuState::Running => "running",
        VmVcpuState::Blocked => "blocked",
        VmVcpuState::Starting => "starting",
    }
}
