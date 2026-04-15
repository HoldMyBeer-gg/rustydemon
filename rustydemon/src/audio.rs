//! Audio playback for the preview panel.
//!
//! [`AudioPlayer`] owns a single rodio [`OutputStream`] + [`Sink`] pair
//! (on non-Linux targets) and exposes the handful of operations the
//! preview panel needs: play a fresh in-memory buffer, toggle pause,
//! stop, query state.  Only one audio clip plays at a time — switching
//! files or clicking Stop wipes the existing sink and reuses the stream.
//!
//! Initialization is lazy and fallible: on machines without a working
//! audio device we silently downgrade to "unavailable" and the preview
//! panel renders a disabled control row instead of crashing the whole
//! app.
//!
//! ## Platform story
//!
//! rodio pulls in cpal, which needs ALSA development headers on Linux.
//! SteamOS doesn't ship them in a read-only-root-friendly way, so we
//! compile a stub on Linux that always reports "unavailable" and keep
//! the real implementation for Windows/macOS where cpal's backends
//! (WASAPI / CoreAudio) are built-in.

/// Returns `true` if `filename` looks like a format our AudioPlayer
/// can decode via rodio: WAV / MP3 / OGG / FLAC.  Used by the preview
/// panel to decide whether to render playback controls.  Kept
/// OS-independent so the UI layer doesn't need cfg-awareness.
pub fn is_audio_filename(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    lower.ends_with(".wav")
        || lower.ends_with(".mp3")
        || lower.ends_with(".ogg")
        || lower.ends_with(".flac")
}

// ── Real implementation: Windows + macOS ─────────────────────────────────────

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use std::io::Cursor;
    use std::time::Instant;

    use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

    pub struct AudioPlayer {
        _stream: OutputStream,
        stream_handle: OutputStreamHandle,
        sink: Option<Sink>,
        current_label: Option<String>,
        started_at: Option<Instant>,
        paused_at: Option<Instant>,
        paused_accumulated_ms: u128,
    }

    impl AudioPlayer {
        pub fn try_new() -> Option<Self> {
            let (stream, stream_handle) = OutputStream::try_default().ok()?;
            Some(AudioPlayer {
                _stream: stream,
                stream_handle,
                sink: None,
                current_label: None,
                started_at: None,
                paused_at: None,
                paused_accumulated_ms: 0,
            })
        }

        pub fn play(&mut self, bytes: Vec<u8>, label: String) -> Result<(), String> {
            self.stop();
            let cursor = Cursor::new(bytes);
            let decoder = Decoder::new(cursor).map_err(|e| format!("decode error: {e}"))?;
            let sink =
                Sink::try_new(&self.stream_handle).map_err(|e| format!("sink error: {e}"))?;
            sink.append(decoder);
            sink.play();
            self.sink = Some(sink);
            self.current_label = Some(label);
            self.started_at = Some(Instant::now());
            self.paused_at = None;
            self.paused_accumulated_ms = 0;
            Ok(())
        }

        pub fn stop(&mut self) {
            if let Some(sink) = self.sink.take() {
                sink.stop();
            }
            self.current_label = None;
            self.started_at = None;
            self.paused_at = None;
            self.paused_accumulated_ms = 0;
        }

        pub fn toggle_pause(&mut self) {
            let Some(sink) = self.sink.as_ref() else {
                return;
            };
            if sink.is_paused() {
                sink.play();
                if let Some(paused_at) = self.paused_at.take() {
                    self.paused_accumulated_ms += paused_at.elapsed().as_millis();
                }
            } else {
                sink.pause();
                self.paused_at = Some(Instant::now());
            }
        }

        pub fn is_playing(&self) -> bool {
            self.sink
                .as_ref()
                .map(|s| !s.is_paused() && !s.empty())
                .unwrap_or(false)
        }

        pub fn is_paused(&self) -> bool {
            self.sink.as_ref().map(|s| s.is_paused()).unwrap_or(false)
        }

        #[allow(dead_code)]
        pub fn is_finished(&self) -> bool {
            self.sink.as_ref().map(|s| s.empty()).unwrap_or(false)
        }

        pub fn current_label(&self) -> Option<&str> {
            self.current_label.as_deref()
        }

        pub fn elapsed_secs(&self) -> f64 {
            let Some(started) = self.started_at else {
                return 0.0;
            };
            let mut ms = started
                .elapsed()
                .as_millis()
                .saturating_sub(self.paused_accumulated_ms);
            if let Some(paused_at) = self.paused_at {
                ms = ms.saturating_sub(paused_at.elapsed().as_millis());
            }
            ms as f64 / 1000.0
        }
    }
}

// ── Linux stub ───────────────────────────────────────────────────────────────
//
// Same public API as the real impl but every method is a no-op.
// `try_new` returns `None` so the app-level lazy initialiser marks
// playback as permanently unavailable, and the UI renders the same
// control row with every button disabled.

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    pub struct AudioPlayer {
        _never: (),
    }

    impl AudioPlayer {
        pub fn try_new() -> Option<Self> {
            None
        }

        pub fn play(&mut self, _bytes: Vec<u8>, _label: String) -> Result<(), String> {
            Err("audio playback is not compiled in on this target".into())
        }

        pub fn stop(&mut self) {}
        pub fn toggle_pause(&mut self) {}
        pub fn is_playing(&self) -> bool {
            false
        }
        pub fn is_paused(&self) -> bool {
            false
        }
        #[allow(dead_code)]
        pub fn is_finished(&self) -> bool {
            false
        }
        pub fn current_label(&self) -> Option<&str> {
            None
        }
        pub fn elapsed_secs(&self) -> f64 {
            0.0
        }
    }
}

pub use imp::AudioPlayer;
