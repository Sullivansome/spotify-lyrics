//! BGRA renderer — layout inspired by Lyrisland’s compact / expanded island views.
#![allow(clippy::too_many_arguments)]
//!
//! Layout is in **logical points**. The raster is `scale`× physical pixels
//! so Retina (2×) does not look like a stretched 1× bitmap.

use fontdue::{Font, FontSettings};

use island_plugin_api::SurfaceFrame;

use crate::lyrics::{LyricsState, SyncedLyrics, is_cjk_char};
use crate::spotify::PlaybackState;

const BG: (u8, u8, u8) = (10, 10, 12);
const FG: (u8, u8, u8) = (245, 245, 247);
const SPOTIFY: (u8, u8, u8) = (30, 215, 96);
const MARQUEE_SPEED: f32 = 30.0;
const MARQUEE_START_DELAY: f64 = 1.5;
const MARQUEE_END_BUFFER: f64 = 0.5;
const MARQUEE_FADE_WIDTH: f32 = 10.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IdleKind {
    #[default]
    NotPlaying,
    Denied,
    Error,
}

pub struct Fonts {
    latin: Font,
    cjk: Option<Font>,
    latin_path: String,
    cjk_path: Option<String>,
}

impl Fonts {
    pub fn load() -> Option<Self> {
        const LATIN: &[&str] = &[
            // Prefer a conventional outline TTF for the out-of-process pixel
            // renderer. SFNS can parse successfully yet yield empty glyph
            // bitmaps when the plugin executable is signed inside an app.
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/SFNSRounded.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        const CJK: &[&str] = &[
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/Library/Fonts/Arial Unicode.ttf",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/Supplemental/Songti.ttc",
            "/System/Library/Fonts/PingFang.ttc",
        ];
        let (latin, latin_path) = load_font(LATIN)?;
        let cjk = load_font(CJK);
        let (cjk, cjk_path) = cjk
            .map(|(font, path)| (Some(font), Some(path)))
            .unwrap_or_default();
        Some(Self {
            latin,
            cjk,
            latin_path,
            cjk_path,
        })
    }

    fn face(&self, ch: char) -> &Font {
        if is_cjk_char(ch) {
            self.cjk.as_ref().unwrap_or(&self.latin)
        } else {
            &self.latin
        }
    }

    pub fn description(&self) -> String {
        match self.cjk_path.as_deref() {
            Some(cjk) => format!("fonts: latin={} · cjk={cjk}", self.latin_path),
            None => format!("fonts: latin={} · cjk=fallback", self.latin_path),
        }
    }
}

fn load_font(paths: &[&str]) -> Option<(Font, String)> {
    for path in paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        for idx in 0..4u32 {
            let settings = FontSettings {
                collection_index: idx,
                scale: 40.0,
                ..FontSettings::default()
            };
            if let Ok(font) = Font::from_bytes(bytes.as_slice(), settings) {
                return Some((font, (*path).to_string()));
            }
        }
    }
    None
}

/// Logical-point canvas backed by a `scale`× BGRA buffer.
struct Raster {
    buf: Vec<u8>,
    pw: u32,
    ph: u32,
    scale: f32,
    logical_w: f32,
    logical_h: f32,
}

impl Raster {
    fn new(logical_w: u32, logical_h: u32, scale: f32) -> Self {
        let scale = scale.clamp(1.0, 3.0);
        let pw = ((logical_w as f32) * scale).round().max(1.0) as u32;
        let ph = ((logical_h as f32) * scale).round().max(1.0) as u32;
        Self {
            buf: vec![0u8; (pw as usize) * (ph as usize) * 4],
            pw,
            ph,
            scale,
            logical_w: logical_w as f32,
            logical_h: logical_h as f32,
        }
    }

    fn sx(&self, v: f32) -> f32 {
        v * self.scale
    }

    fn fill_rect_pt(&mut self, x: f32, y: f32, w: f32, h: f32, c: (u8, u8, u8)) {
        let x0 = self.sx(x).round().max(0.0) as u32;
        let y0 = self.sx(y).round().max(0.0) as u32;
        let rw = self.sx(w).round().max(1.0) as u32;
        let rh = self.sx(h).round().max(1.0) as u32;
        self.fill_rect_px(x0, y0, rw, rh, c.2, c.1, c.0, 255);
    }

