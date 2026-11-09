//! The Minecraft page only sends intents; discovery and updates stay off the GUI thread.
use super::{
    modrinth,
    service::{Action, Handle, Settings, View},
};
use crate::theme::{self, *};
use eframe::egui::{self, FontFamily, FontId, RichText, Ui};

pub struct Page {
    handle: Handle,
    live: bool,
    preview_settings: Option<Settings>,
    hook: Result<String, String>,
    error: String,
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
            error: String::new(),
        }
    }

    pub fn busy(&self) -> bool {
        self.handle.snapshot().busy
    }

    pub fn show(&mut self, ui: &mut Ui) {
        let view = self.handle.snapshot();
        let mut settings = self
            .preview_settings
            .clone()
            .unwrap_or_else(|| self.handle.settings());
        let old = settings.clone();
        let mut action = None;
        profile(ui, &view, &mut settings, &mut action);
        ui.add_space(12.);
        status(ui, &view, &settings, &mut action);
        preferences(ui, &view, &mut settings);
        if settings.direct_hook {
            hook_setup(ui, &view, &self.hook, self.live, &mut action);
        }
        repair(ui, &view, &mut action);
        for error in [&view.error, &self.error] {
            if !error.is_empty() {
                ui.add_space(8.);
                panel().show(ui, |ui| {
                    theme::label(ui, "Needs attention", PINK);
                    body(ui, error, WHITE);
                });
            }
        }
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

fn profile(ui: &mut Ui, view: &View, settings: &mut Settings, action: &mut Option<Action>) {
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::label(ui, "Modrinth instance", STEEL);
        ui.add_space(4.);
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
        if let Some(instance) = view.instances.iter().find(|p| p.id == settings.instance_id) {
            text(
                ui,
                instance.directory.display().to_string(),
                10.5,
                FontFamily::Proportional,
                STEEL,
            )
            .on_hover_text(instance.id.clone());
        } else if view.instances.is_empty() {
            body(
                ui,
                "Import the SMB pack in Modrinth, then refresh to find it here.",
                STEEL,
            );
        } else {
            body(
                ui,
                "Choose the folder you play in. Only this instance will be updated.",
                STEEL,
            );
        }
        ui.add_space(4.);
        ui.add_enabled_ui(!view.busy, |ui| {
            if theme::button(ui, "Refresh Modrinth", false).clicked() {
                *action = Some(Action::Discover);
            }
        });
    });
}

fn status(ui: &mut Ui, view: &View, settings: &Settings, action: &mut Option<Action>) {
    cut_panel(ui, view.selected.is_some() && view.error.is_empty(), |ui| {
        theme::label(
            ui,
            if view.busy {
                "Working on your pack"
            } else {
                "Minecraft // SMB"
            },
            PINK,
        );
        ui.add_space(6.);
        text(ui, view.title.to_uppercase(), 30., display(), WHITE);
        body(ui, &view.detail, STEEL);
        ui.add_space(12.);
        ui.horizontal_wrapped(|ui| {
            theme::label(ui, "Installed", STEEL);
            text(
                ui,
                if view.version.is_empty() {
                    "Not verified"
                } else {
                    &view.version
                },
                13.,
                semibold(),
                WHITE,
            );
            ui.add_space(10.);
            theme::label(ui, "Latest", STEEL);
            text(
                ui,
                if view.latest.is_empty() {
                    "Not checked"
                } else {
                    &view.latest
                },
                13.,
                semibold(),
                WHITE,
            );
        });
        if let Some(instance) = &view.selected {
            text(
                ui,
                format!(
                    "Minecraft {} · {} {}",
                    instance.minecraft, instance.loader, instance.loader_version
                ),
                11.,
                FontFamily::Proportional,
                STEEL,
            );
        }
        ui.add_space(12.);
        ui.add_enabled_ui(
            !view.busy
                && view
                    .selected
                    .as_ref()
                    .is_some_and(|p| p.id == settings.instance_id),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    if theme::button(
                        ui,
                        if settings.launch_after_update {
                            "Play SMB"
                        } else {
                            "Update pack"
                        },
                        true,
                    )
                    .clicked()
                    {
                        *action = Some(Action::Play);
                    }
                    if theme::button(ui, "Check update", false).clicked() {
                        *action = Some(Action::Check);
                    }
                });
            },
        );
        ui.add_space(6.);
        text(
            ui,
            "Your saves, screenshots and personal settings stay yours.",
            11.,
            FontFamily::Proportional,
            STEEL,
        );
    });
}

