use crate::{
    client::Asset,
    system,
    theme::{self, *},
    worker::{self, Icons, Phase, Settings, Setup, SharedSettings, SharedStatus, Status},
};
use eframe::egui::{
    self, Color32, FontFamily, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, Ui, Vec2,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

thread_local! {
    // Tray objects are main-thread only; kept here so the native tray handler
    // can update/remove them even while egui is not painting a hidden window.
    static TRAY: RefCell<Option<(TrayIcon, CheckMenuItem)>> = const { RefCell::new(None) };
}

pub struct Companion {
    settings: SharedSettings,
    status: SharedStatus,
    icons: Icons,
    textures: HashMap<String, Option<TextureHandle>>,
    screenshot: Option<PathBuf>,
    start_hidden: bool,
    capture_at: f64,
    frames: u32,
    quit: bool,
    has_tray: bool,
    autostart: bool,
    save_error: String,
}

impl Companion {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        screenshot: Option<PathBuf>,
        demo: bool,
        connect: bool,
        start_hidden: bool,
    ) -> Self {
        theme::install(&cc.egui_ctx);
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = cc.window_handle() {
            if let RawWindowHandle::Win32(w) = handle.as_raw() {
                system::set_window(w.hwnd.get());
            }
        }
        let settings = Arc::new(Mutex::new(Settings::load()));
        let status = Arc::new(Mutex::new(if demo {
            demo_status()
        } else {
            Status::default()
        }));
        let icons: Icons = Default::default();
        let live = screenshot.is_none() && !demo;
        if connect {
            worker::start(settings.clone(), status.clone(), icons.clone());
        }
        let mut app = Self {
            settings,
            status,
            icons,
            textures: HashMap::new(),
            screenshot,
            start_hidden,
            capture_at: if connect { 6.0 } else { 1.5 },
            frames: 0,
            quit: false,
            has_tray: false,
            autostart: live && system::autostart_enabled(),
            save_error: String::new(),
        };
        if live {
            app.has_tray = create_tray(&cc.egui_ctx, app.settings.clone(), app.status.clone());
        }
        app
    }

    fn save(&mut self, mut s: Settings) {
        s.revision += 1;
        self.save_error = s.save().err().unwrap_or_default();
        sync_tray(s.enabled);
        *self.settings.lock().unwrap() = s;
    }

    /// Texture for an icon fetched from the League client, if it has arrived.
    fn texture(&mut self, ctx: &egui::Context, path: &str) -> Option<egui::TextureId> {
        if path.is_empty() {
            return None;
        }
        if let Some(t) = self.textures.get(path) {
            return t.as_ref().map(TextureHandle::id);
        }
        let bytes = self.icons.lock().unwrap().get(path).cloned()?;
        let handle = bytes
            .and_then(|b| image::load_from_memory(&b).ok())
            .map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                ctx.load_texture(
                    path,
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    egui::TextureOptions::LINEAR,
                )
            });
        let id = handle.as_ref().map(TextureHandle::id);
        self.textures.insert(path.into(), handle);
        id
    }

    fn header(&mut self, ui: &mut Ui, config: &mut Settings, busy: bool) {
        ui.horizontal(|ui| {
            let t = ui.input(|i| i.time) as f32;
            let turn = if busy { quarter_turns(t) } else { 0. };
            shuriken(ui, 38., PINK, turn);
            ui.add_space(6.);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.;
                let mut job = egui::text::LayoutJob::default();
                let fmt = |c| egui::TextFormat {
                    font_id: FontId::new(25., display()),
                    color: c,
                    ..Default::default()
                };
                job.append("SMB ", 0., fmt(WHITE));
                job.append("//", 0., fmt(PINK));
                job.append(" LEAGUE OF LEGENDS", 0., fmt(WHITE));
                ui.label(job);
                theme::label(ui, "Runes & spells companion", STEEL);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if config.enabled { "Pause" } else { "Resume" };
                if theme::button(ui, label, !config.enabled)
                    .on_hover_text("Pause or resume automatic setup")
                    .clicked()
                {
                    config.enabled = !config.enabled;
                }
            });
        });
    }

    fn hero(&mut self, ui: &mut Ui, status: &Status, config: &Settings) {
        let applied = status.setup.as_ref().is_some_and(|s| {
            (s.runes_applied || !config.runes) && (s.spells_applied || !config.spells)
        }) && config.enabled;
        cut_panel(ui, applied, |ui| {
            let (dot, label) = match (config.enabled, status.phase) {
                (false, _) | (_, Phase::Paused) => (STEEL, "Paused"),
                (_, Phase::Offline) => (STEEL_DIM, "League offline"),
                (_, Phase::Idle) => (PINK, "League connected"),
                (_, Phase::ChampSelect) => (PINK, "Champion select"),
                (_, Phase::InGame) => (PINK, "In game"),
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
            let (title, detail) = if config.enabled {
                (status.title.as_str(), status.detail.as_str())
            } else {
                ("Paused", "Your League setup stays exactly as it is.")
            };
            text(ui, title.to_uppercase(), 30., display(), WHITE);
            body(ui, detail, STEEL);
            if let (Some(setup), true) = (&status.setup, config.enabled) {
                ui.add_space(12.);
                hairline(ui);
                ui.add_space(12.);
                self.setup_card(ui, setup, config);
            }
        });
    }

    fn setup_card(&mut self, ui: &mut Ui, setup: &Setup, config: &Settings) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            // Champion portrait from the client, framed like the brand cards.
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(54.), Sense::hover());
            ui.painter().rect_filled(rect, 2, VOID);
            if let Some(tex) = self.texture(&ctx, &setup.champion_icon) {
                let uv = Rect::from_min_max(Pos2::new(0.07, 0.07), Pos2::new(0.93, 0.93));
                ui.painter().image(tex, rect.shrink(1.), uv, Color32::WHITE);
            } else {
                paint_shuriken(ui.painter(), rect.shrink(14.), STEEL_DIM, VOID, 0.);
            }
            ui.painter()
                .rect_stroke(rect, 2, Stroke::new(1., PINK), StrokeKind::Inside);
            ui.add_space(6.);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.;
                text(ui, &setup.champion, 17., semibold(), WHITE);
                ui.horizontal(|ui| {
                    let role = if !setup.mode.is_empty() {
                        setup.mode.clone()
                    } else {
                        lane_name(&setup.lane).to_owned()
                    };
                    if setup.mode.is_empty() && !setup.lane.is_empty() {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.), Sense::hover());
                        paint_role(ui.painter(), r, &setup.lane, PINK);
                    }
                    if !role.is_empty() {
                        theme::label(ui, &role, STEEL);
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Spells read right-to-left: F, then D, so D ends up on the left.
                for (i, key) in [(1usize, "F"), (0, "D")] {
                    self.spell_slot(
                        ui,
                        setup.spells[i].as_ref(),
                        key,
                        setup.spells_applied || !config.spells,
                    );
                }
            });
        });
        ui.add_space(10.);
        ui.horizontal(|ui| {
            let size = 30.;
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
            match setup
                .keystone
                .as_ref()
                .and_then(|a| self.texture(&ctx, &a.icon))
            {
                Some(tex) => {
                    ui.painter().image(tex, rect, full_uv(), Color32::WHITE);
                }
                None => paint_rune(
                    ui.painter(),
                    rect,
                    if setup.runes_applied { PINK } else { STEEL_DIM },
                ),
            }
            if let Some(k) = &setup.keystone {
                resp.on_hover_text(&k.name);
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.;
                let name = match (&setup.keystone, config.runes) {
                    (_, false) => "Runes left as they are".to_owned(),
                    (Some(k), _) => k.name.clone(),
                    (None, _) => "Rune page on its way".to_owned(),
                };
                text(ui, name, 13., semibold(), WHITE);
                let trees = [&setup.primary, &setup.secondary]
                    .iter()
                    .filter_map(|a| a.as_ref().map(|a| a.name.clone()))
                    .collect::<Vec<_>>()
                    .join("  //  ");
                if !trees.is_empty() {
                    text(ui, trees, 11.5, FontFamily::Proportional, STEEL);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for a in [&setup.secondary, &setup.primary].into_iter().flatten() {
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(20.), Sense::hover());
                    if let Some(tex) = self.texture(&ctx, &a.icon) {
                        ui.painter().image(tex, r, full_uv(), Color32::WHITE);
                    }
                    resp.on_hover_text(&a.name);
                }
            });
        });
    }

    /// One summoner spell with the key it sits on, painted in a single
    /// fixed-size cell so it lays out cleanly in any direction.
    fn spell_slot(&mut self, ui: &mut Ui, spell: Option<&Asset>, key: &str, settled: bool) {
        let ctx = ui.ctx().clone();
        let (cell, resp) = ui.allocate_exact_size(Vec2::new(34., 52.), Sense::hover());
        let rect = Rect::from_min_size(cell.min, Vec2::splat(34.));
        ui.painter().rect_filled(rect, 2, VOID);
        match spell.and_then(|s| self.texture(&ctx, &s.icon)) {
            Some(tex) => {
                ui.painter().image(tex, rect, full_uv(), Color32::WHITE);
            }
            None => paint_spell(
                ui.painter(),
                rect.shrink(4.),
                if settled { STEEL } else { STEEL_DIM },
            ),
        }
        ui.painter()
            .rect_stroke(rect, 2, Stroke::new(1., BORDER), StrokeKind::Inside);
        ui.painter().text(
            Pos2::new(cell.center().x, cell.bottom() - 7.),
            egui::Align2::CENTER_CENTER,
            key,
            FontId::new(13., display_bold()),
            STEEL,
        );
        if let Some(s) = spell {
            resp.on_hover_text(format!("{} on {key}", s.name));
        }
    }

    fn automation(&mut self, ui: &mut Ui, config: &mut Settings, status: &Status) {
        section(ui, "Automatic setup");
        let flash = status
            .flash_icon
            .as_ref()
            .and_then(|a| self.texture(ui.ctx(), &a.icon));
        panel().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(config.enabled, |ui| {
                row(
                    ui,
                    |ui, r| paint_rune(ui.painter(), r, PINK),
                    "Runes",
                    "Best page for your pick and role.",
                    48.,
                    |ui| {
                        toggle(ui, &mut config.runes, "Runes");
                    },
                );
                note(ui, &status.rune, config.runes);
                ui.add_space(8.);
                hairline(ui);
                ui.add_space(8.);
                row(
                    ui,
                    |ui, r| paint_spell(ui.painter(), r, PINK),
                    "Summoner spells",
                    "Recommended pair, Flash where you want it.",
                    48.,
                    |ui| {
                        toggle(ui, &mut config.spells, "Summoner spells");
                    },
                );
                note(ui, &status.spell, config.spells);
                ui.add_space(8.);
                hairline(ui);
                ui.add_space(8.);
                ui.add_enabled_ui(config.spells, |ui| {
                    row(
                        ui,
                        |ui, r| match flash {
                            Some(tex) => {
                                ui.painter().image(tex, r, full_uv(), Color32::WHITE);
                            }
                            None => paint_spell(ui.painter(), r, PINK),
                        },
                        "Flash key",
                        "Same key, every game.",
                        82.,
                        |ui| {
                            for key in ["f", "d"] {
                                let chosen = config.flash == key;
                                if keycap(ui, &key.to_uppercase(), chosen, Vec2::new(36., 32.))
                                    .on_hover_text(format!("Flash on {}", key.to_uppercase()))
                                    .clicked()
                                {
                                    config.flash = key.into();
                                }
                            }
                        },
                    );
                });
            });
        });
    }

    fn behaviour(&mut self, ui: &mut Ui) {
        section(ui, "Out of your way");
        panel().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut on = self.autostart;
            let has_tray = self.has_tray;
            row(
                ui,
                |ui, r| paint_shuriken(ui.painter(), r.shrink(3.), PINK, VOID, 0.),
                "Start with Windows",
                "Straight to the tray. Never over your game.",
                48.,
                |ui| {
                    ui.add_enabled_ui(has_tray, |ui| toggle(ui, &mut on, "Start with Windows"));
                },
            );
            if on != self.autostart {
                match system::set_autostart(on) {
                    Ok(()) => self.autostart = on,
                    Err(e) => self.save_error = e,
                }
            }
            ui.add_space(8.);
            text(
                ui,
                "No overlays, no notifications, no focus grabs. Low CPU priority.",
                11.5,
                FontFamily::Proportional,
                STEEL,
            );
        });
    }

    fn troubleshooting(&mut self, ui: &mut Ui, ctx: &egui::Context, config: &mut Settings) {
        ui.add_space(12.);
        egui::CollapsingHeader::new(
            RichText::new("CONNECTION & TROUBLESHOOTING")
                .font(FontId::new(10.5, semibold()))
                .color(STEEL),
        )
        .icon(collapse_icon)
        .show(ui, |ui| {
            body(
                ui,
                "League is found automatically. If not, point to its install folder:",
                STEEL,
            );
            ui.add_space(4.);
            ui.add(
                egui::TextEdit::singleline(&mut config.folder)
                    .hint_text("C:\\Riot Games\\League of Legends")
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(10, 7)),
            );
            ui.add_space(8.);
            ui.horizontal(|ui| {
                if theme::button(ui, "Retry", false).clicked() {
                    config.revision += 1;
                    self.status.lock().unwrap().error.clear();
                }
                if theme::button(ui, "Quit", false).clicked() {
                    self.quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.add_space(8.);
            body(
                ui,
                "Recommendations come from OP.GG and need internet. Your own rune pages \
                      are never edited or deleted; the companion keeps one page of its own. \
                      Close this window to keep running in the tray.",
                STEEL,
            );
        });
    }
}

impl eframe::App for Companion {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(VOID).to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;
        if self.start_hidden && self.frames >= 2 {
            // eframe shows the window after its first frame; it was off-screen
            // and never activated. Tuck it away now.
            self.start_hidden = false;
            if self.has_tray {
                system::park_window();
            } else {
                system::park_window();
                system::minimize_quietly();
            }
        }
        if self.screenshot.is_some() {
            self.capture(ctx);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit && self.screenshot.is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.has_tray {
                system::hide_window();
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
        if self.quit {
            remove_tray();
        }
        let mut config = self.settings.lock().unwrap().clone();
        let old = config.clone();
        let status = self.status.lock().unwrap().clone();
        let busy = status.busy && config.enabled;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(VOID)
                    .inner_margin(egui::Margin::symmetric(20, 18)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.header(ui, &mut config, busy);
                    ui.add_space(16.);
                    self.hero(ui, &status, &config);
                    self.automation(ui, &mut config, &status);
                    self.behaviour(ui);
                    self.troubleshooting(ui, ctx, &mut config);
                    for error in [&status.error, &self.save_error] {
                        if !error.is_empty() {
                            ui.add_space(10.);
                            alert(ui, error);
                        }
                    }
                    ui.add_space(16.);
                    hairline(ui);
                    ui.add_space(8.);
                    ui.horizontal(|ui| {
                        // Brand guide §1.1: explain the name once per environment.
                        ui.label(theme::label_job(
                            "SMB — Sexy Maar Bescheiden",
                            false,
                            STEEL_DIM,
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let line = if !config.enabled {
                                "Automation paused".to_owned()
                            } else if status.last_applied.is_empty() {
                                status.activity.clone()
                            } else {
                                format!("Last setup {}", status.last_applied)
                            };
                            text(ui, line, 11., FontFamily::Proportional, STEEL);
                        });
                    });
                });
            });
        if config != old {
            self.save(config);
        }
        // Repaint only while visible; animate only while working.
        if busy {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else if status.phase == Phase::InGame {
            ctx.request_repaint_after(Duration::from_secs(2));
        } else {
            ctx.request_repaint_after(Duration::from_millis(300));
        }
    }
}

