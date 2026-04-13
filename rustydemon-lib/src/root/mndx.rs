//! MNDX root handler for StarCraft, StarCraft II, and Heroes of the Storm.
//!
//! The MNDX format uses a compressed trie (MAR databases) to store file paths
//! and maps them to content keys via a root entry table.  Three MAR databases
//! are present:
//!   - MAR[0]: package names (e.g. `mods/core.sc2mod/enus.sc2data`)
//!   - MAR[1]: file names stripped of the package prefix
//!   - MAR[2]: complete file paths
//!
//! Reference: CASCExplorer / CascLib `MNDXRootHandler.cs`.

use std::{
    collections::HashMap,
    io::{Cursor, Read, Seek, SeekFrom},
};

use crate::{
    error::CascError,
    jenkins96::jenkins96,
    types::{ContentFlags, LocaleFlags, Md5Hash, RootEntry},
};

use super::RootHandler;

// ── Constants ────────────────────────────────────────────────────────────────

const MNDX_SIGNATURE: u32 = 0x5844_4E4D; // 'MNDX'
const MAR_SIGNATURE: u32 = 0x0052_414D; // 'MAR\0'

/// Find the position of the `rank`-th set bit in an 8-bit value.
/// Returns 7 if fewer than `rank + 1` bits are set (matching CascLib behavior).
/// This replaces the 2048-byte `table_1BA1818` from the reference implementation.
#[inline]
fn select_bit(byte: u32, rank: i32) -> i32 {
    let mut r = rank;
    for i in 0..8i32 {
        if (byte >> i) & 1 != 0 {
            if r == 0 {
                return i;
            }
            r -= 1;
        }
    }
    7
}

// ── Reading helpers ──────────────────────────────────────────────────────────

fn read_i32(r: &mut Cursor<&[u8]>) -> Result<i32, CascError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_u32(r: &mut Cursor<&[u8]>) -> Result<u32, CascError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_i64(r: &mut Cursor<&[u8]>) -> Result<i64, CascError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}

fn read_md5(r: &mut Cursor<&[u8]>) -> Result<Md5Hash, CascError> {
    let mut buf = [0u8; 16];
    r.read_exact(&mut buf)?;
    Ok(Md5Hash::from_bytes(buf))
}

/// Read a CascLib-style array: 4-byte LE byte count, then that many bytes
/// interpreted as `i32` values.
fn read_array_i32(r: &mut Cursor<&[u8]>) -> Result<Vec<i32>, CascError> {
    let byte_count = read_i32(r)? as usize;
    if byte_count == 0 {
        return Ok(Vec::new());
    }
    let count = byte_count / 4;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_i32(r)?);
    }
    Ok(out)
}

/// Read a CascLib-style byte array.
fn read_array_u8(r: &mut Cursor<&[u8]>) -> Result<Vec<u8>, CascError> {
    let byte_count = read_i32(r)? as usize;
    if byte_count == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; byte_count];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

// ── Popcount helper ──────────────────────────────────────────────────────────

/// Parallel popcount returning partial sums in each byte:
///   bits[7:0]   = popcount of input bits [7:0]
///   bits[15:8]  = popcount of input bits [15:0]
///   bits[23:16] = popcount of input bits [23:0]
///   bits[31:24] = popcount of input bits [31:0]
fn popcount_partial(mut v: u32) -> u32 {
    v = ((v >> 1) & 0x5555_5555) + (v & 0x5555_5555);
    v = ((v >> 2) & 0x3333_3333) + (v & 0x3333_3333);
    v = ((v >> 4) & 0x0F0F_0F0F) + (v & 0x0F0F_0F0F);
    v.wrapping_mul(0x0101_0101)
}

#[inline]
fn popcount32(v: u32) -> u32 {
    popcount_partial(v) >> 24
}

// ── Triplet ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Triplet {
    base_value: i32,
    value2: i32,
    value3: i32,
}

fn read_array_triplet(r: &mut Cursor<&[u8]>) -> Result<Vec<Triplet>, CascError> {
    let byte_count = read_i32(r)? as usize;
    if byte_count == 0 {
        return Ok(Vec::new());
    }
    let count = byte_count / 12;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(Triplet {
            base_value: read_i32(r)?,
            value2: read_i32(r)?,
            value3: read_i32(r)?,
        });
    }
    Ok(out)
}

// ── TSparseArray ─────────────────────────────────────────────────────────────

/// Compressed sparse bit array with rank/select queries.
struct TSparseArray {
    item_bits: Vec<u32>,
    total_item_count: i32,
    valid_item_count: i32,
    base_values: Vec<Triplet>,
    array_dwords_38: Vec<i32>,
    array_dwords_50: Vec<i32>,
}

impl TSparseArray {
    fn read(r: &mut Cursor<&[u8]>) -> Result<Self, CascError> {
        let item_bits_i32 = read_array_i32(r)?;
        let item_bits: Vec<u32> = item_bits_i32.into_iter().map(|v| v as u32).collect();
        let total_item_count = read_i32(r)?;
        let valid_item_count = read_i32(r)?;
        let base_values = read_array_triplet(r)?;
        let array_dwords_38 = read_array_i32(r)?;
        let array_dwords_50 = read_array_i32(r)?;
        Ok(TSparseArray {
            item_bits,
            total_item_count,
            valid_item_count,
            base_values,
            array_dwords_38,
            array_dwords_50,
        })
    }

    /// Returns true if the bit at `index` is set.
    fn contains(&self, index: i32) -> bool {
        let idx = index as usize;
        (self.item_bits[idx >> 5] & (1u32 << (idx & 0x1F))) != 0
    }

