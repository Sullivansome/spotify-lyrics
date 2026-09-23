//! Island Spotify plugin — now playing + synced lyrics + transport.
//!
//! UX and integration patterns from **Lyrisland**
//! (`refs/Lyrisland`, https://github.com/EurFelux/Lyrisland):
//! - AppleScript now-playing (no OAuth)
//! - LRCLIB + NetEase synced lyrics
//! - Compact single-line lyrics + expanded context + transport

mod accent;
mod lyrics;
mod preview;
mod render;
mod spotify;
mod worker;

use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};

use island_plugin_api::{
    COMPACT_LYRIC_HEIGHT, COMPACT_LYRIC_WIDTH, HostToPlugin, InputEvent, LogLevel,
};
use island_plugin_sdk::PluginClient;

use lyrics::{LyricsState, fetch_lyrics};
use render::{ControlHit, Fonts, IdleKind, hit_test, render_frame};
use spotify::{
    POLL_INTERVAL, PlaybackState, PollResult, PositionClock, next_track, open_automation_settings,
    play_pause, poll_playback, previous_track,
};
use worker::{LatestWorker, cache_insert};

/// Neutral bars while a new track's artwork color is still loading — never
/// the previous track's color, never a brand-green flash.
const ACCENT_NEUTRAL: (u8, u8, u8) = (150, 152, 158);

