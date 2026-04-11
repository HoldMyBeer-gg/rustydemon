use std::fmt;

// ── MD5 hash ───────────────────────────────────────────────────────────────────

/// A 16-byte content key (CKey) or encoding key (EKey).
///
/// The bytes are stored exactly as they appear on disk. Equality comparisons
/// can be done over all 16 bytes (`==`) or just the 9-byte prefix used by
/// CASC index files ([`Md5Hash::eq9`]).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Md5Hash(pub [u8; 16]);

impl Md5Hash {
    /// Construct from a raw byte array.
    #[inline]
    pub fn from_bytes(b: [u8; 16]) -> Self { Self(b) }

    /// Parse from a 32-character hexadecimal string (case-insensitive).
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 32 { return None; }
        let mut out = [0u8; 16];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hi = hex_nibble(chunk[0])?;
            let lo = hex_nibble(chunk[1])?;
            out[i] = (hi << 4) | lo;
        }
        Some(Self(out))
    }

    /// Format as an uppercase hex string.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// 9-byte prefix equality, matching the CASC index lookup convention.
    ///
    /// Index files store only the first 9 bytes of an encoding key; lookups
    /// must therefore compare just those 9 bytes.
    #[inline]
    pub fn eq9(&self, other: &Md5Hash) -> bool {
        self.0[..9] == other.0[..9]
    }

    /// Borrow the underlying 16 bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 16] { &self.0 }

    /// Returns `true` when all bytes are zero (the default/uninitialised value).
    #[inline]
    pub fn is_zero(&self) -> bool { self.0 == [0u8; 16] }
}

impl fmt::Debug for Md5Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Md5Hash({})", self.to_hex())
    }
}

impl fmt::Display for Md5Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ── 9-byte eKey wrapper (used as HashMap key in index tables) ─────────────────

/// The first 9 bytes of an encoding key, used as a compact map key in all
/// CASC index tables.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EKey9(pub [u8; 9]);

impl EKey9 {
    /// Extract the 9-byte prefix from a full [`Md5Hash`].
    #[inline]
    pub fn from_full(h: &Md5Hash) -> Self {
        let mut buf = [0u8; 9];
        buf.copy_from_slice(&h.0[..9]);
        Self(buf)
    }
}

// ── Index entry ───────────────────────────────────────────────────────────────

/// Location of a BLTE-encoded data block within local data archives.
#[derive(Clone, Copy, Default, Debug)]
pub struct IndexEntry {
    /// Archive file number (`data.NNN`).
    pub index: u32,
    /// Byte offset within the archive file.
    pub offset: u32,
    /// Total size of the stored block (including the 30-byte header).
    pub size: u32,
}

// ── Locale / content flags ────────────────────────────────────────────────────

bitflags::bitflags! {
    /// Per-file locale bitmask stored in the root manifest.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct LocaleFlags: u32 {
        const NONE   = 0x00000000;
        const UNK1   = 0x00000001;
        const EN_US  = 0x00000002;
        const KO_KR  = 0x00000004;
        const UNK8   = 0x00000008;
        const FR_FR  = 0x00000010;
        const DE_DE  = 0x00000020;
        const ZH_CN  = 0x00000040;
        const ES_ES  = 0x00000080;
        const ZH_TW  = 0x00000100;
        const EN_GB  = 0x00000200;
        const EN_CN  = 0x00000400;
        const EN_TW  = 0x00000800;
        const ES_MX  = 0x00001000;
        const RU_RU  = 0x00002000;
        const PT_BR  = 0x00004000;
        const IT_IT  = 0x00008000;
        const PT_PT  = 0x00010000;
        /// All retail WoW locales.
        const ALL_WOW = Self::EN_US.bits() | Self::KO_KR.bits() | Self::FR_FR.bits()
                      | Self::DE_DE.bits() | Self::ZH_CN.bits() | Self::ES_ES.bits()
                      | Self::ZH_TW.bits() | Self::EN_GB.bits() | Self::ES_MX.bits()
                      | Self::RU_RU.bits() | Self::PT_BR.bits() | Self::IT_IT.bits()
                      | Self::PT_PT.bits();
        const ALL     = 0xFFFFFFFF;
    }
}

bitflags::bitflags! {
    /// Per-file content flags stored in the root manifest.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ContentFlags: u32 {
        const NONE              = 0x00000000;
        const HIGH_RES_TEXTURE  = 0x00000001;
        const WINDOWS           = 0x00000008;
        const MACOS             = 0x00000010;
        const ALTERNATE         = 0x00000080;
        const ENCRYPTED         = 0x08000000;
        const NO_NAME_HASH      = 0x10000000;
        const NOT_COMPRESSED    = 0x80000000;
    }
}

// ── Root entry ────────────────────────────────────────────────────────────────

/// One entry from the root manifest: maps a filename hash to a content key.
#[derive(Clone, Copy, Debug)]
pub struct RootEntry {
    /// Content key (MD5 of the raw file data).
    pub ckey: Md5Hash,
    pub locale: LocaleFlags,
    pub content: ContentFlags,
}

// ── Encoding entry ────────────────────────────────────────────────────────────

/// Maps a content key to one or more encoding keys and the logical file size.
#[derive(Clone, Debug)]
pub struct EncodingEntry {
    /// Encoding keys for this content (usually exactly one).
    pub ekeys: Vec<Md5Hash>,
    /// Decoded file size in bytes.
    pub size: u64,
}
