// SPDX-License-Identifier: MPL-2.0

//! How many columns the popup's stats page takes (§2.2), and how big the popup may grow.
//!
//! [`PopupLayout::Auto`] compares an estimate of the popup's height in one column, counted from
//! the sample's devices, with the room the panel's output leaves it. The applet learns its
//! output's logical size from the Wayland output events ([`crate::outputs`]); until then, `Auto`
//! goes by the panel's orientation. The choice is made when the popup opens, the setting or the
//! output changes, or a sample arrives while it is open; never per frame (D6).

use crate::panel::PopupLayout;
use crate::sampler::stats::Stats;
use crate::ui::graph::GRAPH_HEIGHT;

/// The stats page's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Columns {
    One,
    /// CPU, memory and GPU beside network, disk and processes.
    Two,
}

/// The popup's width in one column.
const COLUMN_WIDTH: f32 = 372.0;
/// The popup's padding either side of its content: COSMIC's `space_s`.
const SIDE_PADDING: f32 = 16.0;
/// The space between two columns: the side padding either side of a divider.
const COLUMN_GAP: f32 = 2.0 * SIDE_PADDING + 1.0;

impl Columns {
    /// The popup's width: each column as wide as the single one's content.
    #[must_use]
    pub fn width(self) -> f32 {
        match self {
            Self::One => COLUMN_WIDTH,
            Self::Two => 2.0 * COLUMN_WIDTH - 2.0 * SIDE_PADDING + COLUMN_GAP,
        }
    }
}

/// The room the popup has on its output, logical pixels: the output less the panel and a margin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    pub width: f32,
    pub height: f32,
}

/// What the popup keeps clear of the output's edges beyond the panel's own surface: the panel's
/// padding and margin, the popup's offset from it, and a gap at the far edge. A dock that hides
/// when something overlaps it, as COSMIC's does, gives way to the popup.
const EDGE_MARGIN: f32 = 32.0;

impl Room {
    /// The room on an output of logical size `output` under a panel `panel` thick, along its
    /// top or bottom edge if `horizontal`, else along a side.
    #[must_use]
    pub fn on_output(output: (f32, f32), panel: f32, horizontal: bool) -> Self {
        let (width, height) = output;
        if horizontal {
            Self {
                width: width - EDGE_MARGIN,
                height: height - panel - EDGE_MARGIN,
            }
        } else {
            Self {
                width: width - panel - EDGE_MARGIN,
                height: height - EDGE_MARGIN,
            }
        }
    }

    /// Whether the popup, `one_column_height` high in one column, would scroll in this room
    /// and two columns fit across it.
    fn wants_two(self, one_column_height: f32) -> bool {
        one_column_height > self.height && self.width >= Columns::Two.width()
    }
}

/// The columns for `layout`, given the height the whole popup would need in one column and the
/// `room` it has, if known. Without it, `Auto` takes two columns on a horizontal panel, where
/// height is the scarce side, and one on a vertical panel, where width is.
#[must_use]
pub fn choose(
    layout: PopupLayout,
    one_column_height: f32,
    room: Option<Room>,
    horizontal: bool,
) -> Columns {
    let two = match (layout, room) {
        (PopupLayout::OneColumn, _) => false,
        (PopupLayout::TwoColumns, _) => true,
        (PopupLayout::Auto, Some(room)) => room.wants_two(one_column_height),
        (PopupLayout::Auto, None) => horizontal,
    };
    if two { Columns::Two } else { Columns::One }
}

/// The stats page's height when the room is unknown: it fits a 1080-pixel-high screen.
const PAGE_MAX_HEIGHT: f32 = 680.0;
/// The least the stats page scrolls in, on a tiny screen.
const MIN_PAGE_HEIGHT: f32 = 240.0;
/// The popup's height beside the stats page: its padding above and below, and the footer with
/// the space above it.
const CHROME: f32 = 16.0 + 16.0 + 16.0 + 37.0;

