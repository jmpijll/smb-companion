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
        let waiting = view.instances.is_empty()
            || (settings.direct_hook && view.selected.is_some() && !view.hook_ready);
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
                    "Kept current before you play.",
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
                spaced_hairline(ui);
                let custom = view
                    .selected
                    .as_ref()
                    .is_some_and(|p| !view.hook_ready && !p.pre_launch.is_empty());
                let detail = if view.hook_ready {
                    "Modrinth's own Play button checks too."
                } else if custom {
                    "Your own launch hook stays; not checked."
                } else if settings.direct_hook && view.selected.is_some() {
                    "Paste once at Launch hooks → Pre-launch."
                } else {
                    "Also check when you start from Modrinth."
                };
                row(
                    ui,
                    |ui, r| paint_hook(ui.painter(), r, PINK),
                    "Modrinth launches",
                    detail,
                    48.,
                    |ui| {
                        ui.add_enabled_ui(!custom, |ui| {
                            toggle(ui, &mut settings.direct_hook, "Modrinth launches");
                        });
                    },
                );
                if settings.direct_hook && !view.hook_ready && !custom && view.selected.is_some() {
                    self.hook_note(ui);
                }
            });
        });
    }

    /// The one-time paste, as a single line under its switch.
    fn hook_note(&mut self, ui: &mut Ui) {
        ui.add_space(6.);
        ui.horizontal(|ui| {
            ui.add_space(38.);
            match &self.hook {
                Ok(command) => {
                    let label = if self.copied { "Copied" } else { "Copy hook" };
                    if theme::button(ui, label, !self.copied)
                        .on_hover_text(
                            "In Modrinth: instance settings → Launch hooks → Pre-launch. \
                             Paste, save. The companion notices by itself.",
                        )
                        .clicked()
                    {
                        ui.ctx().copy_text(command.clone());
                        self.copied = true;
                    }
                }
                Err(error) => {
                    text(ui, error, 11., FontFamily::Proportional, STEEL);
                }
            }
        });
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
        if let Some(instance) = &view.selected {
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
                    let (line, color): (String, Color32) = if view.version.is_empty() {
                        ("Not checked yet".into(), STEEL)
                    } else if !view.latest.is_empty() && view.latest != view.version {
                        (format!("{} → {}", view.version, view.latest), PINK)
                    } else if view.latest == view.version {
                        (format!("{} · up to date", view.version), STEEL)
                    } else {
                        (view.version.clone(), STEEL)
                    };
                    theme::label(ui, &line, color);
                    text(
                        ui,
                        format!(
                            "Minecraft {} · {} {}",
                            instance.minecraft,
                            capitalised(&instance.loader),
                            instance.loader_version
                        ),
                        11.,
                        FontFamily::Proportional,
                        STEEL,
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if settings.launch_after_update {
                        "Play"
                    } else {
                        "Update"
                    };
                    let same = instance.id == settings.instance_id;
                    if ui
                        .add_enabled_ui(!view.busy && same, |ui| theme::button(ui, label, true))
                        .inner
                        .clicked()
                    {
                        *action = Some(Action::Play);
                    }
                });
            });
        }
    });
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
    fn loader_names_read_like_names() {
        assert_eq!(capitalised("fabric"), "Fabric");
        assert_eq!(capitalised(""), "");
    }
}
