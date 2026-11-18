//! Diablo III Anim SNOs (`Base/Anim/*.ani`): keyframed bone animation.
//!
//! Offsets in the file count from byte 16.
//!
//! ```text
//! 0x24  u32  SNO of the Appearance (model) the animation is made for
//! 0xA0  f32  half the number of key frames
//! 0xC0  u32  bone count, 0xC4 offset of their 64-byte names
//! 0xD8  translation tracks, 0xEC rotation tracks, 0xFC scale tracks:
//!       each (u32 count, u32 offset, u32 size), one 24-byte entry per
//!       bone holding (u32 key count, u32 offset, u32 size)
//! keys: translation  u32 frame, 3 f32
//!       rotation     u32 frame, 4 i16 (x y z w, divided by 32767)
//!       scale        u32 frame, f32
//! ```
//!
//! Across all 15,258 animations in the game, 14,865 store exactly half
//! their key frame count at 0xA0. That fits keys at 60 a second against a
//! 30 a second game clock, so the export plays keys at 60 a second; the
//! playback rate itself is not stored where this reader has found it.
//! Transforms are relative to the parent bone, as in the model, and bones
//! are matched to the model by name.

use std::sync::Arc;

use super::{ExportAction, PreviewOutput, SiblingFetcher};

const SNO_MAGIC: u32 = 0xDEAD_BEEF;
const DATA_BASE: usize = 16;

/// Key frames per second assumed for playback (see the module notes).
pub const KEYS_PER_SECOND: f32 = 60.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoneTrack {
    pub name: String,
    pub translation: Vec<(u32, [f32; 3])>,
    pub rotation: Vec<(u32, [f32; 4])>,
    pub scale: Vec<(u32, f32)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Animation {
    pub model_sno: u32,
    pub key_frames: u32,
    pub bones: Vec<BoneTrack>,
}

impl Animation {
    pub fn seconds(&self) -> f32 {
        self.key_frames as f32 / KEYS_PER_SECOND
    }
}

pub fn claims(filename: &str, data: &[u8]) -> bool {
    filename.to_ascii_lowercase().ends_with(".ani")
        && data.len() >= 0x110
        && data[..4] == SNO_MAGIC.to_le_bytes()
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
        .ok_or_else(|| format!("truncated at byte {at}"))
}

fn f32_at(d: &[u8], at: usize) -> Result<f32, String> {
    u32_at(d, at).map(f32::from_bits)
}

fn i16_at(d: &[u8], at: usize) -> Result<i16, String> {
    d.get(at..at + 2)
        .map(|b| i16::from_le_bytes(b.try_into().expect("2 bytes")))
        .ok_or_else(|| format!("truncated at byte {at}"))
}

/// Start and key count of bone `bone`'s track in the table whose header
/// sits at `hdr`, after checking `each`-byte keys fit in the file.
fn track(d: &[u8], hdr: usize, bone: usize, each: usize) -> Result<(usize, usize), String> {
    let table = DATA_BASE + u32_at(d, hdr + 4)? as usize;
    let e = table + bone * 24;
    let count = u32_at(d, e)? as usize;
    if count == 0 {
        return Ok((0, 0));
    }
    let start = DATA_BASE + u32_at(d, e + 4)? as usize;
    if start + count * each > d.len() {
        return Err(format!("track of bone {bone} runs past the end"));
    }
    Ok((start, count))
}

pub fn parse(d: &[u8]) -> Result<Animation, String> {
    let half = f32_at(d, 0xA0)?;
    let mut a = Animation {
        model_sno: u32_at(d, 0x24)?,
        key_frames: if half.is_finite() && half > 0.0 {
            (half * 2.0).round() as u32
        } else {
            0
        },
        bones: Vec::new(),
    };
    let count = u32_at(d, 0xC0)? as usize;
    let names = DATA_BASE + u32_at(d, 0xC4)? as usize;
    if names + count * 64 > d.len() {
        return Err("bone names run past the end".into());
    }
    for b in 0..count {
        let raw = &d[names + b * 64..names + (b + 1) * 64];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(64);
        let mut t = BoneTrack {
            name: String::from_utf8_lossy(&raw[..end]).into_owned(),
            ..Default::default()
        };
        let (s, n) = track(d, 0xD8, b, 16)?;
        for k in 0..n {
            let at = s + k * 16;
            t.translation.push((
                u32_at(d, at)?,
                [f32_at(d, at + 4)?, f32_at(d, at + 8)?, f32_at(d, at + 12)?],
            ));
        }
        let (s, n) = track(d, 0xEC, b, 12)?;
        for k in 0..n {
            let at = s + k * 12;
            let mut q = [0f32; 4];
            for (c, v) in q.iter_mut().enumerate() {
                *v = i16_at(d, at + 4 + c * 2)? as f32 / 32767.0;
            }
            let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            if len > 0.0 {
                q.iter_mut().for_each(|v| *v /= len);
            }
            t.rotation.push((u32_at(d, at)?, q));
        }
        let (s, n) = track(d, 0xFC, b, 8)?;
        for k in 0..n {
            let at = s + k * 8;
            t.scale.push((u32_at(d, at)?, f32_at(d, at + 4)?));
        }
        a.bones.push(t);
    }
    Ok(a)
}

pub struct D3AniPreview;

impl super::PreviewPlugin for D3AniPreview {
    fn name(&self) -> &str {
        "Diablo III animation"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        claims(filename, data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        fetch: &SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        let anim = match parse(data) {
            Ok(a) => a,
            Err(e) => {
                out.text = Some(format!("Diablo III animation\n\nCould not read it: {e}"));
                return out;
            }
        };
        let animated = anim
            .bones
            .iter()
            .filter(|b| !b.translation.is_empty() || !b.rotation.is_empty())
            .count();
        let mut text = format!(
            "Diablo III animation: {} key frames, {:.2} s at {} keys a second\n{} bones, {} with keys\nModel: SNO {}",
            anim.key_frames,
            anim.seconds(),
            KEYS_PER_SECOND,
            anim.bones.len(),
            animated,
            anim.model_sno
        );

        let model = (fetch.by_name)(&format!("sno/{}", anim.model_sno))
            .and_then(|d| super::d3_app::parse(&d).ok());
        match model {
            Some(app) => {
                let matched = anim
                    .bones
                    .iter()
                    .filter(|t| app.bones.iter().any(|b| b.name == t.name))
                    .count();
                text.push_str(&format!(
                    " ({} bones, {} matched by name)\n",
                    app.bones.len(),
                    matched
                ));
                let maps = super::d3_app::colour_maps_for(&app, fetch);
                let app = Arc::new(app);
                out.extra_exports.push(ExportAction {
                    label: "Export Animation (glTF)",
                    default_extension: "glb",
                    filter_name: "glTF binary",
                    build: Arc::new(move |data, _path| {
                        let anim = parse(data)?;
                        let mut pngs = Vec::new();
                        for (name, img) in maps.iter() {
                            if let Some(img) = img {
                                pngs.push((
                                    name.clone(),
                                    super::encode_png(&img.rgba, img.width, img.height)?,
                                ));
                            }
                        }
                        let parts = super::d3_app::drawn_parts(&app);
                        Ok(super::d3_app::to_glb(&app, &pngs, Some(&anim), &parts))
                    }),
                });
            }
            None => text.push_str(" (not found, so it cannot be exported)\n"),
        }
        out.text = Some(text);
        out
    }
}
