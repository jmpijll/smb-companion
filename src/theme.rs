//! SMB brand tokens (Brand Guide v1.0, §6 and §17) and the small set of
//! hand-drawn widgets the companion uses. Everything is black, white and
//! neon magenta; game imagery from the League client is the only other colour.
use eframe::egui::{
    self, epaint::Vertex, text::LayoutJob, Align, Color32, FontFamily, FontId, Mesh, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, TextFormat, Ui, Vec2,
};

// §6.1 primary
pub const PINK: Color32 = Color32::from_rgb(0xF9, 0x10, 0x79);
pub const VOID: Color32 = Color32::from_rgb(0x05, 0x03, 0x05);
pub const WHITE: Color32 = Color32::from_rgb(0xFC, 0xFA, 0xFB);
// §6.2 tonal support
pub const PINK_HIGHLIGHT: Color32 = Color32::from_rgb(0xFF, 0x3B, 0x9A);
pub const PINK_DEEP: Color32 = Color32::from_rgb(0x8E, 0x07, 0x3D);
pub const CARBON: Color32 = Color32::from_rgb(0x12, 0x09, 0x0D);
pub const STEEL: Color32 = Color32::from_rgb(0xA9, 0x9D, 0xA2);
// Derived, low-opacity tints of the same family (no new hues).
pub const BORDER: Color32 = Color32::from_rgba_premultiplied(62, 4, 30, 64); // pink @ 25%
pub const HAIRLINE: Color32 = Color32::from_rgb(0x2A, 0x1C, 0x23);
pub const CARBON_HOVER: Color32 = Color32::from_rgb(0x1C, 0x0E, 0x15);
pub const STEEL_DIM: Color32 = Color32::from_rgb(0x5E, 0x54, 0x59);

/// §6.6 `--smb-angle: 12deg`.
pub const ANGLE: f32 = 12.0;
/// §6.6 `--smb-radius: 4px`.
pub const RADIUS: u8 = 4;

pub fn display() -> FontFamily {
    FontFamily::Name("display".into())
}
pub fn display_bold() -> FontFamily {
    FontFamily::Name("display-bold".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// §9: Barlow Condensed for display, Inter for body/UI. Both SIL OFL 1.1, bundled.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.into(), egui::FontData::from_static(bytes).into());
    };
    add("inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    add(
        "inter-semibold",
        include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    );
    add(
        "barlow-black-italic",
        include_bytes!("../assets/fonts/BarlowCondensed-BlackItalic.ttf"),
    );
    add(
        "barlow-extrabold",
        include_bytes!("../assets/fonts/BarlowCondensed-ExtraBold.ttf"),
    );
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    let family = |first: &str| {
        let mut list = vec![first.to_owned()];
        list.extend(fallback.iter().cloned());
        list
    };
    fonts
        .families
        .insert(FontFamily::Proportional, family("inter"));
    fonts.families.insert(semibold(), family("inter-semibold"));
    fonts
        .families
        .insert(display(), family("barlow-black-italic"));
    fonts
        .families
        .insert(display_bold(), family("barlow-extrabold"));
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = VOID;
    v.window_fill = CARBON;
    v.extreme_bg_color = VOID;
    v.faint_bg_color = CARBON;
    v.override_text_color = Some(WHITE);
    v.hyperlink_color = PINK;
    v.selection.bg_fill = PINK_DEEP;
    v.selection.stroke = Stroke::new(1.0_f32, PINK);
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = RADIUS.into();
    v.menu_corner_radius = RADIUS.into();
    v.text_cursor.stroke = Stroke::new(2.0_f32, PINK);
    for (w, fill, stroke) in [
        (&mut v.widgets.noninteractive, CARBON, HAIRLINE),
        (&mut v.widgets.inactive, CARBON, BORDER),
        (&mut v.widgets.hovered, CARBON_HOVER, PINK),
        (&mut v.widgets.active, PINK_DEEP, PINK),
        (&mut v.widgets.open, CARBON_HOVER, PINK),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0_f32, stroke);
        w.corner_radius = 2.into();
        w.expansion = 0.;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, STEEL);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, WHITE);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, WHITE);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, WHITE);
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8., 6.);
        s.spacing.button_padding = Vec2::new(14., 7.);
        s.spacing.interact_size.y = 30.;
        s.interaction.selectable_labels = false;
        // §24.3: UI hover 160–240 ms.
        s.animation_time = 0.18;
        s.text_styles.insert(
            egui::TextStyle::Body,
            FontId::new(13., FontFamily::Proportional),
        );
        s.text_styles
            .insert(egui::TextStyle::Button, FontId::new(12.5, semibold()));
    });
}

