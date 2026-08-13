use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::Context;
use ostool::{build::config::Cargo, run::qemu::QemuConfig};

use super::{
    AXVISOR_NORMAL_GROUP, AxvisorQemuCase,
    assets::axvisor_case_asset_config,
    discover_qemu_cases,
    discovery::{
        discover_test_group_names, qemu_list_error_is_ignorable, test_suite_dir, test_suite_root,
    },
    initramfs::prepare_configured_busybox_initramfs,
    parse_target,
    types::PreparedAxvisorQemuCase,
};
use crate::{
    axvisor::{ArgsTestQemu, Axvisor, build, rootfs},
    context::{AxvisorCliArgs, ResolvedAxvisorRequest, SnapshotPersistence},
    test::{case as test_case, host_probe, qemu as test_qemu},
};

const VCPU_RUNTIME_ERROR: &str = r"VM\[\d+\] run VCpu\[\d+\] get error";

impl Axvisor {
    pub(super) async fn test_qemu(&mut self, args: ArgsTestQemu) -> anyhow::Result<()> {
        if args.list && args.arch.is_none() && args.target.is_none() && args.test_group.is_none() {
            let groups = discover_test_group_names(self.app.workspace_root())?
                .into_iter()
                .filter_map(|group| {
                    let test_suite_dir = match test_suite_dir(self.app.workspace_root(), &group) {
                        Ok(dir) => dir,
                        Err(err) => return Some(Err(err)),
                    };
                    match test_qemu::discover_all_qemu_cases_with_archs(
                        &test_suite_dir,
                        args.test_case.as_deref(),
                        "Axvisor",
                        &group,
                    ) {
                        Ok(case_names) => Some(Ok((group, case_names))),
                        Err(err) => {
                            if qemu_list_error_is_ignorable(err.kind()) {
                                None
                            } else {
                                Some(Err(anyhow::Error::new(err)))
                            }
                        }
                    }
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            if groups.is_empty() {
                anyhow::bail!(
                    "no Axvisor qemu test cases found under {}",
                    test_suite_root(self.app.workspace_root()).display()
                );
            }
            println!("{}", test_qemu::render_qemu_case_forest("axvisor", groups));
            return Ok(());
        }

        let test_group = args.test_group.as_deref().unwrap_or(AXVISOR_NORMAL_GROUP);
        if args.list && args.arch.is_none() && args.target.is_none() {
            let test_suite_dir = test_suite_dir(self.app.workspace_root(), test_group)?;
            let case_names = test_qemu::discover_all_qemu_cases(
                &test_suite_dir,
                args.test_case.as_deref(),
                "Axvisor",
                test_group,
            )
            .map_err(anyhow::Error::new)?;
            println!("{}", test_qemu::render_case_tree(test_group, case_names));
            return Ok(());
        }

        let (arch, target) = parse_target(&args.arch, &args.target)?;
        let cases = discover_qemu_cases(
            self.app.workspace_root(),
            test_group,
            &arch,
            &target,
            args.test_case.as_deref(),
        )?;
        if args.list {
            let case_names = cases.iter().map(|case| case.case.name.as_str());
            println!("{}", test_qemu::render_case_tree(test_group, case_names));
            return Ok(());
        }

        println!(
            "running axvisor qemu tests for arch: {} (target: {}, cases: {})",
            arch,
            target,
            cases.len()
        );

        let request = self.prepare_request(
            axvisor_qemu_test_build_args(&arch, None),
            None,
            None,
            SnapshotPersistence::Discard,
        )?;
        let request = Self::qemu_test_request(request);
        let cases = self
            .prepare_qemu_cases(&request, cases)
            .await
            .context("failed to load Axvisor qemu test cases")?;
        self.app.set_debug_mode(request.debug)?;

        let total = cases.len();
        let suite_started = Instant::now();
        let mut summary = test_qemu::QemuTestSummary::default();
        let asset_config = axvisor_case_asset_config();

        let mut build_groups = test_qemu::prepare_case_build_groups(&cases, |build_config_path| {
            Self::qemu_group_build_context(&request, build_config_path)
        })?;
        let artifact_parent = self.app.workspace_root().join("target");
        std::fs::create_dir_all(&artifact_parent).with_context(|| {
            format!(
                "failed to create Axvisor qemu artifact parent {}",
                artifact_parent.display()
            )
        })?;
        let artifact_directory = tempfile::Builder::new()
            .prefix("axvisor-qemu-artifacts-")
            .tempdir_in(&artifact_parent)
            .context("failed to create temporary Axvisor qemu artifact directory")?;
        let mut build_artifacts = Vec::with_capacity(build_groups.len());

        // Phase 1: Build all build groups first so compilation errors surface
        // before any QEMU time is spent. Preserve each executable immediately:
        // Cargo uses one output path for build groups that differ only in
        // embedded VM configuration, so a later build would otherwise replace
        // the executable belonging to an earlier group.
        for (index, build_group) in build_groups.iter_mut().enumerate() {
            rootfs::ensure_qemu_rootfs_ready(&build_group.request, self.app.workspace_root(), None)
                .await?;
            build_group.cargo = build::load_cargo_config(&build_group.request)?;
            prepare_configured_busybox_initramfs(
                &build_group.request,
                &build_group.cargo,
                self.app.workspace_root(),
            )
            .await?;
            let output = self
                .app
                .build(
                    build_group.cargo.clone(),
                    build_group.request.build_info_path.clone(),
                )
                .await
                .with_context(|| {
                    format!(
                        "failed to build Axvisor qemu test artifact for build group `{}` ({})",
                        build_group.group.build_group,
                        build_group.group.build_config_path.display()
                    )
                })?;
            build_artifacts.push(preserve_qemu_build_artifact(
                output.elf_path(),
                artifact_directory.path(),
                index,
            )?);
        }

        // Phase 2: Run all QEMU tests now that every artifact is available.
        let case_groups = build_groups
            .iter()
            .map(|build_group| build_group.group.cases.as_slice())
            .collect::<Vec<_>>();
        let case_artifacts =
            plan_qemu_case_artifacts(&case_groups, &build_artifacts, |case| case.qemu.to_bin)?;
        let mut completed = 0;
        for case_artifact in case_artifacts {
            completed += 1;
            let build_group = &build_groups[case_artifact.build_group_index];
            let case = case_artifact.case;
            let case_name = &case.case.case.name;
            println!("[{completed}/{total}] axvisor qemu {case_name}");

            let case_started = Instant::now();
            let result = async {
                self.app
                    .prepare_elf_artifact(
                        case_artifact.build_artifact.to_path_buf(),
                        case_artifact.to_bin,
                    )
                    .await
                    .with_context(|| {
                        format!("failed to activate Axvisor qemu artifact for case `{case_name}`")
                    })?;
                self.run_qemu_case(
                    &build_group.request,
                    &build_group.cargo,
                    case,
                    &asset_config,
                )
                .await
            }
            .await
            .with_context(|| format!("axvisor qemu test failed for case `{case_name}`"));
            let duration = case_started.elapsed();
            match result {
                Ok(()) => {
                    println!("ok: {case_name} ({duration:.2?})");
                    summary.pass_with_detail(case_name, format!("{duration:.2?}"));
                }
                Err(err) => {
                    eprintln!("failed: {}: {err:#}", case_name);
                    summary.fail_with_detail(case_name, format!("{duration:.2?}"));
                }
            }
        }

        let total_duration = format!("{:.2?}", suite_started.elapsed());
        summary.finish_with_total_detail("axvisor", "case", Some(total_duration.as_str()))
    }

    async fn prepare_qemu_cases(
        &mut self,
        request: &ResolvedAxvisorRequest,
        cases: Vec<AxvisorQemuCase>,
    ) -> anyhow::Result<Vec<PreparedAxvisorQemuCase>> {
        let mut prepared = Vec::with_capacity(cases.len());
        let mut cargo_by_build_config = BTreeMap::new();
        for case in cases {
            let cargo = Self::qemu_case_cargo_config(
                request,
                &case.build_config_path,
                &mut cargo_by_build_config,
            )?;
            let qemu = self
                .app
                .read_qemu_config_from_path_for_cargo(&cargo, &case.case.qemu_config_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to read Axvisor qemu config for case `{}`",
                        case.case.display_name
                    )
                })?;
            test_qemu::validate_grouped_qemu_commands(&qemu, &case.case, "Axvisor")?;
            prepared.push(PreparedAxvisorQemuCase { case, qemu });
        }

        Ok(prepared)
    }