    /// Get the rank (number of set bits before this position) used as a value index.
    fn get_item_value(&self, index: i32) -> i32 {
        let tri = self.base_values[(index >> 9) as usize];
        let mut base = tri.base_value;

        match ((index >> 6) & 7) - 1 {
            0 => base += tri.value2 & 0x7F,
            1 => base += (tri.value2 >> 7) & 0xFF,
            2 => base += (tri.value2 >> 15) & 0xFF,
            3 => base += (tri.value2 >> 23) & 0x1FF,
            4 => base += tri.value3 & 0x1FF,
            5 => base += (tri.value3 >> 9) & 0x1FF,
            6 => base += (tri.value3 >> 18) & 0x1FF,
            _ => {}
        }

        let dword_index = (index >> 5) as usize;
        if (index & 0x20) != 0 {
            base += popcount32(self.item_bits[dword_index - 1]) as i32;
        }

        let bit_mask = (1u32 << (index as u32 & 0x1F)).wrapping_sub(1);
        base + popcount32(self.item_bits[dword_index] & bit_mask) as i32
    }

    /// Select query on inverted bits (find position of the N-th zero bit).
    /// Corresponds to `sub_1959CB0` in CascLib.
    fn select_zeros(&self, index: i32) -> i32 {
        let mut edx = index;
        let dw_key_shifted = (index >> 9) as usize;

        if (edx & 0x1FF) == 0 {
            return self.array_dwords_38[dw_key_shifted];
        }

        let mut eax = self.array_dwords_38[dw_key_shifted] >> 9;
        let mut bound = (self.array_dwords_38[dw_key_shifted + 1] + 0x1FF) >> 9;

        if (eax + 0x0A) >= bound {
            let mut i = (eax + 1) as usize;
            let mut tri = self.base_values[i];
            i += 1;
            let mut edi = eax << 9;
            let mut ebx = edi - tri.base_value + 0x200;
            while edx >= ebx {
                edi += 0x200;
                tri = self.base_values[i];
                ebx = edi - tri.base_value + 0x200;
                eax += 1;
                i += 1;
            }
        } else {
            while (eax + 1) < bound {
                let mid = (bound + eax) >> 1;
                let ebx = (mid << 9) - self.base_values[mid as usize].base_value;
                if edx < ebx {
                    bound = mid;
                } else {
                    eax = mid;
                }
            }
        }

        let tri = self.base_values[eax as usize];
        edx += tri.base_value - (eax << 9);
        let mut edi = eax << 4;

        let v2 = tri.value2;
        let ecx_init = v2 >> 23;
        let ebx = 0x100 - ecx_init;
        if edx < ebx {
            let ecx2 = (v2 >> 7) & 0xFF;
            let esi = 0x80 - ecx2;
            if edx < esi {
                let a = v2 & 0x7F;
                let c = 0x40 - a;
                if edx >= c {
                    edi += 2;
                    edx = edx + a - 0x40;
                }
            } else {
                let a = (v2 >> 15) & 0xFF;
                let s = 0xC0 - a;
                if edx < s {
                    edi += 4;
                    edx = edx + ecx2 - 0x80;
                } else {
                    edi += 6;
                    edx = edx + a - 0xC0;
                }
            }
        } else {
            let v3 = tri.value3;
            let a = (v3 >> 9) & 0x1FF;
            let ebx2 = 0x180 - a;
            if edx < ebx2 {
                let s = v3 & 0x1FF;
                let a2 = 0x140 - s;
                if edx < a2 {
                    edi += 8;
                    edx = edx + ecx_init - 0x100;
                } else {
                    edi += 0x0A;
                    edx = edx + s - 0x140;
                }
            } else {
                let s = (v3 >> 18) & 0x1FF;
                let c = 0x1C0 - s;
                if edx < c {
                    edi += 0x0C;
                    edx = edx + a - 0x180;
                } else {
                    edi += 0x0E;
                    edx = edx + s - 0x1C0;
                }
            }
        }

        // Final lookup: inverted bits
        let mut ecx = !self.item_bits[edi as usize];
        let mut pc = popcount_partial(ecx);
        let mut esi = (pc >> 24) as i32;

        if edx >= esi {
            edi += 1;
            ecx = !self.item_bits[edi as usize];
            edx -= esi;
            pc = popcount_partial(ecx);
        }

        esi = ((pc >> 8) & 0xFF) as i32;
        edi <<= 5;
        if edx < esi {
            let a = (pc & 0xFF) as i32;
            if edx >= a {
                ecx >>= 8;
                edi += 8;
                edx -= a;
            }
        } else {
            let a = ((pc >> 16) & 0xFF) as i32;
            if edx < a {
                ecx >>= 16;
                edi += 0x10;
                edx -= esi;
            } else {
                ecx >>= 24;
                edi += 0x18;
                edx -= a;
            }
        }

        ecx &= 0xFF;
        select_bit(ecx, edx) + edi
    }

