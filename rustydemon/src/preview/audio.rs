//! Audio file preview plugin (WAV / MP3 / OGG).
//!
//! Parses just enough of each format's header to show format, sample rate,
//! channels, bit depth, and approximate duration. No playback — egui has no
//! audio pipeline, and adding one would pull in a heavy dep (rodio, cpal).

use super::{PreviewOutput, PreviewPlugin};

pub struct AudioPreview;

impl PreviewPlugin for AudioPreview {
    fn name(&self) -> &str {
        "audio"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        let lower = filename.to_ascii_lowercase();
        if !(lower.ends_with(".wav") || lower.ends_with(".mp3") || lower.ends_with(".ogg")) {
            return false;
        }
        data.len() >= 4
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        let lower = filename.to_ascii_lowercase();

        let summary = if lower.ends_with(".wav") {
            parse_wav(data)
        } else if lower.ends_with(".mp3") {
            parse_mp3(data)
        } else if lower.ends_with(".ogg") {
            parse_ogg(data)
        } else {
            None
        };

        let bytes = data.len();
        out.text = Some(match summary {
            Some(s) => format!("{s}\nFile size: {}", format_bytes(bytes)),
            None => format!(
                "Audio file ({}), {} — header not recognised",
                lower.rsplit('.').next().unwrap_or("?"),
                format_bytes(bytes)
            ),
        });
        out
    }
}

fn format_bytes(b: usize) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", b as f64 / (1024.0 * 1024.0))
    }
}

// ── WAV ─────────────────────────────────────────────────────────────────────
//
// RIFF header: "RIFF" (4) + file_size (u32 LE) + "WAVE" (4)
// Chunks: 4-byte id + u32 LE size + payload.
// We look for the `fmt ` chunk for format info and `data` for sample byte count.

fn parse_wav(data: &[u8]) -> Option<String> {
    if data.len() < 44 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }

    let mut pos = 12usize;
    let mut fmt: Option<WavFmt> = None;
    let mut data_bytes: Option<u32> = None;

    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let sz = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        pos += 8;
        if pos + sz > data.len() {
            break;
        }
        match id {
            b"fmt " if sz >= 16 => {
                fmt = Some(WavFmt {
                    format_tag: u16::from_le_bytes(data[pos..pos + 2].try_into().ok()?),
                    channels: u16::from_le_bytes(data[pos + 2..pos + 4].try_into().ok()?),
                    sample_rate: u32::from_le_bytes(data[pos + 4..pos + 8].try_into().ok()?),
                    byte_rate: u32::from_le_bytes(data[pos + 8..pos + 12].try_into().ok()?),
                    bits: u16::from_le_bytes(data[pos + 14..pos + 16].try_into().ok()?),
                });
            }
            b"data" => {
                data_bytes = Some(sz as u32);
            }
            _ => {}
        }
        pos += sz + (sz & 1); // chunks are word-aligned
    }

    let f = fmt?;
    let codec = match f.format_tag {
        0x0001 => "PCM",
        0x0003 => "IEEE float",
        0x0006 => "A-law",
        0x0007 => "µ-law",
        0x0011 => "IMA ADPCM",
        0x0055 => "MP3",
        tag => return Some(format!("WAV (unknown codec 0x{tag:04X})")),
    };

    let duration = if f.byte_rate > 0 {
        data_bytes.map(|b| b as f64 / f.byte_rate as f64)
    } else {
        None
    };

    let mut out = format!(
        "WAV / {codec}\n{} Hz  {} ch  {}-bit",
        f.sample_rate, f.channels, f.bits
    );
    if let Some(d) = duration {
        out.push_str(&format!("\nDuration: {:.2}s", d));
    }
    Some(out)
}

struct WavFmt {
    format_tag: u16,
    channels: u16,
    sample_rate: u32,
    byte_rate: u32,
    bits: u16,
}

