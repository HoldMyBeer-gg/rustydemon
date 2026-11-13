//! Finding installed games without being told where they are.
//!
//! Each operating system has its own usual places: Battle.net's default
//! folders, Steam libraries, and, on Linux and macOS, the Windows prefixes
//! that Proton, Wine, Lutris, Bottles, CrossOver and Whisky keep, since a
//! Battle.net game on those systems lives inside one.
//!
//! [`places`] only names where to look and touches nothing, so it can be
//! checked for every system from any of them.  [`game_dirs`] does the looking.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }
}

/// Somewhere games may be, and how to look there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// A folder whose children may be games.
    Games(PathBuf),
    /// A Steam folder: its libraries hold games and Proton prefixes.
    Steam(PathBuf),
    /// One Windows prefix (a folder holding `drive_c`, or `pfx/drive_c`).
    Prefix(PathBuf),
    /// A folder whose children may be Windows prefixes.
    Prefixes(PathBuf),
    /// A folder whose children are mounted drives.
    Drives(PathBuf),
}

/// The usual places on `os` for a user whose home folder is `home`.
pub fn places(os: Os, home: Option<&Path>) -> Vec<Place> {
    use Place::*;
    let mut out = Vec::new();
    let at_home = |rel: &str| home.map(|h| h.join(rel));

    match os {
        Os::Windows => {
            for letter in 'C'..='Z' {
                let drive = PathBuf::from(format!("{letter}:/"));
                out.push(Games(drive.join("Program Files (x86)")));
                out.push(Games(drive.join("Program Files")));
                out.push(Games(drive.join("Games")));
                out.push(Games(drive.join("Battle.net Games")));
                out.push(Steam(drive.join("Program Files (x86)/Steam")));
                out.push(Steam(drive.join("SteamLibrary")));
            }
        }
        Os::Mac => {
            out.push(Games("/Applications".into()));
            out.extend(at_home("Applications").map(Games));
            out.extend(at_home("Games").map(Games));
            out.extend(at_home("Library/Application Support/Steam").map(Steam));
            out.extend(at_home("Library/Application Support/CrossOver/Bottles").map(Prefixes));
            out.extend(
                at_home("Library/Containers/com.isaacmarovitz.Whisky/Bottles").map(Prefixes),
            );
            out.push(Drives("/Volumes".into()));
        }
        Os::Linux => {
            out.extend(at_home(".local/share/Steam").map(Steam));
            out.extend(at_home(".steam/steam").map(Steam));
            out.extend(at_home(".var/app/com.valvesoftware.Steam/.local/share/Steam").map(Steam));
            out.extend(at_home(".wine").map(Prefix));
            // Lutris keeps one prefix per game here, and plain installs fit too.
            out.extend(at_home("Games").map(Prefixes));
            out.extend(at_home("Games").map(Games));
            out.extend(at_home("Games/Heroic/Prefixes").map(Prefixes));
            out.extend(at_home(".local/share/bottles/bottles").map(Prefixes));
            out.extend(
                at_home(".var/app/com.usebottles.bottles/data/bottles/bottles").map(Prefixes),
            );
            // Removable drives: the Steam Deck's card, and the per-user
            // folders desktop systems mount under.
            out.push(Drives("/run/media".into()));
            out.push(Drives("/mnt".into()));
            if let Some(user) = home.and_then(Path::file_name) {
                out.push(Drives(Path::new("/run/media").join(user)));
                out.push(Drives(Path::new("/media").join(user)));
            }
        }
    }
    out
}

/// The library folders listed in Steam's `libraryfolders.vdf`.
pub fn steam_libraries(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut quoted = line.split('"').skip(1).step_by(2);
            (quoted.next()? == "path").then(|| quoted.next()).flatten()
        })
        // The file doubles backslashes in Windows paths.
        .map(|p| PathBuf::from(p.replace("\\\\", "\\")))
        .collect()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Every folder under `places` that holds a `.build.info`, each once.
