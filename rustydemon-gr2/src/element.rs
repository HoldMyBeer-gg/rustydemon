//! Granny type-tree / element parser.
//!
//! Granny files serialize a tree of named fields.  Each field has a
//! type descriptor in one sector (the "type sector") and its data in
//! another (usually the "data sector").  The type descriptor is a
//! 24-byte (32-bit variant) or 32-byte (64-bit variant) record
//! holding:
//!
//! - a 4-byte type_id (1..=22, where 0 terminates the list)
//! - a pointer-sized name field (patched by the fixup table to point
//!   at a NUL-terminated string in a string sector)
//! - a pointer-sized children field (patched to point at a sub-type
//!   descriptor if this field contains a struct or array)
//! - a 4-byte array_size (>0 means this field is a fixed-size array)
//! - trailing reserved bytes (16 for 32-bit, 20 for 64-bit)
//!
//! The element data lives immediately after the last type descriptor
//! in the iteration, packed with pointer-sized alignment.  Since we
//! don't actually need the fully resolved tree for our current
//! preview use case — just names, counts, and a handful of leaf
//! floats/ints — we keep the walker minimal but faithful.

use crate::error::{GrannyError, Result};
use crate::header::{read_f32, read_i32, read_u32, read_usize, Endian};
use crate::reference::Reference;
use crate::section::{PointerFixup, Section};

/// A named node in the parsed Granny tree.
#[derive(Debug, Clone)]
pub struct Element {
    pub name: String,
    pub value: ElementValue,
}

/// All value shapes we know how to parse.
///
/// Types we recognize but don't fully decode collapse into
/// [`ElementValue::Opaque`] so the walker keeps making progress past
/// them — the alternative (panicking on unknown types, as opengr2
/// does) is unusable on real D2R files.
#[derive(Debug, Clone)]
pub enum ElementValue {
    /// Type 2: inline struct — a recursive child tree.
    Reference(Vec<Element>),
    /// Type 3: dynamic-array-of-struct.
    ReferenceArray(Vec<Vec<Element>>),
    /// Type 4/7: array-of-references — each outer entry resolves
    /// through an additional pointer indirection.
    ArrayOfReferences(Vec<Vec<Element>>),
    /// Type 8: NUL-terminated UTF-8 string.
    String(String),
    /// Type 9: 4×4 transform-ish (flags + translation + rotation + 3x3 scale/shear).
    Transform(Transform),
    /// Type 10: single-precision float.
    F32(f32),
    /// Type 19: signed 32-bit integer.
    I32(i32),
    /// Type that this reader recognizes structurally but doesn't
    /// fully decode — e.g. VariantReference, trig vectors, packed
    /// integers.  The caller still sees the name and can treat the
    /// node as present for structure-summary purposes.
    Opaque(u32),
    /// A fixed-size array wrapping one of the other variants.
    Array(Vec<ElementValue>),
}

/// Granny `Transform` — same layout as in opengr2 / lslib.
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    pub flags: u32,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale_shear: [[f32; 3]; 3],
}

/// Parsed type descriptor for one field.
#[derive(Debug, Clone, Copy)]
struct TypeInfo {
    type_id: u32,
    name_ptr: Option<PointerFixup>,
    children_ptr: Option<PointerFixup>,
    array_size: i32,
}

/// Total on-disk size of a member descriptor, in bytes.
///
/// Layout (64-bit variant):
///
/// ```text
///   +0   i32     type_id
///   +4   u64     name_offset       (string reference, patched via fixup table)
///   +12  u64     definition_offset (struct reference, patched via fixup table)
///   +20  u32     array_size
///   +24  u32[3]  extra tags
///   +36  u64     unknown
/// ```
///
/// 32-bit variant has pointer-sized fields as u32 instead of u64, so
/// the stride is 12 bytes smaller.
fn type_info_stride(bits_64: bool) -> usize {
    if bits_64 {
        44
    } else {
        32
    }
}

fn parse_type_info(
    type_section: &Section,
    type_offset: usize,
    endian: Endian,
    bits_64: bool,
) -> Result<TypeInfo> {
    let stride = type_info_stride(bits_64);
    let data = &type_section.data;
    if type_offset + stride > data.len() {
        return Err(GrannyError::OutOfRange {
            start: type_offset,
            end: type_offset + stride,
            have: data.len(),
        });
    }
    let base = type_offset;
    let ptr_size = if bits_64 { 8 } else { 4 };

    let type_id = read_u32(&data[base..base + 4], endian);
    let name_off = base + 4;
    let children_off = name_off + ptr_size;
    let array_size_off = children_off + ptr_size;

    let array_size = read_i32(&data[array_size_off..array_size_off + 4], endian);

    // Pointers are resolved via the *type sector's* fixup table: the
    // entries we want have src_offset equal to the byte positions of
    // the name/children slots inside this sector's decompressed data.
    let name_ptr = type_section.resolve_pointer(name_off);
    let children_ptr = type_section.resolve_pointer(children_off);

    Ok(TypeInfo {
        type_id,
        name_ptr,
        children_ptr,
        array_size,
    })
}

