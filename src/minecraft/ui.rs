//! The Minecraft page only sends intents; discovery and updates stay off the GUI thread.
//! Same shape as the League page: one status card, one panel of switches,
//! everything else folded away under troubleshooting.
use super::{
    modrinth,
    service::{Action, Handle, Settings, View},
};
use crate::theme::{self, *};
use eframe::egui::{
    self, Color32, FontFamily, FontId, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use std::time::{Duration, Instant};

/// While something is still missing (no SMB instance yet, hook not pasted
/// yet), Modrinth is looked at again on this interval: no refresh buttons.
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

    /// One line for the footer, like League's last activity.
    pub fn status_line(&self) -> String {
        let view = self.handle.snapshot();
        if view.busy {
            "Working on your pack".into()
        } else if view.installing || view.selected.as_ref().is_some_and(|i| !i.ready) {
            "Installing in Modrinth".into()
        } else if view.selected.is_some() && !view.version.is_empty() {
            format!("Pack {}", view.version)
        } else {
            "Waiting for Modrinth".into()
        }
    }

    fn settings(&self) -> Settings {
        self.preview_settings
            .clone()
            .unwrap_or_else(|| self.handle.settings())
    }

    /// Status card and switches.
    pub fn show(&mut self, ui: &mut Ui) {
        let view = self.handle.snapshot();
        let mut settings = self.settings();
        let old = settings.clone();
        let mut action = None;
        // A single SMB instance in Modrinth is the one. Nothing to choose.
        if settings.instance_id.is_empty() && view.instances.len() == 1 {
            settings.instance_id = view.instances[0].id.clone();
        }
        hero(ui, &view, &mut settings, &mut action);
        self.automation(ui, &view, &mut settings);
        // Something is still on its way (Modrinth, the pack, the hook): look
        // again quietly instead of asking the player to refresh.
        let waiting = view.instances.is_empty()
            || view.installing
            || view.selected.as_ref().is_some_and(|i| !i.ready)
            || (self.copied && !view.hook_ready);
        if waiting && !view.busy && self.looked.elapsed() > LOOK_AGAIN {
            self.looked = Instant::now();
            action.get_or_insert(Action::Look);
        }
        self.commit(settings, old, action);
    }

    /// Folded-away instance choice, pack check and repair, and any problem.
    pub fn troubleshooting(&mut self, ui: &mut Ui) {
        let view = self.handle.snapshot();
        let mut settings = self.settings();
        let old = settings.clone();
        let mut action = None;
        ui.add_space(12.);
        egui::CollapsingHeader::new(
            RichText::new("INSTANCE & TROUBLESHOOTING")
                .font(FontId::new(10.5, semibold()))
                .color(STEEL),
        )
        .icon(collapse_icon)
        .show(ui, |ui| {
            body(
                ui,
                "Only this instance is updated. Worlds, screenshots and your own changes stay as they are.",
                STEEL,
            );
            ui.add_space(6.);
            if view.instances.len() > 1 {
                chooser(ui, &view, &mut settings);
            }
            if let Some(i) = &view.selected {
                text(
                    ui,
                    i.directory.display().to_string(),
                    10.5,
                    FontFamily::Proportional,
                    STEEL_DIM,
                )
                .on_hover_text(i.id.clone());
            }
            ui.add_space(8.);
            ui.add_enabled_ui(!view.busy && view.selected.is_some(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if theme::button(ui, "Check pack", false)
                        .on_hover_text("Look for a new SMB pack now, and finish an interrupted update.")
                        .clicked()
                    {
                        action = Some(Action::Check);
                    }
                    if theme::button(ui, "Repair pack", false)
                        .on_hover_text("Put back missing or broken pack files. Your own changes stay.")
                        .clicked()
                    {
                        action = Some(Action::Repair);
                    }
                });
            });
            if view.selected.is_some() {
                ui.add_space(10.);
                self.strict_check(ui, &view, &mut settings);
            }
        });
        for error in [&view.error, &self.error] {
            if !error.is_empty() {
                ui.add_space(10.);
                alert(ui, error);
            }
        }
        self.commit(settings, old, action);
    }

    fn automation(&mut self, ui: &mut Ui, view: &View, settings: &mut Settings) {
        section(ui, "Automatic setup");
        panel().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(!view.busy, |ui| {
                row(
                    ui,
                    |ui, r| paint_sync(ui.painter(), r, PINK),
                    "Pack updates",
                    "Kept current while Minecraft is closed.",
                    48.,
                    |ui| {
                        toggle(ui, &mut settings.automatic, "Pack updates");
                    },
                );
                spaced_hairline(ui);
                row(
                    ui,
                    |ui, r| paint_play(ui.painter(), r, PINK),
                    "Launch after update",
                    "Play goes straight to Modrinth.",
                    48.,
                    |ui| {
                        toggle(ui, &mut settings.launch_after_update, "Launch after update");
                    },
                );
            });
        });
    }

    /// Optional: Modrinth's own Play waits for a pack check. Background
    /// updates already cover it; this makes it strict.
    fn strict_check(&mut self, ui: &mut Ui, view: &View, settings: &mut Settings) {
        let custom = view
            .selected
            .as_ref()
            .is_some_and(|p| !view.hook_ready && !p.pre_launch.is_empty());
        let detail = if view.hook_ready {
            "On. Modrinth's Play waits for a pack check."
        } else if custom {
            "Your own Modrinth launch hook stays as it is."
        } else {
            "Optional. Paste once at Launch hooks → Pre-launch."
        };
        let offer = !view.hook_ready && !custom;
        row(
            ui,
            |ui, r| paint_hook(ui.painter(), r, PINK),
            "Strict launch check",
            detail,
            if offer { 104. } else { 0. },
            |ui| {
                if !offer {
                    return;
                }
                match &self.hook {
                    Ok(command) => {
                        let label = if self.copied { "Copied" } else { "Copy hook" };
                        if theme::button(ui, label, false)
                            .on_hover_text(
                                "In Modrinth: instance settings → Launch hooks → Pre-launch. \
                                 Paste, save. The companion notices by itself.",
                            )
                            .clicked()
                        {
                            ui.ctx().copy_text(command.clone());
                            settings.direct_hook = true;
                            self.copied = true;
                        }
                    }
                    Err(error) => {
                        ui.label(RichText::new("!").color(PINK))
                            .on_hover_text(error);
                    }
                }
            },
        );
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

fn hero(ui: &mut Ui, view: &View, settings: &mut Settings, action: &mut Option<Action>) {
    let ready = view.selected.is_some() && view.error.is_empty() && !view.busy;
    cut_panel(ui, ready, |ui| {
        let (dot, label) = if view.busy {
            (PINK, "Working on your pack")
        } else if view.selected.is_some() {
            (PINK, "Modrinth connected")
        } else {
            (STEEL_DIM, "Modrinth")
        };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.), Sense::hover());
            if dot == PINK {
                ui.painter()
                    .circle_filled(r.center(), 5., PINK.gamma_multiply(0.25));
            }
            ui.painter().circle_filled(r.center(), 3., dot);
            theme::label(ui, label, if dot == PINK { PINK } else { STEEL });
        });
        ui.add_space(4.);
        text(ui, view.title.to_uppercase(), 30., display(), WHITE);
        body(ui, &view.detail, STEEL);
        if view.selected.is_none() && view.instances.len() > 1 {
            ui.add_space(10.);
            chooser(ui, view, settings);
        }
        if view.modrinth_missing {
            let get = pack_card(ui, "Needs Modrinth", None, "Get Modrinth", true);
            if get {
                ui.ctx()
                    .open_url(egui::OpenUrl::new_tab("https://modrinth.com/app"));
            }
        } else if view.instances.is_empty() {
            let line = if view.installing {
                "Waiting for Modrinth"
            } else {
                "Not installed"
            };
            if pack_card(ui, line, None, "Install", !view.busy && !view.installing) {
                *action = Some(Action::Install);
            }
        } else if let Some(instance) = &view.selected {
            let line = if !instance.ready {
                "Installing in Modrinth".to_owned()
            } else if view.version.is_empty() {
                "Not checked yet".to_owned()
            } else if !view.latest.is_empty() && view.latest != view.version {
                format!("{} → {}", view.version, view.latest)
            } else if view.latest == view.version {
                format!("{} · up to date", view.version)
            } else {
                view.version.clone()
            };
            let runtime = format!(
                "Minecraft {} · {} {}",
                instance.minecraft,
                capitalised(&instance.loader),
                instance.loader_version
            );
            let label = if settings.launch_after_update {
                "Play"
            } else {
                "Update"
            };
            let enabled = !view.busy && instance.ready && instance.id == settings.instance_id;
            if pack_card(ui, &line, Some(&runtime), label, enabled) {
                *action = Some(Action::Play);
            }
        }
    });
}