/// The height the stats page may take before it scrolls: what `room` leaves beside the popup's
/// padding and footer, or a height that fits most screens when the room is unknown.
#[must_use]
pub fn page_max_height(room: Option<Room>) -> f32 {
    room.map_or(PAGE_MAX_HEIGHT, |room| {
        (room.height - CHROME).max(MIN_PAGE_HEIGHT)
    })
}

/// The most the whole popup may be high: the stats page's height with the padding and footer.
#[must_use]
pub fn popup_max_height(room: Option<Room>) -> f32 {
    page_max_height(room) + CHROME
}

// The heights the stats page's parts lay out to, as `popup` builds them, for the estimate.
/// A row of body text, or a 16-pixel icon beside it.
const ROW: f32 = 21.0;
/// A section's or a process list's caption heading.
const CAPTION: f32 = 17.0;
/// The space between a section's rows: `space_xxxs`.
const ROW_GAP: f32 = 4.0;
/// The space either side of a divider between sections or blocks: `space_xs`.
const DIVIDER_GAP: f32 = 12.0;
/// The CPU section's head: the big total over the uptime, beside three lines of details.
const CPU_HEAD: f32 = 58.0;
/// The space between the CPU head, its graph and its core bars: `space_xxs`.
const CPU_GAP: f32 = 8.0;
/// The per-core bars.
const CORE_BARS: f32 = 8.0;
/// A filesystem's usage bar.
const USAGE_BAR: f32 = 4.0;
/// The processes each list shows with the popup open; the closed tier has none yet.
const TOP_PROCESSES: usize = sysmon_proto::MAX_TOP as usize;

/// About how high the whole popup is in one column with `stats`, unscrolled, for
/// [`PopupLayout::Auto`]. Each GPU counts its graph, which comes and goes as the card wakes and
/// sleeps, and the processes count as the open tier lists them, so the estimate holds steady.
#[must_use]
pub fn one_column_height(stats: &Stats) -> f32 {
    let sections = [
        Some(cpu()),
        Some(memory()),
        (!stats.gpus.is_empty()).then(|| gpus(stats.gpus.len())),
        Some(network(stats)),
        Some(disk(stats)),
        Some(processes()),
    ];
    let heights: Vec<f32> = sections.into_iter().flatten().collect();
    CHROME + stacked(&heights, 2.0 * DIVIDER_GAP + 1.0)
}

/// The height of `parts` one above the other, `gap` apart.
fn stacked(parts: &[f32], gap: f32) -> f32 {
    parts.iter().sum::<f32>() + gap * count(parts.len().saturating_sub(1))
}

/// A section: its caption, then `rows` high parts.
fn section(rows: &[f32]) -> f32 {
    let mut parts = vec![CAPTION];
    parts.extend_from_slice(rows);
    stacked(&parts, ROW_GAP)
}

/// Blocks set apart by an inset divider.
fn blocks(blocks: &[f32]) -> f32 {
    stacked(blocks, 2.0 * DIVIDER_GAP + 1.0)
}

fn cpu() -> f32 {
    stacked(&[CPU_HEAD, GRAPH_HEIGHT, CORE_BARS], CPU_GAP)
}

fn memory() -> f32 {
    section(&[ROW, ROW, GRAPH_HEIGHT])
}

fn gpus(count: usize) -> f32 {
    let gpu = stacked(&[ROW, GRAPH_HEIGHT, ROW], ROW_GAP);
    section(&[blocks(&vec![gpu; count])])
}

fn network(stats: &Stats) -> f32 {
    let in_use = stats
        .history
        .interfaces
        .iter()
        .filter(|h| h.had_traffic())
        .count();
    if in_use > 1 {
        let interface = stacked(&[ROW, ROW, ROW, GRAPH_HEIGHT], ROW_GAP);
        section(&[blocks(&vec![interface; in_use])])
    } else {
        section(&[ROW, ROW, GRAPH_HEIGHT])
    }
}

