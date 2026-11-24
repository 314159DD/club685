//! The "Chrome skin" main window, drawn with tiny-skia (design option B, 460x660 logical pixels).
//!
//! Everything is laid out in logical coordinates and scaled by the system DPI factor when painted, so hit testing
//! and painting share one geometry.

use crate::install::ModuleState;
use crate::prefs::{PhaseSetting, Prefs, WaveMode};
use crate::rbmod::Source;
use crate::preview::{self, PreviewState};
use crate::state::{diagnostics_text, Snapshot, StateKind};
use club685_render::shapes::*;
use club685_render::text::{self, Face};
use tiny_skia::{Color, FillRule, Mask, Pixmap, Transform};

pub const WIN_W: f32 = 460.0;
pub const WIN_H: f32 = 596.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Minimise,
    Quit,
    Power,
    Wave(WaveMode),
    Phase(PhaseSetting),
    Disco,
    Settings,
    CloseSettings,
    CopyDiag,
    InstallModule,
    RemoveModule,
}

/// What the module state changes in the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    /// The one-time setup is pending: waveform and phase controls are off, INSTALL is shown.
    pub setup: bool,
    /// The bundled module is in the rekordbox folder: Settings offers REMOVE MODULE.
    pub installed: bool,
}

impl Mode {
    pub fn of(snap: &Snapshot) -> Self {
        Mode { setup: snap.module.needs_setup(), installed: snap.module == ModuleState::Installed }
    }
}

/// `[x, y, w, h]` in logical pixels.
pub type R = [f32; 4];

pub fn inside(r: R, x: f32, y: f32) -> bool {
    x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
}

// ---- geometry ---------------------------------------------------------------------------------------------------

const TITLE: R = [11.0, 11.0, 438.0, 30.0];
const LCD: R = [11.0, 51.0, 438.0, 170.0];
const PANEL: R = [11.0, 231.0, 438.0, 318.0];
const PX: f32 = 24.0; // panel content left
const PW: f32 = 412.0; // panel content width

fn red_btn() -> R {
    [423.0, 18.0, 16.0, 16.0]
}
fn yellow_btn() -> R {
    [401.0, 18.0, 16.0, 16.0]
}
fn power_btn() -> R {
    [PX + PW - 120.0, 244.0, 120.0, 44.0]
}
fn wave_btn(i: usize) -> R {
    let w = (PW - 12.0) / 3.0;
    [PX + i as f32 * (w + 6.0), 322.0, w, 44.0]
}
fn phase_btn(i: usize) -> R {
    let w = (PW - 12.0) / 3.0;
    [PX + i as f32 * (w + 6.0), 400.0, w, 44.0]
}

struct Footer {
    disco_hit: R,
    toggle: R,
    settings_hit: R,
    disco_x: f32,
    settings_x: f32,
}

fn footer() -> Footer {
    let wd = text::measure(Face::Orbitron500, 11.0, "DISCO", 2.0);
    let ws = text::measure(Face::Orbitron500, 11.0, "SETTINGS", 2.0);
    let (l, r) = (23.0, 437.0);
    let gap = ((r - l) - wd - 44.0 - ws) / 2.0;
    let tx = l + wd + gap;
    Footer {
        disco_hit: [l - 4.0, 559.0, wd + gap + 44.0 + 8.0, 26.0],
        toggle: [tx, 561.0, 44.0, 22.0],
        settings_hit: [r - ws - 8.0, 559.0, ws + 16.0, 26.0],
        disco_x: l,
        settings_x: r - ws,
    }
}

fn install_btn() -> R {
    [PX, 458.0, PW, 44.0]
}
fn copy_btn(m: Mode) -> R {
    [PX, 494.0, if m.installed { 176.0 } else { 262.0 }, 44.0]
}
fn remove_btn() -> R {
    [PX + 182.0, 494.0, 150.0, 44.0]
}
fn close_btn(m: Mode) -> R {
    if m.installed {
        [PX + 338.0, 494.0, PW - 338.0, 44.0]
    } else {
        [PX + 268.0, 494.0, PW - 268.0, 44.0]
    }
}

/// Clickable regions, front to back.
pub fn hits(settings_open: bool, m: Mode) -> Vec<(Action, R)> {
    let f = footer();
    let mut v = vec![(Action::Quit, red_btn()), (Action::Minimise, yellow_btn()), (Action::Disco, f.disco_hit), (Action::Settings, f.settings_hit)];
    if settings_open {
        v.push((Action::CopyDiag, copy_btn(m)));
        if m.installed {
            v.push((Action::RemoveModule, remove_btn()));
        }
        v.push((Action::CloseSettings, close_btn(m)));
    } else {
        v.push((Action::Power, power_btn()));
        if m.setup {
            v.push((Action::InstallModule, install_btn()));
            return v;
        }
        for (i, w) in [WaveMode::Normal, WaveMode::Split, WaveMode::HideBoth].into_iter().enumerate() {
            v.push((Action::Wave(w), wave_btn(i)));
        }
        for (i, p) in [PhaseSetting::Off, PhaseSetting::Box, PhaseSetting::Line].into_iter().enumerate() {
            v.push((Action::Phase(p), phase_btn(i)));
        }
    }
    v
}

