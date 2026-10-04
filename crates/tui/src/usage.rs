//! Our own memory and CPU, sampled once a second for the footer badge.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::Msg;

/// How often the process is sampled.
const EVERY: Duration = Duration::from_secs(1);

/// Our memory and CPU, rounded to what the footer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub mb: u32,
    /// CPU in tenths of a percent of one core, so 0.4% is 4; above 1000 when several cores
    /// are busy.
    pub cpu_tenths: u32,
}

impl Usage {
    /// Rounds a sample: resident memory in bytes and CPU as a percentage of one core.
    pub fn round(bytes: u64, cpu: f32) -> Self {
        let tenths = (f64::from(cpu.max(0.0)) * 10.0).round();
        Self {
            mb: u32::try_from(bytes / (1024 * 1024)).unwrap_or(u32::MAX),
            // Saturates, and the max above drops NaN.
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            cpu_tenths: tenths.min(f64::from(u32::MAX)) as u32,
        }
    }

    /// ` ▮ 24 MB · 0.4% CPU `, with memory in GB from 1024 MB.
    pub fn label(self) -> String {
        let memory = if self.mb >= 1024 {
            format!("{}.{} GB", self.mb / 1024, self.mb % 1024 * 10 / 1024)
        } else {
            format!("{} MB", self.mb)
        };
        let cpu = format!("{}.{}", self.cpu_tenths / 10, self.cpu_tenths % 10);
        format!(" ▮ {memory} · {cpu}% CPU ")
    }
}

/// Samples this process every second on a thread, sending `Msg::Usage` only when the rounded
/// numbers change; stops once the app has quit.
// ponytail: samples even while the badge is off; start the thread only while it's shown if
// the wake-up ever shows in a profile.
pub fn spawn(tx: Sender<Msg>) {
    let Ok(pid) = sysinfo::get_current_pid() else {
        return;
    };
    thread::spawn(move || {
        let mut system = System::new();
        let mut last = None;
        // The first CPU reading has nothing to compare with, so it's always 0: skipped.
        let mut first = true;
        loop {
            let sample = sample(&mut system, pid);
            if !first && sample.is_some() && sample != last {
                last = sample;
                if sample.is_some_and(|s| tx.send(Msg::Usage(s)).is_err()) {
                    return;
                }
            }
            first = false;
            thread::sleep(EVERY);
        }
    });
}

/// Refreshes only `pid`, only its memory and CPU, and rounds them.
fn sample(system: &mut System, pid: Pid) -> Option<Usage> {
    let what = ProcessRefreshKind::nothing().with_memory().with_cpu();
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, what);
    (system.process(pid)).map(|p| Usage::round(p.memory(), p.cpu_usage()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_round_to_whole_megabytes_and_tenths_of_a_percent() {
        assert_eq!(
            Usage::round(0, 0.0),
            Usage {
                mb: 0,
                cpu_tenths: 0
            }
        );
        assert_eq!(
            Usage::round(1024 * 1024 - 1, 0.04),
            Usage {
                mb: 0,
                cpu_tenths: 0
            }
        );
        assert_eq!(
            Usage::round(24 * 1024 * 1024, 0.44),
            Usage {
                mb: 24,
                cpu_tenths: 4
            }
        );
        assert_eq!(Usage::round(1, f32::NAN).cpu_tenths, 0);
        assert_eq!(Usage::round(1, -3.0).cpu_tenths, 0);
    }

    #[test]
    fn labels_switch_to_gigabytes_and_go_past_one_core() {
        let label = |mb, cpu_tenths| Usage { mb, cpu_tenths }.label();
        assert_eq!(label(24, 4), " ▮ 24 MB · 0.4% CPU ");
        assert_eq!(label(1023, 0), " ▮ 1023 MB · 0.0% CPU ");
        assert_eq!(label(1536, 120), " ▮ 1.5 GB · 12.0% CPU ");
        assert_eq!(label(64, 1532), " ▮ 64 MB · 153.2% CPU ");
    }

    #[test]
    fn this_process_samples_some_memory() {
        let pid = sysinfo::get_current_pid().expect("a pid");
        let usage = sample(&mut System::new(), pid).expect("our own process");
        assert!(usage.mb > 0, "{usage:?}");
    }
}
