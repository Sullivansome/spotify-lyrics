//! Album-art accent color — the same trick Spotify's own client uses:
//! decode the artwork, cluster the pixels, and pick the **vibrant** swatch
//! (saturated, mid-luminance, well represented) instead of the flat average.
//! Conceptually Android-Palette-style extraction, kept dependency-light.

use std::io::Read;
use std::time::Duration;

use crate::spotify::PlaybackState;

/// Brand green — fallback when there is no artwork or extraction fails.
pub const SPOTIFY_GREEN: (u8, u8, u8) = (30, 215, 96);

const ARTWORK_UA: &str =
    "Island-Spotify-Plugin/0.1 (+https://github.com/Sullivansome/spotify-lyrics)";
const SAMPLE: u32 = 32;
const CLUSTERS: usize = 4;
const KMEANS_ITERS: usize = 8;

/// Download the track's artwork and extract a vibrant accent color.
pub fn fetch_accent(track: &PlaybackState) -> Option<(u8, u8, u8)> {
    let url = track.artwork_url.as_deref()?;
    let bytes = http_get_bytes(url)?;
    let img = image::load_from_memory(&bytes).ok()?;
    let small = img
        .resize_to_fill(SAMPLE, SAMPLE, image::imageops::FilterType::Triangle)
        .to_rgb8();
    vibrant_color(small.as_raw())
}

fn http_get_bytes(url: &str) -> Option<Vec<u8>> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .user_agent(ARTWORK_UA)
        .build();
    let resp = agent.get(url).call().ok()?;
    if !(200..300).contains(&resp.status()) {
        return None;
    }
    let mut buf = Vec::new();
    resp.into_reader()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

/// k-means over the RGB pixels, then score each cluster like a "vibrant"
/// swatch: saturation matters most, near-black / near-white clusters lose,
/// bigger clusters win ties. Returns None for empty input.
fn vibrant_color(rgb: &[u8]) -> Option<(u8, u8, u8)> {
    let px: Vec<[f32; 3]> = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .collect();
    if px.len() < CLUSTERS {
        return None;
    }

    // Seed centers with evenly spaced pixels (deterministic, no RNG).
    let mut centers: Vec<[f32; 3]> = (0..CLUSTERS).map(|i| px[i * px.len() / CLUSTERS]).collect();
    let mut assign = vec![0usize; px.len()];

    for _ in 0..KMEANS_ITERS {
        for (i, p) in px.iter().enumerate() {
            let mut best = 0;
            let mut best_d = f32::MAX;
            for (k, c) in centers.iter().enumerate() {
                let d = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2);
                if d < best_d {
                    best_d = d;
                    best = k;
                }
            }
            assign[i] = best;
        }
        for (k, center) in centers.iter_mut().enumerate() {
            let mut sum = [0.0f32; 3];
            let mut n = 0.0f32;
            for (i, p) in px.iter().enumerate() {
                if assign[i] == k {
                    sum[0] += p[0];
                    sum[1] += p[1];
                    sum[2] += p[2];
                    n += 1.0;
                }
            }
            if n > 0.0 {
                *center = [sum[0] / n, sum[1] / n, sum[2] / n];
            }
        }
    }

    let mut best = None;
    let mut best_score = 0.0f32;
    for (k, &[r, g, b]) in centers.iter().enumerate() {
        let count = assign.iter().filter(|&&a| a == k).count() as f32;
        if count == 0.0 {
            continue;
        }
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
        let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        // Saturated clusters win; mud-dark and blown-out clusters are
        // penalized so white/black album art falls back gracefully.
        let lum_penalty = 1.0 - ((lum - 150.0).abs() / 255.0);
        let score = sat * sat * lum_penalty * count.sqrt();
        if score > best_score {
            best_score = score;
            best = Some((r.round() as u8, g.round() as u8, b.round() as u8));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vibrant_prefers_saturated_cluster_over_gray_majority() {
        // 80% gray background, 20% strong red — the red cluster should win.
        let mut rgb = Vec::new();
        for _ in 0..800 {
            rgb.extend_from_slice(&[120, 118, 122]);
        }
        for _ in 0..200 {
            rgb.extend_from_slice(&[220, 40, 50]);
        }
        let (r, g, b) = vibrant_color(&rgb).expect("a color");
        assert!(
            r > 150 && g < 110 && b < 110,
            "expected reddish, got ({r},{g},{b})"
        );
    }

    #[test]
    fn vibrant_returns_none_on_empty_input() {
        assert_eq!(vibrant_color(&[]), None);
    }
}