    fn qemu_case_cargo_config(
        request: &ResolvedAxvisorRequest,
        build_config_path: &Path,
        cargo_by_build_config: &mut BTreeMap<PathBuf, Cargo>,
    ) -> anyhow::Result<Cargo> {
        if let Some(cargo) = cargo_by_build_config.get(build_config_path) {
            return Ok(cargo.clone());
        }

        let mut request = request.clone();
        request.build_info_path = build_config_path.to_path_buf();
        let cargo = build::load_cargo_config(&request)?;
        cargo_by_build_config.insert(build_config_path.to_path_buf(), cargo.clone());
        Ok(cargo)
    }

    fn qemu_group_build_context(
        request: &ResolvedAxvisorRequest,
        build_config_path: &Path,
    ) -> anyhow::Result<(ResolvedAxvisorRequest, Cargo)> {
        let mut request = request.clone();
        request.build_info_path = build_config_path.to_path_buf();
        let cargo = build::load_cargo_config(&request)?;
        request.vmconfigs = build::vmconfigs_from_cargo(&cargo);

        Ok((request, cargo))
    }

    pub(super) fn qemu_test_request(mut request: ResolvedAxvisorRequest) -> ResolvedAxvisorRequest {
        request.smp = None;
        request.vmconfigs.clear();
        request
    }

