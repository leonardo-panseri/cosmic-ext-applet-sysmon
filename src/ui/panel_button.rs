// SPDX-License-Identifier: MPL-2.0

//! The panel button (plan §2.1): one of three layouts of a [`PanelSummary`], with fixed-width
//! value cells so a changing value never re-lays out the panel (D6).

use std::borrow::Cow;

use cosmic::applet::Context;
use cosmic::iced::widget::text::LineHeight;
use cosmic::iced::widget::{row, stack};
use cosmic::iced::{Alignment, Color, Length};
use cosmic::prelude::*;
use cosmic::widget;

use crate::app::Message;
use crate::fl;
use crate::panel::{Layout, PanelSettings, PanelSummary, RowKind, format};

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

/// Advance width of one monospace glyph, relative to the font size (Noto Sans Mono, COSMIC's
/// default monospace font, and most others are 0.6 em).
const MONO_ADVANCE: f32 = 0.6;

/// One shown row: its label, value and whether it takes the warning colour.
struct Row {
    kind: RowKind,
    label: String,
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
                label: label(kind),
                value: value.unwrap_or_else(|| fl!("no-value")),
                warn: button_warns || summary.is_some_and(|s| s.row_warns(kind)),
            }
        })
        .collect();
    let rates = settings.network.then(|| {
        let rate = |r: Option<&String>| r.cloned().unwrap_or_else(|| fl!("no-value"));
        [
            (fl!("download"), rate(summary.and_then(|s| s.down.as_ref()))),
            (fl!("upload"), rate(summary.and_then(|s| s.up.as_ref()))),
        ]
    });

    let (icon_width, icon_height) = applet.suggested_size(true);
    let (major_padding, minor_padding) = applet.suggested_padding(true);
    let height = f32::from(icon_height + 2 * minor_padding);
    let button = match layout {
        Layout::Stacked => {
            // As `applet.text_button`, which takes only a single text. The panel is horizontal.
            let size = font_size(height, STACKED_FONT_SHARE);
            let (labels, values): (Vec<_>, Vec<_>) = rows
                .into_iter()
                .map(|row| {
                    (
                        panel_text(row.label, size, row.warn).into(),
                        value_cell(row.value, row.kind.widest_value(), size, row.warn),
                    )
                })
                .unzip();
            // A column of labels beside a column of values, so the values line up.
            let mut content = row![
                widget::Column::with_children(labels),
                widget::Column::with_children(values),
            ];
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
                content = content
                    .push(widget::Column::with_children(arrows))
                    .push(widget::Column::with_children(rates));
            }
            text_button(content.spacing(size / 2.0), height, major_padding)
        }
        Layout::OneLine => {
            let size = font_size(height, ONE_LINE_FONT_SHARE);
            let mut content = widget::Row::new();
            for row in rows {
                content = content
                    .push(panel_text(row.label, size, row.warn))
                    .push(value_cell(
                        row.value,
                        row.kind.widest_value(),
                        size,
                        row.warn,
                    ));
            }
            for (arrow, rate) in rates.into_iter().flatten() {
                content = content
                    .push(panel_text(arrow, size, button_warns))
                    .push(value_cell(rate, format::WIDEST_RATE, size, button_warns));
            }
            text_button(content.spacing(size / 3.0), height, major_padding)
        }
        Layout::Compact => {
            let Some(row) = rows.into_iter().next() else {
                return applet.button_from_element(widget::Space::new(), true);
            };
            let widest = row.kind.widest_value();
            // The value may use the icon's width and half the padding beside it.
            let room = f32::from(icon_width + minor_padding);
            #[expect(clippy::cast_precision_loss, reason = "a few characters")]
            let chars = widest.chars().count() as f32;
            let size = (room / (chars * MONO_ADVANCE)).floor().max(MIN_FONT_SIZE);
            applet.button_from_element(value_cell(row.value, widest, size, row.warn), true)
        }
    };
    // On press, not release, like COSMIC's own applets.
    button.on_press_down(Message::TogglePopup)
}

fn label(kind: RowKind) -> String {
    match kind {
        RowKind::Cpu => fl!("cpu"),
        RowKind::Memory => fl!("memory"),
        RowKind::Gpu => fl!("gpu"),
        RowKind::Disk => fl!("disk"),
        RowKind::CpuTemp => fl!("temperature"),
    }
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

/// A value, right-aligned in a cell exactly as wide as `widest`.
fn value_cell<'a>(
    value: String,
    widest: &'static str,
    size: f32,
    warn: bool,
) -> Element<'a, Message> {
    let mono = cosmic::font::mono();
    // The stack takes the size of its first layer: the widest value, drawn invisibly.
    stack![
        panel_text(widest, size, false)
            .font(mono)
            .class(cosmic::theme::Text::Color(Color::TRANSPARENT)),
        widget::container(panel_text(value, size, warn).font(mono)).align_right(Length::Fill),
    ]
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
