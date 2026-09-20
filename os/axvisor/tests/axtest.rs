//! Kernel harness for the Axvisor in-source axtest suites.
//!
//! The assertions live at the end of the production source files (gated by
//! `#[cfg(any(test, axtest))]`), registered through the `.axtest_array` linker
//! section. This target only wires the bare-metal modules that the binary owns
//! into the test build, together with narrow host/manager/network stubs, and
//! provides the axtest entry point.

#![cfg_attr(target_os = "none", no_std)]
#![no_main]

extern crate alloc;

// The production pool module under test reports through the host log.
#[macro_use]
extern crate log;

use ax_hal as _;
use ax_std as _;
use axvm as _;

// Compile the production guest-console mux with narrow host/manager adapters
// so its application-layer state machine is exercised by the kernel harness.
// These modules stay reachable from the production binary build; the harness
// compiles them only for their in-file axtest suites.
#[allow(dead_code)]
#[path = "../src/network_console/delivery.rs"]
mod browser_console_delivery;
#[allow(dead_code)]
#[path = "../src/network_console/layout.rs"]
mod browser_console_layout;
mod guest_console_harness;
#[allow(dead_code)]
#[path = "../src/guest_console/terminal.rs"]
mod host_terminal;
mod manager;
mod network_console;
#[path = "../src/vm_pool.rs"]
mod vm_pool;

// These cases exercise the mux-to-network boundary through the stub above and
// therefore must live beside the harness assembly instead of `mux/tests.rs`
// (the binary is also compiled with `--cfg axtest` and has no network console).
#[axtest::tests]
mod tests {
    use axtest::prelude::*;

    fn remove_guest_console(vm_id: usize) {
        use crate::guest_console_harness::mux;

        let identity = mux::backend_identity(vm_id).expect("guest backend must be registered");
        assert!(mux::remove_if_backend(identity));
    }

    #[test]
    fn guest_output_reaches_only_its_network_console() {
        use crate::{guest_console_harness::mux, network_console};

        network_console::reset();
        network_console::set_guest_connected(1);
        network_console::set_guest_connected(2);
        let backend_1 = mux::serial_backend_factory(1).create();
        let backend_2 = mux::serial_backend_factory(2).create();
        mux::mark_running(1);
        mux::mark_running(2);

        backend_1.write(b"starry output\n");
        backend_2.write(b"zephyr output\n");

        ax_assert_eq!(network_console::take_guest_output(1), b"starry output\n");
        ax_assert_eq!(network_console::take_guest_output(2), b"zephyr output\n");
        ax_assert!(network_console::take_guest_output(3).is_empty());
        remove_guest_console(1);
        remove_guest_console(2);
    }

    #[test]
    fn guest_output_skips_network_path_without_a_browser_session() {
        use crate::{guest_console_harness::mux, network_console};

        network_console::reset();
        let backend = mux::serial_backend_factory(1).create();
        mux::mark_running(1);

        backend.write(b"physical console only\n");

        ax_assert!(network_console::take_guest_output(1).is_empty());
        remove_guest_console(1);
    }

    #[test]
    fn unterminated_guest_echo_reaches_browser_without_another_input() {
        use crate::{guest_console_harness::mux, network_console};

        network_console::reset();
        network_console::set_guest_connected(1);
        let backend = mux::serial_backend_factory(1).create();
        mux::mark_running(1);

        backend.write(b"./run_dual_pick.sh");

        ax_assert_eq!(network_console::take_guest_output(1), b"./run_dual_pick.sh");
        remove_guest_console(1);
    }

    #[test]
    fn guest_byte_writes_reach_network_output_in_order() {
        use crate::{guest_console_harness::mux, network_console};

        network_console::reset();
        network_console::set_guest_connected(2);
        let backend = mux::serial_backend_factory(2).create();
        mux::mark_running(2);

        for byte in b"zephyr log line\n" {
            backend.write(core::slice::from_ref(byte));
        }

        ax_assert_eq!(network_console::take_guest_output(2), b"zephyr log line\n");
        remove_guest_console(2);
    }

    #[test]
    fn blocked_physical_output_keeps_guest_bytes_out_of_network_delivery() {
        use crate::{guest_console_harness, network_console};

        network_console::reset();
        network_console::set_guest_connected(1);
        let backend = guest_console_harness::mux::serial_backend_factory(1).create();
        guest_console_harness::mux::mark_running(1);

        guest_console_harness::host::set_output_blocked(true);
        let accepted = backend.try_write(b"retained by uart");
        guest_console_harness::host::set_output_blocked(false);

        ax_assert_eq!(accepted, 0);
        ax_assert!(network_console::take_guest_output(1).is_empty());
        remove_guest_console(1);
    }

