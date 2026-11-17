// SPDX-License-Identifier: MPL-2.0

//! Counts of what the UI thread does between two samples, to check D6 ("redraw only when what's
//! on screen changes"): messages handled, views built and canvases drawn. Counting is a relaxed
//! atomic add; with `SYSMON_DEBUG` the counts since the previous sample are logged as each
//! sample arrives, so the log itself wakes nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// What is counted.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    /// A message handled by `update`.
    Update,
    /// The panel button's view built.
    View,
    /// The popup's view built.
    ViewWindow,
    /// A canvas (graph or core bars) drawn: each frame of the stats page draws every canvas.
    CanvasDraw,
    /// A canvas's geometry rebuilt because what it shows changed.
    CanvasRebuild,
}

static COUNTS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// Counts one `event`.
pub fn count(event: Event) {
    COUNTS[event as usize].fetch_add(1, Ordering::Relaxed);
}

/// The counts since the last call, in [`Event`] order, and resets them.
fn take() -> [u64; 5] {
    std::array::from_fn(|i| COUNTS[i].swap(0, Ordering::Relaxed))
}

/// With `SYSMON_DEBUG`, logs the counts since the previous sample; called as a sample arrives.
pub fn log_on_sample(previous: &mut Option<Instant>) {
    if !crate::probe_client::debug() {
        return;
    }
    let now = Instant::now();
    let [updates, views, window_views, draws, rebuilds] = take();
    let cpu = thread_cpu();
    let cpu_ms = cpu.saturating_sub(LAST_CPU.swap(cpu, Ordering::Relaxed)) / 1_000_000;
    if let Some(since) = previous.replace(now) {
        eprintln!(
            "[{}] redraws: {:.2} s since the last sample: {updates} updates, {views} views, \
             {window_views} popup views, {draws} canvas draws, {rebuilds} canvas rebuilds, \
             {cpu_ms} ms UI-thread CPU",
            crate::probe_client::stamp(),
            (now - since).as_secs_f64(),
        );
    }
}

/// The UI thread's CPU time at the previous log, in nanoseconds.
static LAST_CPU: AtomicU64 = AtomicU64::new(0);

/// This thread's CPU time so far, in nanoseconds, from `/proc/thread-self/schedstat`; 0 if it
/// can't be read.
fn thread_cpu() -> u64 {
    std::fs::read_to_string("/proc/thread-self/schedstat")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}