    /// Select query on non-inverted bits (find position of the N-th set bit).
    /// Corresponds to `sub_1959F50` in CascLib.
    fn select_ones(&self, index: i32) -> i32 {
        let mut edx = index;
        let dw_key_shifted = (index >> 9) as usize;

        if (edx & 0x1FF) == 0 {
            return self.array_dwords_50[dw_key_shifted];
        }

        let item0 = self.array_dwords_50[dw_key_shifted];
        let item1 = self.array_dwords_50[dw_key_shifted + 1];
        let mut eax = item0 >> 9;
        let mut bound = (item1 + 0x1FF) >> 9;

        if (eax + 0x0A) > bound {
            let mut i = (eax + 1) as usize;
            let mut tri = self.base_values[i];
            i += 1;
            while edx >= tri.base_value {
                tri = self.base_values[i];
                eax += 1;
                i += 1;
            }
        } else {
            while (eax + 1) < bound {
                let mid = (bound + eax) >> 1;
                if edx < self.base_values[mid as usize].base_value {
                    bound = mid;
                } else {
                    eax = mid;
                }
            }
        }

        let tri = self.base_values[eax as usize];
        edx -= tri.base_value;
        let mut edi = eax << 4;
        let v2 = tri.value2;
        let ebx = v2 >> 23;
        if edx < ebx {
            let esi = (v2 >> 7) & 0xFF;
            if edx < esi {
                let a = v2 & 0x7F;
                if edx >= a {
                    edi += 2;
                    edx -= a;
                }
            } else {
                let a = (v2 >> 15) & 0xFF;
                if edx < a {
                    edi += 4;
                    edx -= esi;
                } else {
                    edi += 6;
                    edx -= a;
                }
            }
        } else {
            let v3 = tri.value3;
            let a = (v3 >> 9) & 0x1FF;
            if edx < a {
                let s = v3 & 0x1FF;
                if edx < s {
                    edi += 8;
                    edx -= ebx;
                } else {
                    edi += 0x0A;
                    edx -= s;
                }
            } else {
                let s = (v3 >> 18) & 0x1FF;
                if edx < s {
                    edi += 0x0C;
                    edx -= a;
                } else {
                    edi += 0x0E;
                    edx -= s;
                }
            }
        }

        // Final lookup: non-inverted bits
        let mut esi_bits = self.item_bits[edi as usize];
        let mut pc = popcount_partial(esi_bits);
        let mut ecx = (pc >> 24) as i32;

        if edx >= ecx {
            edi += 1;
            esi_bits = self.item_bits[edi as usize];
            edx -= ecx;
            pc = popcount_partial(esi_bits);
        }

        ecx = ((pc >> 8) & 0xFF) as i32;
        edi <<= 5;
        if edx < ecx {
            let a = (pc & 0xFF) as i32;
            if edx >= a {
                edi += 8;
                esi_bits >>= 8;
                edx -= a;
            }
        } else {
            let a = ((pc >> 16) & 0xFF) as i32;
            if edx < a {
                esi_bits >>= 16;
                edi += 0x10;
                edx -= ecx;
            } else {
                esi_bits >>= 24;
                edi += 0x18;
                edx -= a;
            }
        }

        esi_bits &= 0xFF;
        select_bit(esi_bits, edx) + edi
    }
}

// ── TBitEntryArray ───────────────────────────────────────────────────────────

/// Variable-bit-width packed integer array.
struct TBitEntryArray {
    data: Vec<u32>,
    bits_per_entry: i32,
    entry_bit_mask: u32,
    #[allow(dead_code)]
    total_entries: i64,
}

impl TBitEntryArray {
    fn read(r: &mut Cursor<&[u8]>) -> Result<Self, CascError> {
        let data_i32 = read_array_i32(r)?;
        let data: Vec<u32> = data_i32.into_iter().map(|v| v as u32).collect();
        let bits_per_entry = read_i32(r)?;
        let entry_bit_mask = read_i32(r)? as u32;
        let total_entries = read_i64(r)?;
        Ok(TBitEntryArray {
            data,
            bits_per_entry,
            entry_bit_mask,
            total_entries,
        })
    }

    fn get(&self, index: i32) -> i32 {
        let bit_offset = index as i64 * self.bits_per_entry as i64;
        let dword_index = (bit_offset >> 5) as usize;
        let start_bit = (bit_offset & 0x1F) as u32;
        let end_bit = start_bit + self.bits_per_entry as u32;

        let result = if end_bit > 0x20 {
            (self.data[dword_index + 1] << (0x20 - start_bit))
                | (self.data[dword_index] >> start_bit)
        } else {
            self.data[dword_index] >> start_bit
        };

        (result & self.entry_bit_mask) as i32
    }
}

// ── NameFrag ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct NameFrag {
    item_index: i32,
    next_index: i32,
    frag_offs: i32,
}

fn read_array_name_frag(r: &mut Cursor<&[u8]>) -> Result<Vec<NameFrag>, CascError> {
    let byte_count = read_i32(r)? as usize;
    if byte_count == 0 {
        return Ok(Vec::new());
    }
    let count = byte_count / 12;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(NameFrag {
            item_index: read_i32(r)?,
            next_index: read_i32(r)?,
            frag_offs: read_i32(r)?,
        });
    }
    Ok(out)
}

// ── TNameIndexStruct ─────────────────────────────────────────────────────────

/// Name fragment storage: a byte array of concatenated fragments with optional
/// end-of-fragment markers tracked in a sparse array.
struct TNameIndexStruct {
    name_fragments: Vec<u8>,
    fragment_ends: TSparseArray,
}

impl TNameIndexStruct {
    fn read(r: &mut Cursor<&[u8]>) -> Result<Self, CascError> {
        let name_fragments = read_array_u8(r)?;
        let fragment_ends = TSparseArray::read(r)?;
        Ok(TNameIndexStruct {
            name_fragments,
            fragment_ends,
        })
    }

    fn count(&self) -> usize {
        self.name_fragments.len()
    }

    /// Check that the search mask matches the fragment at `frag_offs`, advancing char_index.
    fn check_name_fragment(&self, mask: &[u8], char_index: &mut usize, frag_offs: i32) -> bool {
        if self.fragment_ends.total_item_count == 0 {
            let start_pos = (frag_offs as usize).wrapping_sub(*char_index);
            while *char_index < mask.len() {
                let idx = start_pos + *char_index;
                if idx >= self.name_fragments.len() || self.name_fragments[idx] != mask[*char_index]
                {
                    return false;
                }
                *char_index += 1;
                if start_pos + *char_index < self.name_fragments.len()
                    && self.name_fragments[start_pos + *char_index] == 0
                {
                    return true;
                }
            }
            false
        } else {
            let mut offs = frag_offs as usize;
            while offs < self.name_fragments.len() && *char_index < mask.len() {
                if self.name_fragments[offs] != mask[*char_index] {
                    return false;
                }
                *char_index += 1;
                if self.fragment_ends.contains(offs as i32) {
                    return true;
                }
                offs += 1;
                if *char_index >= mask.len() {
                    return false;
                }
            }
            false
        }
    }