    fn fill_rect_px(&mut self, x: u32, y: u32, rw: u32, rh: u32, b: u8, g: u8, r: u8, a: u8) {
        let x2 = (x + rw).min(self.pw);
        let y2 = (y + rh).min(self.ph);
        let x1 = x.min(self.pw);
        let y1 = y.min(self.ph);
        for yy in y1..y2 {
            for xx in x1..x2 {
                let i = ((yy * self.pw + xx) * 4) as usize;
                self.buf[i] = b;
                self.buf[i + 1] = g;
                self.buf[i + 2] = r;
                self.buf[i + 3] = a;
            }
        }
    }

    fn fill_triangle(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x3: f32,
        y3: f32,
        c: (u8, u8, u8),
    ) {
        let x1 = self.sx(x1);
        let y1 = self.sx(y1);
        let x2 = self.sx(x2);
        let y2 = self.sx(y2);
        let x3 = self.sx(x3);
        let y3 = self.sx(y3);
        let min_x = x1.min(x2).min(x3).floor().max(0.0) as i32;
        let max_x = x1.max(x2).max(x3).ceil().min(self.pw as f32) as i32;
        let min_y = y1.min(y2).min(y3).floor().max(0.0) as i32;
        let max_y = y1.max(y2).max(y3).ceil().min(self.ph as f32) as i32;
        for y in min_y..max_y {
            for x in min_x..max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = edge(x2, y2, x3, y3, px, py);
                let w1 = edge(x3, y3, x1, y1, px, py);
                let w2 = edge(x1, y1, x2, y2, px, py);
                if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                    let i = ((y as u32 * self.pw + x as u32) * 4) as usize;
                    if i + 3 < self.buf.len() {
                        self.buf[i] = c.2;
                        self.buf[i + 1] = c.1;
                        self.buf[i + 2] = c.0;
                        self.buf[i + 3] = 255;
                    }
                }
            }
        }
    }

    fn draw_text(
        &mut self,
        fonts: Option<&Fonts>,
        x: f32,
        y: f32,
        px_size: f32,
        text: &str,
        rgb: (u8, u8, u8),
    ) {
        self.draw_text_with_clip(fonts, x, y, px_size, text, rgb, 0, self.pw);
    }

    fn draw_text_clipped(
        &mut self,
        fonts: Option<&Fonts>,
        x: f32,
        y: f32,
        px_size: f32,
        text: &str,
        rgb: (u8, u8, u8),
        clip_x: f32,
        clip_w: f32,
    ) {
        let clip_x0 = self.sx(clip_x).round().max(0.0) as u32;
        let clip_x1 = self
            .sx(clip_x + clip_w)
            .round()
            .clamp(clip_x0 as f32, self.pw as f32) as u32;
        self.draw_text_with_clip(fonts, x, y, px_size, text, rgb, clip_x0, clip_x1);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text_with_clip(
        &mut self,
        fonts: Option<&Fonts>,
        x: f32,
        y: f32,
        px_size: f32,
        text: &str,
        rgb: (u8, u8, u8),
        clip_x0: u32,
        clip_x1: u32,
    ) {
        let Some(fonts) = fonts else {
            return;
        };
        let size = px_size * self.scale;
        let mut cursor_x = self.sx(x);
        let baseline = self.sx(y);
        for ch in text.chars() {
            let face = fonts.face(ch);
            let (metrics, bitmap) = face.rasterize(ch, size);
            let gx = cursor_x + metrics.xmin as f32;
            let gy = baseline - metrics.ymin as f32 - metrics.height as f32;
            blit_glyph(
                &mut self.buf,
                self.pw,
                self.ph,
                gx.round() as i32,
                gy.round() as i32,
                metrics.width,
                metrics.height,
                &bitmap,
                rgb.2,
                rgb.1,
                rgb.0,
                clip_x0,
                clip_x1,
            );
            cursor_x += metrics.advance_width;
        }
    }

    fn fade_horizontal_edges(&mut self, x: f32, w: f32, fade_leading: bool, fade_trailing: bool) {
        let x0 = self.sx(x).round().max(0.0) as u32;
        let x1 = self.sx(x + w).round().clamp(x0 as f32, self.pw as f32) as u32;
        let fade_w = self.sx(MARQUEE_FADE_WIDTH).round().max(1.0) as u32;

        for yy in 0..self.ph {
            for xx in x0..x1 {
                let leading = if fade_leading && xx < x0 + fade_w {
                    (xx - x0) as f32 / fade_w as f32
                } else {
                    1.0
                };
                let trailing = if fade_trailing && xx + fade_w > x1 {
                    (x1 - xx - 1) as f32 / fade_w as f32
                } else {
                    1.0
                };
                let opacity = leading.min(trailing).clamp(0.0, 1.0);
                let alpha = ((yy * self.pw + xx) * 4 + 3) as usize;
                self.buf[alpha] = (self.buf[alpha] as f32 * opacity).round() as u8;
            }
        }
    }

    fn draw_text_centered(
        &mut self,
        fonts: Option<&Fonts>,
        y: f32,
        px_size: f32,
        text: &str,
        rgb: (u8, u8, u8),
    ) {
        let tw = approx_text_width(fonts, px_size, text);
        let x = (self.logical_w - tw) * 0.5;
        self.draw_text(fonts, x, y, px_size, text, rgb);
    }
}