    async fn load_qemu_case_config(
        &mut self,
        request: &ResolvedAxvisorRequest,
        case: &PreparedAxvisorQemuCase,
        asset_config: &test_case::CaseAssetConfig,
    ) -> anyhow::Result<(QemuConfig, test_case::PreparedCaseAssets)> {
        let mut qemu = case.qemu.clone();
        test_case::apply_grouped_qemu_config(
            &mut qemu,
            &case.case.case,
            &asset_config.grouped_runner,
        );
        test_qemu::apply_timeout_scale(&mut qemu);
        if !qemu
            .fail_regex
            .iter()
            .any(|pattern| pattern == VCPU_RUNTIME_ERROR)
        {
            qemu.fail_regex.push(VCPU_RUNTIME_ERROR.to_string());
        }

        let rootfs_path = rootfs::qemu_rootfs_path(request, self.app.workspace_root(), None)?;
        let prepared_assets = test_case::prepare_case_assets(
            self.app.workspace_root(),
            &request.arch,
            &request.target,
            &case.case.case,
            rootfs_path,
            asset_config.clone(),
        )
        .await?;
        rootfs::patch_qemu_rootfs_path(
            &mut qemu,
            &prepared_assets.rootfs_path,
            crate::rootfs::qemu::RootfsWritePolicy::Discard,
        )?;
        Ok((qemu, prepared_assets))
    }