    #[test]
    fn ordered_queue_pop_notifies_the_blocked_vm_through_the_mux() {
        use crate::{
            guest_console_harness::{host, mux},
            manager, network_console,
        };

        network_console::reset();
        host::reset_output();
        manager::take_notified_vms();
        host::set_ordered_output_available(true);

        // VM[2]'s record owns the only ordered slot, so the pop that releases
        // capacity belongs to a VM other than the one left with retained bytes.
        let backend_2 = mux::serial_backend_factory(2).create();
        mux::mark_running(2);
        ax_assert_eq!(backend_2.try_write(b"vm2\n"), 4);

        network_console::set_guest_connected(1);
        let backend_1 = mux::serial_backend_factory(1).create();
        mux::mark_running(1);
        ax_assert_eq!(backend_1.try_write(b"retained by uart"), 0);
        ax_assert!(network_console::take_guest_output(1).is_empty());
        // Backpressure alone must not wake anything; the wake belongs to the pop.
        ax_assert!(manager::take_notified_vms().is_empty());

        let popped = host::pop_ordered_record().expect("queued record is retained");
        ax_assert_eq!((popped >> 64) as usize, 2);

        // Driving the production pop handler must wake the blocked VM even
        // though the popped record belongs to another VM.
        mux::replay_guest_output(popped, b"vm2\n");
        ax_assert_eq!(manager::take_notified_vms(), alloc::vec![1]);

        // The retry now fits the released slot and reaches the network console.
        ax_assert_eq!(backend_1.try_write(b"retained by uart"), 16);
        ax_assert_eq!(network_console::take_guest_output(1), b"retained by uart");

        remove_guest_console(2);
        remove_guest_console(1);
        host::reset_output();
    }

    #[test]
    fn host_log_pop_notifies_the_blocked_vm_through_the_mux() {
        use crate::{
            guest_console_harness::{host, mux},
            manager,
        };

        host::reset_output();
        manager::take_notified_vms();
        host::set_ordered_output_available(true);

        // An untagged host log record owns the only ordered slot, so the next
        // guest submission is rejected with backpressure.
        ax_assert!(host::queue_host_log_record(b"host log\n"));

        let blocked = mux::serial_backend_factory(1).create();
        mux::mark_running(1);
        ax_assert_eq!(blocked.try_write(b"retained"), 0);
        // Backpressure alone must not wake anything.
        ax_assert!(manager::take_notified_vms().is_empty());

        // Consuming the host record releases the slot; the queue stub itself
        // must not be the thing that wakes the VM.
        let record = host::pop_ordered_host_record().expect("queued host log is retained");
        ax_assert!(manager::take_notified_vms().is_empty());

        // Only routing the popped bytes through the production host-log handler
        // publishes the device-poll request for the blocked VM.
        let _output = mux::route_host_log(&record, 0, 0);
        ax_assert_eq!(manager::take_notified_vms(), alloc::vec![1]);

        remove_guest_console(1);
        host::reset_output();
    }

    #[test]
    fn accepted_console_tail_is_displayed_before_stopping_guest_detaches() {
        use crate::{
            guest_console_harness::{host, mux},
            manager,
        };
        use axvm::VmStatus;

        host::reset_output();
        host::set_ordered_output_available(true);
        manager::set_vm_status(1, Some(VmStatus::Running));
        manager::take_notified_vms();
        let backend = mux::serial_backend_factory(1).create();
        mux::mark_running(1);
        ax_assert!(mux::attach(1).is_ok());
        mux::activate(1);

        let prefix = b"ivc ack seq=5 msg=ack from linux subscribe";
        ax_assert!(host::queue_host_log_record(b""));
        ax_assert_eq!(backend.try_write(prefix), 0);
        let filler = host::pop_ordered_host_record().expect("filler must be queued");
        let _ = mux::route_host_log(&filler, 0, 0);
        ax_assert_eq!(manager::take_notified_vms(), alloc::vec![1]);
        ax_assert_eq!(backend.try_write(prefix), prefix.len());
        let tag = host::pop_ordered_record().expect("accepted prefix must be queued");
        mux::replay_guest_output(tag, prefix);
        ax_assert_eq!(backend.try_write(b"r\n"), 2);

        manager::set_vm_status(1, Some(VmStatus::Stopping));
        ax_assert_eq!(mux::reconcile_vm_states(), None);

        let tag = host::pop_ordered_record().expect("accepted tail must be queued");
        mux::replay_guest_output(tag, b"r\n");
        ax_assert_eq!(
            host::take_host_bytes(),
            b"ivc ack seq=5 msg=ack from linux subscriber\n"
        );

        manager::set_vm_status(1, Some(VmStatus::Stopped));
        ax_assert_eq!(mux::reconcile_vm_states(), Some(1));
        manager::set_vm_status(1, None);
        remove_guest_console(1);
        manager::take_notified_vms();
        host::reset_output();
    }

