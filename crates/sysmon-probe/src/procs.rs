// SPDX-License-Identifier: MPL-2.0

//! The process table: the top processes by CPU and by memory.
//!
//! A scan reads `/proc/stat` (for the elapsed CPU time) and every `/proc/<pid>/stat`. That one
//! file has all a list needs: the name (`comm`), CPU ticks (`utime + stime`, all threads
//! together), the start time (to tell a reused PID) and the resident set (`rss`, the same value
//! as `statm`'s second field), so `statm` is not read. Only the winners' `cmdline` is read, to
//! complete names the kernel cut at 15 characters.
//!
//! **CPU %** is the difference against the previous scan, in percent of the whole CPU:
//! `Δticks / Δjiffies of all CPUs × 100`, as GNOME's and COSMIC's system monitors show it, so
//! 100 % is every core busy (`top` shows percent of one core instead). Both counters come from
//! the kernel in the same unit, so no clock or `CLK_TCK` is involved. A scan with no previous
//! one, or one older than [`STALE_AFTER`] (the popup was closed meanwhile), first takes a
//! baseline and waits [`BASELINE`], so the first list after opening the popup is already real.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use sysmon_proto::{Process, TopProcesses, clip_name};

/// A previous scan older than this isn't a useful baseline: the popup was closed meanwhile.
pub const STALE_AFTER: Duration = Duration::from_secs(10);
/// How long a fresh baseline is measured over.
pub const BASELINE: Duration = Duration::from_millis(250);
/// Most PIDs looked at in one scan, so a fork bomb can't make a scan unbounded (D8).
const MAX_PIDS: usize = 65_536;
/// The kernel cuts `comm` at this many bytes.
const COMM_LEN: usize = 15;
/// Room for a whole `/proc/<pid>/stat` in one read: 52 numbers and a 15-byte `comm`.
const STAT_BUF: usize = 4096;

/// One `/proc/<pid>/stat`, the fields a list needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PidStat {
    pub pid: u32,
    /// `comm`, as the kernel keeps it (at most 15 bytes).
    pub comm: String,
    /// `utime + stime`, in clock ticks.
    pub ticks: u64,
    /// `starttime`: with the PID, identifies the process across scans.
    pub start: u64,
    /// `rss`, in pages.
    pub rss_pages: u64,
}

/// Parses `/proc/<pid>/stat`. `comm` may hold spaces and parentheses, so it runs to the last `)`.
#[must_use]
pub fn parse_stat(text: &str) -> Option<PidStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid = text[..open].trim().parse().ok()?;
    let comm = text.get(open + 1..close)?;
    // Fields from the third (`state`) on, read in order without collecting them: `after`
    // is the last field read, and the next one wanted is `n`.
    let mut fields = text[close + 1..].split_ascii_whitespace();
    let mut after = 2;
    let mut field = |n: usize| {
        let value = fields.nth(n - after - 1)?.parse::<u64>().ok();
        after = n;
        value
    };
    let utime = field(14)?;
    let stime = field(15)?;
    let start = field(22)?;
    let rss_pages = field(24)?;
    Some(PidStat {
        pid,
        comm: comm.to_owned(),
        ticks: utime.saturating_add(stime),
        start,
        rss_pages,
    })
}

/// The `cpu` line of `/proc/stat` summed: the CPU time of all cores together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTotal {
    pub jiffies: u64,
}

/// Parses `/proc/stat` into a [`CpuTotal`].
#[must_use]
pub fn parse_cpu_total(text: &str) -> Option<CpuTotal> {
    let line = text.lines().find(|line| line.starts_with("cpu "))?;
    // guest and guest_nice are already counted in user and nice.
    let jiffies = line
        .split_ascii_whitespace()
        .skip(1)
        .take(8)
        .filter_map(|f| f.parse::<u64>().ok())
        .fold(0u64, u64::saturating_add);
    Some(CpuTotal { jiffies })
}

/// Everything one scan read.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub total: CpuTotal,
    pub pids: Vec<PidStat>,
}