/// The pack row in the status card: block tile, name, one status line, an
/// optional runtime line and the single main button. Returns whether it was clicked.
fn pack_card(ui: &mut Ui, line: &str, runtime: Option<&str>, button: &str, enabled: bool) -> bool {
    ui.add_space(12.);
    hairline(ui);
    ui.add_space(12.);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(54.), Sense::hover());
        ui.painter().rect_filled(rect, 2, VOID);
        paint_block(ui.painter(), rect.shrink(12.), PINK);
        ui.painter()
            .rect_stroke(rect, 2, Stroke::new(1.0_f32, PINK), StrokeKind::Inside);
        ui.add_space(6.);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.;
            text(ui, "SMB pack", 17., semibold(), WHITE);
            let color: Color32 = if line.contains('→') { PINK } else { STEEL };
            theme::label(ui, line, color);
            if let Some(runtime) = runtime {
                text(ui, runtime, 11., FontFamily::Proportional, STEEL);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(enabled, |ui| theme::button(ui, button, true))
                .inner
                .clicked()
        })
        .inner
    })
    .inner
}

/// Which SMB instance to keep current, shown only when there is a choice.
fn chooser(ui: &mut Ui, view: &View, settings: &mut Settings) {
    ui.add_enabled_ui(!view.busy, |ui| {
        let chosen = view.instances.iter().find(|p| p.id == settings.instance_id);
        egui::ComboBox::from_id_salt("minecraft-instance")
            .width(ui.available_width())
            .truncate()
            .selected_text(
                chosen
                    .map(|p| format!("{} · {}", p.name, p.imported_version))
                    .unwrap_or_else(|| "Choose your SMB instance".into()),
            )
            .show_ui(ui, |ui| {
                ui.set_max_width(500.);
                for instance in &view.instances {
                    let folder = instance
                        .directory
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    ui.selectable_value(
                        &mut settings.instance_id,
                        instance.id.clone(),
                        format!(
                            "{} · {} · {}",
                            instance.name, instance.imported_version, folder
                        ),
                    )
                    .on_hover_text(instance.directory.display().to_string());
                }
            });
    });
}

