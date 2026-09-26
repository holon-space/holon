//! How busy the host was while a latency rung measured, read from the test
//! process's own scheduler counters (Martin's rulings D227, D228.a).
//!
//! Wall-time throughput is the SLO, but a busy host inflates it without the
//! tree changing: the drain test went `WindowHeld` with other processes
//! spinning on every core. *Queue wait* — time the process was runnable but
//! not running — measures that load. It cannot tell a slow pipeline from a
//! busy host, so a busy host never turns a failure into a pass: it makes the
//! run INVALID.

use std::fmt;
use std::time::Duration;
use std::time::Instant;

use holon_api::latency_drain::DrainVerdict;

/// Queue wait per unit of own CPU from which the host counts as busy.
/// Idle-host drives measured 0.00–0.02; with other test binaries running
/// the ratio was 0.22–0.31 and the drive already slowed (C/L 0.66–0.70
/// against 0.53); load-caused reds measured 0.64 and above.
pub const BUSY_QUEUE_WAIT_RATIO: f64 = 0.10;

/// What the process spent over one measured stretch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriveUsage {
    pub wall: Duration,
    /// User + system CPU of every thread in the process.
    pub process_cpu: Duration,
    /// Runnable-but-not-running time, summed over the process's threads.
    pub queue_wait: Duration,
    pub disk_written_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostClass {
    Quiet,
    Busy,
}

impl DriveUsage {
    pub fn queue_wait_ratio(&self) -> f64 {
        assert!(
            !self.process_cpu.is_zero(),
            "a measured drive spent no CPU at all: the meter did not bracket the drive"
        );
        self.queue_wait.as_secs_f64() / self.process_cpu.as_secs_f64()
    }

    pub fn class(&self) -> HostClass {
        if self.queue_wait_ratio() < BUSY_QUEUE_WAIT_RATIO {
            HostClass::Quiet
        } else {
            HostClass::Busy
        }
    }

    pub fn disk_write_mb_per_sec(&self) -> f64 {
        self.disk_written_bytes as f64 / 1e6 / self.wall.as_secs_f64()
    }
}

impl fmt::Display for DriveUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "host {:?}: queue-wait ratio {:.2} (queue wait {:.1}s over {:.1}s own CPU; busy \
             from {BUSY_QUEUE_WAIT_RATIO}) in {:.1}s wall, disk writes {:.1} MB/s",
            self.class(),
            self.queue_wait_ratio(),
            self.queue_wait.as_secs_f64(),
            self.process_cpu.as_secs_f64(),
            self.wall.as_secs_f64(),
            self.disk_write_mb_per_sec(),
        )
    }
}

/// The counters over a measured stretch, or why there are none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HostContention {
    Measured(DriveUsage),
    /// This platform has no implementation of the counters.
    Unmeasured,
}

impl fmt::Display for HostContention {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Measured(usage) => usage.fmt(f),
            Self::Unmeasured => f.write_str("host load unmeasured on this platform"),
        }
    }
}

/// Brackets a measured stretch.
pub struct UsageMeter {
    started: Instant,
    #[cfg(target_os = "macos")]
    start: macos::Counters,
}