fn preferences(ui: &mut Ui, view: &View, settings: &mut Settings) {
    section(ui, "Before you play");
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_enabled_ui(!view.busy, |ui| {
            preference(
                ui,
                "Automatic updates",
                "Keep the selected SMB pack current before play.",
                &mut settings.automatic,
            );
            divider(ui);
            preference(
                ui,
                "Launch after update",
                "Hand Play SMB to Modrinth when the pack is ready.",
                &mut settings.launch_after_update,
            );
            divider(ui);
            preference(
                ui,
                "Check direct Modrinth launches",
                "Run the same check through a one-time launch hook.",
                &mut settings.direct_hook,
            );
        });
    });
}

fn preference(ui: &mut Ui, title: &str, detail: &str, on: &mut bool) {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 56.).max(120.);
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 2.;
            text(ui, title, 13., semibold(), WHITE);
            text(ui, detail, 11.5, FontFamily::Proportional, STEEL);
        });
        toggle(ui, on, title);
    });
}

fn divider(ui: &mut Ui) {
    ui.add_space(5.);
    ui.separator();
    ui.add_space(5.);
}

fn hook_setup(
    ui: &mut Ui,
    view: &View,
    command: &Result<String, String>,
    live: bool,
    action: &mut Option<Action>,
) {
    ui.add_space(10.);
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let custom = view.selected.as_ref().filter(|p| !view.hook_ready && !p.pre_launch.is_empty());
        theme::label(ui, if view.hook_ready { "Modrinth hook verified" } else if custom.is_some() { "Custom hook found" } else { "One-time Modrinth setup" },
            if view.hook_ready { PINK } else { STEEL });
        if view.hook_ready {
            body(ui, "Direct launches check this instance before Minecraft starts.", STEEL);
        } else if let Some(instance) = custom {
            body(ui, if instance.global_pre_launch {
                "This instance uses a custom global hook. Keep it unchanged; direct launch checks need a reviewed hook setup."
            } else {
                "This instance already has a custom hook. Keep it unchanged; direct launch checks need a reviewed hook setup."
            }, STEEL);
        } else {
            body(ui, "In Modrinth: Instance settings → Custom game launch hooks → Pre-launch. Paste the companion command, save, then refresh here to verify.", STEEL);
        }
        if !view.hook_ready {
            ui.add_space(6.);
            match command {
                Ok(command) => {
                    ui.add_enabled_ui(live && view.selected.is_some() && custom.is_none(), |ui| {
                        if theme::button(ui, "Copy launch hook", false).on_hover_text(command).clicked() {
                            ui.ctx().copy_text(command.clone());
                        }
                    });
                }
                Err(error) => { body(ui, error, STEEL); }
            }
            ui.add_space(4.);
            ui.add_enabled_ui(!view.busy, |ui| {
                if theme::button(ui, "Refresh to verify", false).clicked() {
                    *action = Some(Action::Discover);
                }
            });
        }
    });
}

fn repair(ui: &mut Ui, view: &View, action: &mut Option<Action>) {
    ui.add_space(10.);
    egui::CollapsingHeader::new(RichText::new("PACK REPAIR").font(FontId::new(10.5, semibold())).color(STEEL))
        .show(ui, |ui| {
            body(ui, "Check and repair pack-owned files against the approved release. Personal config edits and user-added files are preserved; conflicts stop the update.", STEEL);
            ui.add_space(6.);
            ui.add_enabled_ui(!view.busy && view.selected.is_some(), |ui| {
                if theme::button(ui, "Repair pack", false).clicked() {
                    *action = Some(Action::Repair);
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_instance_names_fit_the_compact_window() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let instance = modrinth::Instance {
            id: "local:test".into(),
            name: "SMB duplicate profile with a very long name to distinguish its purpose".into(),
            directory: "C:/Users/Player/AppData/Roaming/ModrinthApp/profiles/SMB personal backup"
                .into(),
            imported_version: "0.13.0".into(),
            ..Default::default()
        };
        let view = View {
            instances: vec![instance.clone()],
            selected: Some(instance.clone()),
            ..Default::default()
        };
        let mut settings = Settings {
            instance_id: instance.id,
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
                profile(ui, &view, &mut settings, &mut None);
                assert!(
                    ui.min_rect().width() <= width + 1.,
                    "Profile widened the compact window"
                );
            });
        });
    }
}