pub fn render_frame(
    logical_w: u32,
    logical_h: u32,
    scale: f32,
    frame_id: u64,
    fonts: Option<&Fonts>,
    idle: IdleKind,
    track: Option<&PlaybackState>,
    lyrics: &LyricsState,
    position: f64,
    pulse: f32,
    reduce_motion: bool,
    accent: (u8, u8, u8),
) -> SurfaceFrame {
    let mut rast = Raster::new(logical_w, logical_h, scale);

    // The host owns the black rounded notch chrome. Plugin frames are
    // content-only so an opaque rectangle cannot square off its corners.
    if logical_h > 48 {
        let hair = fade(FG, 0.12);
        rast.fill_rect_pt(0.0, 0.0, rast.logical_w, 1.0, hair);
        rast.fill_rect_pt(0.0, rast.logical_h - 1.0, rast.logical_w, 1.0, hair);
    }

    match (idle, track) {
        (IdleKind::Denied, _) => render_denied(fonts, &mut rast),
        (IdleKind::Error, _) => render_error(fonts, &mut rast),
        (_, None) => render_idle(fonts, &mut rast),
        (_, Some(t)) => {
            if logical_h <= 48 {
                render_compact(
                    fonts,
                    &mut rast,
                    t,
                    lyrics,
                    position,
                    pulse,
                    reduce_motion,
                    accent,
                );
            } else if logical_w <= 260 {
                // Atoll drop under the camera: narrow column, not a 380-wide card.
                render_lyric_drop(fonts, &mut rast, t, lyrics, position, pulse, accent);
            } else {
                render_expanded(fonts, &mut rast, t, lyrics, position, pulse, accent);
            }
        }
    }

    SurfaceFrame {
        width: rast.pw,
        height: rast.ph,
        frame_id,
        pixels: rast.buf,
    }
}

fn render_idle(fonts: Option<&Fonts>, rast: &mut Raster) {
    render_status(fonts, rast, "Open Spotify", SPOTIFY);
}

fn render_denied(fonts: Option<&Fonts>, rast: &mut Raster) {
    render_status(fonts, rast, "Allow Spotify access", (245, 166, 35));
}

fn render_error(fonts: Option<&Fonts>, rast: &mut Raster) {
    render_status(fonts, rast, "Spotify unavailable", (235, 92, 92));
}

fn render_status(fonts: Option<&Fonts>, rast: &mut Raster, status: &str, accent: (u8, u8, u8)) {
    let marker = 4.0;
    let marker_x = 11.0;
    rast.fill_rect_pt(
        marker_x,
        (rast.logical_h - marker) * 0.5,
        marker,
        marker,
        accent,
    );
    let text_x = 24.0;
    let max_w = (rast.logical_w - text_x - 10.0).max(20.0);
    rast.draw_text(
        fonts,
        text_x,
        rast.logical_h * 0.66,
        12.0,
        &truncate_width(fonts, 12.0, status, max_w),
        fade(FG, 0.68),
    );
}