/// Speed of the accent crossfade (exponential approach, per second).
const ACCENT_FADE_RATE: f32 = 9.0;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--export-market-previews") {
        let output = std::env::args()
            .nth(2)
            .expect("provide an output directory");
        preview::export(std::path::Path::new(&output)).expect("export Market previews");
        return;
    }
    let plugin_id =
        std::env::var("ISLAND_PLUGIN_ID").unwrap_or_else(|_| "com.sullivansome.spotify".into());

    let mut client = connect_with_retry(&plugin_id);
    let _ = client.log(LogLevel::Info, "spotify plugin online (Lyrisland-style)");

    let fonts = Fonts::load();
    if let Some(fonts) = fonts.as_ref() {
        let _ = client.log(LogLevel::Info, fonts.description());
    } else {
        let _ = client.log(
            LogLevel::Warn,
            "no usable system TTF found — status marker remains visible but text cannot render",
        );
    }

    // Match SurfaceReady so even the first frame uses the compact lyric-strip
    // layout. SurfaceResize replaces these with the measured notch slot.
    let mut w = COMPACT_LYRIC_WIDTH;
    let mut h = COMPACT_LYRIC_HEIGHT;
    let mut scale = 2.0f32;
    let mut frame_id = 0u64;
    let mut clock = PositionClock::default();
    let mut last_poll = Instant::now() - POLL_INTERVAL;
    let mut last_track = String::new();
    let mut lyrics_cache: HashMap<String, LyricsState> = HashMap::new();
    let mut lyrics_state = LyricsState::Idle;
    let mut last_state: Option<PlaybackState> = None;
    let mut idle = IdleKind::NotPlaying;
    let mut logged_denied = false;
    let mut logged_error: Option<String> = None;
    let mut presented = false;
    let start = Instant::now();
    let reduce_motion = reduce_motion_enabled();

    let lyrics_worker =
        LatestWorker::new(|track: PlaybackState| (track.track_id.clone(), fetch_lyrics(&track)));
    let accent_worker = LatestWorker::new(|track: PlaybackState| {
        (track.track_id.clone(), accent::fetch_accent(&track))
    });
    let playback_worker = LatestWorker::new(|()| poll_playback());
    let control_worker = LatestWorker::new(|hit| match hit {
        ControlHit::Previous => previous_track(),
        ControlHit::PlayPause => play_pause(),
        ControlHit::Next => next_track(),
    });
    let mut accent_cache: HashMap<String, (u8, u8, u8)> = HashMap::new();
    // Accent crossfade: `accent_shown` eases toward `accent_target` every
    // frame, so color changes are always a smooth transition, never a pop.
    // A track with unknown color starts at neutral — never the previous
    // track's color, never a brand-green flash.
    let mut accent_target = accent::SPOTIFY_GREEN;
    let mut accent_shown = to_vec(accent::SPOTIFY_GREEN);
    let mut last_frame = Instant::now();

    loop {
        for msg in client.drain() {
            match msg {
                HostToPlugin::Shutdown => {
                    let _ = client.log(LogLevel::Info, "shutdown");
                    return;
                }
                HostToPlugin::SurfaceReady {
                    w: nw,
                    h: nh,
                    scale: sc,
                    ..
                }
                | HostToPlugin::SurfaceResize {
                    w: nw,
                    h: nh,
                    scale: sc,
                    ..
                } => {
                    w = nw.clamp(80, 640);
                    h = nh.clamp(20, 320);
                    scale = sc.clamp(1.0, 3.0);
                    let _ = client.log(LogLevel::Info, format!("surface {w}×{h} @{scale}×"));
                }
                HostToPlugin::Input {
                    event: InputEvent::PointerDown { x, y, button: 0 },
                } => {
                    if let Some(hit) = hit_test(w, h, x, y) {
                        match hit {
                            ControlHit::Previous => {
                                control_worker.submit(ControlHit::Previous);
                                let _ = client.log(LogLevel::Debug, "control: previous");
                            }
                            ControlHit::PlayPause => {
                                if idle == IdleKind::Denied {
                                    open_automation_settings();
                                    let _ =
                                        client.log(LogLevel::Info, "opened Automation settings");
                                } else {
                                    control_worker.submit(ControlHit::PlayPause);
                                    let _ = client.log(LogLevel::Debug, "control: play/pause");
                                }
                            }
                            ControlHit::Next => {
                                control_worker.submit(ControlHit::Next);
                                let _ = client.log(LogLevel::Debug, "control: next");
                            }
                        }
                        last_poll = Instant::now() - POLL_INTERVAL;
                    }
                }
                HostToPlugin::Input {
                    event: InputEvent::KeyDown { key },
                } => match key.as_str() {
                    "space" | " " => control_worker.submit(ControlHit::PlayPause),
                    "left" => control_worker.submit(ControlHit::Previous),
                    "right" => control_worker.submit(ControlHit::Next),
                    _ => {}
                },
                _ => {}
            }
        }

        while let Some((track_id, fetched)) = lyrics_worker.take() {
            if track_id != last_track {
                continue;
            }
            let state = match fetched {
                Some(ly) => {
                    let src = ly.source.clone();
                    let n = ly.lines.len();
                    let _ = client.log(LogLevel::Info, format!("lyrics: {src} ({n} lines)"));
                    LyricsState::Ready(ly)
                }
                None => {
                    let _ = client.log(LogLevel::Info, "lyrics: none");
                    LyricsState::Missing
                }
            };
            cache_insert(&mut lyrics_cache, track_id, state.clone());
            lyrics_state = state;
        }

        while let Some((track_id, color)) = accent_worker.take() {
            let Some(color) = color else {
                continue;
            };
            cache_insert(&mut accent_cache, track_id.clone(), color);
            if track_id == last_track {
                // Crossfade from neutral (or the current color) to the real
                // artwork accent over the next frames.
                accent_target = color;
            }
        }

        if last_poll.elapsed() >= POLL_INTERVAL {
            last_poll = Instant::now();
            playback_worker.submit(());
        }
        if let Some(playback) = playback_worker.take() {
            match playback {
                PollResult::Playing(state) => {
                    idle = IdleKind::NotPlaying;
                    logged_denied = false;
                    logged_error = None;
                    clock.calibrate(state.position_secs, state.is_playing);

                    if state.track_id != last_track {
                        last_track = state.track_id.clone();
                        let title = state.title.clone();
                        let artist = state.artist.clone();
                        let _ = client.set_collapsed_meta(
                            Some("♪".into()),
                            Some(truncate(&title, 12)),
                            Some("music".into()),
                        );
                        let _ = client.log(LogLevel::Info, format!("track: {title} — {artist}"));

                        if let Some(cached) = lyrics_cache.get(&state.track_id) {
                            lyrics_state = cached.clone();
                        } else {
                            lyrics_state = LyricsState::Loading;
                            lyrics_worker.submit(state.clone());
                        }

                        // Album-art accent: known color → crossfade to it.
                        // Unknown → snap to neutral and crossfade when the
                        // background extraction lands.
                        if let Some(cached) = accent_cache.get(&state.track_id) {
                            accent_target = *cached;
                        } else if state.artwork_url.is_some() {
                            accent_shown = to_vec(ACCENT_NEUTRAL);
                            accent_target = ACCENT_NEUTRAL;
                            accent_worker.submit(state.clone());
                        } else {
                            accent_target = accent::SPOTIFY_GREEN;
                        }
                    }

                    if state.is_playing {
                        let was_presented = presented;
                        present_if_needed(&mut client, &mut presented);
                        if !was_presented && presented {
                            let _ = client.set_collapsed_meta(
                                Some("♪".into()),
                                Some(truncate(&state.title, 12)),
                                Some("music".into()),
                            );
                            let _ = client.log(LogLevel::Info, "playback active: showing lyrics");
                        }
                    } else if presented {
                        let _ = client.dismiss();
                        presented = false;
                        let _ = client.log(LogLevel::Info, "playback paused: hiding lyrics");
                    }

                    last_state = Some(state);
                }
                PollResult::PermissionDenied => {
                    present_if_needed(&mut client, &mut presented);
                    idle = IdleKind::Denied;
                    last_track.clear();
                    last_state = None;
                    lyrics_state = LyricsState::Idle;
                    let _ = client.set_collapsed_meta(
                        Some("♪".into()),
                        Some("perm".into()),
                        Some("music".into()),
                    );
                    if !logged_denied {
                        logged_denied = true;
                        let _ = client.log(
                            LogLevel::Warn,
                            "Automation denied (-1743). System Settings → Privacy → Automation → allow osascript / island → Spotify",
                        );
                    }
                }
                PollResult::NotRunning => {
                    if presented {
                        let _ = client.dismiss();
                        presented = false;
                    }
                    idle = IdleKind::NotPlaying;
                    last_track.clear();
                    logged_error = None;
                    last_state = None;
                    lyrics_state = LyricsState::Idle;
                    let _ = client.set_collapsed_meta(
                        Some("♪".into()),
                        Some("—".into()),
                        Some("music".into()),
                    );
                }
                PollResult::Error(error) => {
                    present_if_needed(&mut client, &mut presented);
                    idle = IdleKind::Error;
                    last_track.clear();
                    last_state = None;
                    lyrics_state = LyricsState::Idle;
                    let _ = client.set_collapsed_meta(
                        Some("♪".into()),
                        Some("error".into()),
                        Some("music".into()),
                    );
                    if logged_error.as_deref() != Some(error.as_str()) {
                        let _ = client.log(LogLevel::Warn, format!("Spotify poll failed: {error}"));
                        logged_error = Some(error);
                    }
                }
            }
        }

        // Accent crossfade: ease the rendered color toward the target so a
        // new track's artwork color fades in instead of popping.
        let dt = last_frame.elapsed().as_secs_f32();
        last_frame = Instant::now();
        accent_shown = approach(accent_shown, accent_target, dt);
        let accent = from_vec(accent_shown);

        let pos = clock.now();
        let pulse = start.elapsed().as_secs_f32();
        let frame = render_frame(
            w,
            h,
            scale,
            frame_id,
            fonts.as_ref(),
            idle,
            last_state.as_ref(),
            &lyrics_state,
            pos,
            pulse,
            reduce_motion,
            accent,
        );
        let _ = client.submit_frame(frame);
        frame_id = frame_id.wrapping_add(1);

        // 30 fps while anything on screen can move (marquee, playing bars,
        // progress breath, accent fade); drop to a slow cadence when
        // paused/idle so the plugin does not burn CPU repainting a static
        // strip.
        let animating = last_state.as_ref().is_some_and(|state| state.is_playing)
            || accent_shown != to_vec(accent_target);
        let frame_budget = if animating { 33 } else { 120 };
        thread::sleep(Duration::from_millis(frame_budget));
    }
}

