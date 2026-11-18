// SPDX-License-Identifier: MPL-2.0

mod app;
mod config;
mod i18n;
mod launch;
mod outputs;
mod panel;
#[cfg(debug_assertions)]
mod preview;
mod probe_client;
mod redraws;
mod sampler;
mod sources;
mod ui;
mod upower;

/// Sets the tokio runtime's worker count. libcosmic's `applet::run` ignores
/// `Application::Executor` and starts iced's default runtime, with one worker per CPU; each
/// message from the sampler then wakes several idle workers in turn, about 11 wakeups per update
/// where one worker needs about 4 (measured). One worker is what libcosmic's own default executor
/// uses. Not passed on to programs the applet starts ([`launch`]).
pub const TOKIO_WORKER_THREADS: &str = "TOKIO_WORKER_THREADS";

fn main() -> cosmic::iced::Result {
    if std::env::var_os(TOKIO_WORKER_THREADS).is_none() {
        // SAFETY: no other thread runs yet, so none can read the environment meanwhile.
        unsafe { std::env::set_var(TOKIO_WORKER_THREADS, "1") };
    }

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