/// Plain text in a given family/size/colour.
pub fn text(
    ui: &mut Ui,
    value: impl Into<String>,
    size: f32,
    family: FontFamily,
    color: Color32,
) -> Response {
    ui.label(
        egui::RichText::new(value)
            .font(FontId::new(size, family))
            .color(color),
    )
}

/// §9.6/§9.7: uppercase, wide-tracked label, optionally prefixed by `//`.
pub fn label_job(value: &str, slashes: bool, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let format = |color| TextFormat {
        font_id: FontId::new(10.5, semibold()),
        color,
        extra_letter_spacing: 10.5 * 0.22,
        valign: Align::Center,
        ..Default::default()
    };
    if slashes {
        job.append("// ", 0., format(PINK));
    }
    job.append(&value.to_uppercase(), 0., format(color));
    job
}

pub fn label(ui: &mut Ui, value: &str, color: Color32) -> Response {
    ui.label(label_job(value, false, color))
}

/// §17.4/§17.7: Carbon panel, 1px low-opacity pink border, small radius.
pub fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(CARBON)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::symmetric(16, 12))
}

/// Paints a panel whose top-right corner is cut at the brand angle (§10.5).
pub fn cut_panel(ui: &mut Ui, accent: bool, add: impl FnOnce(&mut Ui)) {
    let where_bg = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 18,
            right: 18,
            top: 16,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    let r = inner.response.rect;
    let cut = 26.;
    let pts = vec![
        r.left_top(),
        Pos2::new(r.right() - cut, r.top()),
        Pos2::new(r.right(), r.top() + cut * 0.6),
        r.right_bottom(),
        r.left_bottom(),
    ];
    let stroke = Stroke::new(1.0_f32, if accent { PINK } else { BORDER });
    ui.painter().set(
        where_bg,
        Shape::convex_polygon(pts.clone(), CARBON, Stroke::NONE),
    );
    ui.painter().add(Shape::closed_line(pts, stroke));
    if accent {
        // A single controlled slash (§10.3) running parallel to the cut.
        let a = Pos2::new(r.right() - cut, r.top());
        let b = Pos2::new(r.right(), r.top() + cut * 0.6);
        let n = Vec2::new(0.6, -1.).normalized() * 5.;
        let d = (b - a) * 0.18;
        ui.painter()
            .line_segment([a + n + d, b + n - d], Stroke::new(2.0_f32, PINK_HIGHLIGHT));
    }
}

/// Unit-radius outline of the provisional SMB shuriken mark (§8.2). Matches
/// tools/make_icons.py so the app icon and the in-app mark are identical.
pub fn shuriken_outline() -> Vec<Vec2> {
    let polar = |deg: f32, r: f32| Vec2::angled(deg.to_radians()) * r;
    let steps = 10;
    let mut out = Vec::with_capacity(4 * steps * 2);
    for k in 0..4 {
        let base = 45. + 90. * k as f32;
        let prev = polar(base - 45., 0.24);
        let tip = polar(base, 1.);
        let next = polar(base + 45., 0.24);
        for i in 0..steps {
            out.push(prev + (tip - prev) * (i as f32 / steps as f32));
        }
        for i in 0..steps {
            let t = i as f32 / steps as f32;
            let pull = 1. - 0.28 * (std::f32::consts::PI * t).sin();
            out.push((tip + (next - tip) * t) * pull);
        }
    }
    out
}