    /// Check that the search mask matches AND copy matched bytes to `result`.
    fn check_and_copy_name_fragment(
        &self,
        mask: &[u8],
        char_index: &mut usize,
        result: &mut Vec<u8>,
        frag_offs: i32,
    ) -> bool {
        if self.fragment_ends.total_item_count == 0 {
            let start_pos = (frag_offs as usize).wrapping_sub(*char_index);
            while *char_index < mask.len() {
                let idx = start_pos + *char_index;
                if idx >= self.name_fragments.len() || self.name_fragments[idx] != mask[*char_index]
                {
                    return false;
                }
                result.push(self.name_fragments[idx]);
                *char_index += 1;
                if start_pos + *char_index < self.name_fragments.len()
                    && self.name_fragments[start_pos + *char_index] == 0
                {
                    return true;
                }
            }
            // Copy remaining fragment
            let mut idx = start_pos + *char_index;
            while idx < self.name_fragments.len() && self.name_fragments[idx] != 0 {
                result.push(self.name_fragments[idx]);
                idx += 1;
            }
            true
        } else {
            let mut offs = frag_offs as usize;
            while offs < self.name_fragments.len() && *char_index < mask.len() {
                if self.name_fragments[offs] != mask[*char_index] {
                    return false;
                }
                result.push(self.name_fragments[offs]);
                *char_index += 1;
                if self.fragment_ends.contains(offs as i32) {
                    return true;
                }
                offs += 1;
            }
            // Copy remaining fragment
            while offs < self.name_fragments.len() && !self.fragment_ends.contains(offs as i32) {
                result.push(self.name_fragments[offs]);
                offs += 1;
            }
            true
        }
    }

    /// Copy the fragment at `frag_offs` into `result` (no checking).
    fn copy_name_fragment(&self, result: &mut Vec<u8>, frag_offs: i32) {
        if self.fragment_ends.total_item_count == 0 {
            let mut offs = frag_offs as usize;
            while offs < self.name_fragments.len() && self.name_fragments[offs] != 0 {
                result.push(self.name_fragments[offs]);
                offs += 1;
            }
        } else {
            let mut offs = frag_offs as usize;
            loop {
                if offs >= self.name_fragments.len() {
                    break;
                }
                result.push(self.name_fragments[offs]);
                if self.fragment_ends.contains(offs as i32) {
                    break;
                }
                offs += 1;
            }
        }
    }
}

// ── Search state ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum SearchPhase {
    Initializing,
    Searching,
    Finished,
}

struct PathStop {
    item_index: i32,
    field_4: i32,
    field_8: usize,
    field_c: i32,
    field_10: i32,
}

struct SearchState {
    mask: Vec<u8>,
    item_index: i32,
    char_index: usize,
    result: Vec<u8>,
    path_stops: Vec<PathStop>,
    item_count: usize,
    phase: SearchPhase,
    found_index: i32,
}

impl SearchState {
    fn new(mask: &[u8]) -> Self {
        SearchState {
            mask: mask.to_vec(),
            item_index: 0,
            char_index: 0,
            result: Vec::new(),
            path_stops: Vec::new(),
            item_count: 0,
            phase: SearchPhase::Initializing,
            found_index: -1,
        }
    }

    fn init_search_buffers(&mut self) {
        self.result.clear();
        self.path_stops.clear();
        self.item_index = 0;
        self.char_index = 0;
        self.item_count = 0;
        self.phase = SearchPhase::Searching;
    }
}

// ── MARFileNameDB ────────────────────────────────────────────────────────────

/// MAR file name database: a compressed trie for file name lookup and enumeration.
struct MARFileNameDB {
    struct68_00: TSparseArray,
    file_name_indexes: TSparseArray,
    struct68_d0: TSparseArray,
    frgm_dist_lo_bits: Vec<u8>,
    frgm_dist_hi_bits: TBitEntryArray,
    index_struct_174: TNameIndexStruct,
    next_db: Option<Box<MARFileNameDB>>,
    name_frag_table: Vec<NameFrag>,
    name_frag_index_mask: i32,
    field_214: i32,
}

impl MARFileNameDB {
    fn read(r: &mut Cursor<&[u8]>, check_signature: bool) -> Result<Self, CascError> {
        if check_signature {
            let sig = read_u32(r)?;
            if sig != MAR_SIGNATURE {
                return Err(CascError::InvalidData(format!(
                    "invalid MAR signature: {sig:#010X} (expected {MAR_SIGNATURE:#010X})"
                )));
            }
        }

        let struct68_00 = TSparseArray::read(r)?;
        let file_name_indexes = TSparseArray::read(r)?;
        let struct68_d0 = TSparseArray::read(r)?;
        let frgm_dist_lo_bits = read_array_u8(r)?;
        let frgm_dist_hi_bits = TBitEntryArray::read(r)?;
        let index_struct_174 = TNameIndexStruct::read(r)?;

        let next_db = if struct68_d0.valid_item_count != 0 && index_struct_174.count() == 0 {
            Some(Box::new(MARFileNameDB::read(r, false)?))
        } else {
            None
        };

        let name_frag_table = read_array_name_frag(r)?;
        let name_frag_index_mask = name_frag_table.len() as i32 - 1;
        let field_214 = read_i32(r)?;
        let _dw_bit_mask = read_i32(r)?;

        Ok(MARFileNameDB {
            struct68_00,
            file_name_indexes,
            struct68_d0,
            frgm_dist_lo_bits,
            frgm_dist_hi_bits,
            index_struct_174,
            next_db,
            name_frag_table,
            name_frag_index_mask,
            field_214,
        })
    }

    #[allow(dead_code)]
    fn num_files(&self) -> usize {
        self.file_name_indexes.valid_item_count as usize
    }

    fn get_name_fragment_offset_ex(&self, lo_bits_index: i32, hi_bits_index: i32) -> i32 {
        (self.frgm_dist_hi_bits.get(hi_bits_index) << 8)
            | (self.frgm_dist_lo_bits[lo_bits_index as usize] as i32)
    }

    fn get_name_fragment_offset(&self, lo_bits_index: i32) -> i32 {
        let hi = self.struct68_d0.get_item_value(lo_bits_index);
        self.get_name_fragment_offset_ex(lo_bits_index, hi)
    }

