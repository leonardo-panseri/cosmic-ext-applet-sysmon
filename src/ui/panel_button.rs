// SPDX-License-Identifier: MPL-2.0

//! The panel button (plan §2.1, `docs/panel-builder.md`): its two rows of items in one of two
//! layouts, each item a glyph and a value, with fixed-size glyphs and fixed-width value cells so a
//! changing value never re-lays out the panel (D6). A value starts right after its glyph; items
//! are further apart.
//!
//! - **Stacked**: the rows one above the other, in columns, so the items of both rows line up.
//! - **One line**: the top row, then the bottom row.
//! - On a **vertical panel**, the items go one below the other, each glyph above its value.

use std::borrow::Cow;

use cosmic::applet::Context;
use cosmic::iced::widget::text::LineHeight;
use cosmic::iced::widget::{row, stack};
use cosmic::iced::{Alignment, Color, Length};
use cosmic::prelude::*;
use cosmic::widget::{self, icon};

use crate::app::Message;
use crate::fl;
use crate::panel::{Item, Layout, PanelSettings, PanelSummary, Row, Warning};
use crate::ui::icons::{self, Glyph};

/// Line height of the panel text, relative to its size.
const LINE_HEIGHT: f32 = 1.2;

/// The share of the panel button's height given to one stacked row's font size. Two rows then
/// take `2 × 0.3 × LINE_HEIGHT` = 72 % of the height: 12 px text on an S panel (40 px button).
const STACKED_FONT_SHARE: f32 = 0.3;

/// The share of the panel button's height given to the one-line font size: 14 px on an S panel,
/// the size of COSMIC's own text applets there.
const ONE_LINE_FONT_SHARE: f32 = 0.35;

/// The smallest panel font size, so XS panels stay readable.
const MIN_FONT_SIZE: f32 = 9.0;

/// The gap between an icon and its own value, relative to the font size.
const ICON_GAP: f32 = 0.3;

/// The gap between one item (an icon and its value) and the next, relative to the font size.
const ITEM_GAP: f32 = 0.6;

/// The opacity of the line between the rows on one line, relative to the text colour.
const SEPARATOR_ALPHA: f32 = 0.35;

/// Advance width of one monospace glyph, relative to the font size (Noto Sans Mono, COSMIC's
/// default monospace font, and most others are 0.6 em).
const MONO_ADVANCE: f32 = 0.6;

/// One item as drawn: what it is, its value and whether it takes the warning colour.
struct Cell {
    item: Item,
    value: String,
    warn: bool,
}

/// The cells of `row`: from `summary`, or before the first sample `settings`' items without
/// values. Every cell warns when the whole button does.
fn cells(settings: &PanelSettings, summary: Option<&PanelSummary>, row: Row) -> Vec<Cell> {
    let no_value = || fl!("no-value");
    match summary {
        Some(summary) => summary
            .row(row)
            .iter()
            .map(|shown| Cell {
                item: shown.item.clone(),
                value: shown.value.clone().unwrap_or_else(no_value),
                warn: shown.warns || summary.button_warns,
            })
            .collect(),
        None => settings
            .rows
            .row(row)
            .iter()
            .map(|item| Cell {
                item: item.clone(),
                value: no_value(),
                warn: false,
            })
            .collect(),
    }
}

/// The panel button for `summary` (`None` before the first sample), laid out per `settings`.
/// It does nothing when pressed: the caller says what a press does.
pub fn panel_button<'a>(
    applet: &Context,
    settings: &PanelSettings,
    summary: Option<&PanelSummary>,
) -> widget::Button<'a, Message> {
    let top = cells(settings, summary, Row::Top);
    let bottom = cells(settings, summary, Row::Bottom);
    if top.is_empty() && bottom.is_empty() {
        // Nothing chosen: the CPU icon, so the popup can still be opened.
        let warn = summary.is_some_and(|s| s.button_warns);
        let (_, icon_height) = applet.suggested_size(true);
        return applet.button_from_element(glyph(&Item::Cpu, f32::from(icon_height), warn), true);
    }
    if !applet.is_horizontal() {
        return vertical_button(applet, settings.layout, top, bottom);
    }
    let (_, icon_height) = applet.suggested_size(true);
    let (major_padding, minor_padding) = applet.suggested_padding(true);
    let height = f32::from(icon_height + 2 * minor_padding);
    let content = match settings.layout {
        Layout::Stacked => stacked(top, bottom, font_size(height, STACKED_FONT_SHARE)),
        Layout::OneLine => one_line(top, bottom, font_size(height, ONE_LINE_FONT_SHARE)),
    };
    text_button(content, height, major_padding)
}

