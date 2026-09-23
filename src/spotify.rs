//! Local Spotify Desktop state via AppleScript.
//!
//! Approach mirrors [Lyrisland](https://github.com/EurFelux/Lyrisland)
//! `SpotifyAppleScriptService` — no OAuth; requires Spotify Desktop + Automation permission.

#[cfg(target_os = "macos")]
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct PlaybackState {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub position_secs: f64,
    pub is_playing: bool,
    pub artwork_url: Option<String>,
}

#[cfg(target_os = "macos")]
const POLL_SCRIPT: &str = r#"
if application "Spotify" is running then
    tell application "Spotify"
        set trackId to id of current track
        set trackName to name of current track
        set trackArtist to artist of current track
        set trackAlbum to album of current track
        set trackDuration to duration of current track
        set playerPos to player position
        set playerState to player state as string
        set artURL to artwork url of current track
        return trackId & "||" & trackName & "||" & trackArtist & "||" & trackAlbum & "||" & (trackDuration as string) & "||" & (playerPos as string) & "||" & playerState & "||" & artURL
    end tell
else
    return "NOT_RUNNING"
end if
"#;

/// Result of one AppleScript poll.
#[derive(Debug, Clone)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum PollResult {
    Playing(PlaybackState),
    NotRunning,
    /// macOS Automation denied (`osascript` error -1743).
    PermissionDenied,
    Error(String),
}

/// Poll Spotify Desktop via `osascript`.
pub fn poll_playback() -> PollResult {
    #[cfg(not(target_os = "macos"))]
    {
        PollResult::Error("Spotify plugin is macOS-only".into())
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("osascript");
        command.args(["-e", POLL_SCRIPT]);
        let output = match bounded_output(&mut command) {
            Ok(output) => output,
            Err(error) => return PollResult::Error(error.to_string()),
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}{stderr}");

        if !output.status.success()
            || combined.contains("-1743")
            || combined.contains("Not authorized to send Apple events")
        {
            return PollResult::PermissionDenied;
        }

        let raw = stdout.trim();
        if raw.is_empty() || raw == "NOT_RUNNING" {
            return PollResult::NotRunning;
        }
        match parse_state(raw) {
            Some(s) => PollResult::Playing(s),
            None => PollResult::Error(format!("unparsed: {raw}")),
        }
    }
}

/// Open System Settings → Privacy → Automation (best-effort).
pub fn open_automation_settings() {
    #[cfg(target_os = "macos")]
    {
        let urls = [
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Automation",
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
        ];
        for url in urls {
            if Command::new("open").arg(url).status().is_ok() {
                return;
            }
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn parse_state(raw: &str) -> Option<PlaybackState> {
    let parts: Vec<&str> = raw.split("||").collect();
    if parts.len() < 7 {
        return None;
    }
    let duration = parts[4].parse::<f64>().ok()?;
    let position_secs = parts[5].parse::<f64>().ok()?;
    if !duration.is_finite() || duration < 0.0 || !position_secs.is_finite() || position_secs < 0.0
    {
        return None;
    }
    let duration_ms = duration as u64;
    let is_playing = parts[6].eq_ignore_ascii_case("playing");
    let artwork_url = parts.get(7).and_then(|s| {
        let t = s.trim();
        if t.is_empty() || t == "missing value" {
            None
        } else {
            Some(t.to_string())
        }
    });

    Some(PlaybackState {
        track_id: parts[0].to_string(),
        title: parts[1].to_string(),
        artist: parts[2].to_string(),
        album: parts[3].to_string(),
        duration_ms,
        position_secs,
        is_playing,
        artwork_url,
    })
}

pub fn play_pause() {
    run_cmd("playpause");
}

pub fn next_track() {
    run_cmd("next track");
}

pub fn previous_track() {
    run_cmd("previous track");
}

fn run_cmd(command: &str) {
    #[cfg(target_os = "macos")]
    {
        let source = format!(
            r#"if application "Spotify" is running then
                tell application "Spotify" to {command}
            end if"#
        );
        let _ = bounded_output(Command::new("osascript").arg("-e").arg(source));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = command;
    }
}

/// Interpolate position between polls (same idea as Lyrisland PlaybackSyncEngine).
#[derive(Debug, Clone)]
pub struct PositionClock {
    anchor_pos: f64,
    anchor_at: std::time::Instant,
    playing: bool,
}

impl Default for PositionClock {
    fn default() -> Self {
        Self {
            anchor_pos: 0.0,
            anchor_at: std::time::Instant::now(),
            playing: false,
        }
    }
}

impl PositionClock {
    pub fn calibrate(&mut self, position_secs: f64, is_playing: bool) {
        // Poll jitter (osascript latency) shows up as tiny position snaps,
        // which make scrolling lyrics lurch every poll. Keep the extrapolated
        // clock when the reported drift is small; only re-anchor on real
        // seeks, track changes, or play/pause transitions.
        let drift = (position_secs - self.now()).abs();
        if self.playing == is_playing && drift < 0.5 {
            return;
        }
        self.anchor_pos = position_secs;
        self.anchor_at = std::time::Instant::now();
        self.playing = is_playing;
    }

    pub fn now(&self) -> f64 {
        if self.playing {
            self.anchor_pos + self.anchor_at.elapsed().as_secs_f64()
        } else {
            self.anchor_pos
        }
    }
}

pub const POLL_INTERVAL: Duration = Duration::from_millis(400);

#[cfg(target_os = "macos")]
fn bounded_output(command: &mut Command) -> std::io::Result<std::process::Output> {
    use std::process::Stdio;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output();
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Spotify Automation timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playing_and_paused_without_artwork() {
        let track =
            parse_state("id||Title||Artist||Album||100000||2.5||playing||missing value").unwrap();
        assert_eq!(track.position_secs, 2.5);
        assert!(track.is_playing);
        assert!(track.artwork_url.is_none());
        assert!(
            !parse_state("id||Title||Artist||Album||100000||0||paused")
                .unwrap()
                .is_playing
        );
    }

    #[test]
    fn rejects_missing_or_nonfinite_playback_fields() {
        assert!(parse_state("bad response").is_none());
        for value in ["NaN", "inf", "-1"] {
            assert!(parse_state(&format!("id||T||A||B||1000||{value}||playing")).is_none());
            assert!(parse_state(&format!("id||T||A||B||{value}||0||playing")).is_none());
        }
    }
}