pub fn game_dirs(places: &[Place]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for place in places {
        look(place, &mut found);
    }
    // The same install is often reachable two ways (a symlinked Steam
    // folder, a library listed twice); keep the first spelling of each.
    let mut seen = std::collections::HashSet::new();
    found.retain(|p: &PathBuf| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
    found
}

fn look(place: &Place, found: &mut Vec<PathBuf>) {
    match place {
        Place::Games(dir) => found.extend(
            subdirs(dir)
                .into_iter()
                .filter(|p| p.join(".build.info").exists()),
        ),
        Place::Steam(root) => {
            let mut libraries = vec![root.clone()];
            for vdf in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
                if let Ok(text) = std::fs::read_to_string(root.join(vdf)) {
                    libraries.extend(steam_libraries(&text));
                }
            }
            for lib in libraries {
                look(&Place::Games(lib.join("steamapps/common")), found);
                look(&Place::Prefixes(lib.join("steamapps/compatdata")), found);
            }
        }
        Place::Prefix(prefix) => {
            for drive_c in [prefix.join("drive_c"), prefix.join("pfx/drive_c")] {
                for folder in ["Program Files (x86)", "Program Files", "Games"] {
                    look(&Place::Games(drive_c.join(folder)), found);
                }
            }
        }
        Place::Prefixes(dir) => {
            for prefix in subdirs(dir) {
                look(&Place::Prefix(prefix), found);
            }
        }
        Place::Drives(dir) => {
            for drive in subdirs(dir) {
                look(&Place::Games(drive.join("Games")), found);
                look(&Place::Games(drive.join("Applications")), found);
                look(&Place::Steam(drive.clone()), found);
                look(&Place::Steam(drive.join("SteamLibrary")), found);
            }
        }
    }
}

/// Which launcher an install at `path` came from, for the menu label.
pub fn launcher(path: &Path, product: &str) -> &'static str {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let has = |name: &str| parts.iter().any(|p| p == name);
    let ptr = product.contains("test") || path.to_string_lossy().contains("Public Test");
    if has("common") && has("steamapps") {
        "Steam"
    } else if ptr {
        "Battle.net PTR"
    } else if has("drive_c") {
        // A Windows install inside a Proton, Wine or CrossOver prefix.
        "Battle.net, Wine"
    } else {
        "Battle.net"
    }
}

/// Known product codes and the names shown for them.  Some installs
/// (notably the Mac Battle.net Diablo III) ship with an empty `.build.info`
/// Product column, so the code is inferred from the CDN path (`tpr/diablo3`);
/// those longer spellings are listed alongside the short ones.
const PRODUCT_NAMES: &[(&str, &str)] = &[
    ("fenris", "Diablo IV"),
    ("wow", "World of Warcraft"),
    ("d3", "Diablo III"),
    ("diablo3", "Diablo III"),
    ("hero", "Heroes of the Storm"),
    ("pro", "Overwatch"),
    ("osi", "Diablo II: Resurrected"),
    ("w3", "Warcraft III: Reforged"),
    ("s1", "StarCraft"),
    ("sc1", "StarCraft: Remastered"),
    ("s2", "StarCraft II"),
    ("hs", "Hearthstone"),
    ("viper", "Call of Duty"),
    ("odin", "Call of Duty"),
    ("lazr", "Crash Bandicoot 4"),
    ("fore", "Spyro"),
    ("zeus", "Call of Duty: MW"),
    ("gryphon", "Call of Duty: BO6"),
];

/// A search for installs running on its own thread.  Looking can stall for
/// many seconds (a mapped network drive whose server is away), and the menu
/// that shows the result must keep drawing meanwhile.
#[derive(Clone)]
pub struct Search(std::sync::Arc<std::sync::OnceLock<Vec<(String, PathBuf)>>>);

impl Search {
    /// Run `find` on a new thread, then call `done` (to ask for a repaint).
    pub fn start(
        find: impl FnOnce() -> Vec<(String, PathBuf)> + Send + 'static,
        done: impl FnOnce() + Send + 'static,
    ) -> Self {
        let search = Search(Default::default());
        let slot = search.0.clone();
        std::thread::spawn(move || {
            let _ = slot.set(find());
            done();
        });
        search
    }

    /// What was found, or `None` while still looking.
    pub fn found(&self) -> Option<&[(String, PathBuf)]> {
        self.0.get().map(Vec::as_slice)
    }
}

