//! Read-only VM status axum handlers.
//!
//! JSON is built with `serde_json::json!()` (no hand-written escaping). These
//! handlers are shared by the TCP serving path in [`super::server`] and the
//! `http-test` built-in self-test, so the self-test exercises exactly the same
//! logic the network path dispatches to.

use axum::{Json, extract::Path, http::StatusCode};
use axvm::{AxVMRef, VmVcpuState};
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