    #[test]
    fn pop_notifies_only_blocked_backends_that_are_still_current() {
        use crate::{
            guest_console_harness::{host, mux},
            manager,
        };

        host::reset_output();
        manager::take_notified_vms();
        host::set_ordered_output_available(true);

        // Occupy the only ordered slot so later submissions report backpressure.
        let filler = mux::serial_backend_factory(7).create();
        mux::mark_running(7);
        ax_assert_eq!(filler.try_write(b"filler"), 6);

        let stopped = mux::serial_backend_factory(3).create();
        mux::mark_running(3);
        ax_assert_eq!(stopped.try_write(b"stopped"), 0);
        mux::mark_stopped(3);

        let replaced = mux::serial_backend_factory(4).create();
        mux::mark_running(4);
        ax_assert_eq!(replaced.try_write(b"replaced"), 0);
        let _replacement = mux::serial_backend_factory(4).create();
        mux::mark_running(4);

        let live = mux::serial_backend_factory(5).create();
        mux::mark_running(5);
        ax_assert_eq!(live.try_write(b"live"), 0);
        ax_assert!(manager::take_notified_vms().is_empty());

        let popped = host::pop_ordered_record().expect("queued record is retained");
        mux::replay_guest_output(popped, b"filler");

        // Only the still-live blocked VM may be woken; the stopped incarnation
        // and the replaced generation must not leak into the pop notification.
        ax_assert_eq!(manager::take_notified_vms(), alloc::vec![5]);

        remove_guest_console(7);
        remove_guest_console(3);
        remove_guest_console(4);
        remove_guest_console(5);
        host::reset_output();
    }

    #[test]
    fn browser_delivery_coalesces_ordered_dispatcher_batches() {
        use crate::browser_console_delivery::DeliveryFrame;

        let mut delivery = DeliveryFrame::with_capacity(16);

        delivery.append(b"starry ", 0);
        delivery.append(b"continues", 0);

        ax_assert_eq!(delivery.into_bytes(), b"starry continues");
    }

    #[test]
    fn browser_delivery_reports_source_queue_overflow_before_preserved_bytes() {
        use crate::browser_console_delivery::DeliveryFrame;

        let mut delivery = DeliveryFrame::with_capacity(96);

        delivery.append(b"preserved", 11);

        let output = delivery.into_bytes();
        ax_assert!(
            output.starts_with(b"\r\n[Axvisor browser console dropped 11 queued bytes]\r\n")
        );
        ax_assert!(output.ends_with(b"preserved"));
    }

    #[test]
    fn browser_delivery_queue_preserves_old_output_and_reports_new_overflow() {
        use crate::browser_console_delivery::DeliveryQueue;

        let mut delivery = DeliveryQueue::<8>::new();
        delivery.enqueue(b"old");
        delivery.enqueue(b"overflow");

        let mut output = [0; 8];
        let (len, dropped_bytes) = delivery.dequeue(&mut output);
        ax_assert_eq!(&output[..len], b"old");
        ax_assert_eq!(dropped_bytes, 8);
    }