/// Stop Windows from putting up a "There is no disk in the drive" box when
/// an empty card reader or optical drive is probed; the probe just fails.
#[cfg(windows)]
fn quiet_missing_media() {
    const SEM_FAILCRITICALERRORS: u32 = 0x0001;
    #[link(name = "kernel32")]
    extern "system" {
        fn SetErrorMode(mode: u32) -> u32;
    }
    // SAFETY: takes and returns a plain flag word; no pointers involved.
    unsafe {
        let old = SetErrorMode(SEM_FAILCRITICALERRORS);
        SetErrorMode(old | SEM_FAILCRITICALERRORS);
    }
}

#[cfg(not(windows))]
fn quiet_missing_media() {}

/// Installed games in the usual places for this system, as menu label and
/// folder, sorted by label.
pub fn detect() -> Vec<(String, PathBuf)> {
    quiet_missing_media();
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    labelled(game_dirs(&places(Os::current(), home.as_deref())))
}

fn labelled(dirs: Vec<PathBuf>) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for path in dirs {
        let products = rustydemon_lib::CascConfig::detect_products(&path);
        let dir_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if products.is_empty() {
            found.push((format!("{dir_name} ({})", path.display()), path));
            continue;
        }
        for product in &products {
            let game = PRODUCT_NAMES
                .iter()
                .find(|(uid, _)| product.starts_with(uid))
                .map_or(dir_name.as_str(), |(_, name)| name);
            found.push((
                format!("{game} [{}]", launcher(&path, product)),
                path.clone(),
            ));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.1 == b.1);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh empty folder under the system temp folder, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("rustydemon-installs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        /// Make `rel` a game folder (one holding `.build.info`).
        fn game(&self, rel: &str) -> PathBuf {
            let dir = self.0.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".build.info"), "").unwrap();
            dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn has(places: &[Place], want: Place) -> bool {
        places.contains(&want)
    }

    #[test]
    fn windows_looks_on_every_drive_not_only_c() {
        let p = places(Os::Windows, None);
        assert!(has(&p, Place::Games("C:/Program Files (x86)".into())));
        assert!(has(&p, Place::Games("F:/Games".into())));
        assert!(has(&p, Place::Steam("G:/SteamLibrary".into())));
    }

    #[test]
    fn macos_looks_in_applications_steam_bottles_and_external_drives() {
        let home = Path::new("/Users/crab");
        let p = places(Os::Mac, Some(home));
        assert!(has(&p, Place::Games("/Applications".into())));
        assert!(has(&p, Place::Games("/Users/crab/Applications".into())));
        assert!(has(
            &p,
            Place::Steam("/Users/crab/Library/Application Support/Steam".into())
        ));
        assert!(has(
            &p,
            Place::Prefixes("/Users/crab/Library/Application Support/CrossOver/Bottles".into())
        ));
        assert!(has(&p, Place::Drives("/Volumes".into())));
    }

    #[test]
    fn linux_looks_in_steam_wine_lutris_and_mounted_drives() {
        let home = Path::new("/home/crab");
        let p = places(Os::Linux, Some(home));
        assert!(has(
            &p,
            Place::Steam("/home/crab/.local/share/Steam".into())
        ));
        assert!(has(&p, Place::Prefix("/home/crab/.wine".into())));
        assert!(has(&p, Place::Prefixes("/home/crab/Games".into())));
        // The Deck mounts its card under the user's name.
        assert!(has(&p, Place::Drives("/run/media/crab".into())));
        assert!(has(&p, Place::Drives("/media/crab".into())));
    }

    #[test]
    fn no_system_is_given_another_systems_places() {
        let home = Path::new("/home/crab");
        let text = |os| format!("{:?}", places(os, Some(home)));
        assert!(!text(Os::Linux).contains("Program Files"));
        assert!(!text(Os::Linux).contains("/Applications"));
        assert!(!text(Os::Mac).contains("C:/"));
        assert!(!text(Os::Windows).contains("/Applications"));
    }

    #[test]
    fn reads_library_folders_from_steams_list() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/crab/.local/share/Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            steam_libraries(vdf),
            [
                PathBuf::from("/home/crab/.local/share/Steam"),
                PathBuf::from("D:\\SteamLibrary"),
            ]
        );
    }

    #[test]
    fn finds_a_plain_install() {
        let s = Scratch::new("plain");
        let game = s.game("Applications/World of Warcraft");
        std::fs::create_dir_all(s.0.join("Applications/Not A Game")).unwrap();
        let found = game_dirs(&[Place::Games(s.0.join("Applications"))]);
        assert_eq!(found, [game]);
    }

    /// A Battle.net game installed through Steam as a non-Steam game lives
    /// in a Proton prefix, which nothing looked in before.
    #[test]
    fn finds_a_battle_net_game_inside_a_proton_prefix() {
        let s = Scratch::new("proton");
        let game = s.game(
            "Steam/steamapps/compatdata/4206469918/pfx/drive_c/Program Files (x86)/World of Warcraft",
        );
        let found = game_dirs(&[Place::Steam(s.0.join("Steam"))]);
        assert_eq!(found, [game]);
    }

    #[test]
    fn finds_games_in_every_steam_library() {
        let s = Scratch::new("libs");
        let near = s.game("Steam/steamapps/common/Diablo IV");
        let far = s.game("card/steamapps/common/Overwatch");
        std::fs::write(
            s.0.join("Steam/steamapps/libraryfolders.vdf"),
            format!("\t\t\"path\"\t\t\"{}\"\n", s.0.join("card").display()),
        )
        .unwrap();
        let found = game_dirs(&[Place::Steam(s.0.join("Steam"))]);
        assert_eq!(found, [near, far]);
    }

    #[test]
    fn finds_games_in_wine_prefixes_and_on_mounted_drives() {
        let s = Scratch::new("wine");
        let lutris = s.game("Games/battlenet/drive_c/Program Files (x86)/Hearthstone");
        let card = s.game("media/sdcard/steamapps/common/Diablo IV");
        let external = s.game("media/External/Games/StarCraft");
        let found = game_dirs(&[
            Place::Prefixes(s.0.join("Games")),
            Place::Drives(s.0.join("media")),
        ]);
        assert!(found.contains(&lutris), "{found:?}");
        assert!(found.contains(&card), "{found:?}");
        assert!(found.contains(&external), "{found:?}");
        assert_eq!(found.len(), 3, "{found:?}");
    }

    #[test]
    fn an_install_reachable_two_ways_is_listed_once() {
        let s = Scratch::new("twice");
        s.game("Steam/steamapps/common/Diablo IV");
        let steam = Place::Steam(s.0.join("Steam"));
        assert_eq!(game_dirs(&[steam.clone(), steam]).len(), 1);
    }

    #[test]
    fn labels_say_where_an_install_came_from() {
        let steam = Path::new("/home/crab/.local/share/Steam/steamapps/common/Diablo IV");
        assert_eq!(launcher(steam, "fenris"), "Steam");
        let proton = Path::new(
            "/home/crab/.local/share/Steam/steamapps/compatdata/1/pfx/drive_c/Program Files (x86)/World of Warcraft",
        );
        assert_eq!(launcher(proton, "wow"), "Battle.net, Wine");
        let mac = Path::new("/Applications/World of Warcraft");
        assert_eq!(launcher(mac, "wow"), "Battle.net");
        assert_eq!(launcher(mac, "wowt"), "Battle.net");
        assert_eq!(launcher(mac, "wow_test"), "Battle.net PTR");
    }

    /// The search can stall for many seconds (a mapped network drive whose
    /// server is away), so the menu must be able to draw while it runs.
    #[test]
    fn a_search_reports_nothing_until_it_finishes() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let (told, done) = std::sync::mpsc::channel::<()>();
        let search = Search::start(
            move || {
                wait.recv().unwrap();
                vec![("Game".to_owned(), PathBuf::from("/games/game"))]
            },
            move || told.send(()).unwrap(),
        );
        assert!(search.found().is_none());
        release.send(()).unwrap();
        done.recv().unwrap();
        let found = search.found().expect("finished");
        assert_eq!(found, [("Game".to_owned(), PathBuf::from("/games/game"))]);
    }
}
