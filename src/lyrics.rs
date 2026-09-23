//! Synced lyrics — same sources as Lyrisland:
//! LRCLIB first for Western catalog, NetEase for CJK / misses.
//! https://lrclib.net · https://music.163.com

use std::time::Duration;

use serde::Deserialize;

use crate::spotify::PlaybackState;

#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    pub time: f64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncedLyrics {
    pub lines: Vec<LyricLine>,
    pub source: String,
}

/// Fetch / display state so the UI never stays on “Fetching…” after a miss.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum LyricsState {
    #[default]
    Idle,
    Loading,
    Ready(SyncedLyrics),
    Missing,
}

impl SyncedLyrics {
    /// Last line whose time <= position (binary search).
    pub fn line_index_at(&self, position: f64) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        let mut lo = 0usize;
        let mut hi = self.lines.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.lines[mid].time <= position {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo > 0 { Some(lo - 1) } else { None }
    }

    pub fn current_and_next(&self, position: f64) -> (Option<&str>, Option<&str>) {
        match self.line_index_at(position) {
            Some(i) => {
                let cur = self.lines.get(i).map(|l| l.text.as_str());
                let next = self.lines.get(i + 1).map(|l| l.text.as_str());
                (cur, next)
            }
            None => (None, self.lines.first().map(|l| l.text.as_str())),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibTrack {
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

/// Fetch lyrics: CJK → NetEase first (LRCLIB is empty for 爱河 / ZaZaZsu),
/// otherwise LRCLIB then NetEase.
pub fn fetch_lyrics(track: &PlaybackState) -> Option<SyncedLyrics> {
    if looks_cjk(&track.title) || looks_cjk(&track.artist) {
        if let Some(l) = fetch_netease(track) {
            return Some(l);
        }
        return fetch_lrclib(track);
    }
    if let Some(l) = fetch_lrclib(track) {
        return Some(l);
    }
    fetch_netease(track)
}

pub fn looks_cjk(s: &str) -> bool {
    s.chars().any(is_cjk_char)
}

pub fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{3000}'..='\u{303F}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{FF00}'..='\u{FFEF}'
            | '\u{20000}'..='\u{2A6DF}'
    )
}

fn fetch_lrclib(track: &PlaybackState) -> Option<SyncedLyrics> {
    if let Some(l) = lrclib_exact(track) {
        return Some(l);
    }
    // Structured search, then free-text variants. Score every hit so we
    // don't grab a same-title / wrong-artist result (愛河 ≠ 芒果酱).
    let queries = [
        format!(
            "https://lrclib.net/api/search?track_name={}&artist_name={}",
            urlencoding::encode(&track.title),
            urlencoding::encode(&track.artist)
        ),
        format!(
            "https://lrclib.net/api/search?q={}",
            urlencoding::encode(&format!("{} {}", track.title, track.artist))
        ),
        format!(
            "https://lrclib.net/api/search?q={}",
            urlencoding::encode(&track.title)
        ),
    ];
    let mut best: Option<(i32, SyncedLyrics)> = None;
    for url in queries {
        let Some(body) = http_get(&url, USER_AGENT) else {
            continue;
        };
        let Ok(results) = serde_json::from_str::<Vec<LrcLibTrack>>(&body) else {
            continue;
        };
        for item in results {
            let score = match_score(
                track,
                item.track_name.as_deref().unwrap_or(""),
                item.artist_name.as_deref().unwrap_or(""),
                item.album_name.as_deref(),
                item.duration.map(|d| (d * 1000.0) as u64),
            );
            if score < 80 {
                continue;
            }
            if let Some(ly) = lyrics_from_lrclib(&item)
                && best.as_ref().map(|(s, _)| *s).unwrap_or(0) < score
            {
                best = Some((score, ly));
            }
        }
        if best.as_ref().map(|(s, _)| *s).unwrap_or(0) >= 140 {
            break;
        }
    }
    best.map(|(_, l)| l)
}

fn lrclib_exact(track: &PlaybackState) -> Option<SyncedLyrics> {
    let duration_s = (track.duration_ms / 1000).max(1);
    let url = format!(
        "https://lrclib.net/api/get?track_name={}&artist_name={}&album_name={}&duration={}",
        urlencoding::encode(&track.title),
        urlencoding::encode(&track.artist),
        urlencoding::encode(&track.album),
        duration_s
    );
    let body = http_get(&url, USER_AGENT)?;
    let obj: LrcLibTrack = serde_json::from_str(&body).ok()?;
    lyrics_from_lrclib(&obj)
}

fn lyrics_from_lrclib(obj: &LrcLibTrack) -> Option<SyncedLyrics> {
    if let Some(raw) = obj.synced_lyrics.as_deref() {
        let lines = parse_lrc(raw);
        if !lines.is_empty() {
            return Some(SyncedLyrics {
                lines,
                source: "lrclib".into(),
            });
        }
    }
    if let Some(raw) = obj.plain_lyrics.as_deref() {
        return unsynced_from_plain(raw, "lrclib");
    }
    None
}

fn unsynced_from_plain(raw: &str, source: &str) -> Option<SyncedLyrics> {
    let lines: Vec<LyricLine> = raw
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, text)| LyricLine {
            time: i as f64 * 4.0,
            text: text.to_string(),
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(SyncedLyrics {
        lines,
        source: format!("{source}·plain"),
    })
}

// --- NetEase Cloud Music ---------------------------------------------------

#[derive(Debug, Deserialize)]
struct NeteaseSearch {
    result: Option<NeteaseResult>,
}

#[derive(Debug, Deserialize)]
struct NeteaseResult {
    songs: Option<Vec<NeteaseSong>>,
}

#[derive(Debug, Deserialize)]
struct NeteaseSong {
    id: u64,
    name: String,
    artists: Option<Vec<NeteaseArtist>>,
    album: Option<NeteaseAlbum>,
    duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct NeteaseArtist {
    name: String,
}

#[derive(Debug, Deserialize)]
struct NeteaseAlbum {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NeteaseLyric {
    lrc: Option<NeteaseLrcField>,
}

#[derive(Debug, Deserialize)]
struct NeteaseLrcField {
    lyric: Option<String>,
}

fn fetch_netease(track: &PlaybackState) -> Option<SyncedLyrics> {
    let q = format!("{} {}", track.title, track.artist);
    let url = format!(
        "https://music.163.com/api/search/get?s={}&type=1&limit=10",
        urlencoding::encode(&q)
    );
    let body = http_get(&url, NETEASE_UA)?;
    let parsed: NeteaseSearch = serde_json::from_str(&body).ok()?;
    let songs = parsed.result?.songs?;

    let mut best: Option<(i32, u64)> = None;
    for song in songs {
        let artists = song
            .artists
            .as_ref()
            .map(|a| {
                a.iter()
                    .map(|x| x.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let album = song.album.as_ref().and_then(|a| a.name.as_deref());
        let score = match_score(track, &song.name, &artists, album, song.duration);
        if score < 80 {
            continue;
        }
        if best.as_ref().map(|(s, _)| *s).unwrap_or(0) < score {
            best = Some((score, song.id));
        }
    }
    let song_id = best?.1;
    let lyric_url = format!("https://music.163.com/api/song/lyric?id={song_id}&lv=-1&tv=-1");
    let body = http_get(&lyric_url, NETEASE_UA)?;
    let obj: NeteaseLyric = serde_json::from_str(&body).ok()?;
    let raw = obj.lrc?.lyric?;
    let lines = parse_lrc(&raw);
    if lines.is_empty() {
        return None;
    }
    Some(SyncedLyrics {
        lines,
        source: "netease".into(),
    })
}

/// Cheap title/artist/duration score. 80 ≈ usable, 140+ ≈ confident.
fn match_score(
    track: &PlaybackState,
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration_ms: Option<u64>,
) -> i32 {
    let mut s = 0;
    let nt = norm(title);
    let tt = norm(&track.title);
    if nt.is_empty() || tt.is_empty() {
        return 0;
    }
    if nt == tt {
        s += 100;
    } else if nt.contains(&tt) || tt.contains(&nt) {
        s += 60;
    } else {
        return 0;
    }

    let na = norm(artist);
    let ta = norm(&track.artist);
    if !ta.is_empty() && !na.is_empty() {
        if na == ta || na.contains(&ta) || ta.contains(&na) {
            s += 80;
        } else {
            s += 10;
        }
    }

    if let (Some(album), true) = (album, !track.album.is_empty()) {
        let nal = norm(album);
        let tal = norm(&track.album);
        if nal == tal || nal.contains(&tal) || tal.contains(&nal) {
            s += 15;
        }
    }

    if let Some(d) = duration_ms
        && track.duration_ms > 0
    {
        let diff = d.abs_diff(track.duration_ms);
        if diff < 2_000 {
            s += 40;
        } else if diff < 8_000 {
            s += 15;
        }
    }
    s
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || is_cjk_char(*c))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

// --- HTTP ------------------------------------------------------------------

const USER_AGENT: &str =
    "Island-Spotify-Plugin/0.1 (+https://github.com/Sullivansome/spotify-lyrics)";
const NETEASE_UA: &str = "Mozilla/5.0";

fn http_get(url: &str, ua: &str) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .user_agent(ua)
        .build();
    let resp = agent.get(url).call().ok()?;
    if !(200..300).contains(&resp.status()) {
        return None;
    }
    resp.into_string().ok()
}

/// Parse standard LRC `[mm:ss.xx]` lines (Lyrisland LRCParser-compatible).
pub fn parse_lrc(raw: &str) -> Vec<LyricLine> {
    let parse = parse_lrc_line;
    let mut lines = Vec::new();
    for line in raw.lines() {
        if let Some((time, text)) = parse(line) {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            lines.push(LyricLine {
                time,
                text: text.to_string(),
            });
        }
    }
    lines.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    lines
}

fn parse_lrc_line(line: &str) -> Option<(f64, &str)> {
    let line = line.trim();
    if !line.starts_with('[') {
        return None;
    }
    let close = line.find(']')?;
    let stamp = &line[1..close];
    let text = &line[close + 1..];
    let mut parts = stamp.split(':');
    let minutes: f64 = parts.next()?.parse().ok()?;
    let rest = parts.next()?;
    let mut sec_parts = rest.split('.');
    let seconds: f64 = sec_parts.next()?.parse().ok()?;
    let frac = sec_parts.next().unwrap_or("0");
    let ms = match frac.len() {
        1 => frac.parse::<f64>().ok()? / 10.0,
        2 => frac.parse::<f64>().ok()? / 100.0,
        _ => frac.parse::<f64>().ok()? / 1000.0,
    };
    if !minutes.is_finite()
        || !seconds.is_finite()
        || !ms.is_finite()
        || minutes < 0.0
        || !(0.0..60.0).contains(&seconds)
        || !(0.0..1.0).contains(&ms)
        || parts.next().is_some()
        || sec_parts.next().is_some()
    {
        return None;
    }
    let time = minutes * 60.0 + seconds + ms;
    if !time.is_finite() {
        return None;
    }
    Some((time, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spotify::PlaybackState;

    fn track(title: &str, artist: &str, duration_ms: u64) -> PlaybackState {
        PlaybackState {
            track_id: "t".into(),
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            duration_ms,
            position_secs: 0.0,
            is_playing: true,
            artwork_url: None,
        }
    }

    #[test]
    fn parse_standard_lrc() {
        let lines = parse_lrc("[00:00.65]如果\n[00:02.30]晚霞绕过火车 等着\n[ti:skip]\n");
        assert_eq!(lines.len(), 2);
        assert!((lines[0].time - 0.65).abs() < 0.001);
        assert_eq!(lines[1].text, "晚霞绕过火车 等着");
    }

    #[test]
    fn lyrics_equality_compares_content_and_lookup_handles_edges() {
        let a = SyncedLyrics {
            lines: parse_lrc("[00:01.00]first\n[00:02.00]second"),
            source: "test".into(),
        };
        let b = SyncedLyrics {
            lines: parse_lrc("[00:01.00]other\n[00:02.00]second"),
            source: "test".into(),
        };
        assert_ne!(a, b);
        assert_eq!(a.line_index_at(0.0), None);
        assert_eq!(a.line_index_at(1.0), Some(0));
        assert_eq!(a.line_index_at(100.0), Some(1));
        assert_eq!(a.current_and_next(1.5), (Some("first"), Some("second")));
    }

    #[test]
    fn malformed_timestamps_cannot_poison_binary_search() {
        let lines = parse_lrc(
            "[NaN:01]bad\n[-1:01]bad\n[00:99]bad\n[00:01:02]bad\n[00:01.1.2]bad\n[00:01.00]valid",
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "valid");
    }

    #[test]
    fn score_prefers_same_artist() {
        let t = track("爱河", "ZaZaZsu", 197_000);
        let hit = match_score(&t, "爱河", "ZaZaZsu咂咂苏", None, Some(197_128));
        let miss = match_score(&t, "爱河", "芒果酱", None, Some(258_000));
        assert!(hit >= 140, "hit={hit}");
        assert!(miss < 80 || hit > miss + 50, "miss={miss} hit={hit}");
    }

    #[test]
    #[ignore = "requires the live NetEase lyrics API"]
    fn fetch_ai_he_from_netease() {
        let ly = fetch_lyrics(&track("爱河", "ZaZaZsu", 197_000)).expect("netease lyrics");
        assert_eq!(ly.source, "netease");
        assert!(
            ly.lines.iter().any(|l| l.text.contains("爱河")),
            "expected 爱河 in {:?}",
            ly.lines.iter().map(|l| &l.text).take(8).collect::<Vec<_>>()
        );
    }
}