/// The rows one above the other, in columns: column `i` holds the `i`th item of each row, a
/// column of glyphs beside a column of values as wide as the wider of the two (D6), so the
/// values line up.
fn stacked<'a>(top: Vec<Cell>, bottom: Vec<Cell>, size: f32) -> Element<'a, Message> {
    let columns = top.len().max(bottom.len());
    let mut top = top.into_iter();
    let mut bottom = bottom.into_iter();
    let items = (0..columns)
        .map(|_| {
            let pair = [top.next(), bottom.next()];
            let widest = pair
                .iter()
                .flatten()
                .map(|cell| cell.item.value_kind().widest())
                .max_by_key(|w| w.chars().count())
                .unwrap_or_default();
            let (glyphs, values): (Vec<_>, Vec<_>) = pair
                .into_iter()
                .map(|cell| match cell {
                    Some(cell) => (
                        glyph(&cell.item, size, cell.warn),
                        value_cell(cell.value, widest, size, cell.warn),
                    ),
                    None => (blank_line(size), blank_line(size)),
                })
                .unzip();
            item(
                widget::Column::with_children(glyphs),
                widget::Column::with_children(values),
                size,
            )
        })
        .collect();
    items_row(items, size)
}

/// The top row, then the bottom row, on one line, a thin separator between them.
fn one_line<'a>(top: Vec<Cell>, bottom: Vec<Cell>, size: f32) -> Element<'a, Message> {
    let line = |cells: Vec<Cell>| -> Vec<Element<'a, Message>> {
        cells
            .into_iter()
            .map(|cell| {
                let widest = cell.item.value_kind().widest();
                item(
                    glyph(&cell.item, size, cell.warn),
                    value_cell(cell.value, widest, size, cell.warn),
                    size,
                )
            })
            .collect()
    };
    let both = !top.is_empty() && !bottom.is_empty();
    let mut items = line(top);
    if both {
        items.push(separator(size));
    }
    items.extend(line(bottom));
    items_row(items, size)
}

/// A thin vertical line, as tall as the text at `size`, in the text colour made faint.
fn separator<'a>(size: f32) -> Element<'a, Message> {
    let line = widget::container(widget::Space::new())
        .width(Length::Fixed(1.0))
        .height(Length::Fixed(size))
        .class(cosmic::theme::Container::custom(|theme| {
            let mut color: Color = theme.cosmic().background(theme.transparent).on.into();
            color.a *= SEPARATOR_ALPHA;
            widget::container::Style {
                background: Some(color.into()),
                ..widget::container::Style::default()
            }
        }));
    widget::container(line)
        .center_y(Length::Fixed(size * LINE_HEIGHT))
        .into()
}

/// A line of panel text's height with nothing in it, for a missing cell.
fn blank_line<'a>(size: f32) -> Element<'a, Message> {
    widget::Space::new()
        .height(Length::Fixed(size * LINE_HEIGHT))
        .into()
}

/// The space between the two columns of a vertical panel, pixels.
const COLUMN_GAP: f32 = 2.0;

/// The font size from which two columns of a vertical panel count as readable.
const READABLE_FONT_SIZE: f32 = 9.0;

/// How a vertical panel's items stand.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Vertical {
    /// The top row in a column on the left, the bottom row on the right; else one column, the
    /// top row first.
    two_columns: bool,
    /// The font size of every value.
    size: f32,
}

/// How a vertical panel's items stand in `layout`, when the values may take `room` pixels
/// across and the widest value of the top row has `top` characters, of the bottom row `bottom`
/// (0 for an empty row); no font bigger than `max_size`. Stacked takes two columns when both
/// rows have items and their values fit side by side at [`READABLE_FONT_SIZE`] or more; anything
/// else is one column, sized so the widest value of either row fits.
fn vertical(layout: Layout, room: f32, top: usize, bottom: usize, max_size: f32) -> Vertical {
    #[expect(clippy::cast_precision_loss, reason = "a few characters")]
    let fitting = |width: f32, chars: usize| (width / (chars.max(1) as f32 * MONO_ADVANCE)).floor();
    if layout == Layout::Stacked && top > 0 && bottom > 0 {
        let column = (room - COLUMN_GAP) / 2.0;
        let size = fitting(column, top.max(bottom));
        if size >= READABLE_FONT_SIZE {
            return Vertical {
                two_columns: true,
                size: size.min(max_size),
            };
        }
    }
    Vertical {
        two_columns: false,
        size: fitting(room, top.max(bottom)).clamp(MIN_FONT_SIZE, max_size.max(MIN_FONT_SIZE)),
    }
}