fn render_compact(
    fonts: Option<&Fonts>,
    rast: &mut Raster,
    track: &PlaybackState,
    lyrics: &LyricsState,
    position: f64,
    pulse: f32,
    reduce_motion: bool,
    accent: (u8, u8, u8),
) {
    draw_playing_bars(
        rast,
        12.0,
        rast.logical_h * 0.5,
        track.is_playing,
        pulse,
        accent,
    );

    let text_x = 34.0;
    let max_w = rast.logical_w - text_x - 12.0;
    let ready = match lyrics {
        LyricsState::Ready(ready) => Some(ready),
        _ => None,
    };
    let (cur, next) = ready
        .map(|lyrics| lyrics.current_and_next(position))
        .unwrap_or((None, None));
    let cur = cur.filter(|line| !line.trim().is_empty());
    let next = next.filter(|line| !line.trim().is_empty());

    let (text, color, timing) = if let Some(line) = cur {
        let timing = ready.and_then(|lyrics| {
            let index = lyrics.line_index_at(position)?;
            let current = lyrics.lines.get(index)?;
            if current.text.trim().is_empty() {
                return None;
            }
            let duration = lyrics
                .lines
                .get(index + 1)
                .map(|next| next.time - current.time);
            Some(((position - current.time).max(0.0), duration))
        });
        (line.to_string(), FG, timing)
    } else if let Some(line) = next {
        // Before the first timestamp or across an instrumental row, show the
        // next real line instead of leaving the strip unexplained.
        (line.to_string(), fade(FG, 0.7), None)
    } else {
        let fallback = match lyrics {
            LyricsState::Loading => "Fetching lyrics…".to_string(),
            LyricsState::Missing => format!("No lyrics · {}", track.title),
            LyricsState::Idle if !track.is_playing => "Paused".to_string(),
            LyricsState::Idle | LyricsState::Ready(_) => track.title.clone(),
        };
        (fallback, fade(FG, 0.58), None)
    };

    let size = 13.0;
    let text_w = approx_text_width(fonts, size, &text);
    let overflow = (text_w - max_w).max(0.0);
    if let Some((elapsed, line_duration)) = timing.filter(|_| overflow > 0.0 && !reduce_motion) {
        let offset = marquee_offset(overflow, elapsed, line_duration);
        rast.draw_text_clipped(
            fonts,
            text_x + offset,
            rast.logical_h * 0.66,
            size,
            &text,
            color,
            text_x,
            max_w,
        );
        rast.fade_horizontal_edges(text_x, max_w, offset < -0.5, offset > -overflow + 0.5);
    } else {
        rast.draw_text(
            fonts,
            text_x,
            rast.logical_h * 0.66,
            size,
            &truncate_width(fonts, size, &text, max_w),
            color,
        );
    }
}

fn marquee_offset(overflow: f32, elapsed: f64, line_duration: Option<f64>) -> f32 {
    if overflow <= 0.0 || elapsed <= MARQUEE_START_DELAY {
        return 0.0;
    }

    // Match Lyrisland: use the lyric's remaining time when possible, falling
    // back to a calm constant speed for the final line or very short rows.
    let timed_budget = line_duration
        .map(|duration| duration - MARQUEE_START_DELAY - MARQUEE_END_BUFFER)
        .filter(|budget| *budget > 0.3);
    let scroll_duration = timed_budget.unwrap_or(overflow as f64 / MARQUEE_SPEED as f64);
    let progress = ((elapsed - MARQUEE_START_DELAY) / scroll_duration).clamp(0.0, 1.0) as f32;
    // Smoothstep easing so the scroll eases in/out instead of starting and
    // stopping at full speed. Identity at 0, 0.5, and 1.
    let eased = progress * progress * (3.0 - 2.0 * progress);
    -overflow * eased
}