pub fn hit_test(x: f32, y: f32, settings_open: bool, m: Mode) -> Option<Action> {
    hits(settings_open, m).into_iter().find(|(_, r)| inside(*r, x, y)).map(|(a, _)| a)
}

/// The title bar outside the two round buttons moves the window.
pub fn is_drag_region(x: f32, y: f32) -> bool {
    inside(TITLE, x, y) && !inside([yellow_btn()[0] - 3.0, 14.0, 44.0, 24.0], x, y)
}

// ---- state handed to the painter ---------------------------------------------------------------------------------

pub struct UiState<'a> {
    pub prefs: &'a Prefs,
    pub snap: &'a Snapshot,
    /// Phase, bars and BPM for the mode preview (measured, or extrapolated by `PhaseClock`).
    pub preview: &'a PreviewState,
    pub hover: Option<Action>,
    pub pressed: Option<Action>,
    pub settings_open: bool,
    pub copied: bool,
    /// Seconds, drives the disco glow.
    pub time: f32,
}

// ---- painter ---------------------------------------------------------------------------------------------------

struct P {
    pm: Pixmap,
    s: f32,
}

impl P {
    fn vg(&self, y0: f32, y1: f32, stops: &[(f32, Color)]) -> tiny_skia::Paint<'static> {
        vgradient(y0 * self.s, y1 * self.s, stops)
    }
    fn rr(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, paint: &tiny_skia::Paint) {
        let s = self.s;
        fill_rrect(&mut self.pm, x * s, y * s, w * s, h * s, r * s, paint);
    }
    fn rr_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, color: Color) {
        let s = self.s;
        stroke_rrect(&mut self.pm, x * s, y * s, w * s, h * s, r * s, width * s, &solid(color));
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let s = self.s;
        fill_rect(&mut self.pm, x * s, y * s, w * s, h * s, &solid(color));
    }
    fn text(&mut self, face: Face, px: f32, x: f32, baseline: f32, color: u32, alpha: f32, t: &str, ls: f32) {
        let c = (color >> 16) as u8;
        let s = self.s;
        text::draw(&mut self.pm, face, px * s, x * s, baseline * s, (c, (color >> 8) as u8, color as u8), alpha, t, ls * s);
    }
    fn tw(&self, face: Face, px: f32, t: &str, ls: f32) -> f32 {
        text::measure(face, px * self.s, t, ls * self.s) / self.s
    }
    /// Baseline that vertically centres capital letters on `cy`.
    fn base(&self, face: Face, px: f32, cy: f32) -> f32 {
        cy + text::cap_height(face, px * self.s) / self.s / 2.0
    }
    fn text_c(&mut self, face: Face, px: f32, cx: f32, cy: f32, color: u32, t: &str, ls: f32) {
        let w = self.tw(face, px, t, ls) - ls;
        let b = self.base(face, px, cy);
        self.text(face, px, cx - w / 2.0, b, color, 1.0, t, ls);
    }
    fn circle(&mut self, cx: f32, cy: f32, r: f32, paint: &tiny_skia::Paint) {
        let s = self.s;
        fill_circle(&mut self.pm, cx * s, cy * s, r * s, paint);
    }
    fn circle_stroke(&mut self, cx: f32, cy: f32, r: f32, w: f32, color: Color) {
        let s = self.s;
        stroke_circle(&mut self.pm, cx * s, cy * s, r * s, w * s, &solid(color));
    }
}

fn hsv(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}

pub struct UiRenderer {
    pub scale: f32,
    background: Pixmap,
}

impl UiRenderer {
    pub fn new(scale: f32) -> Self {
        Self { scale, background: draw_background(scale) }
    }

    pub fn size(&self) -> (u32, u32) {
        ((WIN_W * self.scale).round() as u32, (WIN_H * self.scale).round() as u32)
    }

    pub fn paint(&self, st: &UiState) -> Pixmap {
        let mut p = P { pm: self.background.clone(), s: self.scale };
        draw_title_buttons(&mut p, st);
        draw_lcd(&mut p, st);
        if st.settings_open {
            draw_settings(&mut p, st);
        } else {
            draw_controls(&mut p, st);
        }
        draw_footer(&mut p, st);
        p.pm
    }
}

// ---- static background ------------------------------------------------------------------------------------------