/// The button of a vertical panel, as wide as an icon button: the items one below the other,
/// each glyph above its value, both centred, in one column or two ([`vertical`]). The values may
/// use the icon's width and half the padding beside it.
fn vertical_button<'a>(
    applet: &Context,
    layout: Layout,
    top: Vec<Cell>,
    bottom: Vec<Cell>,
) -> widget::Button<'a, Message> {
    let (icon_width, _) = applet.suggested_size(true);
    let (major_padding, minor_padding) = applet.suggested_padding(true);
    let room = f32::from(icon_width + minor_padding);
    let widest = |cells: &[Cell]| {
        cells
            .iter()
            .map(|cell| cell.item.value_kind().widest().chars().count())
            .max()
            .unwrap_or(0)
    };
    // As big as the one-line text of a horizontal panel this thick.
    let max_size = font_size(
        f32::from(icon_width + 2 * minor_padding),
        ONE_LINE_FONT_SHARE,
    );
    let arrangement = vertical(layout, room, widest(&top), widest(&bottom), max_size);
    let size = arrangement.size;
    let content: Element<'a, Message> = if arrangement.two_columns {
        row![
            vertical_column(top.into_iter(), size),
            vertical_column(bottom.into_iter(), size),
        ]
        .spacing(COLUMN_GAP)
        .into()
    } else {
        vertical_column(top.into_iter().chain(bottom), size)
    };
    // As `applet.button_from_element`, but as long as the items need: the panel's width, and
    // the major (shrinkable) padding above and below.
    let width = f32::from(icon_width + 2 * minor_padding);
    widget::button::custom(widget::layer_container(content).center_x(Length::Fixed(width)))
        .padding([major_padding, 0])
        .class(cosmic::theme::Button::AppletIcon)
}

/// `cells` one below the other, each glyph above its value, both centred.
fn vertical_column<'a>(cells: impl Iterator<Item = Cell>, size: f32) -> Element<'a, Message> {
    widget::Column::with_children(cells.map(|cell| {
        let widest = cell.item.value_kind().widest();
        widget::Column::new()
            .align_x(Alignment::Center)
            .push(glyph(&cell.item, size, cell.warn))
            .push(value_cell_aligned(
                cell.value,
                widest,
                size,
                cell.warn,
                Alignment::Center,
            ))
            .into()
    }))
    .align_x(Alignment::Center)
    .spacing((size * ICON_GAP).round())
    .into()
}

/// The panel button's tooltip: the active warnings, one per line. Empty without warnings.
#[must_use]
pub fn tooltip(summary: Option<&PanelSummary>) -> String {
    let lines: Vec<String> = summary
        .into_iter()
        .flat_map(|s| s.warnings.iter().map(warning_text))
        .collect();
    lines.join("\n")
}

fn warning_text(warning: &Warning) -> String {
    match warning.clone() {
        Warning::CpuTemp { celsius, limit } => {
            fl!("warning-cpu-temp", celsius = celsius, limit = limit)
        }
        Warning::GpuTemp {
            card,
            name,
            celsius,
            limit,
        } => {
            let gpu = name.unwrap_or_else(|| fl!("gpu-unnamed", card = card));
            fl!(
                "warning-gpu-temp",
                gpu = gpu,
                celsius = celsius,
                limit = limit
            )
        }
        Warning::Memory { percent, limit } => {
            fl!("warning-memory", percent = percent, limit = limit)
        }
        Warning::Swap { percent, limit } => {
            fl!("warning-swap", percent = percent, limit = limit)
        }
        Warning::Vram {
            card,
            name,
            percent,
            limit,
        } => {
            let gpu = name.unwrap_or_else(|| fl!("gpu-unnamed", card = card));
            fl!("warning-vram", gpu = gpu, percent = percent, limit = limit)
        }
        Warning::Filesystem {
            mount,
            percent,
            limit,
        } => fl!(
            "warning-filesystem",
            mount = mount,
            percent = percent,
            limit = limit
        ),
        Warning::Nvme {
            drive,
            celsius,
            limit,
            rated,
        } if rated => fl!(
            "warning-nvme",
            drive = drive,
            celsius = celsius,
            limit = limit
        ),
        Warning::Nvme {
            drive,
            celsius,
            limit,
            ..
        } => fl!(
            "warning-nvme-override",
            drive = drive,
            celsius = celsius,
            limit = limit
        ),
    }
}

