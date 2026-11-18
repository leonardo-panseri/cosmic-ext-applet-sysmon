// SPDX-License-Identifier: MPL-2.0

//! Hysteresis for the closed tier (D6): which new [`PanelSummary`] is worth a panel redraw.
//! Each panel redraw costs about 24 wakeups, and with CPU load and network rates shown almost
//! every sample changes some text, so while the popup is closed a summary is sent only when a
//! shown value moved enough from the value **last sent**, which is what the panel shows. Each
//! item follows the rule of its kind of value ([`ValueKind`]):
//!
//! - **Percentages** (CPU, RAM, swap, GPU, VRAM, disk usage): by at least 2 points of the shown
//!   whole number.
//! - **Temperatures**: by at least 1 °C of the shown whole number, so any change of their text.
//! - **Rates** (network and disk): by one display step, compared as shown, in bytes per second. While both
//!   the new and the sent rate are below 16 KiB/s (`"16K"`), background chatter, the rate is held
//!   whatever the ratio, `"0B"` included. Otherwise the higher must be at least 1.5 times the lower,
//!   either way: `"100K"` → `"150K"` or `"66K"` is a step, `"149K"` or `"67K"` isn't, nor is a
//!   change of unit alone (`"999K"` → `"1.0M"`); across the floor, `"4.0K"` → `"40K"` is a step,
//!   `"12K"` → `"17K"` isn't.
//! - **Always**: an item or a value appearing or disappearing, any change of the warnings (which
//!   also carry the tooltip's values) or of what takes the warning colour, a GPU item switching
//!   cards, and any text this module can't read back.
//!
//! A held value is sent anyway once the summary last sent is [`CATCH_UP`] old, so the panel never
//! shows a wrong value for long. Settings and orientation changes, and every sample while the
//! popup is open, skip the hysteresis ([`crate::sampler::core`]). Every summary sent is a real
//! sample's, never an average.

use std::time::{Duration, Instant};

use super::{PanelSummary, ShownItem, ValueKind};

/// How long a shown value may lag behind, under the thresholds of
/// [`PanelSummary::moved_enough`], before it is sent anyway.
pub const CATCH_UP: Duration = Duration::from_secs(30);

/// What the panel button shows while the popup is closed, and since when: the last summary
/// sent, and when its sample was taken.
#[derive(Debug, Default)]
pub struct PanelHysteresis {
    sent: Option<(PanelSummary, Instant)>,
}

impl PanelHysteresis {
    /// Whether `summary`, of the sample taken at `at`, is worth sending while the popup is
    /// closed: it [moved enough](PanelSummary::moved_enough) from the summary last sent, or it
    /// differs at all and the summary last sent is at least [`CATCH_UP`] old, so no reading stays
    /// wrong for long.
    #[must_use]
    pub fn worth_sending(&self, summary: &PanelSummary, at: Instant) -> bool {
        self.sent.as_ref().is_none_or(|(sent, sent_at)| {
            summary.moved_enough(sent)
                || (summary != sent && at.saturating_duration_since(*sent_at) >= CATCH_UP)
        })
    }

    /// Whether `summary` differs at all from the summary last sent.
    #[must_use]
    pub fn differs(&self, summary: &PanelSummary) -> bool {
        self.sent.as_ref().is_none_or(|(sent, _)| sent != summary)
    }

    /// Records that `summary`, of the sample taken at `at`, was sent.
    pub fn sent(&mut self, summary: PanelSummary, at: Instant) {
        self.sent = Some((summary, at));
    }
}

impl PanelSummary {
    /// Whether this summary differs enough from `last_sent`, the one the panel button shows, to
    /// be worth a redraw while the popup is closed.
    #[must_use]
    pub fn moved_enough(&self, last_sent: &Self) -> bool {
        let row_moved = |new: &[ShownItem], sent: &[ShownItem]| {
            new.len() != sent.len() || new.iter().zip(sent).any(|(n, s)| item_moved(n, s))
        };
        row_moved(&self.top, &last_sent.top)
            || row_moved(&self.bottom, &last_sent.bottom)
            || self.button_warns != last_sent.button_warns
            || self.warnings != last_sent.warnings
    }
}