    #[test]
    fn browser_delivery_waits_for_notification_without_timer_polling() {
        use core::sync::atomic::{AtomicBool, Ordering};
        use std::{sync::Arc, thread, time::Duration};

        use {
            ax_std::os::arceos::modules::ax_runtime::task::sync::irq::IrqWaitCell,
            ax_std::os::arceos::modules::ax_runtime::task::sync::irq::IrqWorkerWaiter,
            ax_std::os::arceos::modules::ax_runtime::task::thread::current::current_thread_handle,
        };

        let signal = Arc::new(IrqWaitCell::new());
        let waiting = Arc::new(AtomicBool::new(false));
        let woke = Arc::new(AtomicBool::new(false));
        let worker_signal = Arc::clone(&signal);
        let worker_waiting = Arc::clone(&waiting);
        let worker_woke = Arc::clone(&woke);
        let worker = thread::spawn(move || {
            let current =
                current_thread_handle().expect("delivery waiter must bind to its runtime worker");
            let waiter = IrqWorkerWaiter::new(current.wake_handle());
            worker_waiting.store(true, Ordering::Release);
            waiter
                .wait(&worker_signal)
                .expect("delivery waiter must accept one notification cell");
            worker_woke.store(true, Ordering::Release);
        });

        while !waiting.load(Ordering::Acquire) {
            thread::yield_now();
        }
        thread::sleep(Duration::from_millis(30));
        ax_assert!(!woke.load(Ordering::Acquire));

        let _result = signal.notify();
        worker
            .join()
            .expect("delivery waiter must exit after notify");
        ax_assert!(woke.load(Ordering::Acquire));
    }

    #[test]
    fn host_terminal_converts_only_bare_lf_across_batches() {
        use crate::host_terminal::TerminalNewlineNormalizer;

        let mut normalizer = TerminalNewlineNormalizer::new();
        let mut output = Vec::new();
        normalizer
            .write(b"banner\nline\r", |bytes| {
                output.extend_from_slice(bytes);
                Ok::<_, ()>(())
            })
            .unwrap();
        normalizer
            .write(b"\nnext\n", |bytes| {
                output.extend_from_slice(bytes);
                Ok::<_, ()>(())
            })
            .unwrap();

        ax_assert_eq!(output, b"banner\r\nline\r\nnext\r\n");
    }

    #[test]
    fn console_layout_allocates_guest_lanes_in_order_and_frees_them() {
        use crate::browser_console_layout::{LaneAllocation, Layout, MAX_GUEST_CONSOLES};

        let mut layout = Layout::new();
        // The outcome tells the caller whether it may give a lane back later: a
        // lane reported as reused belongs to a VM that already existed, so a
        // creation that is rejected afterwards must not release it.
        ax_assert_eq!(
            layout
                .allocate(2, "zephyr")
                .expect("a free lane must accept a guest"),
            LaneAllocation::Allocated,
        );
        layout
            .allocate(1, "")
            .expect("a free lane must accept a guest");

        let endpoints = layout.endpoints();
        ax_assert_eq!(endpoints.len(), 3);
        ax_assert_eq!(endpoints[0].route, "axvisor");
        ax_assert_eq!(endpoints[0].vm_id, None);
        ax_assert_eq!(endpoints[1].vm_id, Some(2));
        ax_assert_eq!(endpoints[1].display_name, "zephyr");
        ax_assert_eq!(endpoints[1].lane.index(), 1);
        ax_assert_eq!(endpoints[2].vm_id, Some(1));
        ax_assert_eq!(endpoints[2].display_name, "VM 1");
        ax_assert_eq!(endpoints[2].route, "vm-1");

        // Re-registering a VM keeps its lane, and only a free slot is reused.
        ax_assert_eq!(
            layout
                .allocate(2, "zephyr")
                .expect("re-registering a VM is idempotent"),
            LaneAllocation::Reused,
        );
        ax_assert_eq!(layout.endpoints().len(), 3);
        ax_assert_eq!(layout.guest(2).map(|guest| guest.lane.index()), Some(1));
        ax_assert_eq!(layout.release(2).map(|guest| guest.lane.index()), Some(1));
        ax_assert_eq!(
            layout
                .allocate(3, "linux")
                .expect("the freed lane must be reusable"),
            LaneAllocation::Allocated,
        );
        ax_assert_eq!(layout.guest(3).map(|guest| guest.lane.index()), Some(1));
        ax_assert_eq!(layout.release(3).map(|guest| guest.lane.index()), Some(1));
        ax_assert_eq!(layout.release(3).map(|guest| guest.lane.index()), None);

        // A full table rejects the next guest instead of dropping an existing
        // one, and does not disturb the lanes already handed out.
        for vm_id in 0..MAX_GUEST_CONSOLES {
            layout
                .allocate(vm_id, "guest")
                .expect("guest within the lane limit must be accepted");
        }
        ax_assert!(layout.allocate(MAX_GUEST_CONSOLES, "guest").is_err());
        ax_assert_eq!(layout.endpoints().len(), MAX_GUEST_CONSOLES + 1);
        ax_assert_eq!(layout.guest(0).map(|guest| guest.lane.index()), Some(1));
        ax_assert_eq!(
            layout
                .guest(MAX_GUEST_CONSOLES - 1)
                .map(|guest| guest.lane.index()),
            Some(MAX_GUEST_CONSOLES)
        );
    }