fn draw_background(scale: f32) -> Pixmap {
    let w = (WIN_W * scale).round() as u32;
    let h = (WIN_H * scale).round() as u32;
    let mut p = P { pm: Pixmap::new(w, h).unwrap(), s: scale };

    // body: brushed metal
    let body = p.vg(
        0.0,
        WIN_H,
        &[(0.0, hex(0xd9dee5)), (0.18, hex(0x9aa3ae)), (0.40, hex(0xc7cdd5)), (0.70, hex(0x7e8792)), (1.0, hex(0xb3bac3))],
    );
    p.rr(0.0, 0.0, WIN_W, WIN_H, 22.0, &body);
    if let (Some(path), Some(mut mask)) = (rrect(0.0, 0.0, w as f32, h as f32, 22.0 * scale), Mask::new(w, h)) {
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        let mut seed = 0x2545_f491u32;
        let mut y = 0.0f32;
        while y < h as f32 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let r = (seed >> 24) as u8;
            let (col, a) = if r & 1 == 0 { ((255, 255, 255), 6 + r % 14) } else { ((0, 0, 0), 4 + r % 10) };
            let paint = solid(rgba(col.0, col.1, col.2, a));
            if let Some(rc) = tiny_skia::Rect::from_xywh(0.0, y, w as f32, (scale).max(1.0)) {
                p.pm.fill_rect(rc, &paint, Transform::identity(), Some(&mask));
            }
            y += (2.0 * scale).max(2.0);
        }
    }
    p.rr_stroke(0.0, 0.0, WIN_W, WIN_H, 22.0, 1.0, hex(0x4a525c));

    // title bar
    let t = p.vg(TITLE[1], TITLE[1] + TITLE[3], &[(0.0, hex(0x4b5563)), (1.0, hex(0x1f252d))]);
    p.rr(TITLE[0], TITLE[1], TITLE[2], TITLE[3], 14.0, &t);
    p.rr_stroke(TITLE[0], TITLE[1], TITLE[2], TITLE[3], 14.0, 1.0, hex(0x0f1318));
    let cy = TITLE[1] + TITLE[3] / 2.0;
    let clubw = p.tw(Face::Orbitron800, 17.0, "CLUB", 3.0);
    let w685 = p.tw(Face::Orbitron800, 17.0, "685", 3.0);
    let sub = p.tw(Face::Orbitron500, 10.5, "FOR REKORDBOX 6.8.5", 2.0);
    let l = TITLE[0] + 10.0;
    let btns_w = 16.0 + 6.0 + 16.0;
    let right = TITLE[0] + TITLE[2] - 10.0 - btns_w;
    let gap = (right - l - (clubw + w685) - sub) / 2.0;
    let b = p.base(Face::Orbitron800, 17.0, cy);
    p.text(Face::Orbitron800, 17.0, l, b, 0xe6ebf1, 1.0, "CLUB", 3.0);
    p.text(Face::Orbitron800, 17.0, l + clubw, b, 0x7fffd0, 1.0, "685", 3.0);
    let b9 = p.base(Face::Orbitron500, 10.5, cy);
    p.text(Face::Orbitron500, 10.5, l + clubw + w685 + gap, b9, 0xb9c2cc, 1.0, "FOR REKORDBOX 6.8.5", 2.0);

    // LCD housing (inset shadow, glow and content are dynamic)
    p.rr(LCD[0], LCD[1], LCD[2], LCD[3], 12.0, &solid(hex(0x0b130f)));

    // panel
    let pg = p.vg(PANEL[1], PANEL[1] + PANEL[3], &[(0.0, hex(0xc3cad2)), (1.0, hex(0xa2abb5))]);
    p.rr(PANEL[0], PANEL[1], PANEL[2], PANEL[3], 12.0, &pg);
    p.rr_stroke(PANEL[0], PANEL[1], PANEL[2], PANEL[3], 12.0, 1.0, hex(0x5a636e));
    p.rect(PANEL[0] + 12.0, PANEL[1] + 1.0, PANEL[2] - 24.0, 1.0, hex(0xeef2f6));

    // section labels
    let lab = |p: &mut P, y: f32, t: &str| {
        let b = p.base(Face::Orbitron800, 12.5, y + 6.0);
        p.text(Face::Orbitron800, 12.5, PX, b, 0x1d2228, 1.0, t, 2.0);
    };
    lab(&mut p, 304.0, "WAVEFORM");
    lab(&mut p, 382.0, "PHASE METER");
    let by = p.base(Face::Orbitron800, 12.5, 244.0 + 22.0);
    p.text(Face::Orbitron800, 12.5, PX, by, 0x1d2228, 1.0, "POWER", 2.0);

    // footer labels
    let f = footer();
    let fb = p.base(Face::Orbitron500, 11.0, 572.0);
    p.text(Face::Orbitron500, 11.0, f.disco_x, fb, 0x2b3138, 1.0, "DISCO", 2.0);
    p.text(Face::Orbitron500, 11.0, f.settings_x, fb, 0x2b3138, 1.0, "SETTINGS", 2.0);
    p.pm
}

