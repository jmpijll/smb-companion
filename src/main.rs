#![cfg_attr(not(test), windows_subsystem = "windows")]
mod app;
mod client;
mod minecraft;
mod signing;
mod system;
mod theme;
mod update;
mod worker;

use eframe::egui;
use std::{path::PathBuf, sync::Arc};

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    if flag("--minecraft-prelaunch") {
        let result = minecraft::service::prelaunch();
        if let Err(e) = &result {
            system::log(&format!("Minecraft launch blocked: {e}"));
        }
        std::process::exit(if result.is_ok() { 0 } else { 73 });
    }
    if flag("--quit") {
        // Used by the installer and uninstaller: close a running copy cleanly.
        let closed = system::quit_running_instance(std::time::Duration::from_secs(8));
        std::process::exit(if closed { 0 } else { 1 });
    }
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let demo = flag("--demo") || flag("--demo-minecraft");
    let background = flag("--background");
    let live = screenshot.is_none() && !demo;
    // `--screenshot <png> --live` connects to League first (development aid).
    let connect = !demo && (live || flag("--live"));
    if live {
        // A second launch just brings the running copy forward (unless it was
        // the sign-in autostart, which stays silent).
        if !system::claim_single_instance(background) {
            return Ok(());
        }
        system::lower_priority();
        std::panic::set_hook(Box::new(|info| system::log(&format!("Crash: {info}"))));
        system::log(&format!(
            "Started version {}{}{}.",
            env!("CARGO_PKG_VERSION"),
            if system::portable_dir().is_some() {
                " (portable)"
            } else if system::installed() {
                ""
            } else {
                " (not installed)"
            },
            if background { " in the tray" } else { "" }
        ));
    }
    // Never open over a running game: start in the tray instead.
    let start_hidden =
        live && (background || system::game_running() || system::minecraft_running());

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(system::APP_NAME)
        .with_app_id("smb-lol-companion")
        // Fixed width, no resizing: the app sizes the height to its content.
        .with_inner_size([app::WIDTH, 560.])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_active(!start_hidden && screenshot.is_none());
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/app-256.png"))
    {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    if start_hidden || screenshot.is_some() {
        viewport = viewport.with_position([system::OFFSCREEN, system::OFFSCREEN]);
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: !start_hidden && screenshot.is_none(),
        ..Default::default()
    };
    eframe::run_native(
        system::APP_NAME,
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::Companion::new(
                cc,
                screenshot,
                demo,
                connect,
                start_hidden,
            )))
        }),
    )
}
