// SPDX-License-Identifier: MPL-2.0

//! The popup's small canvases (plan §2.2): a 2-minute history graph, an area in the accent colour
//! on `bg_component`, the per-core load bars and a filesystem's usage bar.
//!
//! Each keeps its geometry in a [`canvas::Cache`] and rebuilds it only when the values, the
//! colours or the size changed, so redraws for other reasons (hover, the other widgets) cost no
//! path building.

use std::cell::Cell;
use std::hash::{Hash, Hasher};

use cosmic::iced::mouse::Cursor;
use cosmic::iced::{Color, Length, Point, Rectangle, Size};
use cosmic::prelude::*;
use cosmic::widget::canvas::{self, Frame, Geometry, Path};

use crate::app::Message;
use crate::redraws;
use crate::sampler::history::{HISTORY_LEN, History};

/// The graphs' height.
pub const GRAPH_HEIGHT: f32 = 40.0;
/// The core bars' height.
const BAR_HEIGHT: f32 = 8.0;
/// A usage bar's height.
const USAGE_BAR_HEIGHT: f32 = 4.0;
/// The gap between two core bars.
const BAR_GAP: f32 = 3.0;
/// The fill's opacity under the line.
const AREA_ALPHA: f32 = 0.35;
const LINE_WIDTH: f32 = 1.5;
/// The centre line of a mirrored graph, relative to the text colour.
const AXIS_ALPHA: f32 = 0.25;
/// The bars' unfilled part, relative to the text colour.
const TRACK_ALPHA: f32 = 0.12;

/// A history graph scaled 0–100: CPU or GPU load.
pub fn percent(history: &History) -> Element<'_, Message> {
    canvas(HistoryGraph {
        above: history,
        below: None,
        scale: Scale::Percent,
    })
}

/// Two histories against one scale, `above` over a centre line and `below` mirrored under it:
/// network download and upload. The scale is the larger history's peak.
pub fn mirrored<'a>(above: &'a History, below: &'a History) -> Element<'a, Message> {
    canvas(HistoryGraph {
        above,
        below: Some(below),
        scale: Scale::Peak,
    })
}

/// Two percentages against 0–100 each, `above` over a centre line and `below` mirrored under
/// it: memory and swap in use.
pub fn mirrored_percent<'a>(above: &'a History, below: &'a History) -> Element<'a, Message> {
    canvas(HistoryGraph {
        above,
        below: Some(below),
        scale: Scale::Percent,
    })
}

/// A thin bar filled to `percent` in the accent colour, or the destructive colour if `warn`: a
/// filesystem's used space.
pub fn usage_bar<'a>(percent: f32, warn: bool) -> Element<'a, Message> {
    cosmic::widget::canvas(UsageBar { percent, warn })
        .width(Length::Fill)
        .height(Length::Fixed(USAGE_BAR_HEIGHT))
        .into()
}

/// One short bar per core, filled by its load; in the destructive colour at or above `warn`.
/// A core without a load yet shows empty.
pub fn core_bars(cores: &[Option<f32>], warn: f32) -> Element<'_, Message> {
    cosmic::widget::canvas(CoreBars { cores, warn })
        .width(Length::Fill)
        .height(Length::Fixed(BAR_HEIGHT))
        .into()
}

fn canvas(graph: HistoryGraph<'_>) -> Element<'_, Message> {
    cosmic::widget::canvas(graph)
        .width(Length::Fill)
        .height(Length::Fixed(GRAPH_HEIGHT))
        .into()
}

#[derive(Clone, Copy, Hash)]
enum Scale {
    /// 0 to 100.
    Percent,
    /// 0 to the largest value shown.
    Peak,
}

struct HistoryGraph<'a> {
    above: &'a History,
    below: Option<&'a History>,
    scale: Scale,
}

/// A geometry cache and what it was drawn from.
#[derive(Default)]
pub struct Cached {
    cache: canvas::Cache<cosmic::Renderer>,
    key: Cell<u64>,
}