impl UsageMeter {
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
            #[cfg(target_os = "macos")]
            start: macos::Counters::read(),
        }
    }

    pub fn finish(self) -> HostContention {
        #[cfg(target_os = "macos")]
        {
            HostContention::Measured(
                macos::Counters::read().since(&self.start, self.started.elapsed()),
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self.started;
            HostContention::Unmeasured
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::time::Duration;

    use super::DriveUsage;

    /// How far runnable time may trail CPU time over a stretch: the two
    /// counters are sampled at slightly different instants, so an idle
    /// stretch reads a few ms either way.
    const COUNTER_SKEW: Duration = Duration::from_millis(50);

    pub(super) struct Counters {
        process_cpu: Duration,
        runnable: Duration,
        disk_written_bytes: u64,
    }

    impl Counters {
        pub(super) fn read() -> Self {
            // SAFETY: an all-zero rusage_info_v4 is a valid plain-data value.
            let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is a writable rusage_info_v4, the struct the V4
            // flavor fills.
            let rc = unsafe {
                libc::proc_pid_rusage(
                    std::process::id() as libc::c_int,
                    libc::RUSAGE_INFO_V4,
                    &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
                )
            };
            assert_eq!(
                rc,
                0,
                "proc_pid_rusage(self, V4) failed: {}",
                std::io::Error::last_os_error()
            );
            let mach = mach_to_duration();
            Self {
                process_cpu: mach(info.ri_user_time + info.ri_system_time),
                runnable: mach(info.ri_runnable_time),
                disk_written_bytes: info.ri_diskio_byteswritten,
            }
        }

        pub(super) fn since(&self, start: &Self, wall: Duration) -> DriveUsage {
            let process_cpu = self.process_cpu - start.process_cpu;
            let runnable = self.runnable - start.runnable;
            assert!(
                runnable + COUNTER_SKEW >= process_cpu,
                "the process was runnable {runnable:?} but ran {process_cpu:?}: runnable time \
                 no longer includes running time, so queue wait cannot be derived from it"
            );
            DriveUsage {
                wall,
                process_cpu,
                queue_wait: runnable.saturating_sub(process_cpu),
                disk_written_bytes: self.disk_written_bytes - start.disk_written_bytes,
            }
        }
    }

    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> libc::c_int;
    }

    /// `rusage_info` times are in mach absolute-time units, which are not
    /// nanoseconds on Apple silicon.
    fn mach_to_duration() -> impl Fn(u64) -> Duration {
        let mut tb = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `tb` is a writable struct with the C layout the call fills.
        let rc = unsafe { mach_timebase_info(&mut tb) };
        assert!(
            rc == 0 && tb.denom != 0,
            "mach_timebase_info failed: rc {rc}"
        );
        move |t| Duration::from_nanos((t as u128 * tb.numer as u128 / tb.denom as u128) as u64)
    }
}

/// Why a run judged nothing about the tree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Refusal {
    /// The driver offered a write late while the pipeline had room.
    DriverFellBehind,
    /// The drive failed on wall time while other processes held the CPU.
    HostBusy { queue_wait_ratio: f64 },
    /// The drive failed on wall time on a platform whose load cannot be
    /// measured.
    Unmeasured,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DriverFellBehind => f.write_str(
                "the driver offered a write behind the floor-rate schedule while the pipeline \
                 had room, so the run did not offer the load the proof needs",
            ),
            Self::HostBusy { queue_wait_ratio } => write!(
                f,
                "the drive failed on wall time while the host was busy (queue-wait ratio \
                 {queue_wait_ratio:.2} >= {BUSY_QUEUE_WAIT_RATIO}), and a busy host and a slow \
                 tree look the same from here. Rerun on a quiet host"
            ),
            Self::Unmeasured => f.write_str(
                "the drive failed on wall time, and this platform cannot measure whether the \
                 host was busy. Rerun on macOS on a quiet host",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrainJudgement {
    Pass,
    Fail,
    Invalid(Refusal),
}

/// Judge a drain run from its wall-time verdict and the host's load
/// (Martin's ruling D228.a). Load only ever slows a drive, so a wall-time
/// Pass stands on any host; a wall-time failure is the tree's only on a
/// quiet host.
pub fn judge_drain(wall: &DrainVerdict, host: &HostContention) -> DrainJudgement {
    match wall {
        DrainVerdict::Pass { .. } => DrainJudgement::Pass,
        DrainVerdict::Invalid { .. } => DrainJudgement::Invalid(Refusal::DriverFellBehind),
        DrainVerdict::Late { .. }
        | DrainVerdict::WindowHeld { .. }
        | DrainVerdict::Undelivered { .. } => match host {
            HostContention::Unmeasured => DrainJudgement::Invalid(Refusal::Unmeasured),
            HostContention::Measured(usage) => match usage.class() {
                HostClass::Quiet => DrainJudgement::Fail,
                HostClass::Busy => DrainJudgement::Invalid(Refusal::HostBusy {
                    queue_wait_ratio: usage.queue_wait_ratio(),
                }),
            },
        },
    }
}
