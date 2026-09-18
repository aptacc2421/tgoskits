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
//! A config that parses but names a guest image the filesystem does not have is
//! one of those values: it is a pool file that cannot become a VM.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use ax_std::fs::FileTypeExt;
use axvmconfig::GuestConfig;

/// Directory the pool is read from when the build config does not override it.
pub const DEFAULT_POOL_DIR: &str = "/guest/vm_pool";

/// Directory of configs that the startup path creates as the default guest set.
pub const DEFAULT_VM_CONFIG_DIR: &str = "/guest/vm_default";

/// Pool directory: `[env] AXVISOR_VM_POOL` wins over [`DEFAULT_POOL_DIR`].
pub fn directory() -> &'static str {
    option_env!("AXVISOR_VM_POOL").unwrap_or(DEFAULT_POOL_DIR)
}

/// Every directory the pool is read from, in precedence order.
///
/// The drop-in directory comes first, so a config the operator puts there wins
/// over one of the same id from an extra directory; `AXVISOR_VM_DIRS` appends
/// `:`-separated directories for a host that keeps its configs elsewhere. The
/// startup directory is deliberately *not* part of this list: its configs are
/// already registered as the default guest set, so listing them again would
/// offer every default guest as a candidate for an id that is taken.
pub fn sources() -> Vec<String> {
    let mut sources: Vec<String> = Vec::new();
    let mut push = |directory: &str| {
        let directory = directory.trim();
        if !directory.is_empty() && !sources.iter().any(|known| known == directory) {
            sources.push(directory.to_string());
        }
    };
    push(directory());
    for extra in option_env!("AXVISOR_VM_DIRS").unwrap_or("").split(':') {
        push(extra);
    }
    sources
}

/// One pool config that a start request can turn into a running VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    path: String,
    source: String,
    id: usize,
    name: String,
    toml: String,
}

impl Entry {
    /// Path of the config file, as reported by the filesystem.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Directory this entry was read from.
    pub fn source(&self) -> &str {
        &self.source
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
    /// The config reads its guest images from the filesystem and names one that
    /// does not exist, so creating it would fail partway through.
    MissingImage(String),
}

impl IssueKind {
    /// Stable token naming the kind, for machine readers such as the control
    /// plane's JSON. The `Display` text is human-facing and may change; this
    /// token is the part clients may match on.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DirectoryUnavailable(_) => "directory-unavailable",
            Self::Unreadable(_) => "unreadable",
            Self::Empty => "empty",
            Self::InvalidToml(_) => "invalid-toml",
            Self::DuplicateId { .. } => "duplicate-id",
            Self::MissingImage(_) => "missing-image",
        }
    }
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
            Self::MissingImage(path) => {
                write!(formatter, "names an image that does not exist: {path}")
            }
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
    /// Path of the file that could not be used, or of the directory itself.
    pub fn path(&self) -> &str {
        &self.path
    }

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
    sources: Vec<String>,
    entries: Vec<Entry>,
    issues: Vec<Issue>,
}

impl Pool {
    /// First directory this scan read, the one a new config is written to.
    pub fn directory(&self) -> &str {
        &self.directory
    }