/// Draws the shuriken centred in `rect`, rotated by `turn` radians.
pub fn paint_shuriken(
    painter: &egui::Painter,
    rect: Rect,
    color: Color32,
    hole: Color32,
    turn: f32,
) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    let (s, co) = turn.sin_cos();
    let pts: Vec<Pos2> = shuriken_outline()
        .into_iter()
        .map(|v| c + Vec2::new(v.x * co - v.y * s, v.x * s + v.y * co) * r)
        .collect();
    // The star is star-shaped around its centre, so a triangle fan fills it.
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex {
        pos: c,
        uv: egui::epaint::WHITE_UV,
        color,
    });
    for p in &pts {
        mesh.colored_vertex(*p, color);
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::closed_line(pts, Stroke::new(0.8_f32, color)));
    painter.circle_filled(c, r * 0.12, hole);
}

pub fn shuriken(ui: &mut Ui, size: f32, color: Color32, turn: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_shuriken(ui.painter(), rect, color, VOID, turn);
    response
}

/// A parallelogram skewed by the brand angle.
fn slanted(rect: Rect) -> Vec<Pos2> {
    let dx = rect.height() * ANGLE.to_radians().tan();
    vec![
        Pos2::new(rect.left() + dx, rect.top()),
        rect.right_top(),
        Pos2::new(rect.right() - dx, rect.bottom()),
        rect.left_bottom(),
    ]
}

/// Slanted on/off switch: pink with a black knob when on (§6.4: dark text on pink).
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(44., 22.), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, label)
    });
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let enabled = ui.is_enabled();
    let p = ui.painter();
    let track_fill = if *on { PINK } else { VOID };
    let track_stroke = if *on || response.hovered() {
        PINK
    } else {
        STEEL_DIM
    };
    p.add(Shape::convex_polygon(
        slanted(rect),
        dim(track_fill, enabled),
        Stroke::new(1.0_f32, dim(track_stroke, enabled)),
    ));
    let knob_w = 16.;
    let x = egui::lerp(rect.left() + 4.0..=rect.right() - 4. - knob_w, t);
    let knob = Rect::from_min_size(
        Pos2::new(x, rect.top() + 4.),
        Vec2::new(knob_w, rect.height() - 8.),
    );
    p.add(Shape::convex_polygon(
        slanted(knob),
        dim(if *on { VOID } else { STEEL }, enabled),
        Stroke::NONE,
    ));
    focus_ring(ui, &response, rect);
    response
}

/// Keyboard keycap (D / F). Selected keys are pink with black text.
pub fn keycap(ui: &mut Ui, key: &str, selected: bool, size: Vec2) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::RadioButton,
            ui.is_enabled(),
            selected,
            key,
        )
    });
    let enabled = ui.is_enabled();
    let hovered = response.hovered() && enabled;
    let (fill, stroke, ink) = if selected {
        (PINK, PINK, VOID)
    } else if hovered {
        (CARBON_HOVER, PINK, WHITE)
    } else {
        (VOID, STEEL_DIM, STEEL)
    };
    let p = ui.painter();
    p.rect(
        rect,
        2,
        dim(fill, enabled),
        Stroke::new(1.0_f32, dim(stroke, enabled)),
        StrokeKind::Inside,
    );
    // Keycap lip.
    p.line_segment(
        [
            rect.left_bottom() + Vec2::new(3., -3.),
            rect.right_bottom() + Vec2::new(-3., -3.),
        ],
        Stroke::new(
            1.0_f32,
            dim(if selected { PINK_DEEP } else { HAIRLINE }, enabled),
        ),
    );
    p.text(
        rect.center() - Vec2::new(0., 1.),
        egui::Align2::CENTER_CENTER,
        key,
        FontId::new(size.y * 0.5, display_bold()),
        dim(ink, enabled),
    );
    focus_ring(ui, &response, rect);
    response
}