/// Lyrics hanging under the hardware notch. Camera band is empty in the host;
/// this canvas is *only* the drop, so every row is content.
fn render_lyric_drop(
    fonts: Option<&Fonts>,
    rast: &mut Raster,
    track: &PlaybackState,
    lyrics: &LyricsState,
    position: f64,
    pulse: f32,
    accent: (u8, u8, u8),
) {
    let pad = 10.0;
    let max_w = (rast.logical_w - pad * 2.0).max(24.0);
    let mid_y = rast.logical_h * 0.46;

    let meta = truncate_width(
        fonts,
        10.0,
        &format!("{}  ·  {}", track.title, track.artist),
        max_w,
    );
    rast.draw_text_centered(fonts, 16.0, 10.0, &meta, fade(FG, 0.42));
    draw_playing_bars(
        rast,
        rast.logical_w * 0.5 - 8.0,
        28.0,
        track.is_playing,
        pulse,
        accent,
    );

    match lyrics {
        LyricsState::Ready(ly) => {
            let (cur, next) = ly.current_and_next(position);
            if let Some(line) = cur {
                rast.draw_text_centered(
                    fonts,
                    mid_y,
                    15.0,
                    &truncate_width(fonts, 15.0, line, max_w),
                    FG,
                );
                if let Some(n) = next {
                    rast.draw_text_centered(
                        fonts,
                        mid_y + 22.0,
                        11.0,
                        &truncate_width(fonts, 11.0, n, max_w),
                        fade(FG, 0.38),
                    );
                }
            } else if let Some(first) = ly.lines.first() {
                rast.draw_text_centered(
                    fonts,
                    mid_y,
                    14.0,
                    &truncate_width(fonts, 14.0, &first.text, max_w),
                    fade(FG, 0.45),
                );
            }
        }
        LyricsState::Loading => {
            rast.draw_text_centered(fonts, mid_y, 13.0, "Fetching lyrics…", fade(FG, 0.5));
        }
        LyricsState::Missing | LyricsState::Idle => {
            rast.draw_text_centered(fonts, mid_y, 13.0, "No lyrics", fade(FG, 0.45));
        }
    }

    let dur = (track.duration_ms as f64 / 1000.0).max(1.0);
    let prog = (position / dur).clamp(0.0, 1.0) as f32;
    let bar_w = rast.logical_w - pad * 2.0;
    let bar_y = rast.logical_h - 10.0;
    rast.fill_rect_pt(pad, bar_y, bar_w, 2.0, (40, 42, 40));
    rast.fill_rect_pt(pad, bar_y, (bar_w * prog).max(2.0), 2.0, accent);
}

fn render_expanded(
    fonts: Option<&Fonts>,
    rast: &mut Raster,
    track: &PlaybackState,
    lyrics: &LyricsState,
    position: f64,
    pulse: f32,
    accent: (u8, u8, u8),
) {
    let pad_x = 16.0;
    let header_max = rast.logical_w - 56.0;

    let title = truncate_width(fonts, 12.0, &track.title, header_max * 0.55);
    rast.draw_text(fonts, pad_x, 22.0, 12.0, &title, fade(FG, 0.62));
    let title_w = approx_text_width(fonts, 12.0, &title);
    let rest = (header_max - title_w - 8.0).max(40.0);
    let artist = truncate_width(fonts, 12.0, &track.artist, rest);
    rast.draw_text(
        fonts,
        pad_x + title_w + 6.0,
        22.0,
        12.0,
        &format!("— {artist}"),
        fade(FG, 0.38),
    );

    draw_playing_bars(
        rast,
        rast.logical_w - 28.0,
        18.0,
        track.is_playing,
        pulse,
        accent,
    );
    rast.fill_rect_pt(14.0, 32.0, rast.logical_w - 28.0, 1.0, fade(FG, 0.12));

    match lyrics {
        LyricsState::Ready(ly) => draw_lyrics_stack(fonts, rast, pad_x, ly, position),
        LyricsState::Loading => {
            rast.draw_text(fonts, pad_x, 72.0, 13.0, "Fetching lyrics…", fade(FG, 0.4));
        }
        LyricsState::Missing | LyricsState::Idle => {
            rast.draw_text(
                fonts,
                pad_x,
                66.0,
                13.0,
                "No lyrics for this track",
                fade(FG, 0.4),
            );
            rast.draw_text(
                fonts,
                pad_x,
                86.0,
                11.0,
                "LRCLIB / NetEase had no match",
                fade(FG, 0.28),
            );
        }
    }

    if let LyricsState::Ready(ly) = lyrics {
        rast.draw_text(
            fonts,
            rast.logical_w - 56.0,
            22.0,
            9.0,
            &ly.source,
            fade(FG, 0.28),
        );
    }

    let mid = rast.logical_w / 2.0;
    let cy = rast.logical_h - 36.0;
    let icon = fade(FG, 0.78);
    draw_icon_prev(rast, mid - 48.0, cy, icon);
    if track.is_playing {
        draw_icon_pause(rast, mid, cy, accent);
    } else {
        draw_icon_play(rast, mid, cy, accent);
    }
    draw_icon_next(rast, mid + 48.0, cy, icon);

    let dur = (track.duration_ms as f64 / 1000.0).max(1.0);
    let prog = (position / dur).clamp(0.0, 1.0) as f32;
    let bar_w = rast.logical_w - 32.0;
    rast.fill_rect_pt(16.0, rast.logical_h - 12.0, bar_w, 2.0, (40, 42, 40));
    let fw = (bar_w * prog).max(2.0);
    // Breathe by lifting the accent toward white instead of a green-only hack.
    let lift = (0.5 + 0.5 * pulse.sin()) * 0.18;
    rast.fill_rect_pt(16.0, rast.logical_h - 12.0, fw, 2.0, lighten(accent, lift));
}

