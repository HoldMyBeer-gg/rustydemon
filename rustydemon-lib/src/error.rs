use thiserror::Error;

/// All errors that can originate from `rustydemon-lib`.
#[derive(Debug, Error)]
pub enum CascError {
    /// Underlying I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A file was looked up by name or hash but not found in the root manifest.
    #[error("file not found: {0}")]
    FileNotFound(String),

    /// The encoding table has no entry for the given content key.
    #[error("encoding entry not found for ckey {0}")]
    EncodingNotFound(String),

    /// Neither the local nor CDN index contains the given encoding key.
    #[error("index entry not found for ekey {0}")]
    IndexNotFound(String),

    /// The BLTE block is encrypted and we don't have the key.
    #[error("missing decryption key {0:016X}")]
    MissingKey(u64),

    /// Anything that goes wrong while decoding a BLTE stream.
    #[error("BLTE decode error: {0}")]
    Blte(String),

    /// Binary data didn't match the expected structure.
    #[error("invalid data: {0}")]
    InvalidData(String),

    /// MD5 integrity check failed.
    #[error("hash mismatch: {0}")]
    HashMismatch(String),

    /// A config file (build info, build config, CDN config) was malformed.
    #[error("config error: {0}")]
    Config(String),

    /// A product UID wasn't recognised as any known game type.
    #[error("unknown game product UID: {0}")]
    UnknownGame(String),

    /// Arithmetic overflow detected while computing offsets/sizes.
    #[error("overflow computing {0}")]
    Overflow(&'static str),
}