    async fn run_qemu_case(
        &mut self,
        request: &ResolvedAxvisorRequest,
        cargo: &Cargo,
        case: &PreparedAxvisorQemuCase,
        asset_config: &test_case::CaseAssetConfig,
    ) -> anyhow::Result<()> {
        let prepare_started = Instant::now();
        let (mut qemu, prepared_assets) = self
            .load_qemu_case_config(request, case, asset_config)
            .await?;

        // Optional host->guest TCP probe over QEMU user-mode networking. When
        // `[host_http_probe]` is configured, the host acts as a *client* that
        // dials a management API inside the guest through a hostfwd port and
        // asserts the responses entirely host-side. The assertions live in the
        // typed probe module ([`super::http_probe`]); axbuild only
        // orchestrates: forward the port, run the probe, and report its result.
        // The guard must live for the whole run, so it is spawned here and
        // dropped at scope end (after QEMU exits).
        //
        // The guard also drives QEMU termination: after it stores its verdict it
        // connects to a QMP monitor socket and sends `quit`, so the run ends on
        // the probe result instead of the serial-timeout path. That makes the
        // probe verdict the authoritative test result (no `/__probe_result`
        // relay inside the guest).
        let mut host_probe_guard = None;
        if let Some(probe_config) =
            test_qemu::load_qemu_case_extra_config(&case.case.case.qemu_config_path)?
                .host_http_probe
        {
            let host_port = pick_free_local_port()?;
            let qmp_socket = std::env::temp_dir().join(format!(
                "axvisor-qmp-{}-{}.sock",
                case.case.case.name,
                std::process::id()
            ));
            // Each QEMU option and its value must be a separate argv element
            // (QEMU takes the value of `-netdev`/`-device` from the following
            // argument), matching how the `.toml` config stores them.
            qemu.args.extend([
                "-netdev".to_string(),
                format!(
                    "user,id=net0,hostfwd=tcp::{host_port}-:{}",
                    probe_config.guest_port
                ),
                "-device".to_string(),
                "virtio-net-pci,netdev=net0".to_string(),
                "-qmp".to_string(),
                format!("unix:{},server=on,wait=off", qmp_socket.to_string_lossy()),
            ]);
            // The typed probe owns the create-body fixture (`vm-memory.toml`)
            // read from the case directory; the guard stays orchestration-only.
            let probe_addr = format!("127.0.0.1:{host_port}");
            let probe_owned = probe_config.clone();
            let probe_case_dir = case.case.case.case_dir.clone();
            let probe: host_probe::HostHttpProbeFn = Box::new(move || {
                super::http_probe::run(&probe_addr, &probe_owned, &probe_case_dir)
            });
            host_probe_guard = Some(host_probe::HostHttpProbeGuard::start(
                &probe_config,
                host_port,
                &case.case.case.name,
                Some(qmp_socket),
                probe,
            )?);
        }

        // QEMU's exit code is not the verdict for probe cases: the probe quits
        // QEMU (cleanly, or force-kills it if `quit` is ignored) whether it
        // passed or failed, so the stored probe result decides. For non-probe
        // cases the serial-success path in
        // `run_qemu_with_prepared_case_assets` still applies unchanged.
        let qemu_result = test_case::run_qemu_with_prepared_case_assets(
            &mut self.app,
            cargo,
            qemu,
            None,
            &case.case.case.qemu_config_path,
            prepared_assets,
            test_case::RunPreparedQemuCaseOptions {
                prepare_elapsed: prepare_started.elapsed(),
                qemu_timing_fields: None,
            },
        )
        .await;

        // Joins the probe thread now that QEMU has exited.
        let probe_configured = host_probe_guard.is_some();
        let probe_result = host_probe_guard
            .as_ref()
            .and_then(|guard| guard.take_result());
        let killed_by_probe = host_probe_guard
            .as_ref()
            .map(|guard| guard.killed_by_probe())
            .unwrap_or(false);
        drop(host_probe_guard);

        resolve_probe_verdict(
            &case.case.case.name,
            qemu_result,
            probe_configured,
            probe_result,
            killed_by_probe,
        )
    }
}

