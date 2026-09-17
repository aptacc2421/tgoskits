// Copyright 2025 The Axvisor Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Candidate guest configs that a start request may create on demand.
//!
//! The pool is an Axvisor-owned directory of guest config TOML files, read from
//! the build-config variable `AXVISOR_VM_POOL` when it is set and from
//! [`DEFAULT_POOL_DIR`] otherwise. A pool entry is only a candidate: nothing in
//! this directory is created at startup, so an idle pool costs no guest memory
//! and may list more guests than the machine can run at once. This is the
//! difference from [`DEFAULT_VM_CONFIG_DIR`], whose configs the startup path
//! creates immediately as the default guest set.
//!
//! The directory is the single authority for the candidate set: [`scan`] reads
//! it on every call and keeps no cache, so a config that the host adds,
//! replaces or repairs is visible to the next query without a reboot. Files
//! that cannot be used become [`Issue`] values instead of disappearing, so the
//! shell and the control plane can report why a file in the pool does nothing.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use axvmconfig::GuestConfig;

/// Directory the pool is read from when the build config does not override it.
pub const DEFAULT_POOL_DIR: &str = "/guest/vm_pool";

/// Directory of configs that the startup path creates as the default guest set.
pub const DEFAULT_VM_CONFIG_DIR: &str = "/guest/vm_default";

/// Pool directory: `[env] AXVISOR_VM_POOL` wins over [`DEFAULT_POOL_DIR`].
pub fn directory() -> &'static str {
    option_env!("AXVISOR_VM_POOL").unwrap_or(DEFAULT_POOL_DIR)
}

/// One pool config that a start request can turn into a running VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    path: String,
    id: usize,
    name: String,
    toml: String,
}

impl Entry {
    /// Path of the config file, as reported by the filesystem.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// `base.id` the config asks for, which is also the runtime VM id.
    pub fn id(&self) -> usize {
        self.id
    }

    /// `base.name` from the config.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Raw TOML text, for callers that create the VM themselves.
    pub fn toml(&self) -> &str {
        &self.toml
    }
}

/// Why one pool file cannot be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    /// The pool directory is missing or cannot be opened; the pool is empty.
    DirectoryUnavailable(String),
    /// The file exists but could not be read as UTF-8 text.
    Unreadable(String),
    /// The file holds no text at all.
    Empty,
    /// The file is not a guest config TOML document.
    InvalidToml(String),
    /// An earlier file in the same scan already claims this `base.id`, so this
    /// config cannot be registered while that one is in the pool.
    DuplicateId { id: usize, claimed_by: String },
}

impl core::fmt::Display for IssueKind {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DirectoryUnavailable(error) => {
                write!(formatter, "directory is unavailable: {error}")
            }
            Self::Unreadable(error) => write!(formatter, "cannot be read: {error}"),
            Self::Empty => formatter.write_str("is empty"),
            Self::InvalidToml(error) => write!(formatter, "is not a guest config: {error}"),
            Self::DuplicateId { id, claimed_by } => write!(
                formatter,
                "asks for VM id {id}, which `{claimed_by}` already claims in this pool"
            ),
        }
    }
}

/// One unusable pool file, or the pool directory itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    path: String,
    kind: IssueKind,
}

impl Issue {
    /// What is wrong with the reported path.
    pub fn kind(&self) -> &IssueKind {
        &self.kind
    }
}

impl core::fmt::Display for Issue {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}: {}", self.path, self.kind)
    }
}

/// Result of one pool scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    directory: String,
    entries: Vec<Entry>,
    issues: Vec<Issue>,
}

impl Pool {
    /// Directory this scan read.
    pub fn directory(&self) -> &str {
        &self.directory
    }

    /// Startable configs, in the order the filesystem reported them.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Files that could not become an entry, plus the directory itself when it
    /// could not be opened.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// The entry that claims `id`, if the pool holds a usable config for it.
    pub fn entry(&self, id: usize) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

/// Scan the pool directory reported by [`directory`].
pub fn scan() -> Pool {
    scan_dir(directory())
}

/// Scan one directory of guest configs.
///
/// A missing or unreadable directory yields an empty pool with a single
/// [`IssueKind::DirectoryUnavailable`] issue rather than an error, so callers
/// can render "no pool provisioned" without special-casing failures.
pub fn scan_dir(directory: &str) -> Pool {
    let mut entries = Vec::new();
    let mut issues = Vec::new();

    let read_dir = match ax_std::fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            issues.push(Issue {
                path: directory.to_string(),
                kind: IssueKind::DirectoryUnavailable(error.to_string()),
            });
            return Pool {
                directory: directory.to_string(),
                entries,
                issues,
            };
        }
    };

    for dir_entry in read_dir {
        let path = match dir_entry {
            Ok(dir_entry) => dir_entry.path(),
            Err(error) => {
                issues.push(Issue {
                    path: directory.to_string(),
                    kind: IssueKind::Unreadable(format!("directory entry: {error}")),
                });
                continue;
            }
        };
        if !path.ends_with(".toml") {
            continue;
        }

        let entry = match parse_entry(&path) {
            Ok(entry) => entry,
            Err(kind) => {
                issues.push(Issue { path, kind });
                continue;
            }
        };
        match entries.iter().find(|known| known.id == entry.id) {
            Some(known) => issues.push(Issue {
                path: entry.path,
                kind: IssueKind::DuplicateId {
                    id: entry.id,
                    claimed_by: known.path.clone(),
                },
            }),
            None => entries.push(entry),
        }
    }

    Pool {
        directory: directory.to_string(),
        entries,
        issues,
    }
}

/// Report the pool contents through the host log.
///
/// The startup path only reports the pool: creating a candidate is the start
/// request's job. Reporting here makes a broken pool visible in the serial log
/// before anything asks for it.
pub fn log_startup_state() {
    let pool = scan();
    info!(
        "VM pool `{}`: {} config(s)",
        pool.directory(),
        pool.entries().len()
    );
    for entry in pool.entries() {
        info!(
            "  VM pool entry: VM[{}] `{}` from {}",
            entry.id(),
            entry.name(),
            entry.path()
        );
    }
    log_issues(&pool);
}

/// Log every issue of `pool` through the host log.
///
/// An unprovisioned pool is a normal state and stays at info level; a file that
/// cannot be started is a warning.
pub fn log_issues(pool: &Pool) {
    for issue in pool.issues() {
        match issue.kind() {
            IssueKind::DirectoryUnavailable(_) => info!("VM pool: {issue}"),
            _ => warn!("VM pool: {issue}"),
        }
    }
}

fn parse_entry(path: &str) -> Result<Entry, IssueKind> {
    let toml = ax_std::fs::read_to_string(path)
        .map_err(|error| IssueKind::Unreadable(error.to_string()))?;
    if toml.trim().is_empty() {
        return Err(IssueKind::Empty);
    }

    let config = GuestConfig::from_toml(&toml)
        .map_err(|error| IssueKind::InvalidToml(format!("{error}")))?;
    Ok(Entry {
        path: path.to_string(),
        id: config.base.id,
        name: config.base.name.clone(),
        toml,
    })
}