// ---- dynamic parts ----------------------------------------------------------------------------------------------

fn press_tint(p: &mut P, r: R, radius: f32, st: &UiState, a: Action) {
    if st.pressed == Some(a) && st.hover == Some(a) {
        p.rr(r[0], r[1], r[2], r[3], radius, &solid(rgba(0, 0, 0, 46)));
    } else if st.hover == Some(a) {
        p.rr(r[0], r[1], r[2], r[3], radius, &solid(rgba(255, 255, 255, 46)));
    }
}

fn draw_title_buttons(p: &mut P, st: &UiState) {
    for (r, a, c0, c1, border) in [
        (yellow_btn(), Action::Minimise, 0xfff6b0, 0xd9a400, 0x6b5200),
        (red_btn(), Action::Quit, 0xffb3a8, 0xc4261a, 0x5c0f08),
    ] {
        let (cx, cy) = (r[0] + 8.0, r[1] + 8.0);
        // radial-gradient(circle at 35% 30%, c0, c1 70%)
        let g = radial(r[0] + 5.6, r[1] + 4.8, 10.7, hex(c0), hex(c1));
        p.circle(cx, cy, 8.0, &g);
        p.circle_stroke(cx, cy, 8.0, 1.0, hex(border));
        press_tint(p, r, 8.0, st, a);
    }
}