fn spaced_hairline(ui: &mut Ui) {
    ui.add_space(8.);
    hairline(ui);
    ui.add_space(8.);
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

    fn instance(id: &str) -> modrinth::Instance {
        modrinth::Instance {
            id: id.into(),
            name: "SMB duplicate profile with a very long name to distinguish its purpose".into(),
            directory: "C:/Users/Player/AppData/Roaming/ModrinthApp/profiles/SMB personal backup"
                .into(),
            imported_version: "0.13.0".into(),
            ..Default::default()
        }
    }

    #[test]
    fn long_instance_names_fit_the_compact_window() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let view = View {
            instances: vec![instance("local:a"), instance("local:b")],
            selected: Some(instance("local:a")),
            version: "0.13.0".into(),
            latest: "0.13.0".into(),
            ..Default::default()
        };
        let mut settings = Settings {
            instance_id: "local:a".into(),
            ..Default::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400., 900.),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let width = ui.available_width();
                chooser(ui, &view, &mut settings);
                hero(ui, &view, &mut settings, &mut None);
                assert!(
                    ui.min_rect().width() <= width + 1.,
                    "The Minecraft page widened the compact window"
                );
            });
        });
    }

    #[test]
    fn install_and_get_modrinth_cards_fit_the_compact_window() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        for view in [
            View::default(),
            View {
                installing: true,
                ..Default::default()
            },
            View {
                modrinth_missing: true,
                ..Default::default()
            },
        ] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400., 900.),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let width = ui.available_width();
                    let mut action = None;
                    hero(ui, &view, &mut Settings::default(), &mut action);
                    assert!(ui.min_rect().width() <= width + 1.);
                    assert!(action.is_none());
                });
            });
        }
    }
    #[test]
    fn loader_names_read_like_names() {
        assert_eq!(capitalised("fabric"), "Fabric");
        assert_eq!(capitalised(""), "");
    }
}
