// SPDX-License-Identifier: MPL-2.0

//! The panel button and its popup. For now both show static placeholders: the sampler comes in
//! P1, the stats page in P3 (`docs/implementation-plan.md` §5).

use std::borrow::Cow;

use cosmic::applet::Context;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::widget::text::LineHeight;
use cosmic::iced::widget::{row, stack};
use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Color, Length, Limits};
use cosmic::prelude::*;
use cosmic::widget;

use crate::fl;

/// The widest value a value cell must hold. Each cell is as wide as this text in the monospace
/// font, whatever it shows, so a changing value never re-lays out the panel (plan D6).
const WIDEST_VALUE: &str = "100%";

/// Line height of the panel text, relative to its size.
const LINE_HEIGHT: f32 = 1.2;

/// The share of the panel button's height given to one stacked row's font size. Two rows then
/// take `2 × 0.3 × LINE_HEIGHT` = 72 % of the height: 12 px text on an S panel (40 px button).
const STACKED_FONT_SHARE: f32 = 0.3;

/// The smallest panel font size, so XS panels stay readable.
const MIN_FONT_SIZE: f32 = 9.0;

/// Advance width of one monospace glyph, relative to the font size (Noto Sans Mono, COSMIC's
/// default monospace font, and most others are 0.6 em).
const MONO_ADVANCE: f32 = 0.6;

/// The applet's state.
pub struct AppModel {
    /// Application state managed by the COSMIC runtime.
    core: cosmic::Core,
    /// The popup's window, while it is open.
    popup: Option<Id>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A left click on the panel button.
    TogglePopup,
    /// A right click on the panel button.
    OpenProcessMonitor,
    /// The compositor closed the popup, for example after a click outside it.
    PopupClosed(Id),
}

/// How the panel button lays out its rows (plan §2.1). One line comes in P1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelLayout {
    /// Two small rows in one panel slot, each a label and a value.
    Stacked,
    /// Only the top row's value. Vertical panels always use it.
    Compact,
}

/// One panel button row.
struct PanelRow {
    label: String,
    value: String,
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;

    type Flags = ();

    type Message = Message;

    const APP_ID: &'static str = "io.github.leonardo_panseri.cosmic-ext-applet-sysmon";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        let app = AppModel { core, popup: None };
        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let applet = &self.core.applet;
        // Static placeholders until the sampler lands in P1.
        let rows = [
            PanelRow {
                label: fl!("cpu"),
                value: "12%".into(),
            },
            PanelRow {
                label: fl!("memory"),
                value: "45%".into(),
            },
        ];
        let layout = if applet.is_horizontal() {
            PanelLayout::Stacked
        } else {
            PanelLayout::Compact
        };
        // The button takes only left clicks, so a right click reaches the mouse area. The mouse
        // area lays out as its content, so it changes neither the size nor the hover highlight.
        let button = widget::mouse_area(panel_button(applet, layout, rows))
            .on_right_press(Message::OpenProcessMonitor);
        // Without the autosize wrapper the panel clips the button to icon size.
        applet.autosize_window(button).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let content = widget::container(widget::text::body(fl!("placeholder"))).padding(16);
        self.core.applet.popup_container(content).into()
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    return destroy_popup(id);
                }
                let Some(main) = self.core.main_window_id() else {
                    return Task::none();
                };
                let new_id = Id::unique();
                self.popup = Some(new_id);
                let mut popup_settings = self
                    .core
                    .applet
                    .get_popup_settings(main, new_id, None, None, None);
                popup_settings.positioner.size_limits = Limits::NONE
                    .max_width(372.0)
                    .min_width(300.0)
                    .min_height(60.0)
                    .max_height(1080.0);
                return get_popup(popup_settings);
            }
            // The launch through `flatpak-spawn --host` comes with the popup's button (plan D5).
            Message::OpenProcessMonitor => eprintln!("would open the process monitor"),
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

/// The panel button, with libcosmic's applet sizes and padding so it matches COSMIC's own applets.
fn panel_button<'a>(
    applet: &Context,
    layout: PanelLayout,
    rows: impl IntoIterator<Item = PanelRow>,
) -> widget::Button<'a, Message> {
    let (icon_width, icon_height) = applet.suggested_size(true);
    let (major_padding, minor_padding) = applet.suggested_padding(true);
    let button = match layout {
        PanelLayout::Stacked => {
            // As `applet.text_button`, which takes only a single text. The panel is horizontal.
            let height = f32::from(icon_height + 2 * minor_padding);
            let size = (height * STACKED_FONT_SHARE).round().max(MIN_FONT_SIZE);
            let (labels, values): (Vec<_>, Vec<_>) = rows
                .into_iter()
                .map(|row| {
                    (
                        panel_text(row.label, size).into(),
                        value_cell(row.value, size),
                    )
                })
                .unzip();
            // A column of labels beside a column of values, so the values line up.
            let content = row![
                widget::Column::with_children(labels),
                widget::Column::with_children(values),
            ]
            .spacing(size / 2.0)
            .align_y(Alignment::Center);
            widget::button::custom(widget::layer_container(content).center_y(Length::Fixed(height)))
                .padding([0, major_padding])
                .class(cosmic::theme::Button::AppletIcon)
        }
        PanelLayout::Compact => {
            // The value may use the icon's width and half the padding beside it.
            let room = f32::from(icon_width + minor_padding);
            #[expect(clippy::cast_precision_loss, reason = "a few characters")]
            let chars = WIDEST_VALUE.chars().count() as f32;
            let size = (room / (chars * MONO_ADVANCE)).floor().max(MIN_FONT_SIZE);
            let value = rows
                .into_iter()
                .next()
                .map_or_else(String::new, |row| row.value);
            applet.button_from_element(value_cell(value, size), true)
        }
    };
    // On press, not release, like COSMIC's own applets.
    button.on_press_down(Message::TogglePopup)
}

/// A value, right-aligned in a cell exactly as wide as [`WIDEST_VALUE`].
fn value_cell<'a>(value: String, size: f32) -> Element<'a, Message> {
    let mono = cosmic::font::mono();
    // The stack takes the size of its first layer: the widest value, drawn invisibly.
    stack![
        panel_text(WIDEST_VALUE, size)
            .font(mono)
            .class(cosmic::theme::Text::Color(Color::TRANSPARENT)),
        widget::container(panel_text(value, size).font(mono)).align_right(Length::Fill),
    ]
    .into()
}

/// Panel text at `size`, in the theme's interface font.
fn panel_text<'a>(
    text: impl Into<Cow<'a, str>> + 'a,
    size: f32,
) -> widget::Text<'a, cosmic::Theme> {
    widget::text(text)
        .size(size)
        .line_height(LineHeight::Relative(LINE_HEIGHT))
        .font(cosmic::font::default())
}