/// The `top` processes of `cur` by CPU (against `prev`) and by resident memory. Without `prev`
/// every CPU value is 0. Ties keep the lower PID first.
#[must_use]
pub fn top(prev: Option<&Snapshot>, cur: &Snapshot, top: usize, page_size: u64) -> TopProcesses {
    let elapsed = prev.and_then(|prev| {
        let elapsed = cur.total.jiffies.checked_sub(prev.total.jiffies)?;
        #[expect(clippy::cast_precision_loss, reason = "percentages")]
        let elapsed = elapsed as f64;
        (elapsed > 0.0).then_some(elapsed)
    });
    let before: HashMap<u32, &PidStat> = prev.map_or_else(HashMap::new, |p| {
        p.pids.iter().map(|s| (s.pid, s)).collect()
    });
    let processes: Vec<Process> = cur
        .pids
        .iter()
        .map(|s| {
            let cpu = elapsed
                .zip(before.get(&s.pid).filter(|b| b.start == s.start))
                .map_or(0.0, |(elapsed, b)| {
                    #[expect(clippy::cast_precision_loss, reason = "percentages")]
                    let busy = s.ticks.saturating_sub(b.ticks) as f64;
                    (busy / elapsed * 100.0).clamp(0.0, 100.0)
                });
            Process {
                pid: s.pid,
                name: clip_name(&s.comm),
                #[expect(clippy::cast_possible_truncation, reason = "percentages")]
                cpu: cpu as f32,
                rss: s.rss_pages.saturating_mul(page_size),
            }
        })
        .collect();
    let mut by_cpu = processes.clone();
    by_cpu.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(a.pid.cmp(&b.pid)));
    by_cpu.truncate(top);
    let mut by_memory = processes;
    by_memory.sort_by(|a, b| b.rss.cmp(&a.rss).then(a.pid.cmp(&b.pid)));
    by_memory.truncate(top);
    TopProcesses { by_cpu, by_memory }
}

/// The name to show for a process: its `comm`, unless the kernel cut it at 15 bytes and the
/// program's file name (from `cmdline`) is the complete version of it.
#[must_use]
pub fn display_name(comm: &str, cmdline: &[u8]) -> String {
    if comm.len() < COMM_LEN {
        return clip_name(comm);
    }
    let argv0 = cmdline.split(|&b| b == 0).next().unwrap_or_default();
    let argv0 = String::from_utf8_lossy(argv0);
    // Some programs rewrite their command line as one space-separated string.
    let program = argv0.split_whitespace().next().unwrap_or_default();
    let base = program.rsplit('/').next().unwrap_or_default();
    if base.len() > comm.len() && base.starts_with(comm) {
        clip_name(base)
    } else {
        clip_name(comm)
    }
}

/// What a scan does before reading, given how old the previous scan is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanPlan {
    /// Take a new baseline first: there is none, or it is older than [`STALE_AFTER`].
    pub new_baseline: bool,
    /// How long to wait before the scan, so CPU is measured over at least [`BASELINE`]: shorter
    /// differences are mostly the rounding of whole clock ticks.
    pub wait: Duration,
}

/// The [`ScanPlan`] for a previous scan `age` old (`None`: no previous scan).
#[must_use]
pub fn plan_scan(age: Option<Duration>) -> ScanPlan {
    match age {
        Some(age) if age < STALE_AFTER => ScanPlan {
            new_baseline: false,
            wait: BASELINE.saturating_sub(age),
        },
        _ => ScanPlan {
            new_baseline: true,
            wait: BASELINE,
        },
    }
}

/// Scans a procfs and keeps the previous scan for the next CPU difference.
#[derive(Debug)]
pub struct Scanner {
    proc: &'static Path,
    page_size: u64,
    prev: Option<(Instant, Snapshot)>,
}

impl Scanner {
    /// A scanner of the procfs at `proc` (`/proc`).
    #[must_use]
    pub fn new(proc: &'static Path) -> Self {
        Self {
            proc,
            page_size: rustix::param::page_size() as u64,
            prev: None,
        }
    }

    /// The `n` top processes by CPU and by memory. May wait up to [`BASELINE`] (see
    /// [`plan_scan`]).
    pub fn top(&mut self, n: usize) -> TopProcesses {
        let plan = plan_scan(self.prev.as_ref().map(|(at, _)| at.elapsed()));
        if plan.new_baseline {
            self.prev = self.snapshot().map(|s| (Instant::now(), s));
        }
        std::thread::sleep(plan.wait);
        let Some(cur) = self.snapshot() else {
            return TopProcesses::default();
        };
        let mut result = top(self.prev.as_ref().map(|(_, s)| s), &cur, n, self.page_size);
        for p in result.by_cpu.iter_mut().chain(&mut result.by_memory) {
            let cmdline = std::fs::read(self.proc.join(p.pid.to_string()).join("cmdline"));
            p.name = display_name(&p.name, &cmdline.unwrap_or_default());
        }
        self.prev = Some((Instant::now(), cur));
        result
    }