impl Cached {
    /// The cached geometry, redrawn by `draw` first if `key` changed. The cache itself redraws
    /// when the size changes.
    fn draw(
        &self,
        renderer: &cosmic::Renderer,
        bounds: Rectangle,
        key: u64,
        draw: impl FnOnce(&mut Frame<cosmic::Renderer>),
    ) -> Geometry<cosmic::Renderer> {
        redraws::count(redraws::Event::CanvasDraw);
        if self.key.replace(key) != key {
            self.cache.clear();
        }
        self.cache.draw(renderer, bounds.size(), |frame| {
            redraws::count(redraws::Event::CanvasRebuild);
            draw(frame);
        })
    }
}

/// A hasher over what a canvas draws. Not for anything but telling frames apart.
fn key(hash: impl FnOnce(&mut std::collections::hash_map::DefaultHasher)) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash(&mut hasher);
    hasher.finish()
}

fn hash_color(color: Color, hasher: &mut impl Hasher) {
    for c in [color.r, color.g, color.b, color.a] {
        c.to_bits().hash(hasher);
    }
}

fn hash_history(history: &History, hasher: &mut impl Hasher) {
    history.iter().len().hash(hasher);
    for v in history.iter() {
        v.to_bits().hash(hasher);
    }
}

/// The colours the canvases use, from the theme.
struct Colors {
    background: Color,
    accent: Color,
    text: Color,
    warn: Color,
}

impl Colors {
    fn of(theme: &cosmic::Theme) -> Self {
        // The popup's own layer: translucent under frosted glass, as libcosmic's widgets pick.
        let cosmic = theme.cosmic();
        let layer = cosmic.background(theme.transparent);
        Self {
            background: layer.component.base.into(),
            accent: cosmic.accent_color().into(),
            text: layer.on.into(),
            warn: cosmic.destructive_color().into(),
        }
    }

    fn hash(&self, hasher: &mut impl Hasher) {
        for color in [self.background, self.accent, self.text, self.warn] {
            hash_color(color, hasher);
        }
    }
}

fn with_alpha(mut color: Color, alpha: f32) -> Color {
    color.a *= alpha;
    color
}

impl canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for HistoryGraph<'_> {
    type State = Cached;

    fn draw(
        &self,
        state: &Cached,
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let colors = Colors::of(theme);
        let key = key(|h| {
            colors.hash(h);
            self.scale.hash(h);
            hash_history(self.above, h);
            if let Some(below) = self.below {
                hash_history(below, h);
            }
        });
        let radius = theme.cosmic().corner_radii.radius_xs[0];
        vec![state.draw(renderer, bounds, key, |frame| {
            let size = frame.size();
            frame.fill(
                &Path::rounded_rectangle(Point::ORIGIN, size, radius.into()),
                colors.background,
            );
            let peak = match self.scale {
                Scale::Percent => 100.0,
                Scale::Peak => self
                    .above
                    .iter()
                    .chain(self.below.into_iter().flat_map(History::iter))
                    .fold(0.0, f32::max),
            };
            match self.below {
                None => area(frame, self.above, peak, size.height, size.height, &colors),
                Some(below) => {
                    let middle = (size.height / 2.0).round();
                    frame.fill_rectangle(
                        Point::new(0.0, middle - 0.5),
                        Size::new(size.width, 1.0),
                        with_alpha(colors.text, AXIS_ALPHA),
                    );
                    area(frame, self.above, peak, middle, middle, &colors);
                    area(frame, below, peak, middle, -(size.height - middle), &colors);
                }
            }
        })]
    }
}