fn read_string_at(sections: &[Section], sector: u32, offset: u32) -> Result<String> {
    let sec = sections
        .get(sector as usize)
        .ok_or(GrannyError::BadSectorRef {
            sector,
            count: sections.len(),
        })?;
    let start = offset as usize;
    if start >= sec.data.len() {
        return Err(GrannyError::OutOfRange {
            start,
            end: start,
            have: sec.data.len(),
        });
    }
    // NUL-terminated.  Tolerate files where the string runs to end
    // of sector without a terminator.
    let rest = &sec.data[start..];
    let len = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    std::str::from_utf8(&rest[..len])
        .map(|s| s.to_owned())
        .map_err(|_| GrannyError::BadUtf8)
}

/// Top-level entry point: walk the type tree starting at
/// `(type_section, type_offset)`, pulling data from
/// `(data_section, data_offset)`, and return the fully resolved child
/// list.
pub fn parse_elements(
    sections: &[Section],
    endian: Endian,
    bits_64: bool,
    data_sector_id: u32,
    type_sector_id: u32,
    data_offset: u32,
    type_offset: u32,
) -> Result<Vec<Element>> {
    let mut ctx = Ctx {
        sections,
        endian,
        bits_64,
        depth: 0,
    };
    let mut data_cursor = data_offset as usize;
    let mut out = Vec::new();
    ctx.walk_tree(
        data_sector_id,
        type_sector_id,
        &mut data_cursor,
        type_offset as usize,
        &mut out,
    )?;
    Ok(out)
}

struct Ctx<'a> {
    sections: &'a [Section],
    endian: Endian,
    bits_64: bool,
    depth: u32,
}

impl<'a> Ctx<'a> {
    fn walk_tree(
        &mut self,
        data_sector_id: u32,
        type_sector_id: u32,
        data_cursor: &mut usize,
        type_offset: usize,
        out: &mut Vec<Element>,
    ) -> Result<()> {
        // Bail out if the recursion gets absurdly deep — D2R files
        // observed top out around 8–10 levels, so 64 is generous
        // but still guards against pathological inputs.
        if self.depth > 64 {
            return Err(GrannyError::BitknitDecode("element tree too deep"));
        }
        self.depth += 1;

        let type_sec =
            self.sections
                .get(type_sector_id as usize)
                .ok_or(GrannyError::BadSectorRef {
                    sector: type_sector_id,
                    count: self.sections.len(),
                })?;
        let stride = type_info_stride(self.bits_64);
        let mut type_cursor = type_offset;
        loop {
            let info = parse_type_info(type_sec, type_cursor, self.endian, self.bits_64)?;
            if info.type_id == 0 || info.type_id > 22 {
                break;
            }

            let name = if let Some(ptr) = info.name_ptr {
                read_string_at(self.sections, ptr.dst_sector, ptr.dst_offset)?
            } else {
                String::new()
            };

            let value = if info.array_size > 0 {
                // Fixed-size array of N entries of this type.
                let mut entries = Vec::with_capacity(info.array_size as usize);
                for _ in 0..info.array_size {
                    entries.push(self.read_value(
                        data_sector_id,
                        type_sector_id,
                        data_cursor,
                        type_cursor,
                        &info,
                    )?);
                }
                ElementValue::Array(entries)
            } else {
                self.read_value(
                    data_sector_id,
                    type_sector_id,
                    data_cursor,
                    type_cursor,
                    &info,
                )?
            };

            out.push(Element { name, value });

            type_cursor += stride;
        }

        self.depth -= 1;
        Ok(())
    }