    fn is_single_char(frag_offs: i32) -> bool {
        (frag_offs as u32 & 0xFFFF_FF00) == 0xFFFF_FF00
    }

    // ── CheckNextPathFragment ────────────────────────────────────────────

    fn check_next_path_fragment(&self, state: &mut SearchState) -> bool {
        let name_frag_index =
            ((state.item_index << 5) ^ state.item_index ^ state.mask[state.char_index] as i32)
                & self.name_frag_index_mask;
        let nf = self.name_frag_table[name_frag_index as usize];

        if nf.item_index == state.item_index {
            if Self::is_single_char(nf.frag_offs) {
                state.item_index = nf.next_index;
                state.char_index += 1;
                return true;
            }
            if let Some(ref next) = self.next_db {
                if !next.match_path_fragment(state, nf.frag_offs) {
                    return false;
                }
            } else if !self.index_struct_174.check_name_fragment(
                &state.mask,
                &mut state.char_index,
                nf.frag_offs,
            ) {
                return false;
            }
            state.item_index = nf.next_index;
            return true;
        }

        // Collision path
        let mut collision_index = self.struct68_00.select_zeros(state.item_index) + 1;
        if !self.struct68_00.contains(collision_index) {
            return false;
        }

        state.item_index = collision_index - state.item_index - 1;
        let mut hi_bits_index: i32 = -1;

        loop {
            if self.struct68_d0.contains(state.item_index) {
                if hi_bits_index == -1 {
                    hi_bits_index = self.struct68_d0.get_item_value(state.item_index);
                } else {
                    hi_bits_index += 1;
                }

                let save_char = state.char_index;
                let frag_offs = self.get_name_fragment_offset_ex(state.item_index, hi_bits_index);
                if let Some(ref next) = self.next_db {
                    if next.match_path_fragment(state, frag_offs) {
                        return true;
                    }
                } else if self.index_struct_174.check_name_fragment(
                    &state.mask,
                    &mut state.char_index,
                    frag_offs,
                ) {
                    return true;
                }
                if state.char_index != save_char {
                    return false;
                }
            } else {
                if self.frgm_dist_lo_bits[state.item_index as usize] == state.mask[state.char_index]
                {
                    state.char_index += 1;
                    return true;
                }
            }

            state.item_index += 1;
            collision_index += 1;
            if !self.struct68_00.contains(collision_index) {
                return false;
            }
        }
    }

    // ── sub_1957B80: match path fragment (check only, no copy) ───────────

    fn match_path_fragment(&self, state: &mut SearchState, dw_key: i32) -> bool {
        let mut edi = dw_key;

        loop {
            let nf = self.name_frag_table[(edi & self.name_frag_index_mask) as usize];
            if edi == nf.next_index {
                if !Self::is_single_char(nf.frag_offs) {
                    if let Some(ref next) = self.next_db {
                        if !next.match_path_fragment(state, nf.frag_offs) {
                            return false;
                        }
                    } else if !self.index_struct_174.check_name_fragment(
                        &state.mask,
                        &mut state.char_index,
                        nf.frag_offs,
                    ) {
                        return false;
                    }
                } else {
                    if state.char_index >= state.mask.len()
                        || state.mask[state.char_index] != (nf.frag_offs & 0xFF) as u8
                    {
                        return false;
                    }
                    state.char_index += 1;
                }

                edi = nf.item_index;
                if edi == 0 {
                    return true;
                }
                if state.char_index >= state.mask.len() {
                    return false;
                }
            } else {
                if self.struct68_d0.contains(edi) {
                    let frag_offs = self.get_name_fragment_offset(edi);
                    if let Some(ref next) = self.next_db {
                        if !next.match_path_fragment(state, frag_offs) {
                            return false;
                        }
                    } else if !self.index_struct_174.check_name_fragment(
                        &state.mask,
                        &mut state.char_index,
                        frag_offs,
                    ) {
                        return false;
                    }
                } else {
                    if state.char_index >= state.mask.len()
                        || self.frgm_dist_lo_bits[edi as usize] != state.mask[state.char_index]
                    {
                        return false;
                    }
                    state.char_index += 1;
                }

                if edi <= self.field_214 {
                    return true;
                }
                if state.char_index >= state.mask.len() {
                    return false;
                }

                let eax = self.struct68_00.select_ones(edi);
                edi = eax - edi - 1;
            }
        }
    }

    // ── sub_1958D70: copy path fragment (no check) ──────────────────────

    fn copy_path_fragment(&self, result: &mut Vec<u8>, arg_4: i32) {
        let mut key = arg_4;

        loop {
            let nf = self.name_frag_table[(key & self.name_frag_index_mask) as usize];
            if key == nf.next_index {
                if !Self::is_single_char(nf.frag_offs) {
                    if let Some(ref next) = self.next_db {
                        next.copy_path_fragment(result, nf.frag_offs);
                    } else {
                        self.index_struct_174
                            .copy_name_fragment(result, nf.frag_offs);
                    }
                } else {
                    result.push((nf.frag_offs & 0xFF) as u8);
                }

                key = nf.item_index;
                if key == 0 {
                    return;
                }
            } else {
                if self.struct68_d0.contains(key) {
                    let frag_offs = self.get_name_fragment_offset(key);
                    if let Some(ref next) = self.next_db {
                        next.copy_path_fragment(result, frag_offs);
                    } else {
                        self.index_struct_174.copy_name_fragment(result, frag_offs);
                    }
                } else {
                    result.push(self.frgm_dist_lo_bits[key as usize]);
                }

                if key <= self.field_214 {
                    return;
                }

                key = self.struct68_00.select_ones(key) - key - 1;
            }
        }
    }

    // ── sub_1959010: check and copy path fragment ────────────────────────

