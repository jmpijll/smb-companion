#![cfg_attr(not(test), windows_subsystem = "windows")]
mod app;
mod client;
mod system;
mod theme;
mod worker;

use eframe::egui;
use std::{path::PathBuf, sync::Arc};

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
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
    let demo = flag("--demo");
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
    }
    // Never open over a running game: start in the tray instead.
    let start_hidden = live && (background || system::game_running());

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(system::APP_NAME)
        .with_app_id("smb-lol-companion")
        // Screenshot mode renders the whole page for documentation.
        .with_inner_size([440., if screenshot.is_some() { 900. } else { 760. }])
        .with_min_inner_size([400., 520.])
        .with_active(!start_hidden);
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/app-256.png"))
    {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    if start_hidden {
        viewport = viewport.with_position([system::OFFSCREEN, system::OFFSCREEN]);
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: !start_hidden,
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