impl Companion {
    fn capture(&mut self, ctx: &egui::Context) {
        ctx.request_repaint();
        if ctx.input(|i| i.time) >= self.capture_at {
            self.capture_at = f64::INFINITY;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let path = self.screenshot.take().unwrap();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
            if let Err(e) = image::save_buffer(
                &path,
                &bytes,
                img.size[0] as u32,
                img.size[1] as u32,
                image::ColorType::Rgba8,
            ) {
                eprintln!("Could not save screenshot: {e}");
            }
            self.quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl Drop for Companion {
    fn drop(&mut self) {
        self.status.lock().unwrap().stop = true;
        remove_tray();
    }
}

fn row(
    ui: &mut Ui,
    icon: impl FnOnce(&mut Ui, Rect),
    title: &str,
    subtitle: &str,
    control_width: f32,
    control: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(30.), Sense::hover());
        ui.painter()
            .rect(r, 2, VOID, Stroke::new(1., HAIRLINE), StrokeKind::Inside);
        icon(ui, r.shrink(4.));
        ui.add_space(4.);
        let w = (ui.available_width() - control_width - 8.).max(120.);
        ui.vertical(|ui| {
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 1.;
            text(ui, title, 14., semibold(), WHITE);
            text(ui, subtitle, 11.5, FontFamily::Proportional, STEEL);
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
    });
}

fn note(ui: &mut Ui, value: &str, on: bool) {
    if on && value == "Could not apply" {
        ui.horizontal(|ui| {
            ui.add_space(38.);
            theme::label(ui, value, PINK);
        });
    }
}

fn hairline(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
    ui.painter().rect_filled(r, 0, HAIRLINE);
}

fn alert(ui: &mut Ui, message: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x22, 0x05, 0x12))
        .stroke(Stroke::new(1., PINK_DEEP))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::label_job("Heads up", true, WHITE));
            ui.add_space(4.);
            body(ui, message, WHITE);
        });
}