    fn check_and_copy_path_fragment(&self, state: &mut SearchState, arg_4: i32) -> bool {
        let mut key = arg_4;

        loop {
            let nf = self.name_frag_table[(key & self.name_frag_index_mask) as usize];
            if key == nf.next_index {
                if !Self::is_single_char(nf.frag_offs) {
                    if let Some(ref next) = self.next_db {
                        if !next.check_and_copy_path_fragment(state, nf.frag_offs) {
                            return false;
                        }
                    } else if !self.index_struct_174.check_and_copy_name_fragment(
                        &state.mask,
                        &mut state.char_index,
                        &mut state.result,
                        nf.frag_offs,
                    ) {
                        return false;
                    }
                } else {
                    let ch = (nf.frag_offs & 0xFF) as u8;
                    if state.char_index >= state.mask.len() || ch != state.mask[state.char_index] {
                        return false;
                    }
                    state.result.push(ch);
                    state.char_index += 1;
                }

                key = nf.item_index;
                if key == 0 {
                    return true;
                }
            } else {
                if self.struct68_d0.contains(key) {
                    let frag_offs = self.get_name_fragment_offset(key);
                    if let Some(ref next) = self.next_db {
                        if !next.check_and_copy_path_fragment(state, frag_offs) {
                            return false;
                        }
                    } else if !self.index_struct_174.check_and_copy_name_fragment(
                        &state.mask,
                        &mut state.char_index,
                        &mut state.result,
                        frag_offs,
                    ) {
                        return false;
                    }
                } else {
                    let ch = self.frgm_dist_lo_bits[key as usize];
                    if state.char_index >= state.mask.len() || ch != state.mask[state.char_index] {
                        return false;
                    }
                    state.result.push(ch);
                    state.char_index += 1;
                }

                if key <= self.field_214 {
                    return true;
                }

                key = self.struct68_00.select_ones(key) - key - 1;
            }

            if state.char_index >= state.mask.len() {
                break;
            }
        }

        self.copy_path_fragment(&mut state.result, key);
        true
    }

    // ── sub_1958B00: initial search with copy ────────────────────────────

    fn search_initial_with_copy(&self, state: &mut SearchState) -> bool {
        let item_index =
            (state.mask[state.char_index] as i32 ^ (state.item_index << 5) ^ state.item_index)
                & self.name_frag_index_mask;

        let nf = self.name_frag_table[item_index as usize];

        if state.item_index == nf.item_index {
            let frag_offs = nf.frag_offs;
            if Self::is_single_char(frag_offs) {
                state.result.push((frag_offs & 0xFF) as u8);
                state.item_index = nf.next_index;
                state.char_index += 1;
                return true;
            }

            if let Some(ref next) = self.next_db {
                if !next.check_and_copy_path_fragment(state, frag_offs) {
                    return false;
                }
            } else if !self.index_struct_174.check_and_copy_name_fragment(
                &state.mask,
                &mut state.char_index,
                &mut state.result,
                frag_offs,
            ) {
                return false;
            }
            state.item_index = nf.next_index;
            return true;
        }

        // Collision path
        let mut collision_index = self.struct68_00.select_zeros(state.item_index) + 1;
        if !self.struct68_00.contains(collision_index) {
            return false;
        }

        state.item_index = collision_index - state.item_index - 1;
        let mut var_4: i32 = -1;

        loop {
            if self.struct68_d0.contains(state.item_index) {
                if var_4 == -1 {
                    var_4 = self.struct68_d0.get_item_value(state.item_index);
                } else {
                    var_4 += 1;
                }

                let save_char = state.char_index;
                let frag_offs = self.get_name_fragment_offset_ex(state.item_index, var_4);
                if let Some(ref next) = self.next_db {
                    if next.check_and_copy_path_fragment(state, frag_offs) {
                        return true;
                    }
                } else if self.index_struct_174.check_and_copy_name_fragment(
                    &state.mask,
                    &mut state.char_index,
                    &mut state.result,
                    frag_offs,
                ) {
                    return true;
                }
                if save_char != state.char_index {
                    return false;
                }
            } else {
                let ch = self.frgm_dist_lo_bits[state.item_index as usize];
                if ch == state.mask[state.char_index] {
                    state.result.push(ch);
                    state.char_index += 1;
                    return true;
                }
            }

            state.item_index += 1;
            collision_index += 1;
            if !self.struct68_00.contains(collision_index) {
                return false;
            }
        }
    }

    // ── FindFileInDatabase ───────────────────────────────────────────────

    fn find_file(&self, name: &[u8]) -> Option<i32> {
        let mut state = SearchState::new(name);

        if !name.is_empty() {
            while state.char_index < state.mask.len() {
                if !self.check_next_path_fragment(&mut state) {
                    return None;
                }
            }
        }

        if !self.file_name_indexes.contains(state.item_index) {
            return None;
        }

        Some(self.file_name_indexes.get_item_value(state.item_index))
    }

    // ── EnumerateFiles (one step) ────────────────────────────────────────

