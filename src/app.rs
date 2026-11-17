// SPDX-License-Identifier: MPL-2.0

//! The panel button and its popup. For now both are static placeholders: the sampler and the
//! real panel button come in P1, the stats page in P3 (`docs/implementation-plan.md` §5).

use cosmic::iced::Limits;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::prelude::*;
use cosmic::widget;

use crate::fl;

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
    /// The compositor closed the popup, for example after a click outside it.
    PopupClosed(Id),
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
        let none = fl!("no-value");
        let label = format!("{} {none}  {} {none}", fl!("cpu"), fl!("memory"));
        // `text_button` presses on press, not release, like COSMIC's own applets.
        let button = applet.text_button(applet.text(label), Message::TogglePopup);
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
