//! Command-line arguments for the GUI.
//!
//! `rustydemon [GAME_DIR] [--listfile FILE]`
//!
//! Both are optional.  `--listfile` loads a community listfile as soon as an
//! archive has opened, so a file tree can be shown without going through a
//! file dialog (useful for shortcuts, scripts and UI automation).

use std::path::PathBuf;

/// What the user asked for on the command line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchArgs {
    /// Game directory to open on launch.
    pub game_dir: Option<PathBuf>,
    /// Listfile to apply once an archive is open.
    pub listfile: Option<PathBuf>,
    /// A usage problem to show in the status bar, if any.
    pub problem: Option<String>,
}

/// Parse the arguments that follow the program name.
pub fn parse<I, S>(args: I) -> LaunchArgs
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    const FLAG: &str = "--listfile";
    let mut out = LaunchArgs::default();
    let mut args = args.into_iter().map(Into::into);

    while let Some(arg) = args.next() {
        if arg == FLAG {
            match args.next() {
                Some(value) if !value.is_empty() => out.listfile = Some(PathBuf::from(value)),
                _ => out.problem = Some(format!("{FLAG} needs a file path")),
            }
        } else if let Some(value) = arg.strip_prefix("--listfile=") {
            if value.is_empty() {
                out.problem = Some(format!("{FLAG} needs a file path"));
            } else {
                out.listfile = Some(PathBuf::from(value));
            }
        } else if arg.starts_with('-') {
            // Unknown flag: ignore (see `unknown_flags_are_ignored`).
        } else if out.game_dir.is_none() {
            out.game_dir = Some(PathBuf::from(arg));
        }
    }
    out
}

/// Make a command-line path absolute for display and later use.
///
/// Uses [`std::path::absolute`] and not `canonicalize`: on Windows the
/// latter returns verbatim `\\?\C:\...` paths, which end up in the status
/// bar, and it fails for paths that do not exist.  Symlinks are left as
/// the user typed them.
pub fn absolute(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_resolves_dot_to_the_current_directory() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute(PathBuf::from(".")), cwd);
    }

    #[test]
    fn absolute_leaves_absolute_paths_alone() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute(cwd.clone()), cwd);
    }

    /// A path that does not exist yet must still come back absolute, so the
    /// status bar never shows a bare relative name.
    #[test]
    fn absolute_handles_paths_that_do_not_exist() {
        let got = absolute(PathBuf::from("no-such-dir/list.csv"));
        assert!(got.is_absolute(), "{got:?}");
        assert!(got.ends_with("no-such-dir/list.csv"), "{got:?}");
    }

    /// `std::fs::canonicalize` returns verbatim `\\?\C:\...` paths on
    /// Windows, which then show up in the status bar.
    #[cfg(windows)]
    #[test]
    fn absolute_has_no_verbatim_prefix_on_windows() {
        let got = absolute(std::env::current_dir().unwrap());
        assert!(!got.to_string_lossy().starts_with(r"\\?\"), "{got:?}");
    }

    fn p(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    #[test]
    fn no_arguments() {
        assert_eq!(parse(Vec::<String>::new()), LaunchArgs::default());
    }

    #[test]
    fn game_dir_only() {
        let a = parse(["/games/wow"]);
        assert_eq!(a.game_dir, p("/games/wow"));
        assert_eq!(a.listfile, None);
        assert_eq!(a.problem, None);
    }

    #[test]
    fn game_dir_then_listfile() {
        let a = parse(["/games/wow", "--listfile", "/tmp/list.csv"]);
        assert_eq!(a.game_dir, p("/games/wow"));
        assert_eq!(a.listfile, p("/tmp/list.csv"));
        assert_eq!(a.problem, None);
    }

    #[test]
    fn listfile_before_game_dir() {
        let a = parse(["--listfile", "/tmp/list.csv", "/games/wow"]);
        assert_eq!(a.game_dir, p("/games/wow"));
        assert_eq!(a.listfile, p("/tmp/list.csv"));
    }

    #[test]
    fn listfile_with_equals() {
        let a = parse(["--listfile=/tmp/list.csv", "/games/wow"]);
        assert_eq!(a.game_dir, p("/games/wow"));
        assert_eq!(a.listfile, p("/tmp/list.csv"));
    }

    /// A Windows path with spaces arrives as one argument and must stay whole.
    #[test]
    fn paths_with_spaces_and_backslashes() {
        let a = parse([
            r"C:\Program Files (x86)\World of Warcraft",
            "--listfile",
            r"C:\Users\me\My Lists\verified-listfile.csv",
        ]);
        assert_eq!(a.game_dir, p(r"C:\Program Files (x86)\World of Warcraft"));
        assert_eq!(a.listfile, p(r"C:\Users\me\My Lists\verified-listfile.csv"));
    }

    #[test]
    fn listfile_without_a_value_is_reported() {
        let a = parse(["/games/wow", "--listfile"]);
        assert_eq!(a.game_dir, p("/games/wow"));
        assert_eq!(a.listfile, None);
        assert!(a.problem.as_deref().unwrap().contains("--listfile"));

        let b = parse(["--listfile="]);
        assert_eq!(b.listfile, None);
        assert!(b.problem.is_some());
    }

    /// Unknown flags were silently ignored before this module existed, and
    /// platform launchers add their own (macOS passes `-psn_…` to apps
    /// started from Finder).  Keep ignoring them.
    #[test]
    fn unknown_flags_are_ignored() {
        let a = parse(["-psn_0_12345", "--verbose", "/games/wow"]);
        assert_eq!(a.game_dir, p("/games/wow"));
        assert_eq!(a.problem, None);
    }

    #[test]
    fn only_the_first_game_dir_counts() {
        let a = parse(["/games/wow", "/games/d4"]);
        assert_eq!(a.game_dir, p("/games/wow"));
    }
}