    /// Reads `/proc/stat` and every `/proc/<pid>/stat`. Most of a scan's cost is the kernel
    /// writing those files, so each is read with one `read` into a buffer kept for the whole
    /// scan, instead of `fs::read_to_string`'s `statx`, allocation and second `read` per file.
    fn snapshot(&self) -> Option<Snapshot> {
        let mut buf = Vec::with_capacity(STAT_BUF);
        let total = parse_cpu_total(read_into(&self.proc.join("stat"), &mut buf)?)?;
        let mut pids = Vec::new();
        let mut path = self.proc.to_path_buf();
        for entry in std::fs::read_dir(self.proc).ok()?.flatten().take(MAX_PIDS) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            path.push(name);
            path.push("stat");
            // A process may exit between the listing and the read.
            if let Some(stat) = read_into(&path, &mut buf).and_then(parse_stat) {
                pids.push(stat);
            }
            path.pop();
            path.pop();
        }
        Some(Snapshot { total, pids })
    }
}

/// The text of the file at `path`, read into `buf`: in one `read` when it fits in the space
/// `buf` has, as a procfs file does when read whole at once.
fn read_into<'a>(path: &Path, buf: &'a mut Vec<u8>) -> Option<&'a str> {
    let mut file = std::fs::File::open(path).ok()?;
    buf.clear();
    buf.resize(buf.capacity().max(STAT_BUF), 0);
    let room = buf.len();
    let read = file.read(buf).ok()?;
    buf.truncate(read);
    if read == room {
        // Full: there may be more.
        file.read_to_end(buf).ok()?;
    }
    std::str::from_utf8(buf).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `/proc/<pid>/stat` line from a real machine, with `comm` replaced.
    fn stat_line(pid: u32, comm: &str, utime: u64, stime: u64, start: u64, rss: u64) -> String {
        format!(
            "{pid} ({comm}) S 1 {pid} {pid} 0 -1 4194560 3305 0 0 0 {utime} {stime} 0 0 20 0 1 0 \
             {start} 22106112 {rss} 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0"
        )
    }

    fn pid(pid: u32, comm: &str, ticks: u64, rss_pages: u64) -> PidStat {
        PidStat {
            pid,
            comm: comm.into(),
            ticks,
            start: 1000,
            rss_pages,
        }
    }

    fn snapshot(jiffies: u64, pids: Vec<PidStat>) -> Snapshot {
        Snapshot {
            total: CpuTotal { jiffies },
            pids,
        }
    }

    fn names(list: &[Process]) -> Vec<(&str, f32)> {
        list.iter().map(|p| (p.name.as_str(), p.cpu)).collect()
    }

    #[test]
    fn parse_stat_reads_ticks_start_and_rss() {
        let stat = parse_stat(&stat_line(4242, "firefox", 120, 30, 98765, 5000)).unwrap();
        assert_eq!(
            stat,
            PidStat {
                pid: 4242,
                comm: "firefox".into(),
                ticks: 150,
                start: 98765,
                rss_pages: 5000,
            }
        );
    }

    #[test]
    fn parse_stat_takes_comm_up_to_the_last_parenthesis() {
        let stat = parse_stat(&stat_line(7, "evil) S 1 (x", 1, 2, 3, 4)).unwrap();
        assert_eq!(stat.comm, "evil) S 1 (x");
        assert_eq!((stat.ticks, stat.start, stat.rss_pages), (3, 3, 4));
    }

    #[test]
    fn parse_stat_rejects_truncated_lines() {
        assert_eq!(parse_stat("12 (bash) S 1 2 3"), None);
        assert_eq!(parse_stat(""), None);
    }

    #[test]
    fn read_into_reads_files_longer_than_the_buffer() {
        let dir = std::env::temp_dir().join(format!("sysmon-procs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("long");
        let text = "x".repeat(3 * STAT_BUF + 7);
        std::fs::write(&path, &text).unwrap();
        let mut buf = Vec::new();
        assert_eq!(read_into(&path, &mut buf), Some(text.as_str()));
        std::fs::write(&path, "short").unwrap();
        assert_eq!(read_into(&path, &mut buf), Some("short"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parse_cpu_total_sums_the_cpu_line() {
        let text = "cpu  10 1 5 100 4 1 2 3 7 7\ncpu0 5 0 2 50 2 0 1 1 0 0\n\
                    cpu1 5 1 3 50 2 1 1 2 0 0\nintr 1 2 3\nctxt 99\n";
        assert_eq!(parse_cpu_total(text), Some(CpuTotal { jiffies: 126 }));
        assert_eq!(parse_cpu_total("intr 1\n"), None);
    }

    #[test]
    fn cpu_percent_is_of_the_whole_cpu_over_the_elapsed_jiffies() {
        // 4 cores, 400 jiffies in all: 100 per core, so one busy core is 25 %.
        let prev = snapshot(
            1000,
            vec![
                pid(1, "idle", 0, 1),
                pid(2, "busy", 0, 1),
                pid(3, "two", 0, 1),
            ],
        );
        let cur = snapshot(
            1400,
            vec![
                pid(1, "idle", 0, 1),
                pid(2, "busy", 50, 1),
                pid(3, "two", 200, 1),
            ],
        );
        let top = top(Some(&prev), &cur, 5, 4096);
        assert_eq!(
            names(&top.by_cpu),
            [("two", 50.0), ("busy", 12.5), ("idle", 0.0)]
        );
    }

    #[test]
    fn without_a_previous_scan_cpu_is_zero_and_memory_still_ranks() {
        let cur = snapshot(1400, vec![pid(1, "small", 50, 10), pid(2, "big", 50, 300)]);
        let top = top(None, &cur, 5, 4096);
        assert!(top.by_cpu.iter().all(|p| p.cpu == 0.0));
        let memory: Vec<_> = top
            .by_memory
            .iter()
            .map(|p| (p.name.as_str(), p.rss))
            .collect();
        assert_eq!(memory, [("big", 300 * 4096), ("small", 10 * 4096)]);
    }

    #[test]
    fn a_reused_pid_is_a_new_process() {
        let prev = snapshot(1000, vec![pid(9, "old", 500, 1)]);
        let mut new = pid(9, "new", 520, 1);
        new.start = 2000;
        let top = top(Some(&prev), &snapshot(1400, vec![new]), 5, 4096);
        assert_eq!(names(&top.by_cpu), [("new", 0.0)]);
    }

    #[test]
    fn lists_keep_the_top_n() {
        let pids: Vec<_> = (1..=20)
            .map(|i| pid(i, "p", u64::from(i), u64::from(i)))
            .collect();
        let prev = snapshot(0, (1..=20).map(|i| pid(i, "p", 0, 1)).collect());
        let top = top(Some(&prev), &snapshot(400, pids), 5, 1);
        let cpu_pids: Vec<_> = top.by_cpu.iter().map(|p| p.pid).collect();
        let mem_pids: Vec<_> = top.by_memory.iter().map(|p| p.pid).collect();
        assert_eq!(cpu_pids, [20, 19, 18, 17, 16]);
        assert_eq!(mem_pids, [20, 19, 18, 17, 16]);
    }

    #[test]
    fn display_name_completes_a_cut_comm_from_the_program_name() {
        assert_eq!(
            display_name(
                "cosmic-ext-appl",
                b"/app/bin/cosmic-ext-applet-sysmon\0--flag\0"
            ),
            "cosmic-ext-applet-sysmon"
        );
        // A space-joined command line.
        assert_eq!(
            display_name(
                "Isolated Web Co",
                b"/usr/lib/firefox/firefox -contentproc\0"
            ),
            "Isolated Web Co"
        );
        assert_eq!(display_name("kworker/u32:1-e", b""), "kworker/u32:1-e");
        assert_eq!(display_name("bash", b"/usr/bin/bash-something\0"), "bash");
    }

    #[test]
    fn a_scan_measures_cpu_over_at_least_the_baseline() {
        let plan = |age: Option<u64>| plan_scan(age.map(Duration::from_millis));
        // Popup just opened: no scan, or one from before it was closed.
        let fresh = ScanPlan {
            new_baseline: true,
            wait: BASELINE,
        };
        assert_eq!(plan(None), fresh);
        assert_eq!(plan(Some(10_000)), fresh);
        // The usual tick: the previous scan is the baseline, no wait.
        assert_eq!(
            plan(Some(2000)),
            ScanPlan {
                new_baseline: false,
                wait: Duration::ZERO
            }
        );
        // Asked again right away: wait for the rest of the baseline.
        assert_eq!(
            plan(Some(100)),
            ScanPlan {
                new_baseline: false,
                wait: Duration::from_millis(150)
            }
        );
    }

    #[test]
    fn scanner_reads_this_machine_s_procfs() {
        let mut scanner = Scanner::new(Path::new("/proc"));
        let top = scanner.top(5);
        assert_eq!(top.by_memory.len(), 5);
        assert!(top.by_memory.iter().all(|p| p.rss > 0));
        assert!(top.by_cpu.iter().all(|p| p.cpu >= 0.0));
    }
}