fn draw_lyrics_stack(
    fonts: Option<&Fonts>,
    rast: &mut Raster,
    pad_x: f32,
    ly: &SyncedLyrics,
    position: f64,
) {
    let idx = ly.line_index_at(position);
    let lines = &ly.lines;
    let max_w = rast.logical_w - pad_x - 16.0;

    let show: Vec<(usize, f32, f32, bool)> = match idx {
        None => {
            if let Some(first) = lines.first() {
                rast.draw_text(
                    fonts,
                    pad_x,
                    70.0,
                    14.0,
                    &truncate_width(fonts, 14.0, &first.text, max_w),
                    fade(FG, 0.35),
                );
            }
            vec![]
        }
        Some(i) => {
            let mut v = Vec::new();
            if i > 0 {
                v.push((i - 1, 52.0, 12.0, false));
            }
            v.push((i, if i > 0 { 74.0 } else { 62.0 }, 15.0, true));
            if i + 1 < lines.len() {
                v.push((i + 1, if i > 0 { 96.0 } else { 86.0 }, 12.0, false));
            }
            v
        }
    };

    for (li, y, size, is_cur) in show {
        let text = truncate_width(fonts, size, &lines[li].text, max_w);
        let color = if is_cur {
            FG
        } else {
            let dist = if idx.map(|i| i.abs_diff(li)).unwrap_or(2) == 1 {
                0.5
            } else {
                0.28
            };
            fade(FG, dist)
        };
        rast.draw_text(fonts, pad_x, y, size, &text, color);
    }
}