/// Primary (pink, black text) or secondary (outline) action button (§17.5).
pub fn button(ui: &mut Ui, label: &str, primary: bool) -> Response {
    let enabled = ui.is_enabled();
    button_if(ui, label, primary, enabled)
}

/// A button that is greyed out and ignores clicks unless `enabled`, without a
/// nested scope (which would break a wrapping row of buttons).
pub fn button_if(ui: &mut Ui, label: &str, primary: bool, enabled: bool) -> Response {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_uppercase(), FontId::new(11.5, semibold()), WHITE);
    let size = Vec2::new(galley.size().x + 28. + 1.6 * 11.5, 30.);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let hovered = response.hovered() && enabled;
    let (fill, stroke, ink) = match (primary, hovered) {
        (true, false) => (PINK, PINK, VOID),
        (true, true) => (PINK_HIGHLIGHT, PINK_HIGHLIGHT, VOID),
        (false, false) => (CARBON, BORDER, WHITE),
        (false, true) => (CARBON_HOVER, PINK, WHITE),
    };
    let p = ui.painter();
    p.add(Shape::convex_polygon(
        cut_corners(rect, 6.),
        dim(fill, enabled),
        Stroke::new(1.0_f32, dim(stroke, enabled)),
    ));
    let mut job = LayoutJob::default();
    job.append(
        &label.to_uppercase(),
        0.,
        TextFormat {
            font_id: FontId::new(11.5, semibold()),
            color: dim(ink, enabled),
            extra_letter_spacing: 1.6,
            ..Default::default()
        },
    );
    let g = ui.painter().layout_job(job);
    ui.painter().galley(rect.center() - g.size() / 2., g, WHITE);
    focus_ring(ui, &response, rect);
    response
}

/// Rectangle with its top-left and bottom-right corners clipped diagonally.
pub fn cut_corners(r: Rect, c: f32) -> Vec<Pos2> {
    vec![
        Pos2::new(r.left() + c, r.top()),
        r.right_top(),
        Pos2::new(r.right(), r.bottom() - c),
        Pos2::new(r.right() - c, r.bottom()),
        r.left_bottom(),
        Pos2::new(r.left(), r.top() + c),
    ]
}

fn focus_ring(ui: &Ui, response: &Response, rect: Rect) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(3.),
            2,
            Stroke::new(1.5_f32, WHITE),
            StrokeKind::Outside,
        );
    }
}

fn dim(c: Color32, enabled: bool) -> Color32 {
    if enabled {
        c
    } else {
        c.gamma_multiply(0.45)
    }
}