    fn enumerate_step(&self, state: &mut SearchState) -> bool {
        if state.phase == SearchPhase::Finished {
            return false;
        }

        if state.phase != SearchPhase::Searching {
            state.init_search_buffers();

            while state.char_index < state.mask.len() {
                if !self.search_initial_with_copy(state) {
                    state.phase = SearchPhase::Finished;
                    return false;
                }
            }

            state.path_stops.push(PathStop {
                item_index: state.item_index,
                field_4: 0,
                field_8: state.result.len(),
                field_c: -1,
                field_10: -1,
            });
            state.item_count = 1;

            if self.file_name_indexes.contains(state.item_index) {
                let idx = self.file_name_indexes.get_item_value(state.item_index);
                state.found_index = idx;
                return true;
            }
        }

        loop {
            if state.item_count == state.path_stops.len() {
                let last = &state.path_stops[state.path_stops.len() - 1];
                let collision_index = self.struct68_00.select_zeros(last.item_index) + 1;
                state.path_stops.push(PathStop {
                    item_index: collision_index - last.item_index - 1,
                    field_4: collision_index,
                    field_8: 0,
                    field_c: -1,
                    field_10: -1,
                });
            }

            let ps_idx = state.item_count;
            let field_4 = state.path_stops[ps_idx].field_4;
            state.path_stops[ps_idx].field_4 += 1;

            if self.struct68_00.contains(field_4) {
                state.item_count += 1;
                let ps_item_index = state.path_stops[ps_idx].item_index;

                if self.struct68_d0.contains(ps_item_index) {
                    if state.path_stops[ps_idx].field_c == -1 {
                        state.path_stops[ps_idx].field_c =
                            self.struct68_d0.get_item_value(ps_item_index);
                    } else {
                        state.path_stops[ps_idx].field_c += 1;
                    }

                    let fc = state.path_stops[ps_idx].field_c;
                    let frag_offs = self.get_name_fragment_offset_ex(ps_item_index, fc);
                    if let Some(ref next) = self.next_db {
                        next.copy_path_fragment(&mut state.result, frag_offs);
                    } else {
                        self.index_struct_174
                            .copy_name_fragment(&mut state.result, frag_offs);
                    }
                } else {
                    state
                        .result
                        .push(self.frgm_dist_lo_bits[ps_item_index as usize]);
                }

                state.path_stops[ps_idx].field_8 = state.result.len();

                if self.file_name_indexes.contains(ps_item_index) {
                    if state.path_stops[ps_idx].field_10 == -1 {
                        state.path_stops[ps_idx].field_10 =
                            self.file_name_indexes.get_item_value(ps_item_index);
                    } else {
                        state.path_stops[ps_idx].field_10 += 1;
                    }
                    state.found_index = state.path_stops[ps_idx].field_10;
                    return true;
                }
            } else {
                if state.item_count == 1 {
                    state.phase = SearchPhase::Finished;
                    return false;
                }

                let prev_idx = state.item_count - 1;
                state.path_stops[prev_idx].item_index += 1;

                let prev2_idx = state.item_count - 2;
                let edi = state.path_stops[prev2_idx].field_8;
                state.result.truncate(edi);
                state.item_count -= 1;
            }
        }
    }

    /// Enumerate all files, returning `(file_name_index, path)` pairs.
    fn enumerate_all_files(&self) -> Vec<(i32, String)> {
        let mut results = Vec::new();
        let mut state = SearchState::new(b"");
        while self.enumerate_step(&mut state) {
            let path = String::from_utf8_lossy(&state.result).into_owned();
            results.push((state.found_index, path));
        }
        results
    }
}

// ── MNDX root entry ─────────────────────────────────────────────────────────

struct MndxEntry {
    flags: i32,
    ckey: Md5Hash,
    #[allow(dead_code)]
    file_size: i32,
}

// ── MndxRootHandler ──────────────────────────────────────────────────────────

pub struct MndxRootHandler {
    entries_by_hash: HashMap<u64, Vec<RootEntry>>,
    file_paths: HashMap<u64, String>,
}

impl MndxRootHandler {
    /// Parse an MNDX root file, building the complete file → CKey map.
    pub fn parse(data: &[u8]) -> Result<Self, CascError> {
        let mut r = Cursor::new(data);

        // ── Header ───────────────────────────────────────────────────────
        let signature = read_u32(&mut r)?;
        if signature != MNDX_SIGNATURE {
            return Err(CascError::InvalidData(format!(
                "invalid MNDX signature: {signature:#010X}"
            )));
        }
        let header_version = read_i32(&mut r)?;
        let format_version = read_i32(&mut r)?;
        if !(1..=2).contains(&format_version) {
            return Err(CascError::InvalidData(format!(
                "unsupported MNDX format version {format_version}"
            )));
        }
        if header_version == 2 {
            let _build1 = read_i32(&mut r)?;
            let _build2 = read_i32(&mut r)?;
        }

        let mar_info_offset = read_i32(&mut r)? as u64;
        let mar_info_count = read_i32(&mut r)? as usize;
        let mar_info_size = read_i32(&mut r)? as u64;
        let mndx_entries_offset = read_i32(&mut r)? as u64;
        let mndx_entries_total = read_i32(&mut r)? as usize;
        let mndx_entries_valid = read_i32(&mut r)? as usize;
        let _mndx_entry_size = read_i32(&mut r)?;

        if mar_info_count > 3 {
            return Err(CascError::InvalidData(format!(
                "too many MAR databases: {mar_info_count}"
            )));
        }

        // ── MAR info + databases ─────────────────────────────────────────
        let mut mar_files = Vec::with_capacity(mar_info_count);
        for i in 0..mar_info_count {
            r.seek(SeekFrom::Start(mar_info_offset + mar_info_size * i as u64))?;
            let _mar_index = read_i32(&mut r)?;
            let _mar_data_size = read_i32(&mut r)?;
            let _mar_data_size_hi = read_i32(&mut r)?;
            let mar_data_offset = read_i32(&mut r)? as u64;
            let _mar_data_offset_hi = read_i32(&mut r)?;

            r.seek(SeekFrom::Start(mar_data_offset))?;
            mar_files.push(MARFileNameDB::read(&mut r, true)?);
        }

        if mar_files.len() < 3 {
            return Err(CascError::InvalidData(
                "MNDX requires 3 MAR databases".into(),
            ));
        }

        // ── Root entries ─────────────────────────────────────────────────
        r.seek(SeekFrom::Start(mndx_entries_offset))?;
        let mut all_entries = Vec::with_capacity(mndx_entries_total);
        for _ in 0..mndx_entries_total {
            let flags = read_i32(&mut r)?;
            let ckey = read_md5(&mut r)?;
            let file_size = read_i32(&mut r)?;
            all_entries.push(MndxEntry {
                flags,
                ckey,
                file_size,
            });
        }

        // Build valid-entries index: entry 0 is always valid; then after each
        // entry with bit 31 set, the next entry starts a new valid chain.
        let mut valid_entry_indices: Vec<usize> = Vec::with_capacity(mndx_entries_valid);
        valid_entry_indices.push(0);
        for (i, entry) in all_entries.iter().enumerate().take(mndx_entries_total) {
            if valid_entry_indices.len() >= mndx_entries_valid {
                break;
            }
            if (entry.flags as u32 & 0x8000_0000) != 0 && i + 1 < mndx_entries_total {
                valid_entry_indices.push(i + 1);
            }
        }

        // ── Enumerate packages from MAR[0] ──────────────────────────────
        let package_list = mar_files[0].enumerate_all_files();
        let mut packages: HashMap<i32, String> = HashMap::new();
        let mut package_locales: HashMap<i32, LocaleFlags> = HashMap::new();

        for (idx, path) in &package_list {
            packages.insert(*idx, path.clone());
            let locale = detect_package_locale(path);
            package_locales.insert(*idx, locale);
        }

        // ── Enumerate all files from MAR[2] and resolve CKeys ───────────
        let all_files = mar_files[2].enumerate_all_files();
        let mut entries_by_hash: HashMap<u64, Vec<RootEntry>> = HashMap::new();
        let mut file_paths: HashMap<u64, String> = HashMap::new();

        for (_file_idx, file_path) in &all_files {
            let hash = jenkins96(file_path);

            // Find the longest-matching package
            let pkg_key = find_mndx_package(file_path, &packages);
            let locale = pkg_key
                .and_then(|k| package_locales.get(&k).copied())
                .unwrap_or(LocaleFlags::ALL);

            if let Some(pkg_key) = pkg_key {
                if let Some(pkg_path) = packages.get(&pkg_key) {
                    // Strip package prefix + separator
                    let stripped_start = pkg_path.len() + 1;
                    if stripped_start < file_path.len() {
                        let stripped = &file_path[stripped_start..];
                        let lower = stripped.to_lowercase();

                        // Look up in MAR[1] to get file name index
                        if let Some(fni) = mar_files[1].find_file(lower.as_bytes()) {
                            if let Some(&valid_idx) = valid_entry_indices.get(fni as usize) {
                                // Walk the entry chain to find one matching this package
                                let mut ei = valid_idx;
                                while ei < all_entries.len() {
                                    let entry = &all_entries[ei];
                                    if (entry.flags & 0x00FF_FFFF) == pkg_key {
                                        entries_by_hash.entry(hash).or_default().push(RootEntry {
                                            ckey: entry.ckey,
                                            locale,
                                            content: ContentFlags::NONE,
                                        });
                                        break;
                                    }
                                    if (entry.flags as u32 & 0x8000_0000) != 0 {
                                        break; // terminator
                                    }
                                    ei += 1;
                                }
                            }
                        }
                    }
                }
            }

            file_paths.insert(hash, file_path.clone());
        }

        Ok(MndxRootHandler {
            entries_by_hash,
            file_paths,
        })
    }
}

