//! Messiah textures, stored as `Package/ab/<guid>.6`.
//!
//! ```text
//! [4]  02 02 02 01
//! u8   1, u8 format, u8 mip count, u8 flags
//! [4]  unknown
//! u16  width, u16 height
//! [16] unknown
//! u32  size of the rest
//! [4]  1, format, mip count, flags (again)
//! mip * count, smallest first:
//!   u32 entry size (counting itself), u16 width, u16 height,
//!   u16 unknown, u16 row pitch, u32 data size,
//!   then "NNNN" + data, or "ZZZ4" + u32 size + an LZ4 block
//! ```
//!
//! Every texture seen so far uses format 0x19, which is BC7.

use crate::MessiahError;

const MAGIC: [u8; 4] = [2, 2, 2, 1];

/// Format code for BC7 blocks.
pub const FORMAT_BC7: u8 = 0x19;

pub fn looks_like_texture(data: &[u8]) -> bool {
    data.len() >= 40 && data[..4] == MAGIC
}

#[derive(Clone, Debug)]
pub struct Mip {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Texture {
    pub format: u8,
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Mip>,
}

impl Texture {
    pub fn parse(data: &[u8]) -> Result<Self, MessiahError> {
        if !looks_like_texture(data) {
            return Err(MessiahError::Malformed("not a texture".into()));
        }
        let format = data[5];
        let width = u32::from(u16::from_le_bytes([data[12], data[13]]));
        let height = u32::from(u16::from_le_bytes([data[14], data[15]]));
        let mut mips = Vec::new();
        let mut p = 40;
        while p < data.len() {
            let head = data
                .get(p..p + 20)
                .ok_or_else(|| MessiahError::Malformed(format!("mip header truncated at {p}")))?;
            let size = u32::from_le_bytes(head[..4].try_into().expect("4")) as usize;
            let w = u32::from(u16::from_le_bytes([head[4], head[5]]));
            let h = u32::from(u16::from_le_bytes([head[6], head[7]]));
            let data_size = u32::from_le_bytes(head[12..16].try_into().expect("4")) as usize;
            if size < 20 || p + size > data.len() {
                return Err(MessiahError::Malformed(format!(
                    "mip at {p} has bad size {size}"
                )));
            }
            let body = &data[p + 16..p + size];
            let pixels = match &body[..4] {
                b"NNNN" => body[4..].to_vec(),
                b"ZZZ4" => rustydemon_mpk::decode_payload(body)
                    .map_err(|e| MessiahError::Malformed(e.to_string()))?,
                tag => {
                    return Err(MessiahError::Malformed(format!(
                        "mip at {p} has tag {tag:?}"
                    )))
                }
            };
            if pixels.len() != data_size {
                return Err(MessiahError::Malformed(format!(
                    "mip {w}x{h} holds {} bytes, header says {data_size}",
                    pixels.len()
                )));
            }
            mips.push(Mip {
                width: w,
                height: h,
                data: pixels,
            });
            p += size;
        }
        Ok(Texture {
            format,
            width,
            height,
            mips,
        })
    }

    pub fn largest_mip(&self) -> Option<&Mip> {
        self.mips.iter().max_by_key(|m| m.width * m.height)
    }
}
