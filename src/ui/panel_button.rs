// SPDX-License-Identifier: MPL-2.0

//! The panel button (plan §2.1): one of three layouts of a [`PanelSummary`], each row an icon and
//! a value, with fixed-size icons and fixed-width value cells so a changing value never re-lays
//! out the panel (D6). A value starts right after its icon; items are further apart.

use std::borrow::Cow;

use cosmic::applet::Context;
use cosmic::iced::widget::text::LineHeight;
use cosmic::iced::widget::{row, stack};
use cosmic::iced::{Alignment, Color, Length};
use cosmic::prelude::*;
use cosmic::widget::{self, icon};

use crate::app::Message;
use crate::fl;
use crate::panel::{Layout, PanelSettings, PanelSummary, RowKind, Warning, format};
use crate::ui::icons;

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

/// Advance width of one monospace glyph, relative to the font size (Noto Sans Mono, COSMIC's
/// default monospace font, and most others are 0.6 em).
const MONO_ADVANCE: f32 = 0.6;

/// One shown row: what it shows, its value and whether it takes the warning colour.
struct Row {
    kind: RowKind,
    value: String,
    warn: bool,
}

/// The panel button for `summary` (`None` before the first sample), laid out per `settings`.
pub fn panel_button<'a>(
    applet: &Context,
    settings: &PanelSettings,
    summary: Option<&PanelSummary>,
) -> widget::Button<'a, Message> {
    let layout = settings.layout_for(applet.is_horizontal());
    let shown = settings.shown_rows(layout);
    let button_warns = summary.is_some_and(|s| s.button_warns(&shown));
    let rows: Vec<Row> = shown
        .iter()
        .enumerate()
        .map(|(i, &kind)| {
            let value = summary.and_then(|s| {
                if i == 0 {
                    s.top.clone()
                } else {
                    s.bottom.clone()
                }
            });
            Row {
                kind,
                value: value.unwrap_or_else(|| fl!("no-value")),
                warn: button_warns || summary.is_some_and(|s| s.row_warns(kind)),
            }
        })
        .collect();
    let rates = settings.shows_rates(layout).then(|| {
        let rate = |r: Option<&String>| r.cloned().unwrap_or_else(|| fl!("no-value"));
        [
            (fl!("download"), rate(summary.and_then(|s| s.down.as_ref()))),
            (fl!("upload"), rate(summary.and_then(|s| s.up.as_ref()))),
        ]
    });

    let (_, icon_height) = applet.suggested_size(true);
    let (major_padding, minor_padding) = applet.suggested_padding(true);
    let height = f32::from(icon_height + 2 * minor_padding);
    let button = match layout {
        Layout::Stacked => {
            // As `applet.text_button`, which takes only a single text. The panel is horizontal.
            let size = font_size(height, STACKED_FONT_SHARE);
            let (icons, values): (Vec<_>, Vec<_>) = rows
                .into_iter()
                .map(|row| {
                    (
                        row_icon(row.kind, size, row.warn),
                        value_cell(row.value, row.kind.widest_value(), size, row.warn),
                    )
                })
                .unzip();
            // A column of icons beside a column of values, so the values line up.
            let mut items = vec![item(
                widget::Column::with_children(icons),
                widget::Column::with_children(values),
                size,
            )];
            if let Some(rates) = rates {
                let (arrows, rates): (Vec<_>, Vec<_>) = rates
                    .into_iter()
                    .map(|(arrow, rate)| {
                        (
                            panel_text(arrow, size, button_warns).into(),
                            value_cell(rate, format::WIDEST_RATE, size, button_warns),
                        )
                    })
                    .unzip();
                items.push(item(
                    widget::Column::with_children(arrows),
                    widget::Column::with_children(rates),
                    size,
                ));
            }
            text_button(items_row(items, size), height, major_padding)
        }
        Layout::Compact if !applet.is_horizontal() => match rows.into_iter().next() {
            Some(row) => vertical_button(applet, row),
            None => applet.button_from_element(widget::Space::new(), true),
        },
        // On a horizontal panel only the height limits Compact: its one row on one line.
        Layout::OneLine | Layout::Compact => {
            let size = font_size(height, ONE_LINE_FONT_SHARE);
            let mut items: Vec<_> = rows
                .into_iter()
                .map(|row| {
                    item(
                        row_icon(row.kind, size, row.warn),
                        value_cell(row.value, row.kind.widest_value(), size, row.warn),
                        size,
                    )
                })
                .collect();
            items.extend(rates.into_iter().flatten().map(|(arrow, rate)| {
                item(
                    panel_text(arrow, size, button_warns),
                    value_cell(rate, format::WIDEST_RATE, size, button_warns),
                    size,
                )
            }));
            text_button(items_row(items, size), height, major_padding)
        }
    };
    // On press, not release, like COSMIC's own applets.
    button.on_press_down(Message::TogglePopup)
}