/// League-style lane glyph drawn from simple shapes: a map square with the
/// lane highlighted, claw marks for jungle and a shield for support.
pub fn paint_role(painter: &egui::Painter, rect: Rect, lane: &str, color: Color32) {
    let r = rect.shrink(rect.width() * 0.12);
    let faint = Stroke::new(1.2_f32, color.gamma_multiply(0.35));
    let strong = Stroke::new(2.4_f32, color);
    let inset = r.width() * 0.26;
    match lane {
        "top" | "mid" | "adc" => {
            painter.rect_stroke(r, 1, faint, StrokeKind::Middle);
            match lane {
                "top" => {
                    painter.add(Shape::line(
                        vec![r.left_bottom(), r.left_top(), r.right_top()],
                        strong,
                    ));
                    painter.rect_filled(
                        Rect::from_min_size(r.left_top(), Vec2::splat(inset)),
                        1,
                        color,
                    );
                }
                "mid" => {
                    painter.line_segment([r.left_bottom(), r.right_top()], strong);
                    painter.rect_filled(
                        Rect::from_center_size(r.center(), Vec2::splat(inset * 0.9)),
                        1,
                        color,
                    );
                }
                _ => {
                    painter.add(Shape::line(
                        vec![r.left_bottom(), r.right_bottom(), r.right_top()],
                        strong,
                    ));
                    painter.rect_filled(
                        Rect::from_min_max(r.right_bottom() - Vec2::splat(inset), r.right_bottom()),
                        1,
                        color,
                    );
                }
            }
        }
        "jungle" => {
            // Three claw marks: the brand's controlled slash group (§10.3).
            for i in 0..3 {
                let x = r.left() + r.width() * (0.18 + 0.3 * i as f32);
                painter.add(Shape::line(
                    vec![
                        Pos2::new(x + r.width() * 0.12, r.top()),
                        Pos2::new(x - r.width() * 0.02, r.center().y),
                        Pos2::new(x + r.width() * 0.04, r.bottom()),
                    ],
                    Stroke::new(2.2_f32, color),
                ));
            }
        }
        "support" => {
            let c = r.center();
            let w = r.width() * 0.5;
            let h = r.height() * 0.5;
            painter.add(Shape::convex_polygon(
                vec![
                    c + Vec2::new(-w, -h),
                    c + Vec2::new(w, -h),
                    c + Vec2::new(w, h * 0.1),
                    c + Vec2::new(0., h),
                    c + Vec2::new(-w, h * 0.1),
                ],
                Color32::TRANSPARENT,
                strong,
            ));
            painter.line_segment(
                [c + Vec2::new(0., -h * 0.45), c + Vec2::new(0., h * 0.45)],
                Stroke::new(2.0_f32, color),
            );
            painter.line_segment(
                [
                    c + Vec2::new(-w * 0.45, -h * 0.05),
                    c + Vec2::new(w * 0.45, -h * 0.05),
                ],
                Stroke::new(2.0_f32, color),
            );
        }
        _ => {
            painter.rect_stroke(r, 1, faint, StrokeKind::Middle);
            painter.circle_stroke(r.center(), r.width() * 0.22, strong);
        }
    }
}

/// Rune glyph used when the client's rune art is not available.
pub fn paint_rune(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let a = rect.width() * 0.34;
    let pts = vec![
        c + Vec2::new(0., -a),
        c + Vec2::new(a * 0.8, 0.),
        c + Vec2::new(0., a),
        c + Vec2::new(-a * 0.8, 0.),
    ];
    painter.add(Shape::closed_line(pts, Stroke::new(1.8_f32, color)));
    painter.circle_filled(c, a * 0.26, color);
}

/// Summoner-spell glyph used when the client's spell art is not available.
pub fn paint_spell(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = rect.width() / 28.;
    painter.add(Shape::line(
        vec![
            c + Vec2::new(4., -12.) * s,
            c + Vec2::new(-7., 2.) * s,
            c + Vec2::new(1., 2.) * s,
            c + Vec2::new(-4., 12.) * s,
            c + Vec2::new(8., -2.) * s,
            c + Vec2::new(0., -2.) * s,
        ],
        Stroke::new(1.8_f32, color),
    ));
}

/// Item-set glyph: a shop bag, drawn in the same line weight as the others.
pub fn paint_item(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = rect.width() / 28.;
    let stroke = Stroke::new(1.8_f32, color);
    painter.add(Shape::closed_line(
        vec![
            c + Vec2::new(-9., -4.) * s,
            c + Vec2::new(9., -4.) * s,
            c + Vec2::new(11., 11.) * s,
            c + Vec2::new(-11., 11.) * s,
        ],
        stroke,
    ));
    painter.add(Shape::line(
        vec![
            c + Vec2::new(-4., -4.) * s,
            c + Vec2::new(-4., -8.) * s,
            c + Vec2::new(-2., -11.) * s,
            c + Vec2::new(2., -11.) * s,
            c + Vec2::new(4., -8.) * s,
            c + Vec2::new(4., -4.) * s,
        ],
        stroke,
    ));
}