fn present_if_needed(client: &mut PluginClient, presented: &mut bool) {
    if !*presented
        && client
            .present(55, None, true, Some("spotify".into()))
            .is_ok()
    {
        *presented = true;
        // One compact lyric line beneath the hardware notch. The host resizes
        // its width to match the measured notch and ears.
        let _ = client.preferred_size(COMPACT_LYRIC_WIDTH, COMPACT_LYRIC_HEIGHT);
    }
}

fn connect_with_retry(plugin_id: &str) -> PluginClient {
    match PluginClient::connect(plugin_id.to_string()) {
        Ok(c) => return c,
        Err(e) => eprintln!("spotify-plugin: connect failed: {e}"),
    }
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(100));
        if let Ok(c) = PluginClient::connect(plugin_id.to_string()) {
            return c;
        }
    }
    eprintln!("spotify-plugin: giving up");
    std::process::exit(1);
}

fn to_vec(c: (u8, u8, u8)) -> (f32, f32, f32) {
    (c.0 as f32, c.1 as f32, c.2 as f32)
}

fn from_vec(v: (f32, f32, f32)) -> (u8, u8, u8) {
    (
        v.0.round().clamp(0.0, 255.0) as u8,
        v.1.round().clamp(0.0, 255.0) as u8,
        v.2.round().clamp(0.0, 255.0) as u8,
    )
}

/// Exponential approach toward the target color; snaps when close enough
/// that the difference is invisible.
fn approach(current: (f32, f32, f32), target: (u8, u8, u8), dt: f32) -> (f32, f32, f32) {
    let t = 1.0 - (-ACCENT_FADE_RATE * dt).exp();
    let next = (
        current.0 + (target.0 as f32 - current.0) * t,
        current.1 + (target.1 as f32 - current.1) * t,
        current.2 + (target.2 as f32 - current.2) * t,
    );
    let close = (next.0 - target.0 as f32).abs() < 0.5
        && (next.1 - target.1 as f32).abs() < 0.5
        && (next.2 - target.2 as f32).abs() < 0.5;
    if close { to_vec(target) } else { next }
}

fn truncate(s: &str, n: usize) -> String {
    let c = s.chars().count();
    if c <= n {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(n.saturating_sub(1)).collect();
        o.push('…');
        o
    }
}

fn reduce_motion_enabled() -> bool {
    let requested_by_env = std::env::var("ISLAND_REDUCE_MOTION")
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    if requested_by_env {
        return true;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("defaults")
            .args(["read", "com.apple.universalAccess", "reduceMotion"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                matches!(
                    String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .to_ascii_lowercase()
                        .as_str(),
                    "1" | "true" | "yes"
                )
            })
    }

    #[cfg(not(target_os = "macos"))]
    false
}