// ── RootHandler trait ────────────────────────────────────────────────────────

impl RootHandler for MndxRootHandler {
    fn count(&self) -> usize {
        self.entries_by_hash.len()
    }

    fn get_all_entries(&self, hash: u64) -> &[RootEntry] {
        self.entries_by_hash
            .get(&hash)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn all_entries(&self) -> Box<dyn Iterator<Item = (u64, &RootEntry)> + '_> {
        Box::new(
            self.entries_by_hash
                .iter()
                .flat_map(|(&hash, entries)| entries.iter().map(move |e| (hash, e))),
        )
    }

    fn hash_for_file_data_id(&self, _id: u32) -> Option<u64> {
        None // MNDX doesn't use FileDataIds
    }

    fn file_data_id_for_hash(&self, _hash: u64) -> Option<u32> {
        None
    }

    fn builtin_paths(&self) -> Vec<(u64, String)> {
        self.file_paths
            .iter()
            .map(|(&hash, path)| (hash, path.clone()))
            .collect()
    }
}

// ── Package locale detection ─────────────────────────────────────────────────

/// Find the longest-matching package for a file path.
fn find_mndx_package(file_path: &str, packages: &HashMap<i32, String>) -> Option<i32> {
    let mut best_key = None;
    let mut best_len = 0;

    for (&key, pkg_path) in packages {
        let pkg_len = pkg_path.len();
        if pkg_len < file_path.len() && pkg_len > best_len && file_path[..pkg_len] == **pkg_path {
            best_key = Some(key);
            best_len = pkg_len;
        }
    }

    best_key
}

/// Detect the locale from a package path by looking for a 4-character locale
/// code before `.sc2data`, `.sc2assets`, `.stormdata`, or `.stormassets`.
fn detect_package_locale(path: &str) -> LocaleFlags {
    let lower = path.to_lowercase();
    for suffix in &[".sc2data", ".sc2assets", ".stormdata", ".stormassets"] {
        if let Some(pos) = lower.find(suffix) {
            if pos >= 4 {
                let code = &lower[pos - 4..pos];
                if let Some(locale) = locale_from_code(code) {
                    return locale;
                }
            }
        }
    }
    LocaleFlags::ALL
}

fn locale_from_code(code: &str) -> Option<LocaleFlags> {
    match code {
        "enus" => Some(LocaleFlags::EN_US),
        "kokr" => Some(LocaleFlags::KO_KR),
        "frfr" => Some(LocaleFlags::FR_FR),
        "dede" => Some(LocaleFlags::DE_DE),
        "zhcn" => Some(LocaleFlags::ZH_CN),
        "eses" => Some(LocaleFlags::ES_ES),
        "zhtw" => Some(LocaleFlags::ZH_TW),
        "engb" => Some(LocaleFlags::EN_GB),
        "esmx" => Some(LocaleFlags::ES_MX),
        "ruru" => Some(LocaleFlags::RU_RU),
        "ptbr" => Some(LocaleFlags::PT_BR),
        "itit" => Some(LocaleFlags::IT_IT),
        "ptpt" => Some(LocaleFlags::PT_PT),
        _ => None,
    }
}
