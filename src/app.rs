use crate::{
    client::{self, Asset},
    minecraft::ui::Page as MinecraftPage,
    system,
    theme::{self, *},
    update,
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
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

struct Tray {
    icon: TrayIcon,
    status: SharedStatus,
    tip: String,
}

thread_local! {
    // Tray objects are main-thread only; kept here so the native tray handler
    // can update/remove them even while egui is not painting a hidden window.
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Fixed window width; the height follows the content.
pub const WIDTH: f32 = 420.;
const MARGIN: f32 = 16.;

pub struct Companion {
    live: bool,
    minecraft_selected: bool,
    minecraft: MinecraftPage,
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
        let minecraft_selected =
            std::env::args().any(|arg| arg == "--minecraft" || arg == "--demo-minecraft");
        let demo = demo || std::env::args().any(|arg| arg == "--demo-minecraft");
        let live = screenshot.is_none() && !demo;
        let settings = Arc::new(Mutex::new(if live {
            Settings::load()
        } else {
            Settings {
                items: demo,
                ..Default::default()
            }
        }));
        let status = Arc::new(Mutex::new(if demo {
            demo_status()
        } else {
            Status::default()
        }));
        let icons: Icons = Default::default();
        if connect && live {
            worker::start(settings.clone(), status.clone(), icons.clone());
        }
        if live {
            update::start(status.clone());
        }
        let mut app = Self {
            live,
            minecraft_selected,
            // `--screenshot <png> --live` also reads Modrinth (read-only at first).
            minecraft: MinecraftPage::new(connect && !demo, demo || !connect),
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
            app.has_tray = create_tray(&cc.egui_ctx, app.status.clone());
        }
        app
    }

    fn save(&mut self, mut s: Settings) {
        s.revision += 1;
        if self.live {
            self.save_error = s.save().err().unwrap_or_default();
        }
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

    fn header(&mut self, ui: &mut Ui, busy: bool) {
        ui.horizontal(|ui| {
            let t = ui.input(|i| i.time) as f32;
            let turn = if busy { quarter_turns(t) } else { 0. };
            shuriken(ui, 34., PINK, turn);
            ui.add_space(6.);
            let mut job = egui::text::LayoutJob::default();
            let fmt = |c| egui::TextFormat {
                font_id: FontId::new(25., display()),
                color: c,
                valign: egui::Align::Center,
                ..Default::default()
            };
            job.append("SMB ", 0., fmt(WHITE));
            job.append("//", 0., fmt(PINK));
            job.append(" COMPANION", 0., fmt(WHITE));
            ui.label(job);
        });
        ui.add_space(10.);
        ui.horizontal(|ui| {
            if theme::button(ui, "League", !self.minecraft_selected).clicked() {
                self.minecraft_selected = false;
            }
            if theme::button(ui, "Minecraft", self.minecraft_selected).clicked() {
                self.minecraft_selected = true;
            }
        });
    }

    fn hero(&mut self, ui: &mut Ui, status: &Status, config: &Settings) {
        let (title, on) = match status.phase {
            Phase::Paused => ("Off", false),
            Phase::Offline => ("Disconnected", false),
            Phase::Idle => ("Connected", true),
            Phase::ChampSelect => ("Champ select", true),
            Phase::InGame => ("In game", true),
        };
        cut_panel(ui, on, |ui| {
            headline(ui, title, on, |_| ());
            if let (Some(setup), Phase::ChampSelect) = (&status.setup, status.phase) {
                ui.add_space(10.);
                hairline(ui);
                ui.add_space(10.);
                self.setup_card(ui, setup, config, &status.keys);
            }
        });
    }

    fn setup_card(&mut self, ui: &mut Ui, setup: &Setup, config: &Settings, keys: &[String; 2]) {
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
                .rect_stroke(rect, 2, Stroke::new(1.0_f32, PINK), StrokeKind::Inside);
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
                for i in [1usize, 0] {
                    self.spell_slot(
                        ui,
                        setup.spells[i].as_ref(),
                        &keys[i],
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
        if config.items && !setup.skills.is_empty() {
            ui.add_space(8.);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(30.), Sense::hover());
                paint_item(
                    ui.painter(),
                    r.shrink(3.),
                    if setup.items_applied { PINK } else { STEEL_DIM },
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.;
                    text(
                        ui,
                        format!("Skills {}", setup.skills),
                        13.,
                        semibold(),
                        WHITE,
                    );
                    let note = if setup.items_applied {
                        "Item set waiting in your shop"
                    } else {
                        "Item set on its way"
                    };
                    text(ui, note, 11.5, FontFamily::Proportional, STEEL);
                });
            });
        }
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
            .rect_stroke(rect, 2, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
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

    fn switches(&mut self, ui: &mut Ui, config: &mut Settings, status: &Status) {
        let cap = |label: &str| (label.chars().count() as f32 * 9. + 18.).max(36.);
        let caps_width = cap(&status.keys[0]) + cap(&status.keys[1]) + 10.;
        let flash = status
            .flash_icon
            .as_ref()
            .and_then(|a| self.texture(ui.ctx(), &a.icon));
        row(
            ui,
            |ui, r| paint_rune(ui.painter(), r, PINK),
            "Runes",
            "",
            48.,
            |ui| {
                toggle(ui, &mut config.runes, "Runes");
            },
        );
        spaced_hairline(ui);
        row(
            ui,
            |ui, r| paint_spell(ui.painter(), r, PINK),
            "Summoner spells",
            "",
            48.,
            |ui| {
                toggle(ui, &mut config.spells, "Summoner spells");
            },
        );
        spaced_hairline(ui);
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
                "",
                caps_width,
                |ui| {
                    for (key, label) in [("f", &status.keys[1]), ("d", &status.keys[0])] {
                        let chosen = config.flash == key;
                        if keycap(ui, label, chosen, Vec2::new(cap(label), 30.))
                            .on_hover_text(format!("Flash on {label}"))
                            .clicked()
                        {
                            config.flash = key.into();
                        }
                    }
                },
            );
        });
        spaced_hairline(ui);
        row(
            ui,
            |ui, r| paint_item(ui.painter(), r, PINK),
            "Item set & skill order",
            "",
            48.,
            |ui| {
                toggle(ui, &mut config.items, "Item set & skill order");
            },
        );
    }

    /// Shared by both pages, at the end of the switches.
    fn startup(&mut self, ui: &mut Ui) {
        let mut on = self.autostart;
        let has_tray = self.has_tray && self.live;
        row(
            ui,
            |ui, r| paint_shuriken(ui.painter(), r.shrink(3.), PINK, VOID, 0.),
            "Start with Windows",
            "",
            48.,
            |ui| {
                ui.add_enabled_ui(has_tray, |ui| toggle(ui, &mut on, "Start with Windows"));
            },
        );
        if self.live && on != self.autostart {
            match system::set_autostart(on) {
                Ok(()) => self.autostart = on,
                Err(e) => self.save_error = e,
            }
        }
    }

    /// Retry, the activity log, quitting, and any problem.
    fn advanced(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        config: &mut Settings,
        status: &Status,
    ) {
        let problem = if self.minecraft_selected {
            self.minecraft.problem()
        } else {
            !status.error.is_empty()
        } || !self.save_error.is_empty();
        theme::advanced(
            ui,
            if self.minecraft_selected {
                "advanced-minecraft"
            } else {
                "advanced-league"
            },
            problem,
            |ui| {
                if self.minecraft_selected {
                    self.minecraft.details(ui);
                }
                ui.horizontal_wrapped(|ui| {
                    if self.minecraft_selected {
                        self.minecraft.buttons(ui);
                    } else if theme::button(ui, "Retry", false).clicked() {
                        config.revision += 1;
                        self.status.lock().unwrap().error.clear();
                    }
                    if theme::button_if(ui, "Activity log", false, self.live).clicked() {
                        let dir = system::data_dir();
                        let log = dir.join("activity.log");
                        let _ = std::process::Command::new("explorer")
                            .arg(if log.is_file() {
                                format!("/select,{}", log.display())
                            } else {
                                dir.display().to_string()
                            })
                            .spawn();
                    }
                    if theme::button(ui, "Quit", false).clicked() {
                        self.quit = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                if !system::installed() && !status.update.is_empty() {
                    ui.add_enabled_ui(self.live, |ui| {
                        ui.hyperlink_to(
                            RichText::new(format!("Version {} is out", status.update))
                                .font(FontId::new(11.5, FontFamily::Proportional))
                                .color(PINK),
                            format!("https://github.com/{}/releases/latest", client::REPOSITORY),
                        );
                    });
                }
                if self.minecraft_selected {
                    self.minecraft.problems(ui);
                } else {
                    theme::problem(ui, &status.error);
                }
                theme::problem(ui, &self.save_error);
            },
        );
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
        if system::quit_requested() && !self.quit {
            self.quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
        let busy = if self.minecraft_selected {
            self.minecraft.busy()
        } else {
            status.busy
        };
        let content = egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(VOID)
                    .inner_margin(egui::Margin::symmetric(20, MARGIN as i8)),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    self.header(ui, busy);
                    ui.add_space(14.);
                    if self.minecraft_selected {
                        self.minecraft.hero(ui);
                    } else {
                        self.hero(ui, &status, &config);
                    }
                    ui.add_space(12.);
                    panel().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if self.minecraft_selected {
                            self.minecraft.switches(ui);
                        } else {
                            self.switches(ui, &mut config, &status);
                        }
                        spaced_hairline(ui);
                        self.startup(ui);
                    });
                    self.advanced(ui, ctx, &mut config, &status);
                })
                .response
                .rect
                .bottom()
            })
            .inner;
        // The window is exactly as tall as what it shows: no resizing, no
        // scrolling, no empty space.
        let height = (content + MARGIN).ceil();
        let inner = ctx.input(|i| i.viewport().inner_rect.map(|r| r.height()));
        if inner.is_some_and(|h| (h - height).abs() >= 1.) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(WIDTH, height)));
            ctx.request_repaint();
        }
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

