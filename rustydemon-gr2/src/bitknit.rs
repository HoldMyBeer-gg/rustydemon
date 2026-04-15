//! Pure-Rust Bitknit2 decoder.
//!
//! Bitknit2 is a rANS-based codec from RAD Game Tools used by Granny3D
//! to compress section payloads.  This implementation is a direct
//! port of Mackenzie Straight's MIT-licensed C++ reference in
//! [pybg3's `src/rans.h`](https://github.com/eiz/pybg3), which itself
//! faithfully implements the format documented at
//! [libbg3/docs/bitknit2.txt](https://github.com/eiz/libbg3/blob/main/docs/bitknit2.txt).
//!
//! Historical note: this file previously held a port of powzix/ooz's
//! `bitknit.cpp`.  That worked for Kraken-wrapped Bitknit streams but
//! *not* for the standalone Bitknit2 streams Granny produces, because
//! Granny's section payloads start with a 2-byte magic (`0x75B1`),
//! use a quantum-based decoder (2^16-byte output quanta), and have a
//! different rANS init sequence (two u16s giving a merged 32-bit
//! state split by an in-band nibble).  Once I swapped references
//! everything lined up; the pybg3 port is kept as close to its
//! structure as is reasonable in safe Rust.
//!
//! Attribution: pybg3 `src/rans.h` © 2024 Mackenzie Straight, MIT.

use crate::error::{GrannyError, Result};

// ── Bitstream: sequence of little-endian u16 words ────────────────────────────

/// Forward-only u16 reader.  Mirrors pybg3's `bounded_stack` used for
/// reading only (we never push back into the source stream during
/// decode).
struct WordStream<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> WordStream<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn is_at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn peek(&self) -> Result<u16> {
        if self.pos + 2 > self.data.len() {
            return Err(GrannyError::BitknitDecode("src truncated (peek)"));
        }
        Ok(u16::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
        ]))
    }

    fn pop(&mut self) -> Result<u16> {
        let v = self.peek()?;
        self.pos += 2;
        Ok(v)
    }
}

// ── Frequency tables (one type per vocab size) ────────────────────────────────
//
// We need three different sizes: 300 (command word), 40 (cache ref),
// 21 (copy offset length).  Const generics can't index arrays with
// size `VOCAB + 1` on stable Rust without feature flags, so we
// hand-roll three specialised types with a shared trait.

const FREQUENCY_BITS: u32 = 15;
const TOTAL_SUM: u32 = 1 << FREQUENCY_BITS; // 0x8000
const LOOKUP_BITS: u32 = 10;
const LOOKUP_SIZE: usize = 1 << LOOKUP_BITS as usize; // 1024
const LOOKUP_SHIFT: u32 = FREQUENCY_BITS - LOOKUP_BITS; // 5
const ADAPT_INTERVAL: u32 = 1024;

/// Common interface all three vocab-specific tables implement.  The
/// rANS state uses this in `pop_cdf` without caring about the vocab.
trait CdfTable {
    fn frequency(&self, sym: u32) -> u32;
    fn sum_below(&self, sym: u32) -> u32;
    fn find_symbol(&self, code: u32) -> u32;
}