/// An item's glyph at the text's `size`, centred in a line of panel text; in the destructive
/// colour if `warn`. Always the same size, whatever the value (D6).
pub(crate) fn glyph<'a>(item: &Item, size: f32, warn: bool) -> Element<'a, Message> {
    let content: Element<'a, Message> = match icons::item(item) {
        Glyph::Icon(handle) => {
            // A few pixels, far inside u16.
            #[expect(clippy::cast_possible_truncation, reason = "see above")]
            #[expect(clippy::cast_sign_loss, reason = "see above")]
            let pixels = size.round() as u16;
            let mut icon = icon::icon(handle).size(pixels);
            if warn {
                icon = icon.class(cosmic::theme::Svg::custom(|theme| {
                    cosmic::iced::widget::svg::Style {
                        color: Some(theme.cosmic().destructive_color().into()),
                    }
                }));
            }
            icon.into()
        }
        Glyph::Text(text) => panel_text(text, size, warn).into(),
    };
    widget::container(content)
        .center_y(Length::Fixed(size * LINE_HEIGHT))
        .into()
}

/// The font size for text taking `share` of the button's `height`.
fn font_size(height: f32, share: f32) -> f32 {
    (height * share).round().max(MIN_FONT_SIZE)
}

/// A button holding text `content`, with the panel's height and padding.
fn text_button<'a>(
    content: impl Into<Element<'a, Message>>,
    height: f32,
    major_padding: u16,
) -> widget::Button<'a, Message> {
    let content = widget::Row::new().push(content).align_y(Alignment::Center);
    widget::button::custom(widget::layer_container(content).center_y(Length::Fixed(height)))
        .padding([0, major_padding])
        .class(cosmic::theme::Button::AppletIcon)
}

/// A value, left-aligned in a cell exactly as wide as `widest`, so it starts right after its
/// icon and the room a shorter value leaves falls between items.
fn value_cell<'a>(
    value: String,
    widest: &'static str,
    size: f32,
    warn: bool,
) -> Element<'a, Message> {
    value_cell_aligned(value, widest, size, warn, Alignment::Start)
}

/// A value, aligned per `align` in a cell exactly as wide as `widest`.
fn value_cell_aligned<'a>(
    value: String,
    widest: &'static str,
    size: f32,
    warn: bool,
    align: Alignment,
) -> Element<'a, Message> {
    let mono = cosmic::font::mono();
    // The stack takes the size of its first layer: the widest value, drawn invisibly.
    stack![
        panel_text(widest, size, false)
            .font(mono)
            .class(cosmic::theme::Text::Color(Color::TRANSPARENT)),
        widget::container(panel_text(value, size, warn).font(mono))
            .width(Length::Fill)
            .align_x(align),
    ]
    .into()
}

/// One item of the panel button: an icon (or arrow) and its value, close together.
fn item<'a>(
    icon: impl Into<Element<'a, Message>>,
    value: impl Into<Element<'a, Message>>,
    size: f32,
) -> Element<'a, Message> {
    row![icon.into(), value.into()]
        .spacing((size * ICON_GAP).round())
        .into()
}