fn disk(stats: &Stats) -> f32 {
    let mount = ROW + ROW_GAP + USAGE_BAR;
    let mut disks: Vec<f32> = stats
        .disks
        .iter()
        .map(|disk| {
            let mut rows = vec![ROW];
            if !disk.name.is_empty() {
                rows.push(ROW);
            }
            rows.extend(std::iter::repeat_n(mount, disk.mounts.len()));
            stacked(&rows, ROW_GAP)
        })
        .collect();
    if !stats.other_mounts.is_empty() {
        let mut rows = vec![ROW];
        rows.extend(std::iter::repeat_n(mount, stats.other_mounts.len()));
        disks.push(stacked(&rows, ROW_GAP));
    }
    if disks.is_empty() {
        section(&[ROW])
    } else {
        section(&[blocks(&disks)])
    }
}

fn processes() -> f32 {
    let mut list = vec![CAPTION];
    list.extend(std::iter::repeat_n(ROW, TOP_PROCESSES));
    section(&[stacked(&list, ROW_GAP)])
}

#[expect(clippy::cast_precision_loss, reason = "a handful of parts")]
fn count(n: usize) -> f32 {
    n as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Room = Room {
        width: 1920.0,
        height: 1080.0 - 48.0 - 48.0,
    };

    #[test]
    fn auto_takes_two_columns_only_when_one_would_scroll() {
        assert_eq!(
            choose(PopupLayout::Auto, 1200.0, Some(LAPTOP), true),
            Columns::Two
        );
        assert_eq!(
            choose(PopupLayout::Auto, 900.0, Some(LAPTOP), true),
            Columns::One
        );
    }

    #[test]
    fn a_chosen_layout_holds_whatever_the_room() {
        for room in [Some(LAPTOP), None] {
            for (height, horizontal) in [(400.0, true), (2000.0, false)] {
                assert_eq!(
                    choose(PopupLayout::OneColumn, height, room, horizontal),
                    Columns::One
                );
                assert_eq!(
                    choose(PopupLayout::TwoColumns, height, room, horizontal),
                    Columns::Two
                );
            }
        }
    }

    #[test]
    fn without_the_room_auto_follows_the_panel() {
        assert_eq!(choose(PopupLayout::Auto, 400.0, None, true), Columns::Two);
        assert_eq!(choose(PopupLayout::Auto, 2000.0, None, false), Columns::One);
    }

    #[test]
    fn auto_keeps_one_column_where_two_would_not_fit_across() {
        let portrait = Room {
            width: 700.0,
            height: 800.0,
        };
        assert_eq!(
            choose(PopupLayout::Auto, 1200.0, Some(portrait), true),
            Columns::One
        );
    }

    #[test]
    fn the_room_is_the_output_less_the_panel_and_a_margin() {
        assert_eq!(
            Room::on_output((1920.0, 1080.0), 32.0, true),
            Room {
                width: 1888.0,
                height: 1016.0
            }
        );
        assert_eq!(
            Room::on_output((1920.0, 1080.0), 48.0, false),
            Room {
                width: 1840.0,
                height: 1048.0
            }
        );
    }

    #[test]
    fn the_page_scrolls_only_beyond_the_room_left_by_the_footer() {
        assert_eq!(page_max_height(Some(LAPTOP)), 899.0);
        assert_eq!(page_max_height(None), 680.0);
        // A tiny screen still shows a few rows.
        let tiny = Room {
            width: 800.0,
            height: 200.0,
        };
        assert_eq!(page_max_height(Some(tiny)), MIN_PAGE_HEIGHT);
    }

    #[test]
    fn the_estimate_matches_the_page_as_laid_out() {
        // The preview's laptop, measured in its screenshot: 1335 px in one column with the
        // NVIDIA card suspended, which the estimate counts with its 44 px graph.
        let estimate = one_column_height(&crate::preview::fixture());
        assert!((estimate - 1379.0).abs() <= 4.0, "{estimate}");
    }

    #[test]
    fn the_estimate_holds_while_a_gpu_wakes_and_sleeps() {
        let suspended = crate::preview::fixture();
        let mut awake = suspended.clone();
        for gpu in &mut awake.gpus {
            gpu.status = crate::sources::drm::RuntimeStatus::Active;
            gpu.busy = Some(12);
        }
        assert_eq!(one_column_height(&suspended), one_column_height(&awake));
    }
}