/// The Compact button of a vertical panel, as square as an icon button: the value may use the
/// icon's width and half the padding beside it, and the icon goes above it, both centred, if the
/// button's height leaves room.
fn vertical_button<'a>(applet: &Context, row: Row) -> widget::Button<'a, Message> {
    let (icon_width, icon_height) = applet.suggested_size(true);
    let (major_padding, minor_padding) = applet.suggested_padding(true);
    let widest = row.kind.widest_value();
    let room = f32::from(icon_width + minor_padding);
    #[expect(clippy::cast_precision_loss, reason = "a few characters")]
    let chars = widest.chars().count() as f32;
    let size = (room / (chars * MONO_ADVANCE)).floor().max(MIN_FONT_SIZE);
    let mut content = widget::Column::new().align_x(Alignment::Center);
    // On a vertical panel the major (shrinkable) padding is above and below.
    if 2.0 * size * LINE_HEIGHT <= f32::from(icon_height + 2 * major_padding) {
        content = content.push(row_icon(row.kind, size, row.warn));
    }
    let value = value_cell_aligned(row.value, widest, size, row.warn, Alignment::Center);
    applet.button_from_element(content.push(value), true)
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
        } => fl!(
            "warning-nvme",
            drive = drive,
            celsius = celsius,
            limit = limit
        ),
    }
}

/// A row's icon at the text's `size`, centred in a line of panel text; in the destructive colour
/// if `warn`. Always the same size, whatever the value (D6).
fn row_icon<'a>(kind: RowKind, size: f32, warn: bool) -> Element<'a, Message> {
    // A few pixels, far inside u16.
    #[expect(clippy::cast_possible_truncation, reason = "see above")]
    #[expect(clippy::cast_sign_loss, reason = "see above")]
    let pixels = size.round() as u16;
    let mut icon = icon::icon(icons::row(kind)).size(pixels);
    if warn {
        icon = icon.class(cosmic::theme::Svg::custom(|theme| {
            cosmic::iced::widget::svg::Style {
                color: Some(theme.cosmic().destructive_color().into()),
            }
        }));
    }
    widget::container(icon)
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

    /// `text` without Fluent's bidi isolation marks around arguments.
    fn plain(text: &str) -> String {
        text.replace(['\u{2068}', '\u{2069}'], "")
    }

    #[test]
    fn the_tooltip_lists_each_warning_on_its_own_line() {
        let summary = PanelSummary {
            top: None,
            bottom: None,
            down: None,
            up: None,
            gpu_card: None,
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
                Warning::Memory {
                    percent: 93,
                    limit: 90,
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
                },
            ],
        };
        assert_eq!(
            plain(&tooltip(Some(&summary))),
            "CPU at 91°C, limit 85°C\n\
             GPU card1 at 88°C, limit 85°C\n\
             Memory 93% full, limit 90%\n\
             /mnt/storage 97% full, limit 90%\n\
             Samsung SSD 980 PRO at 74°C, its limit 70°C"
        );
    }

    #[test]
    fn no_warnings_no_tooltip() {
        assert_eq!(tooltip(None), "");
        let summary = PanelSummary {
            top: Some("12%".into()),
            bottom: None,
            down: None,
            up: None,
            gpu_card: None,
            warnings: vec![],
        };
        assert_eq!(tooltip(Some(&summary)), "");
    }
}