/// The panel button's items side by side, further apart than an icon from its value.
fn items_row(items: Vec<Element<'_, Message>>, size: f32) -> Element<'_, Message> {
    widget::Row::with_children(items)
        .spacing((size * ITEM_GAP).round())
        .into()
}

/// Panel text at `size`, in the theme's interface font; in the destructive colour if `warn`.
fn panel_text<'a>(
    text: impl Into<Cow<'a, str>> + 'a,
    size: f32,
    warn: bool,
) -> widget::Text<'a, cosmic::Theme> {
    let text = widget::text(text)
        .size(size)
        .line_height(LineHeight::Relative(LINE_HEIGHT))
        .font(cosmic::font::default());
    if warn {
        text.class(cosmic::theme::Text::Custom(|theme| {
            cosmic::iced::widget::text::Style {
                color: Some(theme.cosmic().destructive_color().into()),
                ..cosmic::iced::widget::text::Catalog::style(theme, &cosmic::theme::Text::Default)
            }
        }))
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows' widest values: "100%" and "999K" are 4 characters, "100°C" 5.
    const PERCENT: usize = 4;
    const CELSIUS: usize = 5;

    /// The room on S and L vertical panels: the icon and half the padding beside it.
    const S: f32 = 20.0 + 10.0;
    const L: f32 = 32.0 + 16.0;

    fn one(size: f32) -> Vertical {
        Vertical {
            two_columns: false,
            size,
        }
    }

    #[test]
    fn one_line_is_one_column_sized_to_the_widest_value() {
        assert_eq!(
            vertical(Layout::OneLine, S, PERCENT, PERCENT, 14.0),
            one(12.0)
        );
        assert_eq!(
            vertical(Layout::OneLine, S, PERCENT, CELSIUS, 14.0),
            one(10.0)
        );
        // Never above the cap, nor below the smallest size.
        assert_eq!(vertical(Layout::OneLine, L, PERCENT, 0, 18.0), one(18.0));
        assert_eq!(
            vertical(Layout::OneLine, 10.0, CELSIUS, 0, 14.0),
            one(MIN_FONT_SIZE)
        );
    }

    #[test]
    fn stacked_takes_two_columns_where_both_fit_at_a_readable_size() {
        assert_eq!(
            vertical(Layout::Stacked, L, PERCENT, PERCENT, 22.0),
            Vertical {
                two_columns: true,
                size: 9.0
            }
        );
        assert_eq!(
            vertical(Layout::Stacked, 64.0, PERCENT, PERCENT, 22.0),
            Vertical {
                two_columns: true,
                size: 12.0
            }
        );
    }

    #[test]
    fn stacked_falls_back_to_one_column_where_two_would_be_too_small() {
        // S: 14 px a column fits 4 characters at 5 px.
        assert_eq!(
            vertical(Layout::Stacked, S, PERCENT, PERCENT, 14.0),
            one(12.0)
        );
        // L, but a temperature needs 5 characters: 7 px.
        assert_eq!(
            vertical(Layout::Stacked, L, PERCENT, CELSIUS, 22.0),
            one(16.0)
        );
    }

    #[test]
    fn stacked_with_an_empty_row_is_one_column() {
        assert_eq!(vertical(Layout::Stacked, 64.0, PERCENT, 0, 22.0), one(22.0));
        assert_eq!(vertical(Layout::Stacked, 64.0, 0, PERCENT, 22.0), one(22.0));
    }

    /// `text` without Fluent's bidi isolation marks around arguments.
    fn plain(text: &str) -> String {
        text.replace(['\u{2068}', '\u{2069}'], "")
    }

    #[test]
    fn the_tooltip_lists_each_warning_on_its_own_line() {
        let summary = PanelSummary {
            top: vec![],
            bottom: vec![],
            button_warns: true,
            warnings: vec![
                Warning::CpuTemp {
                    celsius: 91,
                    limit: 85,
                },
                Warning::GpuTemp {
                    card: "card1".into(),
                    name: None,
                    celsius: 88,
                    limit: 85,
                },
                Warning::Vram {
                    card: "card0".into(),
                    name: Some("RTX 4060".into()),
                    percent: 96,
                    limit: 90,
                },
                Warning::Memory {
                    percent: 93,
                    limit: 90,
                },
                Warning::Swap {
                    percent: 61,
                    limit: 50,
                },
                Warning::Filesystem {
                    mount: "/mnt/storage".into(),
                    percent: 97,
                    limit: 90,
                },
                Warning::Nvme {
                    drive: "Samsung SSD 980 PRO".into(),
                    celsius: 74,
                    limit: 70,
                    rated: true,
                },
                Warning::Nvme {
                    drive: "WD SN770".into(),
                    celsius: 66,
                    limit: 65,
                    rated: false,
                },
            ],
        };
        assert_eq!(
            plain(&tooltip(Some(&summary))),
            "CPU at 91°C, limit 85°C\n\
             GPU card1 at 88°C, limit 85°C\n\
             RTX 4060 VRAM 96% full, limit 90%\n\
             Memory 93% full, limit 90%\n\
             Swap 61% full, limit 50%\n\
             /mnt/storage 97% full, limit 90%\n\
             Samsung SSD 980 PRO at 74°C, its limit 70°C\n\
             WD SN770 at 66°C, limit 65°C"
        );
    }

    #[test]
    fn no_warnings_no_tooltip() {
        assert_eq!(tooltip(None), "");
        let summary = PanelSummary {
            top: vec![],
            bottom: vec![],
            button_warns: false,
            warnings: vec![],
        };
        assert_eq!(tooltip(Some(&summary)), "");
    }
}