macro_rules! define_table {
    ($name:ident, $vocab:expr, $min_prob:expr) => {
        struct $name {
            sums: [u16; $vocab + 1],
            lookup: [u16; LOOKUP_SIZE],
            freq_acc: [u16; $vocab],
            adapt_counter: u32,
        }

        impl $name {
            const VOCAB: usize = $vocab;
            const MIN_PROB: usize = $min_prob;
            const EQUI: usize = Self::VOCAB - Self::MIN_PROB;
            // (total_sum - vocab) / interval, truncated.
            const FREQ_INCR: u16 =
                ((TOTAL_SUM as usize - Self::VOCAB) / ADAPT_INTERVAL as usize) as u16;
            // 1 + total_sum - vocab - freq_incr * interval
            const LAST_FREQ_INCR: u16 = (1 + TOTAL_SUM as usize
                - Self::VOCAB
                - (Self::FREQ_INCR as usize) * ADAPT_INTERVAL as usize)
                as u16;

            fn new() -> Self {
                let mut t = Self {
                    sums: [0u16; $vocab + 1],
                    lookup: [0u16; LOOKUP_SIZE],
                    freq_acc: [1u16; $vocab],
                    adapt_counter: 0,
                };
                // Equiprobable over first (vocab - min_prob) symbols,
                // min-prob over the rest.  Matches the spec's
                // Initialize Adaptive Model algorithm.
                for i in 0..Self::EQUI {
                    t.sums[i] = ((TOTAL_SUM as usize - Self::MIN_PROB) * i / Self::EQUI) as u16;
                }
                for i in Self::EQUI..=Self::VOCAB {
                    t.sums[i] = (TOTAL_SUM as usize - Self::VOCAB + i) as u16;
                }
                t.finish_update();
                t
            }

            fn finish_update(&mut self) {
                let mut code: u32 = 0;
                let mut sym: usize = 0;
                let mut next = self.sums[1] as u32;
                while code < (1u32 << FREQUENCY_BITS) {
                    if code < next {
                        self.lookup[(code >> LOOKUP_SHIFT) as usize] = sym as u16;
                        code += 1u32 << LOOKUP_SHIFT;
                    } else {
                        sym += 1;
                        next = self.sums[sym + 1] as u32;
                    }
                }
            }

            fn observe(&mut self, sym: u32) {
                self.freq_acc[sym as usize] =
                    self.freq_acc[sym as usize].wrapping_add(Self::FREQ_INCR);
                self.adapt_counter = (self.adapt_counter + 1) % ADAPT_INTERVAL;
                if self.adapt_counter == 0 {
                    self.freq_acc[sym as usize] =
                        self.freq_acc[sym as usize].wrapping_add(Self::LAST_FREQ_INCR);
                    // Running sum in u32 to avoid u16 wrap.
                    // Match C u32 semantics exactly: the divide-by-2 of
                    // a negative difference must round the way unsigned
                    // wrap-around does, not the way signed truncation
                    // does.  For odd negatives the two differ by one
                    // and the error accumulates over 1024+ symbols,
                    // drifting the probability model out of sync with
                    // the encoder.
                    let mut sum: u32 = 0;
                    for i in 1..=Self::VOCAB {
                        sum = sum.wrapping_add(self.freq_acc[i - 1] as u32);
                        let old = self.sums[i] as u32;
                        let diff = sum.wrapping_sub(old);
                        let delta = diff >> 1;
                        self.sums[i] = old.wrapping_add(delta) as u16;
                        self.freq_acc[i - 1] = 1;
                    }
                    self.finish_update();
                }
            }
        }

        impl CdfTable for $name {
            #[inline]
            fn frequency(&self, sym: u32) -> u32 {
                (self.sums[sym as usize + 1] - self.sums[sym as usize]) as u32
            }
            #[inline]
            fn sum_below(&self, sym: u32) -> u32 {
                self.sums[sym as usize] as u32
            }
            #[inline]
            fn find_symbol(&self, code: u32) -> u32 {
                let mut sym = self.lookup[(code >> LOOKUP_SHIFT) as usize] as u32;
                while code >= self.sums[sym as usize + 1] as u32 {
                    sym += 1;
                }
                sym
            }
        }
    };
}

define_table!(CommandWordTable, 300, 36);
define_table!(CacheRefTable, 40, 0);
define_table!(CopyOffsetTable, 21, 0);

// ── rANS state ────────────────────────────────────────────────────────────────

/// A single rANS stream register.  Bitknit2 uses two of these,
/// interleaved — each decode op operates on one and then the pair is
/// swapped so the next op uses the other.
#[derive(Debug, Clone, Copy)]
struct RansState {
    bits: u32,
}

const REFILL_THRESHOLD: u32 = 1 << 16;

impl RansState {
    fn new_with(bits: u32) -> Self {
        Self { bits }
    }

    #[inline]
    fn maybe_refill(&mut self, src: &mut WordStream<'_>) -> Result<()> {
        if self.bits < REFILL_THRESHOLD {
            let w = src.pop()? as u32;
            self.bits = (self.bits << 16) | w;
        }
        Ok(())
    }