fn spaced_hairline(ui: &mut Ui) {
    ui.add_space(8.);
    hairline(ui);
    ui.add_space(8.);
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

fn create_tray(ctx: &egui::Context, status: SharedStatus) -> bool {
    let menu = Menu::new();
    let open = MenuItem::new("Open SMB Companion", true, None);
    let quit = MenuItem::new("Quit", true, None);
    if menu
        .append_items(&[&open, &PredefinedMenuItem::separator(), &quit])
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
        .with_tooltip("SMB Companion")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
    else {
        return false;
    };
    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    TRAY.with(|t| {
        *t.borrow_mut() = Some(Tray {
            icon: tray,
            status: status.clone(),
            tip: String::new(),
        })
    });
    // A main-thread timer keeps the tooltip current while the window is
    // hidden: hovering the tray icon answers "did it work?" without a click.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetTimer(
            std::ptr::null_mut(),
            0,
            2000,
            Some(tray_tick),
        );
    }

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
        } else if event.id == quit_id {
            status.lock().unwrap().stop = true;
            remove_tray();
            std::process::exit(0);
        }
    }));
    true
}

unsafe extern "system" fn tray_tick(_: *mut std::ffi::c_void, _: u32, _: usize, _: u32) {
    TRAY.with(|t| {
        let Ok(mut t) = t.try_borrow_mut() else {
            return;
        };
        let Some(t) = t.as_mut() else { return };
        let tip = match t.status.lock() {
            Ok(s) => tooltip(&s),
            Err(_) => return,
        };
        if tip != t.tip {
            let _ = t.icon.set_tooltip(Some(&tip));
            t.tip = tip;
        }
    });
}

/// Tray tooltip: the state in one line (Windows allows 127 characters).
fn tooltip(status: &Status) -> String {
    let pick = |s: &Setup| match s.mode.as_str() {
        "" => client::champion_label(&s.champion, &s.lane),
        m => format!("{} · {m}", s.champion),
    };
    let state = match status.phase {
        Phase::Paused => "off".to_owned(),
        Phase::Offline => "League disconnected".to_owned(),
        Phase::Idle => "League connected".to_owned(),
        Phase::InGame => "in game".to_owned(),
        Phase::ChampSelect => match &status.setup {
            Some(s) if s.runes_applied || s.spells_applied => pick(s),
            _ => "champ select".to_owned(),
        },
    };
    format!("SMB Companion · {state}")
        .chars()
        .take(127)
        .collect()
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
            skills: "E Q W, max Q > W > E".into(),
            runes_applied: true,
            spells_applied: true,
            items_applied: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}
