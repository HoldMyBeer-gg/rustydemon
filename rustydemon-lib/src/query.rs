//! Unified path query language shared by the CLI and the GUI search bar.
//!
//! A [`PathQuery`] accepts one of three auto-detected forms:
//!
//! | Form | Example | Meaning |
//! |------|---------|---------|
//! | Glob | `"sylvanas*.wmo"` | Glob against full virtual paths.  `**/` is auto-prepended unless the pattern already starts with `/` or `**/`, so a bare filename pattern matches anywhere in the tree. |
//! | Folder | `"base/meta/Sound"` | Literal directory, resolved recursively. |
//! | File | `"Interface/Icons/INV_Sword_04.blp"` | Literal single file, case-insensitive. |
//!
//! The query auto-detects glob vs literal by scanning for glob metacharacters
//! (`*`, `?`, `{`, `[`).  Construct with [`PathQuery::parse`] and resolve
//! against a [`CascFolder`] with [`PathQuery::resolve`].

use globset::{GlobBuilder, GlobMatcher};

use crate::{
    entry::{CascFile, CascFolder},
    error::CascError,
};

/// A parsed path query that can be resolved against a virtual tree.
#[derive(Debug)]
pub enum PathQuery {
    /// A compiled glob matcher applied to full virtual paths.
    Glob {
        /// The pattern actually compiled (after `**/` auto-anchoring).
        pattern: String,
        matcher: GlobMatcher,
    },
    /// A literal folder path, exported recursively.
    Folder(String),
    /// A literal single-file path, case-insensitive.
    File(String),
}

impl PathQuery {
    /// Parse a query string, auto-detecting its form.
    ///
    /// Returns [`CascError::InvalidData`] if the input looks like a glob
    /// but isn't a valid one (e.g. unbalanced brackets).
    pub fn parse(query: &str) -> Result<Self, CascError> {
        if is_glob(query) {
            let pattern = anchor_pattern(query);
            // Case-insensitive matching so users can type `interface/icons/
            // inv_sword_0*.blp` and still hit entries that the community
            // listfile stores as `Interface/ICONS/INV_Sword_04.blp`.  The
            // rest of the lib (CascFolder navigation, literal-path
            // fallback) is already case-insensitive; this keeps glob
            // matching consistent with that behaviour.
            let matcher = GlobBuilder::new(&pattern)
                .case_insensitive(true)
                .build()
                .map_err(|e| CascError::InvalidData(format!("invalid glob '{query}': {e}")))?
                .compile_matcher();
            Ok(PathQuery::Glob { pattern, matcher })
        } else {
            // We can't disambiguate folder vs file without the tree, so we
            // default to Folder and fall through to File in `resolve` when
            // the folder lookup fails.
            Ok(PathQuery::Folder(query.to_owned()))
        }
    }

    /// Parse a query and resolve it against `tree` in one step.
    ///
    /// Returns an empty `Vec` when the query is valid but matches nothing.
    /// Returns an error only when the query itself is malformed or when
    /// a literal path doesn't exist in the tree.
    pub fn run(query: &str, tree: &CascFolder) -> Result<Vec<CascFile>, CascError> {
        PathQuery::parse(query)?.resolve(tree)
    }

    /// Resolve this query against a virtual tree.
    pub fn resolve(&self, tree: &CascFolder) -> Result<Vec<CascFile>, CascError> {
        match self {
            PathQuery::Glob { matcher, .. } => Ok(tree
                .walk_files()
                .filter(|f| matcher.is_match(&f.full_path))
                .cloned()
                .collect()),
            PathQuery::Folder(path) => {
                if let Some(folder) = tree.navigate(path) {
                    return Ok(folder.walk_files().cloned().collect());
                }
                // Fall through to file lookup.
                let lower = path.to_lowercase();
                let matched: Vec<CascFile> = tree
                    .walk_files()
                    .filter(|f| f.full_path.to_lowercase() == lower)
                    .cloned()
                    .collect();
                if matched.is_empty() {
                    Err(CascError::FileNotFound(path.clone()))
                } else {
                    Ok(matched)
                }
            }
            PathQuery::File(path) => {
                let lower = path.to_lowercase();
                let matched: Vec<CascFile> = tree
                    .walk_files()
                    .filter(|f| f.full_path.to_lowercase() == lower)
                    .cloned()
                    .collect();
                if matched.is_empty() {
                    Err(CascError::FileNotFound(path.clone()))
                } else {
                    Ok(matched)
                }
            }
        }
    }

    /// `true` if this query uses glob matching (vs literal folder/file).
    pub fn is_glob(&self) -> bool {
        matches!(self, PathQuery::Glob { .. })
    }
}

/// Heuristic: does the query string contain any glob metacharacters?
fn is_glob(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'*' | b'?' | b'{' | b'['))
}

/// Prepend `**/` unless the pattern already anchors itself from the root.
///
/// This is what makes `"sylvanas*.wmo"` match anywhere in the tree while
/// still letting `"/textures/*.tex"` or `"**/cinematics/*.vid"` stay
/// root-anchored.
fn anchor_pattern(query: &str) -> String {
    if query.starts_with('/') {
        query.trim_start_matches('/').to_owned()
    } else if query.starts_with("**/") {
        query.to_owned()
    } else {
        format!("**/{query}")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{build_tree, CascFolder};

    fn tree() -> CascFolder {
        build_tree([
            (1, "base/meta/Sound/foo.snd".into(), None),
            (2, "base/meta/Sound/bar.snd".into(), None),
            (3, "base/meta/Music/themesong.mus".into(), None),
            (
                4,
                "base/meta/Power/Necromancer_SkeletonWarrior.pow".into(),
                None,
            ),
            (
                5,
                "base/meta/Power/Necromancer_ArmyoftheDead.pow".into(),
                None,
            ),
            (
                6,
                "base/meta/Power/Barbarian_WhirlwindOfDeath.pow".into(),
                None,
            ),
            (7, "Interface/Icons/INV_Sword_04.blp".into(), None),
        ])
    }

    #[test]
    fn folder_query_recurses() {
        let q = PathQuery::parse("base/meta/Sound").unwrap();
        let hits = q.resolve(&tree()).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn glob_anywhere() {
        let q = PathQuery::parse("*Necromancer*.pow").unwrap();
        assert!(q.is_glob());
        let hits = q.resolve(&tree()).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn glob_path_anchored() {
        let q = PathQuery::parse("base/meta/Power/*.pow").unwrap();
        let hits = q.resolve(&tree()).unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn glob_doublestar_explicit() {
        let q = PathQuery::parse("**/*.blp").unwrap();
        let hits = q.resolve(&tree()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].full_path, "Interface/Icons/INV_Sword_04.blp");
    }

    #[test]
    fn literal_file_falls_through_from_folder() {
        // "Interface/Icons/INV_Sword_04.blp" isn't a folder — the Folder
        // variant must fall through to a file lookup automatically.
        let q = PathQuery::parse("Interface/Icons/INV_Sword_04.blp").unwrap();
        let hits = q.resolve(&tree()).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn unknown_literal_errors() {
        let q = PathQuery::parse("does/not/exist").unwrap();
        assert!(q.resolve(&tree()).is_err());
    }

    #[test]
    fn empty_glob_is_not_error() {
        // Valid glob that simply matches nothing should return an empty
        // vec, not an error — callers like the GUI need to display
        // "0 results" without treating it as a parse failure.
        let q = PathQuery::parse("*.nonexistent").unwrap();
        let hits = q.resolve(&tree()).unwrap();
        assert_eq!(hits.len(), 0);
    }
}