    #[inline]
    fn pop_bits(&mut self, src: &mut WordStream<'_>, nbits: u32) -> Result<u32> {
        let sym = self.bits & ((1u32 << nbits) - 1);
        self.bits >>= nbits;
        self.maybe_refill(src)?;
        Ok(sym)
    }

    #[inline]
    fn pop_cdf<T: CdfTable>(&mut self, src: &mut WordStream<'_>, table: &T) -> Result<u32> {
        let code = self.bits & ((1u32 << FREQUENCY_BITS) - 1);
        let sym = table.find_symbol(code);
        let freq = table.frequency(sym);
        let sum_below = table.sum_below(sym);
        // Use wrapping arithmetic to match the C reference's u32
        // behaviour at the corner of the valid state range where
        // (bits >> 15) * freq can touch 2^32.
        self.bits = (self.bits >> FREQUENCY_BITS)
            .wrapping_mul(freq)
            .wrapping_add(code)
            .wrapping_sub(sum_below);
        self.maybe_refill(src)?;
        Ok(sym)
    }
}

// ── LRU cache for copy offsets ────────────────────────────────────────────────

/// Register-packed 8-entry LRU cache.  `entry_order` is a u32 holding
/// eight 4-bit slot indices (position 0 at bits 0..=3, position 7 at
/// bits 28..=31).  Initially `0x76543210` so slot i is at position i.
///
/// See Fabian Giesen's writeup:
/// https://fgiesen.wordpress.com/2016/03/07/repeated-match-offsets-in-bitknit/
#[derive(Debug)]
struct LruOffsetCache {
    entries: [u32; 8],
    entry_order: u32,
}

impl LruOffsetCache {
    fn new() -> Self {
        Self {
            entries: [1; 8],
            entry_order: 0x76543210,
        }
    }

    /// Cache hit at position `index` (0..8).  Returns the entry and
    /// rotates the order so that position 0 now points to the hit.
    fn hit(&mut self, index: u32) -> u32 {
        let slot = (self.entry_order >> (index * 4)) & 0xF;
        // `16 << (index*4)` overflows u32 for index=7 (16 << 28 =
        // 0x100000000).  Compute in u64 then cast — on u32 it wraps
        // to 0 so the -1 underflow gives 0xFFFFFFFF, which is the
        // rotate-all mask and happens to be correct for index=7.
        let rotate_mask = (((16u64) << (index * 4)).wrapping_sub(1)) as u32;
        let rotated_order = ((self.entry_order << 4) | slot) & rotate_mask;
        self.entry_order = (self.entry_order & !rotate_mask) | rotated_order;
        self.entries[slot as usize]
    }

    /// Cache miss: replace the "least recently used" slot (position 7
    /// in the LRU order, which stores whatever was in position 6
    /// immediately before).  Matches pybg3 exactly.
    fn insert(&mut self, value: u32) {
        let slot7 = (self.entry_order >> 28) as usize;
        let slot6 = ((self.entry_order >> 24) & 0xF) as usize;
        self.entries[slot7] = self.entries[slot6];
        self.entries[slot6] = value;
    }
}

// ── Public decoder entry point ────────────────────────────────────────────────

/// Decode a complete Bitknit2 stream into a buffer of exactly
/// `dst_len` bytes.  `src` is the raw sector payload from the Granny
/// file (starts with the 2-byte `0x75B1` magic).
pub fn decode_sector(src: &[u8], dst_len: usize) -> Result<Vec<u8>> {
    let mut dst = vec![0u8; dst_len];
    let mut state = DecoderState::new();
    state.decode(src, &mut dst)?;
    Ok(dst)
}

struct DecoderState {
    command_word_models: [CommandWordTable; 4],
    cache_reference_models: [CacheRefTable; 4],
    copy_offset_model: CopyOffsetTable,
    offset_cache: LruOffsetCache,
    delta_offset: usize,
}

