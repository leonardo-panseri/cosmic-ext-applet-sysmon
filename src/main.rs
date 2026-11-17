// SPDX-License-Identifier: MPL-2.0

mod app;
mod config;
mod i18n;
mod launch;
mod panel;
#[cfg(debug_assertions)]
mod preview;
mod probe_client;
mod sampler;
mod sources;
mod ui;
mod upower;

fn main() -> cosmic::iced::Result {
    // Get the system's preferred languages.
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    // Enable localizations to be applied.
    i18n::init(&requested_languages);

    #[cfg(debug_assertions)]
    if std::env::var_os("SYSMON_PREVIEW").is_some() {
        return preview::run();
    }

    // Starts the applet's event loop with `()` as the application's flags.
    cosmic::applet::run::<app::AppModel>(())
}
