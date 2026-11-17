// SPDX-License-Identifier: MPL-2.0

//! The sampler: a thread that reads the system every tick (D2) and sends the UI what changed (D6).
//!
//! [`spawn`] starts it. The UI sends [`Control`] messages through the returned [`SamplerHandle`]
//! and receives [`Update`]s. The thread sleeps in `recv_timeout` on its control channel, so it
//! wakes exactly once per tick, or when told something, and a control message takes effect at
//! once. It stops when either side of the channels is dropped.
//!
//! The probe ([`probe_client`]) is called from this thread too, so its blocking calls never
//! reach the UI.

pub mod core;
pub mod history;
pub mod nvml_schedule;
pub mod rates;
pub mod readers;
pub mod stats;

use std::fmt::Write;
use std::sync::mpsc;
use std::time::Instant;

use cosmic::iced::futures::channel::mpsc as async_mpsc;

use self::core::Core;
pub use self::core::{Control, Update};
use self::readers::Readers;
use self::stats::Stats;
use crate::config::VramCache;
use crate::panel::{PanelSettings, format};
use crate::probe_client;

/// The UI's end of the control channel.
#[derive(Debug, Clone)]
pub struct SamplerHandle(mpsc::Sender<Control>);

impl SamplerHandle {
    /// Sends `control` to the sampler. Does nothing if it has stopped.
    pub fn send(&self, control: Control) {
        let _ = self.0.send(control);
    }
}

/// Starts the sampler thread with the panel button's `settings`.
///
/// # Panics
/// If the OS can't start a thread.
#[must_use]
pub fn spawn(settings: PanelSettings) -> (SamplerHandle, async_mpsc::UnboundedReceiver<Update>) {
    let (control_tx, control_rx) = mpsc::channel();
    let (update_tx, update_rx) = async_mpsc::unbounded();
    std::thread::Builder::new()
        .name("sysmon-sampler".into())
        .spawn(move || {
            let core = Core::new(settings).forced_open(probe_client::debug_open());
            run(core, &control_rx, &update_tx);
        })
        .expect("failed to start the sampler thread");
    (SamplerHandle(control_tx), update_rx)
}

fn run(
    mut core: Core,
    control: &mpsc::Receiver<Control>,
    updates: &async_mpsc::UnboundedSender<Update>,
) {
    let vram = VramCache::open(<crate::app::AppModel as cosmic::Application>::APP_ID);
    let mut readers = Readers::new(probe_client::connect(), vram);
    readers.debug_probe();
    loop {
        let now = Instant::now();
        let deadline = core.deadline(now);
        let update = if deadline <= now {
            let update = core.sample(readers.read(now, core.wants()), now);
            if probe_client::debug_open()
                && let Some(update) = &update
            {
                log_gpus(&update.stats);
            }
            update
        } else {
            match control.recv_timeout(deadline - now) {
                Ok(message) => core.control(message),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        };
        if let Some(update) = update
            && updates.unbounded_send(update).is_err()
        {
            return;
        }
    }
}

/// With `SYSMON_DEBUG_OPEN`, one line per GPU of each sample.
fn log_gpus(stats: &Stats) {
    let stamp = probe_client::stamp();
    for gpu in stats.gpus_nvidia_first() {
        let mut line = format!(
            "[{stamp}] gpu {} {:?} {:?}",
            gpu.card,
            gpu.name.as_deref().unwrap_or("?"),
            gpu.status
        );
        if let Some(busy) = gpu.busy {
            let _ = write!(line, " util {busy}%");
        }
        if let Some(c) = gpu.celsius {
            let _ = write!(line, " {}", format::celsius(c));
        }
        if let Some(w) = gpu.watts {
            let _ = write!(line, " {w:.1}W");
        }
        if let (Some(used), Some(total)) = (gpu.vram_used, gpu.vram_total) {
            let _ = write!(
                line,
                " VRAM {} / {}",
                format::used(used),
                format::size(total)
            );
        }
        eprintln!("{line}");
    }
}