impl DecoderState {
    fn new() -> Self {
        Self {
            command_word_models: [
                CommandWordTable::new(),
                CommandWordTable::new(),
                CommandWordTable::new(),
                CommandWordTable::new(),
            ],
            cache_reference_models: [
                CacheRefTable::new(),
                CacheRefTable::new(),
                CacheRefTable::new(),
                CacheRefTable::new(),
            ],
            copy_offset_model: CopyOffsetTable::new(),
            offset_cache: LruOffsetCache::new(),
            delta_offset: 1,
        }
    }

    fn decode(&mut self, src: &[u8], dst: &mut [u8]) -> Result<()> {
        let mut stream = WordStream::new(src);
        // 1. Read and validate magic.
        let magic = stream.pop()?;
        if magic != 0x75B1 {
            return Err(GrannyError::BitknitDecode("missing Bitknit2 magic 0x75B1"));
        }

        let mut dst_pos: usize = 0;
        let dst_len = dst.len();
        // 2. Decode quanta until the whole buffer is filled.
        while dst_pos < dst_len {
            if stream.is_at_end() {
                return Err(GrannyError::BitknitDecode(
                    "src exhausted before dst filled",
                ));
            }
            self.decode_quantum(&mut stream, dst, &mut dst_pos)?;
        }
        Ok(())
    }

    fn decode_quantum(
        &mut self,
        src: &mut WordStream<'_>,
        dst: &mut [u8],
        dst_pos: &mut usize,
    ) -> Result<()> {
        let dst_len = dst.len();
        // Quantum ends at the next 64K boundary, or end-of-buffer.
        let boundary = (*dst_pos & !0xFFFF) + 0x10000;
        let quantum_end = boundary.min(dst_len);

        // Raw-quantum shortcut: a zero word at the start means copy
        // the rest of the quantum uncompressed from src.
        if src.peek()? == 0 {
            src.pop()?;
            let to_copy = quantum_end - *dst_pos;
            // Source words remaining × 2.
            let src_remaining = src.data.len().saturating_sub(src.pos);
            if to_copy > src_remaining {
                return Err(GrannyError::BitknitDecode(
                    "raw quantum longer than remaining src",
                ));
            }
            dst[*dst_pos..*dst_pos + to_copy]
                .copy_from_slice(&src.data[src.pos..src.pos + to_copy]);
            *dst_pos += to_copy;
            src.pos += to_copy;
            return Ok(());
        }

        let (mut s1, mut s2) = self.decode_initial_state(src)?;

        // First byte of the entire decoded output is 8 bits pulled
        // straight from the rANS state — no delta encoding applied.
        if *dst_pos == 0 {
            let b = Self::pop_bits(&mut s1, &mut s2, src, 8)?;
            dst[*dst_pos] = b as u8;
            *dst_pos += 1;
        }

        while *dst_pos < quantum_end {
            let model_index = *dst_pos & 3;
            let command = Self::pop_model(
                &mut s1,
                &mut s2,
                src,
                &mut self.command_word_models[model_index],
            )?;
            if command >= 256 {
                self.decode_copy(&mut s1, &mut s2, src, dst, dst_pos, command)?;
            } else {
                let back = (*dst_pos).wrapping_sub(self.delta_offset);
                let back_byte = if self.delta_offset > *dst_pos {
                    0
                } else {
                    dst[back]
                };
                dst[*dst_pos] = (command as u8).wrapping_add(back_byte);
                *dst_pos += 1;
            }
        }

        // At end of quantum both streams must be at the terminal
        // "freshly-initialised" state of 2^16 (`refill_threshold`).
        if s1.bits != REFILL_THRESHOLD || s2.bits != REFILL_THRESHOLD {
            return Err(GrannyError::BitknitDecode(
                "rANS stream corrupt at quantum end",
            ));
        }
        Ok(())
    }

