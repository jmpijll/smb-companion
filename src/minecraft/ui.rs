//! The Minecraft page only sends intents; discovery and updates stay off the GUI thread.
//! Same shape as the League page: one status line with the Play button, one
//! switch, everything else under Advanced.
use super::{
    modrinth,
    service::{Action, Handle, Settings, View},
};
use crate::theme::{self, *};
use eframe::egui::{self, FontFamily, Ui};
use std::time::{Duration, Instant};

/// While something is on its way (Modrinth installing, the hook being
/// pasted), Modrinth is looked at again on this interval: no refresh buttons.
const LOOK_AGAIN: Duration = Duration::from_secs(4);

pub struct Page {
    handle: Handle,
    live: bool,
    preview_settings: Option<Settings>,
    hook: Result<String, String>,
    copied: bool,
    error: String,
    looked: Instant,
}

impl Page {
    pub fn new(live: bool, demo: bool) -> Self {
        Self {
            handle: Handle::new(live, demo),
            live,
            preview_settings: None,
            hook: std::env::current_exe()
                .map_err(|e| format!("Could not find the companion executable: {e}"))
                .and_then(|exe| modrinth::hook_command(&exe)),
            copied: false,
            error: String::new(),
            looked: Instant::now(),
        }
    }

    pub fn busy(&self) -> bool {
        self.handle.snapshot().busy
    }

    /// Something went wrong that the player may want to look at.
    pub fn problem(&self) -> bool {
        !self.handle.snapshot().error.is_empty() || !self.error.is_empty()
    }

    fn settings(&self) -> Settings {
        self.preview_settings
            .clone()
            .unwrap_or_else(|| self.handle.settings())
    }

    /// The state in a word, and the one button that matters.
    pub fn hero(&mut self, ui: &mut Ui) {
        let view = self.handle.snapshot();
        let ready = view.selected.as_ref().is_some_and(|i| i.ready);
        let mut action = None;
        cut_panel(ui, ready && !view.busy, |ui| {
            let button = if view.modrinth_missing {
                Some(("Get Modrinth", true))
            } else if let Some(i) = &view.selected {
                Some(("Play", i.ready && !view.busy && !view.installing))
            } else if view.discovered && view.instances.is_empty() && view.error.is_empty() {
                Some(("Install", !view.busy && !view.installing))
            } else {
                None
            };
            let clicked = headline(ui, &view.headline(), ready || view.busy, |ui| {
                button.is_some_and(|(label, enabled)| {
                    theme::button_if(ui, label, true, enabled).clicked()
                })
            });
            if !view.version.is_empty() {
                let newer = !view.latest.is_empty() && view.latest != view.version;
                let line = if newer {
                    format!("SMB {} \u{2192} {}", view.version, view.latest)
                } else {
                    format!("SMB {}", view.version)
                };
                ui.horizontal(|ui| {
                    ui.add_space(20.);
                    theme::label(ui, &line, if newer { PINK } else { STEEL });
                });
            }
            if clicked {
                if view.modrinth_missing {
                    ui.ctx()
                        .open_url(egui::OpenUrl::new_tab("https://modrinth.com/app"));
                } else if view.selected.is_some() {
                    action = Some(Action::Play);
                } else {
                    action = Some(Action::Install);
                }
            }
        });
        // Something is still on its way: look again quietly instead of asking
        // the player to refresh.
        let waiting = view.instances.is_empty()
            || view.installing
            || view.selected.as_ref().is_some_and(|i| !i.ready)
            || (self.copied && !view.hook_ready);
        if waiting && !view.busy && self.looked.elapsed() > LOOK_AGAIN {
            self.looked = Instant::now();
            action.get_or_insert(Action::Look);
        }
        let settings = self.settings();
        self.commit(settings.clone(), settings, action);
    }

    /// The page's switch, inside the shared panel.
    pub fn switches(&mut self, ui: &mut Ui) {
        let mut settings = self.settings();
        let old = settings.clone();
        row(
            ui,
            |ui, r| paint_sync(ui.painter(), r, PINK),
            "Automatic updates",
            "",
            48.,
            |ui| {
                toggle(ui, &mut settings.automatic, "Automatic updates");
            },
        );
        self.commit(settings, old, None);
    }