fn collapse_icon(ui: &mut Ui, openness: f32, response: &egui::Response) {
    let r = response.rect;
    let c = r.center();
    let s = r.width() * 0.3;
    let rot = openness * std::f32::consts::FRAC_PI_2;
    let pts = [
        Vec2::new(-s * 0.5, -s),
        Vec2::new(s * 0.6, 0.),
        Vec2::new(-s * 0.5, s),
    ]
    .map(|v| c + egui::emath::Rot2::from_angle(rot) * v)
    .to_vec();
    ui.painter()
        .add(egui::Shape::convex_polygon(pts, PINK, Stroke::NONE));
}

fn full_uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.))
}

fn lane_name(lane: &str) -> &'static str {
    match lane {
        "top" => "Top",
        "jungle" => "Jungle",
        "mid" => "Mid",
        "adc" => "Bot",
        "support" => "Support",
        _ => "",
    }
}

/// §24.2: the shuriken turns a quarter at a time, with a short ease.
fn quarter_turns(t: f32) -> f32 {
    let step = 0.55;
    let n = (t / step).floor();
    let f = ((t / step) - n).min(1.);
    let ease = 1. - (1. - (f / 0.45).min(1.)).powi(3);
    (n + ease) * std::f32::consts::FRAC_PI_2
}

