//! `GET /api/manifest` — what this hypervisor build can serve.
//!
//! The browser frontend builds its navigation from this response: a panel that
//! is not declared here has no UI, and every declared panel must be backed by
//! routes that this same build registered. The declaration is therefore derived
//! from the build's features rather than from the listener configuration, so a
//! `http-axum`-only build advertises VM management but not terminals, and a
//! `browser-console`-only build advertises the reverse.
//!
//! | `kind`    | Backing routes                                | Declared by        |
//! | --------- | --------------------------------------------- | ------------------ |
//! | `vms`     | `GET /api/vms`, `GET /api/vms/pool`           | `http-axum`        |
//! | `console` | `GET /api/consoles`, `GET /ws/vm-{id}`        | `browser-console`  |
//! | `shell`   | `GET /ws/axvisor`                             | `browser-console`  |
//!
//! `verbs` are the operations a panel may use: `read` (list/detail), `write`
//! (create, start, stop, delete), `stream` (live output on a WebSocket). VM
//! verbs are not narrowed per VM status — `GET /api/vms/{id}` reports the status
//! and the control plane rejects a transition the state machine does not allow,
//! so a panel disables actions from the status it already has.

use axum::Json;
use serde_json::{Value, json};

/// Control-plane contract version. Bumped when the response shape changes in a
/// way an older frontend cannot ignore.
const PROTO: u8 = 1;

pub(super) async fn get_manifest() -> Json<Value> {
    let panels = build_panels();
    Json(json!({
        "proto": PROTO,
        "panels": panels,
    }))
}

fn build_panels() -> Vec<Value> {
    let mut panels = Vec::new();

    #[cfg(feature = "http-axum")]
    panels.push(json!({
        "kind": "vms",
        "title": "虚拟机",
        "verbs": ["read", "write"],
    }));

    #[cfg(feature = "browser-console")]
    {
        panels.push(json!({
            "kind": "console",
            "title": "客户机终端",
            "verbs": ["read", "write", "stream"],
        }));
        panels.push(json!({
            "kind": "shell",
            "title": "管理终端",
            "verbs": ["read", "write", "stream"],
        }));
    }

    panels
}