    /// Under Advanced: which instance, in one quiet line.
    pub fn details(&mut self, ui: &mut Ui) {
        let view = self.handle.snapshot();
        if let Some(i) = &view.selected {
            text(
                ui,
                format!(
                    "{} · Minecraft {} · {} {}",
                    i.name,
                    i.minecraft,
                    capitalised(&i.loader),
                    i.loader_version
                ),
                11.,
                FontFamily::Proportional,
                STEEL_DIM,
            )
            .on_hover_text(i.directory.display().to_string());
        }
    }

    /// Under Advanced: pack check and repair, the optional launch hook.
    pub fn buttons(&mut self, ui: &mut Ui) {
        let view = self.handle.snapshot();
        let mut settings = self.settings();
        let old = settings.clone();
        let mut action = None;
        let enabled = !view.busy && view.selected.is_some();
        if theme::button_if(ui, "Check pack", false, enabled)
            .on_hover_text("Look for a new SMB pack now.")
            .clicked()
        {
            action = Some(Action::Check);
        }
        if theme::button_if(ui, "Repair pack", false, enabled)
            .on_hover_text("Put back missing or changed pack files. Your settings stay.")
            .clicked()
        {
            action = Some(Action::Repair);
        }
        if enabled {
            self.hook_button(ui, &view, &mut settings);
        }
        self.commit(settings, old, action);
    }

    /// Under Advanced: any problem.
    pub fn problems(&self, ui: &mut Ui) {
        problem(ui, &self.handle.snapshot().error);
        problem(ui, &self.error);
    }

    /// Optional: makes Modrinth's own Play wait for a pack check. Background
    /// checks already cover it, so this stays out of the way.
    fn hook_button(&mut self, ui: &mut Ui, view: &View, settings: &mut Settings) {
        let custom = view
            .selected
            .as_ref()
            .is_some_and(|p| !view.hook_ready && !p.pre_launch.is_empty());
        if view.hook_ready || custom {
            return;
        }
        if let Ok(command) = &self.hook {
            let label = if self.copied {
                "Copied"
            } else {
                "Copy launch hook"
            };
            if theme::button(ui, label, false)
                .on_hover_text(
                    "Modrinth: instance settings \u{2192} Launch hooks \u{2192} Pre-launch. Paste, save.",
                )
                .clicked()
            {
                ui.ctx().copy_text(command.clone());
                settings.direct_hook = true;
                self.copied = true;
            }
        }
    }

    fn commit(&mut self, settings: Settings, old: Settings, action: Option<Action>) {
        // Preview settings are held in memory. Even an interactive demo cannot
        // persist preferences, start a worker action or touch a real profile.
        if !self.live {
            self.preview_settings = Some(settings);
            return;
        }
        if settings != old {
            if let Err(error) = self.handle.save(settings) {
                self.error = error;
                return;
            }
            self.error.clear();
        }
        if let Some(action) = action {
            self.handle.send(action);
        }
    }
}

fn capitalised(word: &str) -> String {
    let mut c = word.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(view: View) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(380., 900.),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let width = ui.available_width();
                let mut page = Page::new(false, false);
                page.handle = Handle::with_view(view.clone());
                page.hero(ui);
                page.switches(ui);
                page.details(ui);
                ui.horizontal_wrapped(|ui| page.buttons(ui));
                page.problems(ui);
                assert!(
                    ui.min_rect().width() <= width + 1.,
                    "The Minecraft page widened the compact window"
                );
            });
        });
    }

    #[test]
    fn every_state_fits_the_compact_window() {
        let instance = modrinth::Instance {
            id: "local:a".into(),
            name: "SMB duplicate profile with a very long name to distinguish its purpose".into(),
            directory: "C:/Users/Player/AppData/Roaming/ModrinthApp/profiles/SMB personal backup"
                .into(),
            imported_version: "0.13.0".into(),
            minecraft: "26.3".into(),
            loader: "fabric".into(),
            loader_version: "0.20.1".into(),
            ready: true,
            ..Default::default()
        };
        for view in [
            View::default(),
            View {
                discovered: true,
                ..Default::default()
            },
            View {
                installing: true,
                ..Default::default()
            },
            View {
                modrinth_missing: true,
                ..Default::default()
            },
            View {
                discovered: true,
                selected: Some(instance.clone()),
                instances: vec![instance.clone()],
                version: "0.13.0".into(),
                latest: "0.14.0".into(),
                error: "A long problem description that has to wrap inside Advanced without widening anything at all".into(),
                ..Default::default()
            },
        ] {
            run(view);
        }
    }

    #[test]
    fn loader_names_read_like_names() {
        assert_eq!(capitalised("fabric"), "Fabric");
        assert_eq!(capitalised(""), "");
    }
}