    fn read_value(
        &mut self,
        data_sector_id: u32,
        type_sector_id: u32,
        data_cursor: &mut usize,
        type_cursor: usize,
        info: &TypeInfo,
    ) -> Result<ElementValue> {
        let data_sec =
            self.sections
                .get(data_sector_id as usize)
                .ok_or(GrannyError::BadSectorRef {
                    sector: data_sector_id,
                    count: self.sections.len(),
                })?;
        let _ = type_sector_id; // kept for parallelism with opengr2
        let _ = type_cursor;
        let endian = self.endian;
        let bits_64 = self.bits_64;
        let ptr_size = if bits_64 { 8usize } else { 4 };

        let v = match info.type_id {
            1 => {
                // VariantReference: skip pointer-sized field.
                advance_data(data_cursor, ptr_size, data_sec)?;
                ElementValue::Opaque(1)
            }
            2 => {
                // Reference: pointer → sub-tree in data sector.
                let pos = *data_cursor;
                advance_data(data_cursor, ptr_size, data_sec)?;
                let ptr = data_sec.resolve_pointer(pos);
                let children = if let (Some(ptr), Some(child_type)) = (ptr, info.children_ptr) {
                    let mut cursor = ptr.dst_offset as usize;
                    let mut child = Vec::new();
                    self.walk_tree(
                        ptr.dst_sector,
                        child_type.dst_sector,
                        &mut cursor,
                        child_type.dst_offset as usize,
                        &mut child,
                    )?;
                    child
                } else {
                    Vec::new()
                };
                ElementValue::Reference(children)
            }
            3 => {
                // ArrayOfReference: u32 size + pointer to contiguous struct array.
                let size_pos = *data_cursor;
                if size_pos + 4 + ptr_size > data_sec.data.len() {
                    return Err(GrannyError::OutOfRange {
                        start: size_pos,
                        end: size_pos + 4 + ptr_size,
                        have: data_sec.data.len(),
                    });
                }
                let size = read_u32(&data_sec.data[size_pos..size_pos + 4], endian);
                *data_cursor += 4 + ptr_size;

                let mut entries = Vec::new();
                if size > 0 {
                    let data_ptr = data_sec.resolve_pointer(size_pos + 4);
                    if let (Some(data_ptr), Some(child_type)) = (data_ptr, info.children_ptr) {
                        let mut cursor = data_ptr.dst_offset as usize;
                        for _ in 0..size {
                            let mut child = Vec::new();
                            self.walk_tree(
                                data_ptr.dst_sector,
                                child_type.dst_sector,
                                &mut cursor,
                                child_type.dst_offset as usize,
                                &mut child,
                            )?;
                            entries.push(child);
                        }
                    }
                }
                ElementValue::ReferenceArray(entries)
            }
            4 => {
                // ArrayOfPointers: u32 size + pointer to array of pointers.
                let size_pos = *data_cursor;
                if size_pos + 4 + ptr_size > data_sec.data.len() {
                    return Err(GrannyError::OutOfRange {
                        start: size_pos,
                        end: size_pos + 4 + ptr_size,
                        have: data_sec.data.len(),
                    });
                }
                let size = read_u32(&data_sec.data[size_pos..size_pos + 4], endian);
                *data_cursor += 4 + ptr_size;

                let mut entries = Vec::new();
                let ptr = data_sec.resolve_pointer(size_pos + 4);
                if let (Some(ptr), Some(child_type)) = (ptr, info.children_ptr) {
                    let target_sec = self.sections.get(ptr.dst_sector as usize).ok_or(
                        GrannyError::BadSectorRef {
                            sector: ptr.dst_sector,
                            count: self.sections.len(),
                        },
                    )?;
                    for i in 0..size {
                        let src_off = ptr.dst_offset as usize + i as usize * ptr_size;
                        if let Some(elem_ptr) = target_sec.resolve_pointer(src_off) {
                            let mut cursor = elem_ptr.dst_offset as usize;
                            let mut child = Vec::new();
                            self.walk_tree(
                                elem_ptr.dst_sector,
                                child_type.dst_sector,
                                &mut cursor,
                                child_type.dst_offset as usize,
                                &mut child,
                            )?;
                            entries.push(child);
                        }
                    }
                }
                ElementValue::ArrayOfReferences(entries)
            }
            5 => {
                // VariantPointer: 2 × ptr_size.
                advance_data(data_cursor, 2 * ptr_size, data_sec)?;
                ElementValue::Opaque(5)
            }
            6 => {
                // Unknown leaf; skip pointer-sized.
                advance_data(data_cursor, ptr_size, data_sec)?;
                ElementValue::Opaque(6)
            }
            7 => {
                // Inline struct-of-struct: type_ptr + u32 size + data_ptr.
                let pos = *data_cursor;
                if pos + ptr_size + 4 + ptr_size > data_sec.data.len() {
                    return Err(GrannyError::OutOfRange {
                        start: pos,
                        end: pos + 2 * ptr_size + 4,
                        have: data_sec.data.len(),
                    });
                }
                let size = read_u32(&data_sec.data[pos + ptr_size..pos + ptr_size + 4], endian);
                *data_cursor += 2 * ptr_size + 4;

                let mut entries = Vec::new();
                let type_ptr = data_sec.resolve_pointer(pos);
                let data_ptr = data_sec.resolve_pointer(pos + ptr_size + 4);
                if let (Some(type_ptr), Some(data_ptr)) = (type_ptr, data_ptr) {
                    let mut cursor = data_ptr.dst_offset as usize;
                    for _ in 0..size {
                        let mut child = Vec::new();
                        self.walk_tree(
                            data_ptr.dst_sector,
                            type_ptr.dst_sector,
                            &mut cursor,
                            type_ptr.dst_offset as usize,
                            &mut child,
                        )?;
                        entries.push(child);
                    }
                }
                ElementValue::ArrayOfReferences(entries)
            }
            8 => {
                // String: pointer to NUL-terminated UTF-8.
                let pos = *data_cursor;
                advance_data(data_cursor, ptr_size, data_sec)?;
                let s = if let Some(ptr) = data_sec.resolve_pointer(pos) {
                    read_string_at(self.sections, ptr.dst_sector, ptr.dst_offset)?
                } else {
                    String::new()
                };
                ElementValue::String(s)
            }
            9 => {
                // Transform: flags u32 + 3 floats + 4 floats + 9 floats = 68 bytes.
                let pos = *data_cursor;
                if pos + 68 > data_sec.data.len() {
                    return Err(GrannyError::OutOfRange {
                        start: pos,
                        end: pos + 68,
                        have: data_sec.data.len(),
                    });
                }
                let flags = read_u32(&data_sec.data[pos..pos + 4], endian);
                let f = |off: usize| read_f32(&data_sec.data[pos + off..pos + off + 4], endian);
                let t = [f(4), f(8), f(12)];
                let r = [f(16), f(20), f(24), f(28)];
                let ss_raw = [
                    f(32),
                    f(36),
                    f(40),
                    f(44),
                    f(48),
                    f(52),
                    f(56),
                    f(60),
                    f(64),
                ];
                *data_cursor += 68;
                ElementValue::Transform(Transform {
                    flags,
                    translation: t,
                    rotation: r,
                    scale_shear: [
                        [ss_raw[0], ss_raw[1], ss_raw[2]],
                        [ss_raw[3], ss_raw[4], ss_raw[5]],
                        [ss_raw[6], ss_raw[7], ss_raw[8]],
                    ],
                })
            }
            10 => {
                // Real f32.
                let pos = *data_cursor;
                if pos + 4 > data_sec.data.len() {
                    return Err(GrannyError::OutOfRange {
                        start: pos,
                        end: pos + 4,
                        have: data_sec.data.len(),
                    });
                }
                let v = read_f32(&data_sec.data[pos..pos + 4], endian);
                *data_cursor += 4;
                ElementValue::F32(v)
            }
            // Types 11..=18 are various small ints / half floats / normals —
            // size varies.  Treat them as opaque + skip a sensible number of
            // bytes so the struct cursor stays aligned.  See Granny docs:
            //   11 Int8 / 12 UInt8 / 13 BinormalInt8 / 14 NormalUInt8
            //   15 Int16 / 16 UInt16 / 17 BinormalInt16 / 18 NormalUInt16
            11..=14 => {
                advance_data(data_cursor, 1, data_sec)?;
                ElementValue::Opaque(info.type_id)
            }
            15..=18 => {
                advance_data(data_cursor, 2, data_sec)?;
                ElementValue::Opaque(info.type_id)
            }
            19 => {
                // Int32.
                let pos = *data_cursor;
                if pos + 4 > data_sec.data.len() {
                    return Err(GrannyError::OutOfRange {
                        start: pos,
                        end: pos + 4,
                        have: data_sec.data.len(),
                    });
                }
                let v = read_i32(&data_sec.data[pos..pos + 4], endian);
                *data_cursor += 4;
                ElementValue::I32(v)
            }
            20 => {
                // UInt32 — store as I32 so callers have one numeric leaf type.
                let pos = *data_cursor;
                if pos + 4 > data_sec.data.len() {
                    return Err(GrannyError::OutOfRange {
                        start: pos,
                        end: pos + 4,
                        have: data_sec.data.len(),
                    });
                }
                let v = read_u32(&data_sec.data[pos..pos + 4], endian);
                *data_cursor += 4;
                ElementValue::I32(v as i32)
            }
            21 => {
                // HalfFloat — 2 bytes, not actually decoded.
                advance_data(data_cursor, 2, data_sec)?;
                ElementValue::Opaque(21)
            }
            22 => {
                // EmptyReference — pointer-sized.
                advance_data(data_cursor, ptr_size, data_sec)?;
                ElementValue::Opaque(22)
            }
            other => return Err(GrannyError::UnknownElementType(other)),
        };
        // Suppress unused warning when compiling without a specific arm.
        let _ = read_usize;
        let _ = Reference {
            sector: 0,
            position: 0,
        };
        Ok(v)
    }
}

fn advance_data(cursor: &mut usize, n: usize, sec: &Section) -> Result<()> {
    if *cursor + n > sec.data.len() {
        return Err(GrannyError::OutOfRange {
            start: *cursor,
            end: *cursor + n,
            have: sec.data.len(),
        });
    }
    *cursor += n;
    Ok(())
}
