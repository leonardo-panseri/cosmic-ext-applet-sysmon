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
pub mod rates;
pub mod readers;
pub mod stats;

use std::sync::mpsc;
use std::time::Instant;

use cosmic::iced::futures::channel::mpsc as async_mpsc;

use self::core::Core;
pub use self::core::{Control, Update};
use self::readers::Readers;
use crate::panel::PanelSettings;
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
        .spawn(move || run(Core::new(settings), &control_rx, &update_tx))
        .expect("failed to start the sampler thread");
    (SamplerHandle(control_tx), update_rx)
}

fn run(
    mut core: Core,
    control: &mpsc::Receiver<Control>,
    updates: &async_mpsc::UnboundedSender<Update>,
) {
    let mut readers = Readers::new(probe_client::connect());
    readers.debug_probe();
    loop {
        let now = Instant::now();
        let deadline = core.deadline(now);
        let update = if deadline <= now {
            core.sample(readers.read(now, core.popup_open()), now)
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