    fn decode_copy(
        &mut self,
        s1: &mut RansState,
        s2: &mut RansState,
        src: &mut WordStream<'_>,
        dst: &mut [u8],
        dst_pos: &mut usize,
        command: u32,
    ) -> Result<()> {
        let model_index = *dst_pos & 3;
        // Copy length: short-form for commands 256..=287, extended
        // variable-width bit extraction for 288..=299.
        let copy_length = if command < 288 {
            command - 254
        } else {
            let nb = command - 287;
            let extra = Self::pop_bits(s1, s2, src, nb)?;
            (1u32 << nb) + extra + 32
        } as usize;

        let cache_ref =
            Self::pop_model(s1, s2, src, &mut self.cache_reference_models[model_index])?;

        let copy_offset = if cache_ref < 8 {
            self.offset_cache.hit(cache_ref) as usize
        } else {
            let copy_offset_length = Self::pop_model(s1, s2, src, &mut self.copy_offset_model)?;
            let mut copy_offset_bits = Self::pop_bits(s1, s2, src, copy_offset_length & 0xF)?;
            if copy_offset_length >= 16 {
                copy_offset_bits = (copy_offset_bits << 16) | src.pop()? as u32;
            }
            // (32 << nb) + (bits << 5) - 32 + (cache_ref - 7)
            // min offset when nb=0, bits=0, cache_ref=8: 32+0-32+1 = 1.
            let off = (32u32 << copy_offset_length)
                .wrapping_add(copy_offset_bits << 5)
                .wrapping_sub(32)
                .wrapping_add(cache_ref - 7);
            self.offset_cache.insert(off);
            off as usize
        };

        if copy_offset == 0 || copy_offset > *dst_pos {
            return Err(GrannyError::BitknitDecode("invalid copy offset"));
        }
        if copy_length > dst.len() - *dst_pos {
            return Err(GrannyError::BitknitDecode("invalid copy length"));
        }
        self.delta_offset = copy_offset;
        // Byte-forward copy so dist < len gives the LZ run-fill pattern.
        for i in 0..copy_length {
            dst[*dst_pos + i] = dst[*dst_pos + i - copy_offset];
        }
        *dst_pos += copy_length;
        Ok(())
    }

    fn decode_initial_state(&mut self, src: &mut WordStream<'_>) -> Result<(RansState, RansState)> {
        let init_0 = src.pop()? as u32;
        let init_1 = src.pop()? as u32;
        let mut merged = RansState::new_with((init_0 << 16) | init_1);
        let split = merged.pop_bits(src, 4)?;
        let mut s1 = RansState::new_with(merged.bits >> split);
        s1.maybe_refill(src)?;
        // State 2: high bits from merged, low bits from stream, then
        // mask off the bits that went into state1 and set the guard
        // bit at position (16 + split).
        let low = src.pop()? as u32;
        let mut s2_bits = (merged.bits << 16) | low;
        let guard = 1u32 << (16 + split);
        s2_bits &= guard - 1;
        s2_bits |= guard;
        let s2 = RansState::new_with(s2_bits);
        Ok((s1, s2))
    }

    /// Extract `nbits` bits from the current stream and swap s1/s2.
    /// Matches pybg3's free-function `pop_bits(nbits, state1, state2)`.
    #[inline]
    fn pop_bits(
        s1: &mut RansState,
        s2: &mut RansState,
        src: &mut WordStream<'_>,
        nbits: u32,
    ) -> Result<u32> {
        let result = s1.pop_bits(src, nbits)?;
        std::mem::swap(s1, s2);
        Ok(result)
    }

    /// Decode one symbol from the given model and swap s1/s2.
    #[inline]
    fn pop_model<T: CdfTable + Observable>(
        s1: &mut RansState,
        s2: &mut RansState,
        src: &mut WordStream<'_>,
        model: &mut T,
    ) -> Result<u32> {
        let result = s1.pop_cdf(src, model)?;
        model.observe(result);
        std::mem::swap(s1, s2);
        Ok(result)
    }
}

// Trait for "has an observe() method" — the decoder's `pop_model` uses
// it polymorphically without caring which table type.  We implement
// it on the three concrete tables via the macro below.
trait Observable {
    fn observe(&mut self, sym: u32);
}

impl Observable for CommandWordTable {
    fn observe(&mut self, sym: u32) {
        self.observe(sym)
    }
}
impl Observable for CacheRefTable {
    fn observe(&mut self, sym: u32) {
        self.observe(sym)
    }
}
impl Observable for CopyOffsetTable {
    fn observe(&mut self, sym: u32) {
        self.observe(sym)
    }
}