    #[cfg(feature = "fs")]
    #[test]
    fn vm_pool_scan_lists_only_configs_that_can_become_a_vm() {
        use crate::vm_pool::scan_dir;

        let root = "/tmp/axvisor-vm-pool-scan";
        reset_test_dir(root);
        let kernel = format!("{root}/kernel.bin");
        fs::write(&kernel, b"guest kernel").expect("write kernel image fixture");

        let entry_toml = |id: usize, name: &str, source: &str| {
            format!(
                "[base]\nid = {id}\nname = \"{name}\"\n\n[kernel]\nimage_location = \"{source}\"\nkernel_path = \"{kernel}\"\n"
            )
        };
        fs::write(format!("{root}/named.toml"), entry_toml(7, "named", "fs"))
            .expect("write filesystem entry");
        // A memory-backed entry reads no guest path at runtime: its build-time
        // kernel path is allowed to be absent from the guest filesystem.
        fs::write(
            format!("{root}/embedded.toml"),
            format!(
                "[base]\nid = 6\nname = \"embedded\"\n\n[kernel]\nimage_location = \"memory\"\nkernel_path = \"/no/such/embedded-image\"\n"
            ),
        )
        .expect("write memory entry");
        // Only the `.toml` suffix makes a file a pool candidate.
        fs::write(format!("{root}/notes.txt"), b"[base]\nid = 9\n").expect("write note");
        fs::write(format!("{root}/empty.toml"), b"").expect("write empty file");
        fs::write(format!("{root}/broken.toml"), b"base = { id = 1,").expect("write broken file");
        fs::write(format!("{root}/binary.toml"), [0xff, 0xfe, 0xfd]).expect("write binary file");
        let absent = format!("{root}/absent.bin");
        fs::write(
            format!("{root}/missing.toml"),
            format!(
                "[base]\nid = 8\nname = \"missing\"\n\n[kernel]\nimage_location = \"fs\"\nkernel_path = \"{absent}\"\n"
            ),
        )
        .expect("write missing-image entry");

        let pool = scan_dir(root);

        ax_assert_eq!(pool.directory(), root);
        let mut ids: alloc::vec::Vec<usize> =
            pool.entries().iter().map(|entry| entry.id()).collect();
        ids.sort();
        ax_assert_eq!(ids, [6, 7]);
        let named = pool
            .entries()
            .iter()
            .find(|entry| entry.id() == 7)
            .expect("the filesystem entry must be listed");
        ax_assert!(named.path().ends_with("named.toml"));
        ax_assert!(named.toml().contains("id = 7"));

        let mut reported: alloc::vec::Vec<_> = pool
            .issues()
            .iter()
            .map(|issue| format!("{} {}", issue.kind().as_str(), issue.path()))
            .collect();
        reported.sort();
        let mut expected = [
            format!("empty {root}/empty.toml"),
            format!("invalid-toml {root}/broken.toml"),
            format!("missing-image {root}/missing.toml"),
            format!("unreadable {root}/binary.toml"),
        ];
        expected.sort();
        ax_assert_eq!(reported, expected);
        // The reported reason names the file that is missing, not just the
        // config that asked for it.
        let missing = pool
            .issues()
            .iter()
            .find(|issue| issue.path().ends_with("missing.toml"))
            .expect("the missing image must be reported");
        ax_assert!(format!("{missing}").contains(absent.as_str()));

        // Two configs claiming one id: the first one listed wins and the other
        // is reported, whichever the filesystem enumerates first.
        let dupes = format!("{root}/dupes");
        fs::create_dir(&dupes).expect("create duplicate fixture directory");
        fs::write(format!("{dupes}/a.toml"), entry_toml(5, "a", "fs")).expect("write a.toml");
        fs::write(format!("{dupes}/b.toml"), entry_toml(5, "b", "fs")).expect("write b.toml");
        let dupe_pool = scan_dir(&dupes);
        ax_assert_eq!(dupe_pool.entries().len(), 1);
        ax_assert_eq!(dupe_pool.issues().len(), 1);
        ax_assert_eq!(dupe_pool.issues()[0].kind().as_str(), "duplicate-id");
        ax_assert_eq!(dupe_pool.entries()[0].id(), 5);
        ax_assert!(dupe_pool.entries()[0].path() != dupe_pool.issues()[0].path());

        // A directory that is not there yields no entries and one reason.
        let absent_dir = "/tmp/axvisor-vm-pool-absent";
        let _ = remove_path(
            absent_dir,
            RemoveOptions {
                recursive: true,
                force: true,
                ..RemoveOptions::default()
            },
        );
        let absent_pool = scan_dir(absent_dir);
        ax_assert_eq!(absent_pool.entries().len(), 0);
        ax_assert_eq!(absent_pool.issues().len(), 1);
        ax_assert_eq!(
            absent_pool.issues()[0].kind().as_str(),
            "directory-unavailable"
        );

        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove pool fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn vm_pool_reads_several_directories_in_precedence_order() {
        use crate::vm_pool::{browse, scan_dirs, sources};

        // The drop-in directory comes first, so a config an operator drops in
        // shadows a same-id config from a directory that is only read.
        ax_assert_eq!(sources().first(), Some(&"/guest/vm_pool".to_string()));

        let root = "/tmp/axvisor-vm-pool-multi";
        reset_test_dir(root);
        let kernel = format!("{root}/kernel.bin");
        fs::write(&kernel, b"guest kernel").expect("write kernel image fixture");
        let entry_toml = |id: usize, name: &str| {
            format!(
                "[base]\nid = {id}\nname = \"{name}\"\n\n[kernel]\nimage_location = \"fs\"\nkernel_path = \"{kernel}\"\n"
            )
        };

        let first = format!("{root}/first");
        let second = format!("{root}/second");
        fs::create_dir(&first).expect("create first fixture directory");
        fs::create_dir(&second).expect("create second fixture directory");
        fs::write(format!("{first}/only.toml"), entry_toml(1, "only-first"))
            .expect("write first-only entry");
        fs::write(format!("{second}/other.toml"), entry_toml(2, "only-second"))
            .expect("write second-only entry");
        fs::write(format!("{first}/shadow.toml"), entry_toml(3, "from-first"))
            .expect("write shadowing entry");
        fs::write(
            format!("{second}/shadow.toml"),
            entry_toml(3, "from-second"),
        )
        .expect("write shadowed entry");

        let pool = scan_dirs(&[first.clone(), second.clone()]);

        ax_assert_eq!(pool.directory(), first.as_str());
        ax_assert_eq!(pool.sources(), [first.clone(), second.clone()].as_slice());
        let mut names: alloc::vec::Vec<&str> =
            pool.entries().iter().map(|entry| entry.name()).collect();
        names.sort();
        ax_assert_eq!(names, ["from-first", "only-first", "only-second"]);
        // Every entry says which folder it came from, which is what makes a
        // duplicate id traceable to two files.
        let from_second = pool
            .entries()
            .iter()
            .find(|entry| entry.name() == "only-second")
            .expect("the second directory must contribute entries");
        ax_assert_eq!(from_second.source(), second.as_str());
        let shadow = pool
            .issues()
            .iter()
            .find(|issue| issue.path().ends_with("second/shadow.toml"))
            .expect("the shadowed file must be reported");
        ax_assert_eq!(shadow.kind().as_str(), "duplicate-id");

        // Browsing walks one directory: folders are listed as folders to enter
        // rather than as unreadable files, and only `.toml` files are entries.
        fs::write(format!("{first}/notes.txt"), b"not a config").expect("write note");
        let folder = browse(&first);
        ax_assert_eq!(folder.path(), first.as_str());
        ax_assert_eq!(folder.parent(), Some(root));
        ax_assert!(folder.directories().is_empty());
        ax_assert_eq!(folder.entries().len(), 2);
        ax_assert!(folder.issues().is_empty());

        let nested = browse(root);
        let mut subdirectories: alloc::vec::Vec<&str> = nested
            .directories()
            .iter()
            .map(|directory| directory.name())
            .collect();
        subdirectories.sort();
        ax_assert_eq!(subdirectories, ["first", "second"]);
        // `kernel.bin` is neither a directory nor a `.toml`, so it is not
        // reported as a problem: browsing must not turn a normal file into noise.
        ax_assert!(nested.issues().is_empty());

        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove multi-directory fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn vm_pool_save_only_writes_validated_configs_inside_the_directory() {
        use crate::vm_pool::{SaveError, save_in, scan_dir};

        let root = "/tmp/axvisor-vm-pool-save";
        reset_test_dir(root);
        let kernel = format!("{root}/kernel.bin");
        fs::write(&kernel, b"guest kernel").expect("write kernel image fixture");
        let valid = format!(
            "[base]\nid = 4\nname = \"saved\"\n\n[kernel]\nimage_location = \"fs\"\nkernel_path = \"{kernel}\"\n"
        );

        // A name that could escape the directory is refused before any write,
        // so a request cannot place a file anywhere it likes.
        for name in [
            "",
            "  ",
            "guest",
            "guest.toml.bak",
            "../escape.toml",
            ".hidden.toml",
        ] {
            ax_assert!(matches!(
                save_in(root, name, &valid),
                Err(SaveError::InvalidName(_))
            ));
        }
        ax_assert!(fs::metadata(&format!("{root}/../escape.toml")).is_err());

        // Text that is not a guest config is refused too: the pool only ever
        // holds files that can become a VM.
        ax_assert!(matches!(
            save_in(root, "broken.toml", "base = { id = 1,"),
            Err(SaveError::InvalidToml(_))
        ));
        ax_assert!(fs::metadata(&format!("{root}/broken.toml")).is_err());

        // The happy path is observable: the file lands where it was asked to
        // and the next scan lists it as a candidate.
        let path = save_in(root, "saved.toml", &valid).expect("save a valid config");
        ax_assert_eq!(path, format!("{root}/saved.toml"));
        ax_assert_eq!(
            fs::read_to_string(&path).expect("read the saved config"),
            valid
        );
        let pool = scan_dir(root);
        ax_assert_eq!(pool.entries().len(), 1);
        ax_assert_eq!(pool.entries()[0].id(), 4);
        ax_assert_eq!(pool.entries()[0].name(), "saved");

        // Writing again replaces the file instead of appending to it.
        save_in(root, "saved.toml", &valid).expect("save a second time");
        let rewritten = scan_dir(root);
        ax_assert_eq!(rewritten.entries().len(), 1);

        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove save fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn touch_preserves_content_and_updates_times() {
        let path = "/tmp/axvisor-touch-regression";
        let touch_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let _ = fs::remove_file(path);
        fs::write(path, b"preserve me").expect("create touch fixture");

        touch_file_at(path, touch_time).expect("touch fixture");

        let metadata = fs::metadata(path).expect("read touched metadata");
        ax_assert_eq!(fs::read(path).expect("read touched file"), b"preserve me");
        let accessed = unix_seconds(metadata.accessed().expect("read atime"));
        let modified = unix_seconds(metadata.modified().expect("read mtime"));
        ax_assert_eq!(accessed, unix_seconds(touch_time));
        ax_assert_eq!(modified, unix_seconds(touch_time));

        let unsupported_time = UNIX_EPOCH + Duration::from_secs(u32::MAX as u64 + 1);
        let error = touch_file_at(path, unsupported_time)
            .expect_err("timestamps that would be truncated must fail");
        ax_assert_eq!(error.kind(), ErrorKind::InvalidInput);
        fs::remove_file(path).expect("remove touch fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn cp_file_to_existing_directory_uses_source_basename() {
        let root = "/tmp/axvisor-cp-file-regression";
        reset_test_dir(root);
        let source = format!("{root}/source.txt");
        let destination = format!("{root}/destination");
        fs::write(&source, b"copied payload").expect("create copy source");
        fs::create_dir(&destination).expect("create copy destination");

        copy_path(&source, &destination, CopyMode::File).expect("copy file into directory");

        ax_assert_eq!(
            fs::read(format!("{destination}/source.txt")).expect("read copied file"),
            b"copied payload"
        );
        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove copy fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn cp_rejects_copying_file_onto_itself_without_truncating_it() {
        let path = "/tmp/axvisor-cp-self-file-regression";
        let _ = fs::remove_file(path);
        fs::write(path, b"keep this payload").expect("create self-copy fixture");

        let error = copy_path(path, path, CopyMode::File)
            .expect_err("copying a file onto itself must fail");

        ax_assert_eq!(error.kind(), ErrorKind::InvalidInput);
        ax_assert_eq!(
            fs::read(path).expect("read self-copy fixture"),
            b"keep this payload"
        );
        fs::remove_file(path).expect("remove self-copy fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn cp_recursive_directory_to_existing_directory_uses_source_basename() {
        let root = "/tmp/axvisor-cp-dir-regression";
        reset_test_dir(root);
        let source = format!("{root}/source-dir");
        let destination = format!("{root}/destination");
        fs::create_dir(&source).expect("create recursive copy source");
        fs::write(format!("{source}/child.txt"), b"recursive payload")
            .expect("create recursive copy child");
        fs::create_dir(&destination).expect("create recursive copy destination");

        copy_path(&source, &destination, CopyMode::Recursive)
            .expect("copy directory into directory");

        ax_assert_eq!(
            fs::read(format!("{destination}/source-dir/child.txt"))
                .expect("read recursively copied file"),
            b"recursive payload"
        );
        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove recursive copy fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn cp_recursive_rejects_copying_directory_into_itself() {
        let root = "/tmp/axvisor-cp-self-regression";
        reset_test_dir(root);
        let source = format!("{root}/dir");
        fs::create_dir(&source).expect("create recursive copy source");
        fs::create_dir(format!("{source}/dir")).expect("create recursion guard");

        let error = copy_path(&source, &source, CopyMode::Recursive)
            .expect_err("recursive copy into itself must fail");

        ax_assert_eq!(error.kind(), ErrorKind::InvalidInput);
        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove self-copy fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn cp_recursive_rejects_copying_directory_into_descendant() {
        let root = "/tmp/axvisor-cp-descendant-regression";
        reset_test_dir(root);
        let source = format!("{root}/dir");
        let destination = format!("{source}/subdir");
        fs::create_dir(&source).expect("create recursive copy source");
        fs::create_dir(&destination).expect("create descendant destination");
        fs::create_dir(format!("{destination}/dir")).expect("create recursion guard");

        let error = copy_path(&source, &destination, CopyMode::Recursive)
            .expect_err("recursive copy into a descendant must fail");

        ax_assert_eq!(error.kind(), ErrorKind::InvalidInput);
        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove descendant-copy fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn cp_recursive_rejects_nonexistent_descendant_before_creation() {
        let root = "/tmp/axvisor-cp-new-descendant-regression";
        reset_test_dir(root);
        let source = format!("{root}/dir");
        let destination = format!("{source}/subdir");
        fs::create_dir(&source).expect("create recursive copy source");

        let error = ensure_recursive_destination_outside_source(&source, &destination)
            .expect_err("nonexistent descendant must be rejected before creation");

        ax_assert_eq!(error.kind(), ErrorKind::InvalidInput);
        ax_assert!(!fs::exists(&destination).expect("check descendant was not created"));
        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove nonexistent-descendant fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn mv_renames_file_on_same_filesystem() {
        let root = "/tmp/axvisor-mv-regression";
        reset_test_dir(root);
        let source = format!("{root}/source.txt");
        let destination = format!("{root}/destination.txt");
        fs::write(&source, b"moved payload").expect("create move source");

        move_file_or_dir(&source, &destination).expect("move file");

        ax_assert!(!fs::exists(&source).expect("check move source"));
        ax_assert_eq!(
            fs::read(&destination).expect("read move destination"),
            b"moved payload"
        );
        remove_path(
            root,
            RemoveOptions {
                recursive: true,
                ..RemoveOptions::default()
            },
        )
        .expect("remove move fixture");
    }

    #[cfg(feature = "fs")]
    #[test]
    fn rm_does_not_follow_a_directory_symlink() {
        let metadata = metadata_for_remove("/var/run").expect("inspect rootfs directory symlink");

        ax_assert!(metadata.file_type().is_symlink());
        ax_assert!(!metadata.is_dir());
    }

    #[cfg(feature = "fs")]
    fn reset_test_dir(path: &str) {
        let _ = remove_path(
            path,
            RemoveOptions {
                recursive: true,
                force: true,
                ..RemoveOptions::default()
            },
        );
        fs::create_dir(path).expect("create test directory");
    }

    #[cfg(feature = "fs")]
    fn unix_seconds(time: SystemTime) -> u64 {
        time.duration_since(UNIX_EPOCH)
            .expect("test time must not predate Unix epoch")
            .as_secs()
    }
}
