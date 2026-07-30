//! x86_64 implementations of AxVM platform capability hooks.

use super::X86_64Arch;
use crate::architecture::{GuestBootPlatform, HostTimePlatform};

impl HostTimePlatform for X86_64Arch {
    fn register_timer_callback() {
        ax_std::os::arceos::modules::ax_task::register_timer_callback(|_| {
            crate::check_timer_events();
        });
    }
}

impl GuestBootPlatform for X86_64Arch {}