/// Combine the QEMU runner result, probe configuration/result, and whether the
/// probe force-killed QEMU into the final case verdict.
///
/// Precedence:
/// 1. A genuine serial failure (the ostool matcher reported a `fail_regex`
///    match) always wins, even when the probe force-killed QEMU afterwards —
///    QEMU ignored QMP `quit`, so the matcher's own kill also failed to stop it
///    and the probe's SIGKILL finished the job. The probe may only override the
///    *termination* it caused, never a real failure the matcher already observed.
/// 2. When the probe force-killed QEMU (QMP `quit` was ignored), the stored
///    probe verdict is authoritative for that termination.
/// 3. Any other QEMU failure (boot failure, guest crash, exit-status error)
///    wins regardless of probe configuration.
/// 4. Otherwise the probe verdict decides.
fn resolve_probe_verdict(
    case_name: &str,
    qemu_result: anyhow::Result<()>,
    probe_configured: bool,
    probe_result: Option<anyhow::Result<()>>,
    killed_by_probe: bool,
) -> anyhow::Result<()> {
    match (qemu_result, probe_configured, probe_result, killed_by_probe) {
        (Err(err), ..) if qemu_serial_failure(&err) => Err(err),
        (_, true, Some(verdict), true) => verdict,
        (Err(err), ..) => Err(err),
        (Ok(()), false, ..) => Ok(()),
        (Ok(()), true, Some(Ok(())), _) => Ok(()),
        (Ok(()), true, Some(Err(err)), _) => Err(err),
        (Ok(()), true, None, _) => {
            anyhow::bail!("host http probe for `{case_name}` produced no verdict")
        }
    }
}

/// Whether a QEMU runner error is a genuine serial failure rather than the
/// exit-status error left behind when the host probe force-kills QEMU. The two
/// are distinguishable by error shape: the ostool matcher reports a matched
/// `fail_regex` as `Fail pattern matched '<regex>': <excerpt>` (stream-match
/// path), while a probe-attributable kill surfaces only as QEMU's stderr log
/// (exit-status path), which never contains that marker.
fn qemu_serial_failure(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string().starts_with("Fail pattern matched '"))
}

/// Pick a free loopback port for the QEMU hostfwd listen, then release it so
/// QEMU can bind it. A freshly-assigned ephemeral port avoids stale-port
/// collisions from CI runner reuse (the same ports are never parked on a
/// previous run's leftover QEMU). A small bind-release-bind TOCTOU window
/// exists but is acceptable for a local test harness.
fn pick_free_local_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .context("failed to pick a free local port for QEMU hostfwd")?;
    Ok(listener.local_addr()?.port())
}

fn axvisor_qemu_test_build_args(arch: &str, config: Option<PathBuf>) -> AxvisorCliArgs {
    AxvisorCliArgs {
        config,
        arch: Some(arch.to_string()),
        target: None,
        smp: None,
        debug: false,
        vmconfigs: Vec::new(),
    }
}