    /// Every directory this scan read, in precedence order.
    pub fn sources(&self) -> &[String] {
        &self.sources
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

/// Scan every directory reported by [`sources`].
pub fn scan() -> Pool {
    scan_dirs(&sources())
}

/// Scan one directory of guest configs.
///
/// A missing or unreadable directory yields an empty pool with a single
/// [`IssueKind::DirectoryUnavailable`] issue rather than an error, so callers
/// can render "no pool provisioned" without special-casing failures.
pub fn scan_dir(directory: &str) -> Pool {
    scan_dirs(&[directory.to_string()])
}

/// Scan `directories` in order.
///
/// The first directory that offers a given `base.id` wins and later copies are
/// reported as [`IssueKind::DuplicateId`], so a precedence order (see
/// [`sources`]) decides which of two files with the same id is startable.
/// Nothing is cached: a config the host adds, replaces or removes is visible to
/// the next scan.
pub fn scan_dirs(directories: &[String]) -> Pool {
    let mut entries: Vec<Entry> = Vec::new();
    let mut issues = Vec::new();

    for directory in directories {
        let read_dir = match ax_std::fs::read_dir(directory) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                issues.push(Issue {
                    path: directory.clone(),
                    kind: IssueKind::DirectoryUnavailable(error.to_string()),
                });
                continue;
            }
        };

        for dir_entry in read_dir {
            let path = match dir_entry {
                Ok(dir_entry) => dir_entry.path(),
                Err(error) => {
                    issues.push(Issue {
                        path: directory.clone(),
                        kind: IssueKind::Unreadable(format!("directory entry: {error}")),
                    });
                    continue;
                }
            };
            if !path.ends_with(".toml") {
                continue;
            }

            let entry = match parse_entry(&path, directory) {
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
    }

    Pool {
        directory: directories.first().cloned().unwrap_or_default(),
        sources: directories.to_vec(),
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
    ensure_directories();
    let pool = scan();
    info!(
        "VM pool: {} folder(s): {}",
        pool.sources().len(),
        pool.sources().join(", ")
    );
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

fn parse_entry(path: &str, source: &str) -> Result<Entry, IssueKind> {
    let toml = ax_std::fs::read_to_string(path)
        .map_err(|error| IssueKind::Unreadable(error.to_string()))?;
    if toml.trim().is_empty() {
        return Err(IssueKind::Empty);
    }

    let config = GuestConfig::from_toml(&toml)
        .map_err(|error| IssueKind::InvalidToml(format!("{error}")))?;
    // A config that names an image which is not there cannot become a VM, so it
    // is reported instead of listed as startable. The file may be provisioned
    // later; the next scan picks it up because nothing is cached.
    if let Some(issue) = unusable_image(&config) {
        return Err(issue);
    }
    Ok(Entry {
        path: path.to_string(),
        source: source.to_string(),
        id: config.base.id,
        name: config.base.name.clone(),
        toml,
    })
}

/// One entry of a browsed folder: a subdirectory or a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    path: String,
    parent: Option<String>,
    directories: Vec<Directory>,
    entries: Vec<Entry>,
    issues: Vec<Issue>,
}

/// One subdirectory that can be browsed into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory {
    name: String,
    path: String,
}

impl Directory {
    /// Last component of the path.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Path to browse into.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Folder {
    /// Directory that was read.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Directory one level up, unless this is the filesystem root.
    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    /// Subdirectories, by name.
    pub fn directories(&self) -> &[Directory] {
        &self.directories
    }

    /// Startable configs in this folder, by name.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// `.toml` files that cannot be started, plus the folder itself when it
    /// could not be read.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }
}

/// Browse one directory of the guest filesystem.
///
/// This is what lets the operator pick a config from anywhere instead of only
/// from the pool: every `.toml` is parsed on the spot, so the result already
/// says which files are startable and why the others are not. A directory that
/// cannot be read comes back as an empty [`Folder`] with a
/// [`IssueKind::DirectoryUnavailable`] issue, the same shape a pool scan uses.
pub fn browse(path: &str) -> Folder {
    let mut directories = Vec::new();
    let mut entries = Vec::new();
    let mut issues = Vec::new();

    match ax_std::fs::read_dir(path) {
        Ok(read_dir) => {
            for dir_entry in read_dir {
                let (entry_path, is_directory) = match dir_entry {
                    Ok(dir_entry) => (dir_entry.path(), dir_entry.file_type().is_dir()),
                    Err(error) => {
                        issues.push(Issue {
                            path: path.to_string(),
                            kind: IssueKind::Unreadable(format!("directory entry: {error}")),
                        });
                        continue;
                    }
                };
                let name = entry_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&entry_path)
                    .to_string();
                // `ax_std::fs::metadata` opens the path, which fails on a
                // directory, so the entry's own type is what decides whether
                // this is a folder to walk into or a file to try to parse.
                if is_directory {
                    directories.push(Directory {
                        name,
                        path: entry_path,
                    });
                } else if entry_path.ends_with(".toml") {
                    match parse_entry(&entry_path, path) {
                        Ok(entry) => entries.push(entry),
                        Err(kind) => issues.push(Issue {
                            path: entry_path,
                            kind,
                        }),
                    }
                }
            }
        }
        Err(error) => issues.push(Issue {
            path: path.to_string(),
            kind: IssueKind::DirectoryUnavailable(error.to_string()),
        }),
    }

    directories.sort_by(|left, right| left.name.cmp(&right.name));
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    issues.sort_by(|left, right| left.path.cmp(&right.path));

    let parent = {
        let trimmed = path.trim_end_matches('/');
        match trimmed.rsplit_once('/') {
            // `/guest` and `/guest/` both sit directly under the root.
            Some(("", _)) => Some("/".to_string()),
            Some((parent, _)) => Some(parent.to_string()),
            // No separator at all: a relative single component, or the root.
            None => None,
        }
    };

    Folder {
        path: path.to_string(),
        parent,
        directories,
        entries,
        issues,
    }
}

/// Why a config could not be stored in the pool directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The name is not a plain `*.toml` file name.
    InvalidName(String),
    /// The text is not a guest config TOML document.
    InvalidToml(String),
    /// The pool directory or the file could not be written.
    Unwritable(String),
}