// ── MP3 ─────────────────────────────────────────────────────────────────────
//
// Scan past an optional ID3v2 tag, then look at the first MPEG frame header
// (4 bytes starting with 11 sync bits).

fn parse_mp3(data: &[u8]) -> Option<String> {
    let mut pos = 0usize;

    // Skip ID3v2 tag if present.
    if data.len() >= 10 && &data[..3] == b"ID3" {
        let sz = ((data[6] as u32 & 0x7F) << 21)
            | ((data[7] as u32 & 0x7F) << 14)
            | ((data[8] as u32 & 0x7F) << 7)
            | (data[9] as u32 & 0x7F);
        pos = 10 + sz as usize;
    }

    // Find sync.
    while pos + 4 <= data.len() {
        if data[pos] == 0xFF && (data[pos + 1] & 0xE0) == 0xE0 {
            break;
        }
        pos += 1;
    }
    if pos + 4 > data.len() {
        return None;
    }

    let b1 = data[pos + 1];
    let b2 = data[pos + 2];
    let b3 = data[pos + 3];

    let version_bits = (b1 >> 3) & 0x03;
    let layer_bits = (b1 >> 1) & 0x03;
    let bitrate_idx = (b2 >> 4) & 0x0F;
    let srate_idx = (b2 >> 2) & 0x03;
    let channel_mode = (b3 >> 6) & 0x03;

    let version = match version_bits {
        0 => "2.5",
        2 => "2",
        3 => "1",
        _ => return None,
    };
    let layer = match layer_bits {
        3 => "I",
        2 => "II",
        1 => "III",
        _ => return None,
    };

    // Layer III bitrate tables (kbps) for MPEG 1 / MPEG 2,2.5.
    static BR_V1_L3: [u32; 15] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ];
    static BR_V2_L3: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    static SR_V1: [u32; 3] = [44100, 48000, 32000];
    static SR_V2: [u32; 3] = [22050, 24000, 16000];
    static SR_V25: [u32; 3] = [11025, 12000, 8000];

    let bitrate = if version == "1" {
        BR_V1_L3.get(bitrate_idx as usize).copied()
    } else {
        BR_V2_L3.get(bitrate_idx as usize).copied()
    }?;

    let sample_rate = match version {
        "1" => SR_V1.get(srate_idx as usize).copied(),
        "2" => SR_V2.get(srate_idx as usize).copied(),
        "2.5" => SR_V25.get(srate_idx as usize).copied(),
        _ => None,
    }?;

    let channels = match channel_mode {
        3 => 1,
        _ => 2,
    };

    Some(format!(
        "MP3 (MPEG {version} Layer {layer})\n{sample_rate} Hz  {channels} ch  {bitrate} kbps"
    ))
}

// ── OGG Vorbis ──────────────────────────────────────────────────────────────
//
// First page is "OggS" + version + header_type + granule + serial + seqno +
// crc + segments + segment_table. The first packet is the identification
// header: "\x01vorbis" + version(u32) + channels(u8) + sample_rate(u32) + ...

fn parse_ogg(data: &[u8]) -> Option<String> {
    if data.len() < 58 || &data[..4] != b"OggS" {
        return None;
    }
    // Page header is 27 bytes + segment_table (n bytes, n = byte 26).
    let n_segments = data[26] as usize;
    let header_len = 27 + n_segments;
    if data.len() < header_len + 30 {
        return None;
    }
    let payload = &data[header_len..];
    if &payload[..7] != b"\x01vorbis" {
        return Some("OGG (non-Vorbis)".into());
    }
    let channels = payload[11];
    let sample_rate = u32::from_le_bytes(payload[12..16].try_into().ok()?);
    let bitrate_nom = u32::from_le_bytes(payload[20..24].try_into().ok()?);

    let mut out = format!("OGG Vorbis\n{sample_rate} Hz  {channels} ch");
    if bitrate_nom > 0 {
        out.push_str(&format!("  ~{} kbps", bitrate_nom / 1000));
    }
    Some(out)
}