pub(super) fn preserve_qemu_build_artifact(
    source: &Path,
    artifact_directory: &Path,
    build_group_index: usize,
) -> anyhow::Result<PathBuf> {
    let file_name = source.file_name().with_context(|| {
        format!(
            "Axvisor qemu build artifact {} has no file name",
            source.display()
        )
    })?;
    let group_directory = artifact_directory.join(format!("group-{build_group_index}"));
    std::fs::create_dir_all(&group_directory).with_context(|| {
        format!(
            "failed to create Axvisor qemu build-group artifact directory {}",
            group_directory.display()
        )
    })?;
    let destination = group_directory.join(file_name);
    std::fs::copy(source, &destination).with_context(|| {
        format!(
            "failed to preserve Axvisor qemu build artifact {} at {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(destination)
}

#[derive(Debug)]
pub(super) struct QemuCaseArtifact<'case, 'artifact, T> {
    pub(super) build_group_index: usize,
    pub(super) case: &'case T,
    pub(super) build_artifact: &'artifact Path,
    pub(super) to_bin: bool,
}

pub(super) fn plan_qemu_case_artifacts<'case, 'artifact, T>(
    case_groups: &[&[&'case T]],
    build_artifacts: &'artifact [PathBuf],
    to_bin: impl Fn(&T) -> bool,
) -> anyhow::Result<Vec<QemuCaseArtifact<'case, 'artifact, T>>> {
    anyhow::ensure!(
        case_groups.len() == build_artifacts.len(),
        "Axvisor qemu build-group count ({}) does not match preserved artifact count ({})",
        case_groups.len(),
        build_artifacts.len()
    );
    Ok(case_groups
        .iter()
        .zip(build_artifacts)
        .enumerate()
        .flat_map(|(build_group_index, (cases, build_artifact))| {
            cases.iter().map({
                let to_bin = &to_bin;
                move |case| QemuCaseArtifact {
                    build_group_index,
                    case: *case,
                    build_artifact,
                    to_bin: to_bin(*case),
                }
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{qemu_serial_failure, resolve_probe_verdict};

    fn ok() -> anyhow::Result<()> {
        Ok(())
    }

    fn err(message: &str) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("{message}"))
    }

    #[test]
    fn serial_failure_is_detected_by_matcher_marker() {
        let error =
            err("Fail pattern matched '(?i)\\bpanic(?:ked)?\\b': panicked at os/axvisor/...");
        assert!(qemu_serial_failure(&error.unwrap_err()));
    }

    #[test]
    fn serial_failure_detection_survives_context_wrapping() {
        let inner = anyhow::anyhow!("Fail pattern matched '(?i)kernel panic': Kernel panic");
        let wrapped = inner.context("qemu run failed");
        assert!(qemu_serial_failure(&wrapped));
    }

    #[test]
    fn probe_kill_stderr_error_is_not_a_serial_failure() {
        let error = err("failed to run QEMU: qemu-system-aarch64: terminating on signal 9");
        assert!(!qemu_serial_failure(&error.unwrap_err()));
    }

    #[test]
    fn empty_stderr_error_is_not_a_serial_failure() {
        assert!(!qemu_serial_failure(&err("").unwrap_err()));
    }

    #[test]
    fn serial_failure_wins_over_successful_probe_after_force_kill() {
        // The reviewer's case: QEMU ignored QMP `quit` AND the serial matcher
        // already reported a fail_regex match; the probe's SIGKILL must not let
        // a passing probe verdict swallow that real failure.
        let verdict = resolve_probe_verdict(
            "case",
            err("Fail pattern matched '(?i)\\bpanic(?:ked)?\\b': panicked at os/axvisor/..."),
            true,
            Some(ok()),
            true,
        );
        let message = verdict.unwrap_err().to_string();
        assert!(message.starts_with("Fail pattern matched '"), "{message}");
    }

    #[test]
    fn pure_probe_force_kill_lets_probe_verdict_win() {
        // Probe force-killed a hung QEMU with no prior serial failure: the
        // probe verdict is authoritative for that termination.
        assert!(resolve_probe_verdict("case", err(""), true, Some(ok()), true).is_ok());
        let probe_fail = resolve_probe_verdict("case", err(""), true, Some(err("probe 404")), true);
        assert!(probe_fail.unwrap_err().to_string().contains("probe 404"));
    }

    #[test]
    fn real_qemu_error_wins_when_probe_did_not_kill() {
        // The serial matcher (or another QEMU error) already failed the run and
        // QEMU exited on its own, so the probe never force-killed it: the QEMU
        // error is the verdict.
        assert!(
            resolve_probe_verdict("case", err("boot failed"), true, Some(ok()), false).is_err()
        );
    }

    #[test]
    fn non_probe_case_uses_qemu_exit_status() {
        assert!(resolve_probe_verdict("case", ok(), false, None, false).is_ok());
        assert!(resolve_probe_verdict("case", err("boot failed"), false, None, false).is_err());
    }

    #[test]
    fn probe_verdict_decides_on_clean_qemu_exit() {
        assert!(resolve_probe_verdict("case", ok(), true, Some(ok()), false).is_ok());
        assert!(resolve_probe_verdict("case", ok(), true, Some(err("probe 404")), false).is_err());
    }

    #[test]
    fn missing_probe_verdict_on_clean_qemu_exit_fails() {
        let verdict = resolve_probe_verdict("case", ok(), true, None, false);
        assert!(verdict.unwrap_err().to_string().contains("no verdict"));
    }
}