// Shared page building blocks: League and Minecraft use the same rows.

pub fn row(
    ui: &mut Ui,
    icon: impl FnOnce(&mut Ui, Rect),
    title: &str,
    subtitle: &str,
    control_width: f32,
    control: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(30.), Sense::hover());
        ui.painter().rect(
            r,
            2,
            VOID,
            Stroke::new(1.0_f32, HAIRLINE),
            StrokeKind::Inside,
        );
        icon(ui, r.shrink(4.));
        ui.add_space(4.);
        let w = (ui.available_width() - control_width - 8.).max(120.);
        if subtitle.is_empty() {
            ui.allocate_ui_with_layout(
                Vec2::new(w, 30.),
                egui::Layout::left_to_right(Align::Center),
                |ui| text(ui, title, 14., semibold(), WHITE),
            );
        } else {
            ui.vertical(|ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 1.;
                text(ui, title, 14., semibold(), WHITE);
                text(ui, subtitle, 11.5, FontFamily::Proportional, STEEL);
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
    });
}

pub fn hairline(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
    ui.painter().rect_filled(r, 0, HAIRLINE);
}

/// A page's state: a status dot and one or two words, large. `action` sits
/// on the right of the same line (Play, Install).
pub fn headline<R>(ui: &mut Ui, title: &str, on: bool, action: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::new(12., 30.), Sense::hover());
        if on {
            ui.painter()
                .circle_filled(r.center(), 6., PINK.gamma_multiply(0.25));
        }
        ui.painter()
            .circle_filled(r.center(), 3.5, if on { PINK } else { STEEL_DIM });
        ui.add_space(2.);
        text(
            ui,
            title.to_uppercase(),
            28.,
            display(),
            if on { WHITE } else { STEEL },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), action)
            .inner
    })
    .inner
}

/// Everything a player rarely needs, folded away. A pink dot on the header is
/// the only sign of a problem.
pub fn advanced(ui: &mut Ui, id: &str, problem: bool, add: impl FnOnce(&mut Ui)) {
    ui.add_space(10.);
    let header = egui::CollapsingHeader::new(label_job("Advanced", false, STEEL))
        .id_salt(id)
        // Screenshots of the folded-away part (development aid).
        .default_open(std::env::args().any(|a| a == "--advanced"))
        .icon(collapse_icon)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.;
            add(ui)
        })
        .header_response
        .rect;
    if problem {
        ui.painter().circle_filled(
            egui::pos2(header.right() - 6., header.center().y),
            3.5,
            PINK,
        );
    }
}

/// A problem, inside Advanced only.
pub fn problem(ui: &mut Ui, message: &str) {
    if !message.is_empty() {
        text(ui, message, 11.5, FontFamily::Proportional, PINK_HIGHLIGHT);
    }
}

pub fn collapse_icon(ui: &mut Ui, openness: f32, response: &egui::Response) {
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

/// Update glyph: two chasing arrows.
pub fn paint_sync(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let r = rect.width() * 0.32;
    let stroke = Stroke::new(1.8_f32, color);
    for start in [0.25_f32, 3.39] {
        let arc: Vec<Pos2> = (0..=10)
            .map(|i| {
                let a = start + i as f32 * 0.24;
                c + Vec2::angled(a) * r
            })
            .collect();
        let tip = *arc.last().unwrap();
        let dir = (tip - arc[arc.len() - 2]).normalized();
        let side = Vec2::new(-dir.y, dir.x);
        painter.add(Shape::line(arc, stroke));
        painter.add(Shape::line(
            vec![
                tip - dir * 4. + side * 3.5,
                tip,
                tip - dir * 4. - side * 3.5,
            ],
            stroke,
        ));
    }
}