fn draw_lcd(p: &mut P, st: &UiState) {
    let s = p.s;
    let (x, y, w, h) = (LCD[0], LCD[1], LCD[2], LCD[3]);
    // inset shadow, 0 2px 8px black
    let sh = p.vg(y + 2.0, y + 12.0, &[(0.0, rgba(0, 0, 0, 255)), (1.0, rgba(0, 0, 0, 0))]);
    if let (Some(path), Some(mut mask)) = (rrect((x + 2.0) * s, (y + 2.0) * s, (w - 4.0) * s, (h - 4.0) * s, 10.0 * s), Mask::new(p.pm.width(), p.pm.height())) {
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        if let Some(rc) = tiny_skia::Rect::from_xywh((x + 2.0) * s, (y + 2.0) * s, (w - 4.0) * s, 10.0 * s) {
            p.pm.fill_rect(rc, &sh, Transform::identity(), Some(&mask));
        }
        if st.prefs.disco {
            let t = st.time;
            for i in 0..3 {
                let fi = i as f32;
                let cx = x + w * (0.5 + 0.42 * (t * 1.3 + fi * 2.1).sin());
                let cy = y + h * (0.5 + 0.38 * (t * 0.9 + fi * 1.7).cos());
                let (r, g, b) = hsv(t * 70.0 + fi * 120.0, 0.85, 1.0);
                let paint = radial(cx * s, cy * s, 120.0 * s, rgba(r, g, b, 90), rgba(r, g, b, 0));
                p.pm.fill_rect(tiny_skia::Rect::from_xywh((x + 2.0) * s, (y + 2.0) * s, (w - 4.0) * s, (h - 4.0) * s).unwrap(), &paint, Transform::identity(), Some(&mask));
            }
        }
    }
    let border = if st.prefs.disco {
        let (r, g, b) = hsv(st.time * 90.0, 0.7, 1.0);
        rgb(r, g, b)
    } else {
        hex(0x2b3138)
    };
    p.rr_stroke(x, y, w, h, 12.0, 2.0, border);

    let snap = st.snap;
    let (cx0, cw) = (x + 2.0 + 14.0, w - 4.0 - 28.0);
    let top = y + 2.0 + 12.0;
    // row 1: three spans, space-between
    let a = format!("STATUS: {}", snap.kind.word());
    let b = format!("DECKS {}/2", snap.decks_found);
    let c = match snap.master {
        Some(m) => format!("MASTER: {}", m + 1),
        None => "MASTER: -".to_string(),
    };
    let (wa, wb, wc) = (p.tw(Face::Mono, 15.0, &a, 0.0), p.tw(Face::Mono, 15.0, &b, 0.0), p.tw(Face::Mono, 15.0, &c, 0.0));
    let gap = (cw - wa - wb - wc) / 2.0;
    let base = p.base(Face::Mono, 15.0, top + 8.0);
    let col = match snap.kind {
        StateKind::Error => 0xff8c7a,
        StateKind::Setup => 0xffae5c,
        StateKind::Off => 0x4fbf98,
        _ => 0x7fffd0,
    };
    p.text(Face::Mono, 15.0, cx0, base, col, 1.0, &a, 0.0);
    p.text(Face::Mono, 15.0, cx0 + wa + gap, base, 0x7fffd0, 1.0, &b, 0.0);
    p.text(Face::Mono, 15.0, cx0 + wa + gap + wb + gap, base, 0x7fffd0, 1.0, &c, 0.0);

    let pv = st.preview;
    let live = pv.live;

    // the mode preview: the waveform band as CLUB685 will present it with the current settings
    let py = top + 20.0;
    let band = preview::paint_preview(st.prefs.waveform, st.prefs.phase, pv, s);
    p.pm.draw_pixmap((cx0 * s).round() as i32, (py * s).round() as i32, band.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
    p.rr_stroke(cx0 - 1.0, py - 1.0, preview::PREV_W + 2.0, preview::PREV_H + 2.0, 3.0, 1.0, hex(0x24503f));
    let tag = |p: &mut P, right: bool, y: f32, t: &str, col: u32| {
        let w = p.tw(Face::Mono, 11.0, t, 0.5) + 8.0;
        let x = if right { cx0 + preview::PREV_W - w - 3.0 } else { cx0 + 3.0 };
        p.rr(x, y, w, 13.0, 3.0, &solid(rgba(0, 0, 0, 200)));
        let b = p.base(Face::Mono, 11.0, y + 6.5);
        p.text(Face::Mono, 11.0, x + 4.0, b, col, 1.0, t, 0.5);
    };
    if pv.demo {
        tag(p, true, py + 3.0, "DEMO 128 BPM", 0xffae5c);
    }
    if st.prefs.waveform == WaveMode::Normal && st.prefs.phase != PhaseSetting::Off {
        tag(p, false, py + preview::PREV_H - 16.0, "METER NEEDS SPLIT OR HIDE BOTH", 0xffae5c);
    }

    // strip under it: the bar counter of each deck and where the numbers come from
    let sy = py + preview::PREV_H + 5.0;
    let cwid = p.tw(Face::Mono, 14.0, "0", 0.0);
    let bl = p.base(Face::Mono, 14.0, sy + 7.0);
    let bar_txt = |b: Option<(u32, u8)>| b.map_or("--.-".to_string(), |(a, n)| format!("{a:02}.{n}"));
    for d in 0..2 {
        let label = format!("D{} BARS", d + 1);
        let val = bar_txt(pv.bars[d]);
        let lw = p.tw(Face::Mono, 14.0, &label, 0.0);
        let total = lw + 6.0 + val.chars().count() as f32 * cwid;
        let x0 = if d == 0 { cx0 } else { cx0 + cw - total };
        p.text(Face::Mono, 14.0, x0, bl, 0x3a8f72, 1.0, &label, 0.0);
        p.text(Face::Mono, 14.0, x0 + lw + 6.0, bl, if live { 0x7fffd0 } else { 0x2f705a }, 1.0, &val, 0.0);
    }
    let src_txt = match snap.source {
        Source::Module => "MODULE",
        Source::Silent => "NO DATA",
        Source::Pending => "...",
    };
    let sw = p.tw(Face::Mono, 14.0, src_txt, 0.0);
    p.text(Face::Mono, 14.0, cx0 + (cw - sw) / 2.0, bl, if snap.source == Source::Module { 0x4fbf98 } else { 0xffae5c }, 1.0, src_txt, 0.0);

    // dashed separator and the status lines
    let sep = sy + 18.0;
    let dash = solid(hex(0x24503f));
    hline_dashed(&mut p.pm, cx0 * s, (cx0 + cw) * s, (sep + 0.5) * s, s, 3.0 * s, &dash);
    let previewing = !live && matches!(snap.kind, StateKind::Searching | StateKind::Ready) && !snap.module.needs_setup();
    let fit = |p: &P, t: String| {
        let mut line = t;
        while p.tw(Face::Mono, 14.0, &line, 0.0) > cw && line.chars().count() > 4 {
            line.pop();
            line.pop();
            line.push('…');
        }
        line
    };
    let l1 = sep + 8.0 + 11.0;
    let (a_txt, a_col) = if previewing { (preview_line(st.prefs), 0xffae5c) } else { (format!("> {}", snap.line), 0x4fbf98) };
    let a_txt = fit(p, a_txt);
    p.text(Face::Mono, 14.0, cx0, l1, a_col, 1.0, &a_txt, 0.0);
    let l2 = if previewing {
        format!("> {}", snap.line)
    } else if snap.kind == StateKind::Setup {
        "> Click INSTALL below. Windows asks once.".to_string()
    } else if snap.source == Source::Module {
        format!("> module {:.0} pkt/s · meter {:.0} fps", snap.udp_pps, snap.meter_fps)
    } else {
        "> waiting for the rekordbox module".to_string()
    };
    let l2 = fit(p, l2);
    p.text(Face::Mono, 14.0, cx0, l1 + 17.0, 0x3a8f72, 1.0, &l2, 0.0);
}

/// "Preview: Split + Box · switch to rekordbox to apply".
pub fn preview_line(prefs: &Prefs) -> String {
    format!("Preview: {} + {} · switch to rekordbox to apply", preview::wave_name(prefs.waveform), preview::meter_name(prefs.phase))
}

fn button(p: &mut P, r: R, radius: f32, label: &str, selected: bool, st: &UiState, a: Action) {
    let (g, border, face, col) = if selected {
        (p.vg(r[1], r[1] + r[3], &[(0.0, hex(0x3d4652)), (1.0, hex(0x1d232b))]), 0x0f1318, Face::Orbitron800, 0x7fffd0)
    } else {
        (p.vg(r[1], r[1] + r[3], &[(0.0, hex(0xeef1f4)), (1.0, hex(0xb6bec7))]), 0x5a636e, Face::Orbitron500, 0x1d2228)
    };
    p.rr(r[0], r[1], r[2], r[3], radius, &g);
    p.rr_stroke(r[0], r[1], r[2], r[3], radius, 1.0, hex(border));
    p.text_c(face, 12.5, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0, col, label, 0.0);
    press_tint(p, r, radius, st, a);
}

fn draw_controls(p: &mut P, st: &UiState) {
    let prefs = st.prefs;
    // power pill
    let r = power_btn();
    let (g, border, col, label) = if prefs.power {
        (p.vg(r[1], r[1] + r[3], &[(0.0, hex(0xa8ffe0)), (1.0, hex(0x2fbf8c))]), 0x136b4c, 0x0a2e21, "ON")
    } else {
        (p.vg(r[1], r[1] + r[3], &[(0.0, hex(0x3d4652)), (1.0, hex(0x1d232b))]), 0x0f1318, 0x8a96a3, "OFF")
    };
    p.rr(r[0], r[1], r[2], r[3], 22.0, &g);
    p.rr_stroke(r[0], r[1], r[2], r[3], 22.0, 1.0, hex(border));
    let tw = p.tw(Face::Orbitron800, 14.0, label, 2.0) - 2.0;
    let b = p.base(Face::Orbitron800, 14.0, r[1] + r[3] / 2.0);
    p.text(Face::Orbitron800, 14.0, r[0] + (r[2] - tw) / 2.0, b, col, 1.0, label, 2.0);
    press_tint(p, r, 22.0, st, Action::Power);

    if st.snap.module.needs_setup() {
        draw_setup(p, st);
        return;
    }
    for (i, (w, l)) in [(WaveMode::Normal, "NORMAL"), (WaveMode::Split, "SPLIT"), (WaveMode::HideBoth, "HIDE BOTH")].into_iter().enumerate() {
        button(p, wave_btn(i), 10.0, l, prefs.waveform == w, st, Action::Wave(w));
    }
    for (i, (ph, l)) in [(PhaseSetting::Off, "OFF"), (PhaseSetting::Box, "BOX"), (PhaseSetting::Line, "LINE")].into_iter().enumerate() {
        button(p, phase_btn(i), 10.0, l, prefs.phase == ph, st, Action::Phase(ph));
    }
}

/// The setup step: the waveform and phase controls stay visible but dimmed, with the INSTALL button below them.
fn draw_setup(p: &mut P, st: &UiState) {
    let prefs = st.prefs;
    for (i, (w, l)) in [(WaveMode::Normal, "NORMAL"), (WaveMode::Split, "SPLIT"), (WaveMode::HideBoth, "HIDE BOTH")].into_iter().enumerate() {
        button_off(p, wave_btn(i), 10.0, l, prefs.waveform == w);
    }
    for (i, (ph, l)) in [(PhaseSetting::Off, "OFF"), (PhaseSetting::Box, "BOX"), (PhaseSetting::Line, "LINE")].into_iter().enumerate() {
        button_off(p, phase_btn(i), 10.0, l, prefs.phase == ph);
    }
    let r = install_btn();
    let g = p.vg(r[1], r[1] + r[3], &[(0.0, hex(0xa8ffe0)), (1.0, hex(0x2fbf8c))]);
    p.rr(r[0], r[1], r[2], r[3], 22.0, &g);
    p.rr_stroke(r[0], r[1], r[2], r[3], 22.0, 1.0, hex(0x136b4c));
    p.text_c(Face::Orbitron800, 14.0, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0, 0x0a2e21, "INSTALL", 2.0);
    press_tint(p, r, 22.0, st, Action::InstallModule);
}

/// A control that cannot be used yet: drawn flat and faded, no hover.
fn button_off(p: &mut P, r: R, radius: f32, label: &str, selected: bool) {
    p.rr(r[0], r[1], r[2], r[3], radius, &solid(hex(if selected { 0x7e8792 } else { 0xaab2bb })));
    p.rr_stroke(r[0], r[1], r[2], r[3], radius, 1.0, hex(0x8d96a0));
    p.text_c(Face::Orbitron500, 12.5, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0, 0x6b747e, label, 0.0);
}

fn draw_settings(p: &mut P, st: &UiState) {
    // dark inset screen over the panel
    let (x, y, w, h) = (PX, 244.0, PW, 240.0);
    p.rr(x, y, w, h, 10.0, &solid(hex(0x0b130f)));
    p.rr_stroke(x, y, w, h, 10.0, 2.0, hex(0x2b3138));
    let mut by = y + 22.0;
    p.text(Face::Orbitron800, 12.0, x + 14.0, by, 0x7fffd0, 1.0, "SETTINGS / ABOUT", 2.0);
    by += 18.0;
    let intro = [format!("CLUB685 {} · local UDP 127.0.0.1:47990 only, no telemetry", env!("CARGO_PKG_VERSION"))];
    for l in &intro {
        p.text(Face::Mono, 11.0, x + 14.0, by, 0x4fbf98, 1.0, l, 0.0);
        by += 15.0;
    }
    by += 2.0;
    p.rect(x + 14.0, by - 10.0, w - 28.0, 1.0, hex(0x24503f));
    let diag = diagnostics_text(st.snap, st.prefs);
    for l in diag.lines().skip(1) {
        let mut line = l.to_string();
        while p.tw(Face::Mono, 11.0, &line, 0.0) > w - 28.0 && line.chars().count() > 4 {
            line.pop();
            line.pop();
            line.push('…');
        }
        p.text(Face::Mono, 11.0, x + 14.0, by, 0x7fffd0, 0.92, &line, 0.0);
        by += 13.5;
    }
    let m = Mode::of(st.snap);
    button(p, copy_btn(m), 10.0, if st.copied { "COPIED" } else { "COPY DIAGNOSTICS" }, st.copied, st, Action::CopyDiag);
    if m.installed {
        button(p, remove_btn(), 10.0, "REMOVE MODULE", false, st, Action::RemoveModule);
    }
    button(p, close_btn(m), 10.0, "CLOSE", false, st, Action::CloseSettings);
}

fn draw_footer(p: &mut P, st: &UiState) {
    let f = footer();
    let r = f.toggle;
    let g = p.vg(r[1], r[1] + r[3], &[(0.0, hex(0x5a636e)), (1.0, hex(0x2b3138))]);
    p.rr(r[0], r[1], r[2], r[3], 11.0, &g);
    if st.prefs.disco {
        let (cr, cg, cb) = hsv(st.time * 90.0, 0.6, 1.0);
        p.rr(r[0] + 1.0, r[1] + 1.0, r[2] - 2.0, r[3] - 2.0, 10.0, &solid(rgba(cr, cg, cb, 70)));
    }
    p.rr_stroke(r[0], r[1], r[2], r[3], 11.0, 1.0, hex(0x0f1318));
    let kx = if st.prefs.disco { r[0] + r[2] - 2.0 - 8.0 - 1.0 } else { r[0] + 2.0 + 8.0 + 1.0 };
    let (k0, k1) = if st.prefs.disco { (0xe8fff6, 0x2fbf8c) } else { (0xffffff, 0x9aa3ae) };
    let kg = radial(kx - 8.0 + 5.6, r[1] + 3.0 + 4.8, 10.7, hex(k0), hex(k1));
    p.circle(kx, r[1] + r[3] / 2.0, 8.0, &kg);
    if st.hover == Some(Action::Disco) {
        p.rr(r[0], r[1], r[2], r[3], 11.0, &solid(rgba(255, 255, 255, 30)));
    }
    if st.hover == Some(Action::Settings) || st.settings_open {
        let ws = text::measure(Face::Orbitron500, 11.0 * p.s, "SETTINGS", 2.0 * p.s) / p.s;
        p.rect(f.settings_x, 581.0, ws - 2.0, 1.0, hex(0x2b3138));
    }
}

/// App icon: chrome rounded square with a green "685" LCD, `size` pixels square (straight RGBA via tiny-skia).
pub fn draw_icon(size: u32) -> Pixmap {
    let s = size as f32 / 64.0;
    let mut p = P { pm: Pixmap::new(size, size).unwrap(), s };
    let body = p.vg(0.0, 64.0, &[(0.0, hex(0xe3e8ee)), (0.35, hex(0x9aa3ae)), (0.65, hex(0xc7cdd5)), (1.0, hex(0x7e8792))]);
    p.rr(1.0, 1.0, 62.0, 62.0, 14.0, &body);
    p.rr_stroke(1.0, 1.0, 62.0, 62.0, 14.0, 1.5, hex(0x4a525c));
    p.rr(8.0, 14.0, 48.0, 36.0, 7.0, &solid(hex(0x0b130f)));
    p.rr_stroke(8.0, 14.0, 48.0, 36.0, 7.0, 1.5, hex(0x2b3138));
    let cell = 9.0;
    for i in 0..4 {
        let col = if i == 0 { 0x7fffd0 } else { 0x1f4f3e };
        p.rr(12.0 + i as f32 * (cell + 1.8), 36.0, cell, 8.0, 1.5, &solid(hex(col)));
    }
    p.text_c(Face::Orbitron800, 15.0, 32.0, 25.0, 0x7fffd0, "685", 0.5);
    p.pm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_regions_do_not_overlap_in_the_main_view() {
        let h = hits(false, Mode::default());
        for (i, (a, ra)) in h.iter().enumerate() {
            for (b, rb) in h.iter().skip(i + 1) {
                let overlap = ra[0] < rb[0] + rb[2] && rb[0] < ra[0] + ra[2] && ra[1] < rb[1] + rb[3] && rb[1] < ra[1] + ra[3];
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn title_buttons_are_not_drag_regions() {
        assert!(is_drag_region(100.0, 26.0));
        assert!(!is_drag_region(430.0, 26.0));
        assert!(!is_drag_region(409.0, 26.0));
        assert!(!is_drag_region(100.0, 100.0));
        assert_eq!(hit_test(431.0, 26.0, false, Mode::default()), Some(Action::Quit));
        assert_eq!(hit_test(409.0, 26.0, false, Mode::default()), Some(Action::Minimise));
    }

    #[test]
    fn hit_test_finds_the_controls() {
        assert_eq!(hit_test(370.0, 260.0, false, Mode::default()), Some(Action::Power));
        assert_eq!(hit_test(30.0, 330.0, false, Mode::default()), Some(Action::Wave(WaveMode::Normal)));
        assert_eq!(hit_test(230.0, 330.0, false, Mode::default()), Some(Action::Wave(WaveMode::Split)));
        assert_eq!(hit_test(400.0, 410.0, false, Mode::default()), Some(Action::Phase(PhaseSetting::Line)));
        assert_eq!(hit_test(30.0, 330.0, true, Mode::default()), None);
        assert_eq!(hit_test(100.0, 510.0, true, Mode::default()), Some(Action::CopyDiag));
    }

    #[test]
    fn setup_disables_the_waveform_and_phase_controls() {
        let setup = Mode { setup: true, installed: false };
        assert_eq!(hit_test(30.0, 330.0, false, setup), None);
        assert_eq!(hit_test(400.0, 410.0, false, setup), None);
        assert_eq!(hit_test(370.0, 260.0, false, setup), Some(Action::Power));
        assert_eq!(hit_test(200.0, 480.0, false, setup), Some(Action::InstallModule));
        assert_eq!(hit_test(200.0, 480.0, false, Mode::default()), None);
    }

    #[test]
    fn remove_module_is_in_settings_only_when_installed() {
        let inst = Mode { setup: false, installed: true };
        assert_eq!(hit_test(250.0, 510.0, true, inst), Some(Action::RemoveModule));
        assert_eq!(hit_test(250.0, 510.0, true, Mode::default()), Some(Action::CopyDiag));
        assert_eq!(hit_test(420.0, 510.0, true, inst), Some(Action::CloseSettings));
        let h = hits(true, inst);
        for (i, (a, ra)) in h.iter().enumerate() {
            for (b, rb) in h.iter().skip(i + 1) {
                let overlap = ra[0] < rb[0] + rb[2] && rb[0] < ra[0] + ra[2] && ra[1] < rb[1] + rb[3] && rb[1] < ra[1] + ra[3];
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn setup_window_renders() {
        let mut snap = Snapshot::default();
        snap.module = ModuleState::Missing;
        snap.kind = StateKind::Setup;
        snap.line = "One-time setup: install the rekordbox module".to_string();
        let pv = PreviewState::from_snapshot(&snap);
        let r = UiRenderer::new(1.0);
        let pm = r.paint(&UiState { prefs: &Prefs::default(), snap: &snap, preview: &pv, hover: None, pressed: None, settings_open: false, copied: false, time: 0.0 });
        assert_eq!((pm.width(), pm.height()), r.size());
    }

    #[test]
    fn preview_status_line_names_both_settings() {
        let p = Prefs { waveform: WaveMode::HideBoth, phase: PhaseSetting::Line, ..Prefs::default() };
        assert_eq!(preview_line(&p), "Preview: Hide Both + Line · switch to rekordbox to apply");
        assert_eq!(preview_line(&Prefs::default()), "Preview: Normal + no meter · switch to rekordbox to apply");
    }

    #[test]
    fn window_renders_at_two_scales() {
        let snap = Snapshot::default();
        let prefs = Prefs::default();
        for s in [1.0f32, 1.25] {
            let r = UiRenderer::new(s);
            let pv = PreviewState::from_snapshot(&snap);
            let pm = r.paint(&UiState { prefs: &prefs, snap: &snap, preview: &pv, hover: None, pressed: None, settings_open: false, copied: false, time: 0.0 });
            assert_eq!((pm.width(), pm.height()), r.size());
            // corners are transparent, the middle is opaque
            assert_eq!(pm.pixel(0, 0).unwrap().alpha(), 0);
            assert_eq!(pm.pixel(pm.width() / 2, pm.height() / 2).unwrap().alpha(), 255);
        }
    }
}