impl core::fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidName(name) => write!(
                formatter,
                "`{name}` is not a file name (a plain name ending in .toml is required)"
            ),
            Self::InvalidToml(error) => write!(formatter, "not a guest config: {error}"),
            Self::Unwritable(error) => write!(formatter, "cannot be written: {error}"),
        }
    }
}

/// Create a directory if it is not there yet, reporting the failure as text.
///
/// `ax_std::fs::create_dir_all` is not usable on the guest filesystem (it
/// reports "recursive directory creation is not supported"), so the pool only
/// ever ensures one level. An existing directory is left untouched, which is
/// what makes this callable on every start and on every save.
fn ensure_directory(directory: &str) -> Result<(), String> {
    if ax_std::fs::read_dir(directory).is_ok() {
        return Ok(());
    }
    ax_std::fs::create_dir(directory).map_err(|error| error.to_string())
}

/// Create every pool directory that can be created, logging what cannot.
///
/// Called on start so the drop-in folder exists and is visible in a browse, and
/// so an unprovisioned pool reads as "empty folder" rather than "missing
/// folder". A failure is not fatal: the pool is allowed to be absent, and a
/// read-only filesystem simply keeps it that way.
pub fn ensure_directories() {
    for directory in sources() {
        if let Err(error) = ensure_directory(&directory) {
            info!("VM pool: cannot create `{directory}`: {error}");
        }
    }
}

/// Store a guest config in the drop-in pool directory, returning its path.
///
/// This is how a config reaches the pool on a machine whose shell cannot write
/// a multi-line file: the control plane validates the text and writes it as a
/// pool file, after which it is a candidate like any other. The name is
/// restricted to a plain `*.toml` file name so the request cannot write outside
/// the pool directory.
pub fn save(name: &str, toml: &str) -> Result<String, SaveError> {
    save_in(&directory(), name, toml)
}

/// Store a guest config in a specific directory, returning its path.
///
/// [`save`] passes the drop-in pool directory; taking the directory as an
/// argument is what makes the write path testable against a temporary fixture
/// instead of the deployed pool. Validation is identical in both cases: the name
/// must be a plain `*.toml` file name (no separators, no leading dot) and the
/// text must parse as a guest config before anything is written.
pub fn save_in(directory: &str, name: &str, toml: &str) -> Result<String, SaveError> {
    let name = name.trim();
    if name.is_empty() || !name.ends_with(".toml") || name.contains('/') || name.starts_with('.') {
        return Err(SaveError::InvalidName(name.to_string()));
    }
    GuestConfig::from_toml(toml).map_err(|error| SaveError::InvalidToml(format!("{error}")))?;

    // The guest filesystem cannot create parent directories — `create_dir_all`
    // reports "recursive directory creation is not supported" — so an absent
    // drop-in folder is created one level deep, which is all the pool needs.
    ensure_directory(directory).map_err(SaveError::Unwritable)?;
    let path = format!("{directory}/{name}");
    ax_std::fs::write(&path, toml).map_err(|error| SaveError::Unwritable(error.to_string()))?;
    Ok(path)
}
///
/// The first guest image a config names but that is missing from the guest
/// filesystem.
///
/// Only a config that reads its images from the guest filesystem names files
/// this plane can look up: a config whose images are embedded in the hypervisor
/// (`image_location = "memory"`) has nothing to check here, and its kernel path
/// is allowed to be absent from the guest filesystem. A filesystem config that
/// names an absent kernel, ramdisk or DTB cannot become a VM, so it is reported
/// rather than listed: the operator would otherwise pick a config that fails on
/// start.
fn unusable_image(config: &GuestConfig) -> Option<IssueKind> {
    if config.kernel.image_location.as_deref() != Some("fs") {
        return None;
    }

    [
        Some(config.kernel.kernel_path.as_str()),
        config.kernel.ramdisk_path.as_deref(),
        config.kernel.dtb_path.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|path| !path.is_empty())
    .find(|path| ax_std::fs::metadata(path).is_err())
    .map(|path| IssueKind::MissingImage(path.to_string()))
}