fn draw_playing_bars(
    rast: &mut Raster,
    cx: f32,
    cy: f32,
    playing: bool,
    pulse: f32,
    accent: (u8, u8, u8),
) {
    let max_h = 14.0f32;
    for i in 0..3 {
        let phase = pulse * 4.0 + i as f32 * 0.9;
        let scale = if playing {
            0.35 + 0.65 * ((phase.sin() * 0.5 + 0.5).max(0.0))
        } else {
            0.35
        };
        let bh = (max_h * scale).max(3.0);
        let bx = cx + i as f32 * 5.0;
        let by = (cy - bh / 2.0).max(0.0);
        rast.fill_rect_pt(bx, by, 3.0, bh, accent);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlHit {
    Previous,
    PlayPause,
    Next,
}

pub fn hit_test(width: u32, height: u32, x: f32, y: f32) -> Option<ControlHit> {
    if height <= 48 {
        return None;
    }
    // Narrow Atoll drop has no transport row — tap the body to play/pause.
    if width <= 260 {
        return Some(ControlHit::PlayPause);
    }
    let mid = width as f32 / 2.0;
    let cy = height as f32 - 36.0;
    if y < cy - 16.0 || y > cy + 18.0 {
        return None;
    }
    if (x - (mid - 48.0)).abs() < 22.0 {
        Some(ControlHit::Previous)
    } else if (x - mid).abs() < 22.0 {
        Some(ControlHit::PlayPause)
    } else if (x - (mid + 48.0)).abs() < 22.0 {
        Some(ControlHit::Next)
    } else {
        None
    }
}

fn draw_icon_play(rast: &mut Raster, cx: f32, cy: f32, c: (u8, u8, u8)) {
    rast.fill_triangle(cx - 5.0, cy - 8.0, cx - 5.0, cy + 8.0, cx + 8.0, cy, c);
}

fn draw_icon_pause(rast: &mut Raster, cx: f32, cy: f32, c: (u8, u8, u8)) {
    rast.fill_rect_pt(cx - 6.0, cy - 7.0, 4.0, 14.0, c);
    rast.fill_rect_pt(cx + 3.0, cy - 7.0, 4.0, 14.0, c);
}

fn draw_icon_prev(rast: &mut Raster, cx: f32, cy: f32, c: (u8, u8, u8)) {
    rast.fill_rect_pt(cx - 8.0, cy - 6.0, 2.0, 12.0, c);
    rast.fill_triangle(cx + 7.0, cy - 7.0, cx + 7.0, cy + 7.0, cx - 5.0, cy, c);
}

fn draw_icon_next(rast: &mut Raster, cx: f32, cy: f32, c: (u8, u8, u8)) {
    rast.fill_triangle(cx - 7.0, cy - 7.0, cx - 7.0, cy + 7.0, cx + 5.0, cy, c);
    rast.fill_rect_pt(cx + 6.0, cy - 6.0, 2.0, 12.0, c);
}

fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

fn fade(c: (u8, u8, u8), a: f32) -> (u8, u8, u8) {
    let t = a.clamp(0.0, 1.0);
    (
        (BG.0 as f32 + (c.0 as f32 - BG.0 as f32) * t) as u8,
        (BG.1 as f32 + (c.1 as f32 - BG.1 as f32) * t) as u8,
        (BG.2 as f32 + (c.2 as f32 - BG.2 as f32) * t) as u8,
    )
}

/// Lift a color toward white by `t` (0..1) — used for the progress "breath".
fn lighten(c: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    (
        (c.0 as f32 + (255.0 - c.0 as f32) * t) as u8,
        (c.1 as f32 + (255.0 - c.1 as f32) * t) as u8,
        (c.2 as f32 + (255.0 - c.2 as f32) * t) as u8,
    )
}

fn truncate_width(fonts: Option<&Fonts>, size: f32, text: &str, max_w: f32) -> String {
    if approx_text_width(fonts, size, text) <= max_w {
        return text.to_string();
    }
    let ell_w = approx_text_width(fonts, size, "…");
    let mut out = String::new();
    let mut used = 0.0;
    for ch in text.chars() {
        let cw = char_width(fonts, size, ch);
        if used + cw + ell_w > max_w {
            out.push('…');
            break;
        }
        out.push(ch);
        used += cw;
    }
    out
}

fn char_width(fonts: Option<&Fonts>, size: f32, ch: char) -> f32 {
    match fonts {
        Some(f) => f.face(ch).metrics(ch, size).advance_width,
        None => {
            if is_cjk_char(ch) {
                size
            } else {
                size * 0.52
            }
        }
    }
}

fn approx_text_width(fonts: Option<&Fonts>, size: f32, text: &str) -> f32 {
    text.chars().map(|ch| char_width(fonts, size, ch)).sum()
}

fn blit_glyph(
    buf: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    gw: usize,
    gh: usize,
    bitmap: &[u8],
    b: u8,
    g: u8,
    r: u8,
    clip_x0: u32,
    clip_x1: u32,
) {
    for row in 0..gh {
        for col in 0..gw {
            let cover = bitmap[row * gw + col];
            if cover < 12 {
                continue;
            }
            let px = x + col as i32;
            let py = y + row as i32;
            if px < 0
                || py < 0
                || px as u32 >= width
                || py as u32 >= height
                || (px as u32) < clip_x0
                || (px as u32) >= clip_x1
            {
                continue;
            }
            let i = ((py as u32 * width + px as u32) * 4) as usize;
            let a = cover as f32 / 255.0;
            buf[i] = lerp(buf[i], b, a);
            buf[i + 1] = lerp(buf[i + 1], g, a);
            buf[i + 2] = lerp(buf[i + 2], r, a);
            buf[i + 3] = 255;
        }
    }
}

fn lerp(dst: u8, src: u8, a: f32) -> u8 {
    (dst as f32 * (1.0 - a) + src as f32 * a).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::LyricLine;

    #[test]
    fn compact_surface_renders_lyrics_in_the_strip() {
        let fonts = Fonts::load().expect("a system font is required to render lyric text");
        let track = PlaybackState {
            title: "Test Track".into(),
            artist: "Test Artist".into(),
            duration_ms: 180_000,
            is_playing: true,
            ..PlaybackState::default()
        };
        let lyrics = LyricsState::Ready(SyncedLyrics {
            source: "test".into(),
            lines: vec![
                LyricLine {
                    time: 0.0,
                    text: "Current lyric".into(),
                },
                LyricLine {
                    time: 20.0,
                    text: "Next lyric".into(),
                },
            ],
        });

        let frame = render_frame(
            245,
            24,
            2.0,
            1,
            Some(&fonts),
            IdleKind::NotPlaying,
            Some(&track),
            &lyrics,
            10.0,
            0.0,
            false,
            SPOTIFY,
        );

        assert_eq!((frame.width, frame.height), (490, 48));
        frame.validate().expect("valid BGRA frame");

        // The compact strip places one lyric line to the right of the playing
        // bars. Inspect only the interior of that text region:
        // the one-point top/bottom hairlines must not satisfy this assertion.
        let row_bytes = frame.width as usize * 4;
        let text_region: Vec<&[u8]> = frame
            .pixels
            .chunks_exact(row_bytes)
            .skip(4)
            .take(frame.height as usize - 8)
            .flat_map(|row| row[80 * 4..].chunks_exact(4))
            .collect();
        let changed = text_region.into_iter().filter(|pixel| pixel[3] > 0).count();
        assert!(
            changed > 100,
            "lyric band should contain substantial visible glyph pixels, got {changed}"
        );
    }

    #[test]
    fn compact_status_is_not_blank_without_fonts() {
        let frame = render_frame(
            245,
            24,
            2.0,
            1,
            None,
            IdleKind::NotPlaying,
            None,
            &LyricsState::Idle,
            0.0,
            0.0,
            false,
            SPOTIFY,
        );
        let row_bytes = frame.width as usize * 4;
        let changed = frame
            .pixels
            .chunks_exact(row_bytes)
            .skip(4)
            .take(frame.height as usize - 8)
            .flat_map(|row| row.chunks_exact(4))
            .filter(|pixel| pixel[3] > 0)
            .count();
        assert!(
            changed >= 64,
            "status marker should remain visible, got {changed}"
        );
    }

    #[test]
    fn compact_surface_leaves_corners_transparent_for_host_chrome() {
        let frame = render_frame(
            245,
            24,
            2.0,
            1,
            None,
            IdleKind::NotPlaying,
            None,
            &LyricsState::Idle,
            0.0,
            0.0,
            false,
            SPOTIFY,
        );
        let corner = |x: u32, y: u32| {
            let index = ((y * frame.width + x) * 4 + 3) as usize;
            frame.pixels[index]
        };
        assert_eq!(corner(0, 0), 0);
        assert_eq!(corner(frame.width - 1, 0), 0);
        assert_eq!(corner(0, frame.height - 1), 0);
        assert_eq!(corner(frame.width - 1, frame.height - 1), 0);
    }

    #[test]
    fn marquee_pauses_scrolls_and_holds_at_the_end() {
        let overflow = 90.0;
        let line_duration = Some(8.0);

        assert_eq!(marquee_offset(overflow, 1.4, line_duration), 0.0);
        assert_eq!(marquee_offset(overflow, 1.5, line_duration), 0.0);
        assert_eq!(marquee_offset(overflow, 8.0, line_duration), -overflow);

        let halfway = marquee_offset(overflow, 4.5, line_duration);
        assert!(
            halfway < -40.0 && halfway > -50.0,
            "expected midpoint offset, got {halfway}"
        );
    }

    #[test]
    fn marquee_falls_back_to_constant_speed_without_a_next_line() {
        let overflow = 60.0;
        assert_eq!(marquee_offset(overflow, 1.5, None), 0.0);
        assert_eq!(marquee_offset(overflow, 2.5, None), -30.0);
        assert_eq!(marquee_offset(overflow, 4.0, None), -overflow);
    }
}