/// Whether the item `new` moved enough from `sent`, the item in its place last sent.
fn item_moved(new: &ShownItem, sent: &ShownItem) -> bool {
    new.item != sent.item
        || new.warns != sent.warns
        || new.card != sent.card
        || value_moved(
            new.item.value_kind(),
            new.value.as_deref(),
            sent.value.as_deref(),
        )
}

/// A panel value read back from its text, as [`super::format`] wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// `"45%"`.
    Percent(i32),
    /// A network rate as shown, `"1.2K"` or `"340K"`, in tenths of a byte per second.
    Rate(u64),
}

/// The units of [`super::format::rate`], smallest first, each 1024 times the last.
const RATE_UNITS: [char; 6] = ['B', 'K', 'M', 'G', 'T', 'P'];

/// Rates below this, bytes per second, are background chatter: a rate moving between two of them
/// is held, whatever the ratio.
const RATE_FLOOR: u64 = 16 * 1024;

impl Shown {
    fn parse(text: &str) -> Option<Self> {
        if let Some(percent) = text.strip_suffix('%') {
            return percent.parse().ok().map(Self::Percent);
        }
        let unit_char = text.chars().last()?;
        let unit = RATE_UNITS.iter().position(|&u| u == unit_char)?;
        let number = &text[..text.len() - unit_char.len_utf8()];
        let (whole, tenth) = number.split_once('.').unwrap_or((number, "0"));
        let whole: u64 = whole.parse().ok()?;
        let tenth: u64 = tenth.parse().ok().filter(|t| *t < 10)?;
        let scale = 1024_u64.checked_pow(u32::try_from(unit).ok()?)?;
        whole
            .checked_mul(10)?
            .checked_add(tenth)?
            .checked_mul(scale)
            .map(Self::Rate)
    }
}

/// Whether a shown value of `kind` moved enough from `sent` to `new`, by the rules of the
/// module's documentation.
fn value_moved(kind: ValueKind, new: Option<&str>, sent: Option<&str>) -> bool {
    match (new, sent) {
        (None, None) => false,
        (Some(new), Some(sent)) if new == sent => false,
        (Some(new), Some(sent)) => match (kind, Shown::parse(new), Shown::parse(sent)) {
            (ValueKind::Percent, Some(Shown::Percent(new)), Some(Shown::Percent(sent))) => {
                (new - sent).abs() >= 2
            }
            (ValueKind::Rate, Some(Shown::Rate(new)), Some(Shown::Rate(sent))) => {
                rate_stepped(new, sent)
            }
            _ => true,
        },
        _ => true,
    }
}

