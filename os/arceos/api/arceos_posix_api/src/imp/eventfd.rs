//! `eventfd` implementation.
//!
//! An `eventfd` exposes a 64-bit counter as a file descriptor: reads return the
//! current counter and reset it to zero, writes add an 8-byte value to it.
//! Async I/O runtimes use it to wake a blocked `epoll_wait` (e.g. mio's
//! `Waker` creates one with `EFD_CLOEXEC | EFD_NONBLOCK` and writes `1` to
//! wake). Blocking reads/writes yield cooperatively like the rest of ArceOS.

use alloc::sync::Arc;
use core::ffi::{c_int, c_uint};

use ax_errno::{LinuxError, LinuxResult};
use ax_io::PollState;
use ax_sync::Mutex;

use super::fd_ops::{FileLike, add_file_like};
use crate::ctypes;

const EFD_SUPPORTED_FLAGS: u32 = ctypes::EFD_SEMAPHORE | ctypes::EFD_CLOEXEC | ctypes::EFD_NONBLOCK;

pub struct EventFd {
    inner: Mutex<EventFdInner>,
}

struct EventFdInner {
    counter: u64,
    semaphore: bool,
    nonblocking: bool,
    readiness_version: u64,
}

impl EventFd {
    pub fn new(initval: u32, semaphore: bool, nonblocking: bool) -> Self {
        Self {
            inner: Mutex::new(EventFdInner {
                counter: initval as u64,
                semaphore,
                nonblocking,
                readiness_version: 0,
            }),
        }
    }
}

impl FileLike for EventFd {
    fn read(&self, buf: &mut [u8]) -> LinuxResult<usize> {
        if buf.len() < 8 {
            return Err(LinuxError::EINVAL);
        }
        loop {
            let mut inner = self.inner.lock();
            if inner.counter == 0 {
                if inner.nonblocking {
                    return Err(LinuxError::EAGAIN);
                }
                drop(inner);
                crate::sys_sched_yield(); // Wait for a writer.
                continue;
            }
            let old_readable = inner.counter > 0;
            let value = if inner.semaphore {
                inner.counter -= 1;
                1
            } else {
                let v = inner.counter;
                inner.counter = 0;
                v
            };
            let new_readable = inner.counter > 0;
            if old_readable != new_readable {
                inner.readiness_version = inner.readiness_version.wrapping_add(1);
            }
            buf[..8].copy_from_slice(&value.to_ne_bytes());
            return Ok(8);
        }
    }

    fn write(&self, buf: &[u8]) -> LinuxResult<usize> {
        // Linux copies only the first 8 bytes, so any buffer of at least 8
        // bytes is accepted; smaller buffers fail with EINVAL.
        if buf.len() < 8 {
            return Err(LinuxError::EINVAL);
        }
        let value = u64::from_ne_bytes(buf[..8].try_into().unwrap());
        // A write of UINT64_MAX always fails with EINVAL (fs/eventfd.c).
        if value == u64::MAX {
            return Err(LinuxError::EINVAL);
        }
        loop {
            let mut inner = self.inner.lock();
            let old_readable = inner.counter > 0;
            // The counter saturates at UINT64_MAX - 1; a write whose addition
            // would reach or exceed UINT64_MAX blocks, or fails with EAGAIN
            // if nonblocking. `u64::MAX - value` never underflows because
            // `value` is at most UINT64_MAX - 1.
            if inner.counter >= u64::MAX - value {
                if inner.nonblocking {
                    return Err(LinuxError::EAGAIN);
                }
                drop(inner);
                crate::sys_sched_yield(); // Wait for a reader to drain the counter.
                continue;
            }
            inner.counter += value;
            let new_readable = inner.counter > 0;
            if old_readable != new_readable {
                inner.readiness_version = inner.readiness_version.wrapping_add(1);
            }
            return Ok(8);
        }
    }

    fn stat(&self) -> LinuxResult<ctypes::stat> {
        let st_mode = 0o100000 | 0o600u32; // S_IFREG | rw-------
        Ok(ctypes::stat {
            st_ino: 1,
            st_nlink: 1,
            st_mode,
            ..Default::default()
        })
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn core::any::Any + Send + Sync> {
        self
    }

    fn poll(&self) -> LinuxResult<PollState> {
        let inner = self.inner.lock();
        Ok(PollState {
            readable: inner.counter > 0,
            writable: inner.counter < u64::MAX,
            readiness_version: inner.readiness_version,
        })
    }

    fn set_nonblocking(&self, nonblocking: bool) -> LinuxResult {
        self.inner.lock().nonblocking = nonblocking;
        Ok(())
    }
}

/// Create a file descriptor for event notification.
pub fn sys_eventfd(initval: c_uint, flags: c_int) -> c_int {
    debug!("sys_eventfd <= initval: {initval} flags: {flags:#x}");
    syscall_body!(sys_eventfd, {
        let flags = flags as u32;
        if flags & !EFD_SUPPORTED_FLAGS != 0 {
            return Err(LinuxError::EINVAL);
        }
        let eventfd = EventFd::new(
            initval,
            flags & ctypes::EFD_SEMAPHORE != 0,
            flags & ctypes::EFD_NONBLOCK != 0,
        );
        add_file_like(Arc::new(eventfd))
    })
}
