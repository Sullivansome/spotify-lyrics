//! Offline authoring utility: real renderer, original sample copy, no Spotify/network access.
use crate::{
    lyrics::{LyricLine, LyricsState, SyncedLyrics},
    render::{Fonts, IdleKind, render_frame},
    spotify::PlaybackState,
};

pub fn export(output: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(output)?;
    let fonts = Fonts::load().ok_or("Preview export requires a usable system font")?;
    let track = PlaybackState {
        title: "A little closer".into(),
        artist: "Island preview".into(),
        duration_ms: 180_000,
        is_playing: true,
        ..Default::default()
    };
    let lyrics = LyricsState::Ready(SyncedLyrics {
        source: "Sample".into(),
        lines: [
            (0.0, "A little room for the things you love"),
            (10.0, "Your music, a little closer"),
            (20.0, "And space for everything else"),
        ]
        .into_iter()
        .map(|(time, text)| LyricLine {
            time,
            text: text.into(),
        })
        .collect(),
    });
    for (name, w, h) in [("compact", 245, 24), ("expanded", 380, 180)] {
        let frame = render_frame(
            w,
            h,
            2.0,
            0,
            Some(&fonts),
            IdleKind::NotPlaying,
            Some(&track),
            &lyrics,
            12.0,
            0.0,
            true,
            (163, 213, 181),
        );
        let mut pixels = frame.pixels;
        for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
            pixel.swap(0, 2);
        }
        image::save_buffer(
            output.join(format!("{name}.png")),
            &pixels,
            frame.width,
            frame.height,
            image::ColorType::Rgba8,
        )?;
    }
    Ok(())
}