/// Fills the area between `baseline` and the values of `history`, newest at the right edge,
/// and strokes its top. A value of `peak` reaches `reach` pixels up from the baseline; a
/// negative `reach` goes down.
fn area(
    frame: &mut Frame<cosmic::Renderer>,
    history: &History,
    peak: f32,
    baseline: f32,
    reach: f32,
    colors: &Colors,
) {
    let count = history.iter().len();
    if count < 2 || peak <= 0.0 {
        return;
    }
    let width = frame.width();
    // Full history spans the width; a shorter one is drawn at the same pace from the right.
    #[expect(clippy::cast_precision_loss, reason = "at most HISTORY_LEN points")]
    let step = width / (HISTORY_LEN - 1) as f32;
    #[expect(clippy::cast_precision_loss, reason = "at most HISTORY_LEN points")]
    let start = width - step * (count - 1) as f32;
    // Keep the line inside the frame.
    let inset = LINE_WIDTH / 2.0 * reach.signum();
    let point = |i: usize, v: f32| {
        #[expect(clippy::cast_precision_loss, reason = "at most HISTORY_LEN points")]
        let x = start + step * i as f32;
        let y = baseline - (v / peak).clamp(0.0, 1.0) * (reach - inset);
        Point::new(x, y)
    };
    let line = Path::new(|b| {
        for (i, v) in history.iter().enumerate() {
            if i == 0 {
                b.move_to(point(i, v));
            } else {
                b.line_to(point(i, v));
            }
        }
    });
    let fill = Path::new(|b| {
        b.move_to(Point::new(start, baseline));
        for (i, v) in history.iter().enumerate() {
            b.line_to(point(i, v));
        }
        b.line_to(Point::new(width, baseline));
        b.close();
    });
    frame.fill(&fill, with_alpha(colors.accent, AREA_ALPHA));
    frame.stroke(
        &line,
        canvas::Stroke::default()
            .with_color(colors.accent)
            .with_width(LINE_WIDTH),
    );
}

struct UsageBar {
    percent: f32,
    warn: bool,
}

impl canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for UsageBar {
    type State = Cached;

    fn draw(
        &self,
        state: &Cached,
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let colors = Colors::of(theme);
        let key = key(|h| {
            colors.hash(h);
            self.percent.to_bits().hash(h);
            self.warn.hash(h);
        });
        vec![state.draw(renderer, bounds, key, |frame| {
            let size = frame.size();
            let radius = size.height / 2.0;
            frame.fill(
                &Path::rounded_rectangle(Point::ORIGIN, size, radius.into()),
                with_alpha(colors.text, TRACK_ALPHA),
            );
            let share = self.percent.clamp(0.0, 100.0) / 100.0;
            if share <= 0.0 {
                return;
            }
            let color = if self.warn {
                colors.warn
            } else {
                colors.accent
            };
            frame.fill(
                &Path::rounded_rectangle(
                    Point::ORIGIN,
                    Size::new((size.width * share).max(size.height), size.height),
                    radius.into(),
                ),
                color,
            );
        })]
    }
}

struct CoreBars<'a> {
    cores: &'a [Option<f32>],
    warn: f32,
}

impl canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for CoreBars<'_> {
    type State = Cached;

    fn draw(
        &self,
        state: &Cached,
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let colors = Colors::of(theme);
        let key = key(|h| {
            colors.hash(h);
            self.warn.to_bits().hash(h);
            for core in self.cores {
                core.map(f32::to_bits).hash(h);
            }
        });
        vec![state.draw(renderer, bounds, key, |frame| {
            let count = self.cores.len();
            if count == 0 {
                return;
            }
            let size = frame.size();
            #[expect(clippy::cast_precision_loss, reason = "a few hundred cores at most")]
            let width = ((size.width - BAR_GAP * (count - 1) as f32) / count as f32).max(1.0);
            let radius = (size.height / 2.0).min(width / 2.0);
            for (i, core) in self.cores.iter().enumerate() {
                #[expect(clippy::cast_precision_loss, reason = "a few hundred cores at most")]
                let x = (width + BAR_GAP) * i as f32;
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(x, 0.0),
                        Size::new(width, size.height),
                        radius.into(),
                    ),
                    with_alpha(colors.text, TRACK_ALPHA),
                );
                let load = core.unwrap_or(0.0).clamp(0.0, 100.0);
                if load <= 0.0 {
                    continue;
                }
                let color = if load >= self.warn {
                    colors.warn
                } else {
                    colors.accent
                };
                let filled = (width * load / 100.0).max(size.height.min(width));
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(x, 0.0),
                        Size::new(filled, size.height),
                        radius.into(),
                    ),
                    color,
                );
            }
        })]
    }
}