/// Whether a rate moved one display step from `sent` to `new`, both in tenths of a byte per
/// second: one of them is at least [`RATE_FLOOR`], and the higher is at least 1.5 times the lower.
fn rate_stepped(new: u64, sent: u64) -> bool {
    let (low, high) = (new.min(sent), new.max(sent));
    high >= 10 * RATE_FLOOR && 2 * high >= 3 * low
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::{DiskRef, GpuRef, Item, NetRef, Warning};

    fn shown(item: Item, value: &str) -> ShownItem {
        ShownItem {
            item,
            value: Some(value.into()),
            warns: false,
            card: None,
        }
    }

    /// A summary showing CPU load `top` over RAM `bottom`, with no warnings.
    fn rows(top: &str, bottom: &str) -> PanelSummary {
        PanelSummary {
            top: vec![shown(Item::Cpu, top)],
            bottom: vec![shown(Item::Ram, bottom)],
            button_warns: false,
            warnings: vec![],
        }
    }

    /// A summary showing only `item` at `value`.
    fn one(item: Item, value: &str) -> PanelSummary {
        PanelSummary {
            top: vec![shown(item, value)],
            bottom: vec![],
            button_warns: false,
            warnings: vec![],
        }
    }

    #[test]
    fn a_percentage_moves_enough_at_two_points() {
        let sent = rows("12%", "45%");
        assert!(!rows("13%", "45%").moved_enough(&sent));
        assert!(!rows("11%", "46%").moved_enough(&sent));
        assert!(rows("14%", "45%").moved_enough(&sent));
        assert!(rows("12%", "43%").moved_enough(&sent));
        assert!(rows("0%", "45%").moved_enough(&rows("2%", "45%")));
        assert!(!rows("100%", "45%").moved_enough(&rows("99%", "45%")));
    }

    #[test]
    fn every_percentage_item_follows_the_percentage_rule() {
        for item in [
            Item::Swap,
            Item::GpuUsage(GpuRef::Auto),
            Item::Vram(GpuRef::Pci("0000:01:00.0".into())),
            Item::DiskUsage("/mnt/storage".into()),
        ] {
            let sent = one(item.clone(), "50%");
            assert!(!one(item.clone(), "51%").moved_enough(&sent), "{item:?}");
            assert!(one(item.clone(), "52%").moved_enough(&sent), "{item:?}");
        }
    }

    #[test]
    fn a_temperature_moves_enough_at_one_shown_degree() {
        for item in [
            Item::CpuTemp,
            Item::GpuTemp(GpuRef::Auto),
            Item::NvmeTemp(crate::panel::DiskId {
                model: "Samsung SSD".into(),
                name: "nvme0n1".into(),
            }),
        ] {
            let sent = one(item.clone(), "62°C");
            assert!(!one(item.clone(), "62°C").moved_enough(&sent), "{item:?}");
            assert!(one(item.clone(), "63°C").moved_enough(&sent), "{item:?}");
            assert!(one(item.clone(), "61°C").moved_enough(&sent), "{item:?}");
        }
    }

    #[test]
    fn another_item_in_a_place_always_moves() {
        // 62 % and 62 °C are different items, say after a settings change.
        assert!(one(Item::Cpu, "62%").moved_enough(&one(Item::CpuTemp, "62°C")));
        assert!(one(Item::Cpu, "62%").moved_enough(&one(Item::Ram, "62%")));
    }

    /// A summary showing the rates `down` and `up`, after the items of [`rows`].
    fn rates(down: &str, up: &str) -> PanelSummary {
        let mut summary = rows("12%", "45%");
        summary.top.push(shown(Item::Download(NetRef::All), down));
        summary.bottom.push(shown(Item::Upload(NetRef::All), up));
        summary
    }

    /// Whether the download rate moved enough from `sent` to `new`.
    fn rate_moved(new: &str, sent: &str) -> bool {
        rates(new, "1.0K").moved_enough(&rates(sent, "1.0K"))
    }

    #[test]
    fn rates_below_the_floor_are_held_whatever_the_ratio() {
        assert!(!rate_moved("15K", "1.0K"));
        assert!(!rate_moved("1.0K", "15K"));
        assert!(!rate_moved("2.1K", "983B"));
        assert!(!rate_moved("15K", "0B"));
        assert!(!rate_moved("0B", "15K"));
        assert!(!rate_moved("1B", "0B"));
    }

    #[test]
    fn a_rate_above_the_floor_moves_at_one_and_a_half_times() {
        assert!(!rate_moved("149K", "100K"));
        assert!(rate_moved("150K", "100K"));
        assert!(!rate_moved("67K", "100K"));
        assert!(rate_moved("66K", "100K"));
        assert!(!rate_moved("400K", "340K"));
        assert!(rate_moved("1.5M", "1.0M"));
        assert!(rate_moved("0B", "12M"));
    }

    #[test]
    fn a_unit_change_alone_is_not_a_step() {
        assert!(!rate_moved("1.0M", "999K"));
        assert!(!rate_moved("999K", "1.0M"));
    }

    #[test]
    fn crossing_the_floor_moves_at_one_and_a_half_times() {
        assert!(rate_moved("40K", "4.0K"));
        assert!(rate_moved("4.0K", "40K"));
        assert!(!rate_moved("17K", "12K"));
        assert!(rate_moved("18K", "12K"));
        assert!(rate_moved("16K", "0B"));
        assert!(rate_moved("0B", "16K"));
    }

    #[test]
    fn the_upload_rate_follows_the_same_rule() {
        let sent = rates("1.0K", "100K");
        assert!(!rates("1.0K", "149K").moved_enough(&sent));
        assert!(rates("1.0K", "150K").moved_enough(&sent));
    }

    #[test]
    fn a_disk_rate_follows_the_rate_rule() {
        let item = Item::DiskWrite(DiskRef::All);
        let sent = one(item.clone(), "100K");
        assert!(!one(item.clone(), "149K").moved_enough(&sent));
        assert!(one(item.clone(), "150K").moved_enough(&sent));
        assert!(!one(item.clone(), "15K").moved_enough(&one(item, "0B")));
    }

    #[test]
    fn a_value_or_an_item_appearing_or_disappearing_always_moves() {
        let sent = rates("340K", "1.0K");
        let no_rates = rows("12%", "45%");
        assert!(no_rates.moved_enough(&sent));
        assert!(sent.moved_enough(&no_rates));
        let mut missing = rows("12%", "45%");
        missing.bottom[0].value = None;
        assert!(missing.moved_enough(&rows("12%", "45%")));
        assert!(rows("12%", "45%").moved_enough(&missing));
    }

    #[test]
    fn a_warning_change_always_moves() {
        let sent = rows("12%", "89%");
        let warns = PanelSummary {
            warnings: vec![Warning::Memory {
                percent: 90,
                limit: 90,
            }],
            ..rows("12%", "90%")
        };
        assert!(warns.moved_enough(&sent));
        assert!(sent.moved_enough(&warns));
        // A warning's own value is part of the tooltip's text.
        let hotter = PanelSummary {
            warnings: vec![Warning::Memory {
                percent: 91,
                limit: 90,
            }],
            ..warns.clone()
        };
        assert!(hotter.moved_enough(&warns));
        // What takes the colour.
        let mut coloured = warns.clone();
        coloured.bottom[0].warns = true;
        assert!(coloured.moved_enough(&warns));
        let button = PanelSummary {
            button_warns: true,
            ..warns.clone()
        };
        assert!(button.moved_enough(&warns));
    }

    #[test]
    fn an_auto_gpu_switching_cards_always_moves() {
        let item = Item::GpuUsage(GpuRef::Auto);
        let mut sent = one(item.clone(), "33%");
        sent.top[0].card = Some("card1".into());
        let mut other = one(item, "34%");
        other.top[0].card = Some("card0".into());
        assert!(other.moved_enough(&sent));
    }

    #[test]
    fn the_first_summary_is_always_worth_sending() {
        let hysteresis = PanelHysteresis::default();
        assert!(hysteresis.worth_sending(&rows("12%", "45%"), Instant::now()));
    }

    #[test]
    fn a_held_value_catches_up_after_thirty_seconds() {
        let mut hysteresis = PanelHysteresis::default();
        let t0 = Instant::now();
        hysteresis.sent(rows("11%", "45%"), t0);
        let drifted = rows("12%", "45%");
        assert!(!hysteresis.worth_sending(&drifted, t0 + Duration::from_secs(2)));
        assert!(!hysteresis.worth_sending(&drifted, t0 + Duration::from_secs(29)));
        assert!(hysteresis.worth_sending(&drifted, t0 + Duration::from_secs(30)));
        // Nothing to catch up with: the same text is never sent again.
        assert!(!hysteresis.worth_sending(&rows("11%", "45%"), t0 + Duration::from_secs(60)));
    }

    #[test]
    fn the_catch_up_counts_from_the_last_send() {
        let mut hysteresis = PanelHysteresis::default();
        let t0 = Instant::now();
        hysteresis.sent(rows("11%", "45%"), t0);
        hysteresis.sent(rows("14%", "45%"), t0 + Duration::from_secs(20));
        let drifted = rows("13%", "45%");
        assert!(!hysteresis.worth_sending(&drifted, t0 + Duration::from_secs(30)));
        assert!(hysteresis.worth_sending(&drifted, t0 + Duration::from_secs(50)));
    }
}