fn create_tray(ctx: &egui::Context, settings: SharedSettings, status: SharedStatus) -> bool {
    let menu = Menu::new();
    let open = MenuItem::new("Open SMB LoL Companion", true, None);
    let enabled = settings.lock().unwrap().enabled;
    let auto = CheckMenuItem::new("Automatic runes & spells", true, enabled, None);
    let quit = MenuItem::new("Quit", true, None);
    if menu
        .append_items(&[&open, &auto, &PredefinedMenuItem::separator(), &quit])
        .is_err()
    {
        return false;
    }
    let Ok(img) = image::load_from_memory(include_bytes!("../assets/icon/shuriken-64.png")) else {
        return false;
    };
    let img = img
        .resize_exact(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let Ok(icon) = Icon::from_rgba(img.into_raw(), 32, 32) else {
        return false;
    };
    let Ok(tray) = TrayIconBuilder::new()
        .with_tooltip("SMB LoL Companion · runes & spells")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
    else {
        return false;
    };
    let (open_id, auto_id, quit_id) = (open.id().clone(), auto.id().clone(), quit.id().clone());
    TRAY.with(|t| *t.borrow_mut() = Some((tray, auto)));

    // These run on the main thread from the tray's own window procedure, so
    // they work while the egui window is hidden and not painting.
    let c = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            system::show_window();
            c.request_repaint();
        }
    }));
    let c = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == open_id {
            system::show_window();
            c.request_repaint();
        } else if event.id == auto_id {
            let on = TRAY.with(|t| t.borrow().as_ref().map(|(_, a)| a.is_checked()));
            let mut s = settings.lock().unwrap().clone();
            s.enabled = on.unwrap_or(!s.enabled);
            s.revision += 1;
            let _ = s.save();
            *settings.lock().unwrap() = s;
            c.request_repaint();
        } else if event.id == quit_id {
            status.lock().unwrap().stop = true;
            remove_tray();
            std::process::exit(0);
        }
    }));
    true
}

fn sync_tray(enabled: bool) {
    TRAY.with(|t| {
        if let Some((_, auto)) = t.borrow().as_ref() {
            auto.set_checked(enabled);
        }
    });
}

fn remove_tray() {
    TRAY.with(|t| drop(t.borrow_mut().take()));
}

fn demo_status() -> Status {
    let asset = |name: &str| Asset {
        name: name.into(),
        icon: String::new(),
    };
    Status {
        phase: Phase::ChampSelect,
        title: "Setup applied".into(),
        detail: "That went reasonably well.".into(),
        activity: "Confirmed by the League client".into(),
        last_applied: "20:00".into(),
        rune: "Applied and confirmed".into(),
        spell: "Applied and confirmed".into(),
        setup: Some(Setup {
            champion: "Lee Sin".into(),
            lane: "jungle".into(),
            keystone: Some(asset("Conqueror")),
            primary: Some(asset("Precision")),
            secondary: Some(asset("Inspiration")),
            spells: [Some(asset("Smite")), Some(asset("Flash"))],
            runes_applied: true,
            spells_applied: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}
