//! # rustydemon-lib
//!
//! A Rust library for reading CASC (Content-Addressable Storage Container)
//! archives used by several popular games.
//!
//! ## Quick start — local installation
//!
//! ```no_run
//! use rustydemon_lib::{CascHandler, LocaleFlags, SearchQuery};
//!
//! // Open a local game installation.
//! let mut casc = CascHandler::open_local("/path/to/game", "wow")?;
//! casc.set_locale(LocaleFlags::EN_US);
//!
//! // Load a community listfile (optional — needed for filename-based access).
//! let listfile = std::fs::read_to_string("community-listfile.csv")?;
//! casc.load_listfile(&listfile);
//!
//! // Open a file by path.
//! let bytes = casc.open_file_by_name(
//!     "Interface/Glues/Models/UI_MainMenu/UI_MainMenu.m2"
//! )?;
//! println!("File is {} bytes", bytes.len());
//!
//! // Global search — all keys, not just the active folder.
//! let results = casc.search(
//!     SearchQuery::new().filename(".blp").limit(50)
//! );
//! for r in results {
//!     println!("{:016X}  {:?}", r.hash, r.filename);
//! }
//! # Ok::<(), rustydemon_lib::CascError>(())
//! ```
//!
//! ## Architecture
//!
//! The lookup chain for a named file is:
//!
//! ```text
//! filename
//!   → Jenkins96 hash
//!     → RootHandler (root manifest)
//!       → CKey (content key)
//!         → EncodingHandler (encoding file)
//!           → EKey (encoding key)
//!             → LocalIndexHandler (*.idx files)
//!               → (archive index, offset, size)
//!                 → data.NNN file
//!                   → BLTE decode
//!                     → raw file bytes
//! ```
//!
//! ## Global search
//!
//! [`CascHandler::search`] iterates *every* entry in the root manifest,
//! matching the regedit behaviour of searching all keys and values rather than
//! only those in the currently selected folder.  Use [`SearchQuery`] to filter
//! by filename substring, hash prefix, content key, locale, or content flags.

// ── Public modules ─────────────────────────────────────────────────────────────

pub mod archive_index;
pub mod blte;
#[cfg(feature = "cdn")]
pub mod cdn;
pub mod config;
pub mod encoding;
pub mod entry;
pub mod error;
pub mod game;
pub mod handler;
pub mod jenkins96;
pub mod key_service;
pub mod local_index;
pub mod query;
pub mod root;
pub mod salsa20;
pub mod search;
pub mod static_container;

mod types;

// ── Convenience re-exports ─────────────────────────────────────────────────────

pub use config::CascConfig;
pub use entry::{CascFile, CascFolder};
pub use error::CascError;
pub use game::GameType;
pub use handler::{prepare_listfile, CascHandler, PreparedLoad};
pub use jenkins96::jenkins96;
pub use query::PathQuery;
pub use search::{SearchQuery, SearchResult};
pub use types::{ContentFlags, EKey9, EncodingEntry, IndexEntry, LocaleFlags, Md5Hash, RootEntry};
