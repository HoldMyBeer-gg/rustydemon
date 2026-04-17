//! Rusty Demon CLI — headless batch exporter and file inspector for CASC
//! archives.
//!
//! Two subcommands:
//!
//! - **`export`** — walk a CASC installation, match virtual paths, and
//!   write files to a host directory (the original `rustydemon-cli`
//!   behavior).
//! - **`inspect`** — read a local file from disk and print a
//!   format-specific text summary (Granny3D, M2, WMO, BLP, D2R
//!   `.texture`).  Useful for quick validation without launching the GUI.
//!
//! ## Examples
//!
//! ```text
//! rustydemon-cli export \
//!     --archive "/home/deck/.steam/steam/steamapps/common/Diablo IV" \
//!     --path base/meta/Sound \
//!     --output ./out
//!
//! rustydemon-cli inspect ~/Downloads/LadySylvanasWindrunner.m2
//! ```

mod inspect;
pub mod render;

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use rustydemon_lib::{CascConfig, CascFile, CascHandler, GameType, PathQuery};

/// Rusty Demon CLI — batch export and inspect CASC archive files.
#[derive(Debug, Parser)]
#[command(name = "rustydemon-cli", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Batch export files from a CASC archive.
    Export(ExportArgs),
    /// Inspect a local file and print format-specific metadata.
    Inspect(inspect::InspectArgs),
    /// Smoke-test a CASC archive: open it, read one file, report PASS/FAIL.
    ///
    /// Useful for regression testing across games and launchers.
    /// Exits 0 if the archive opens and at least one file can be extracted,
    /// 1 on any failure.
    Probe(ProbeArgs),
}

/// Arguments for the `export` subcommand.
#[derive(Debug, Parser)]
struct ExportArgs {
    /// Game installation root (the directory that contains `.build.info`
    /// for Battle.net installs, or `Data/.build.config` for Steam installs).
    #[arg(long, short = 'a')]
    archive: PathBuf,

    /// What to export.  Three forms, auto-detected:
    ///
    /// 1. Literal folder, exported recursively: `World/Maps/Azeroth`
    /// 2. Literal file, exported alone: `Interface/Icons/INV_Sword_04.blp`
    /// 3. Glob (contains `*`, `?`, or `{...}`) matched against full virtual
    ///    paths.  `**/` is auto-prepended unless the pattern already starts
    ///    with `/` or `**/`.  Examples:
    ///    `"sylvanas*.wmo"` (anywhere),
    ///    `"textures/*.tex"` (any textures dir),
    ///    `"**/cinematics/*.vid"` (literal anchor).
    ///
    /// For WoW archives you must also pass `--listfile` so path resolution
    /// has something to work with, or use `--fdid` instead.
    #[arg(long, short = 'p', required_unless_present = "fdid")]
    path: Option<String>,

    /// Host directory to write files into.  Will be created if missing.
    #[arg(long, short = 'o')]
    output: PathBuf,

    /// Product UID (e.g. `fenris`, `wow`).  Auto-detected from `.build.info`
    /// if omitted.
    #[arg(long)]
    product: Option<String>,

    /// Community listfile (CSV or plain text).  Required for WoW and any
    /// other game whose root manifest doesn't carry built-in path names —
    /// without it, `--path` has no tree to resolve against.  D4 and other
    /// TVFS-based archives ignore this because they self-describe.
    ///
    /// Download from https://github.com/wowdev/wow-listfile for WoW.
    #[arg(long, short = 'l')]
    listfile: Option<PathBuf>,

    /// Export a single file by FileDataID instead of `--path`.  Mutually
    /// exclusive with `--path`; lets you extract a known file from a WoW
    /// archive without loading a listfile.
    #[arg(long, conflicts_with = "path")]
    fdid: Option<u32>,

    /// Flatten output: drop all files directly into `--output` instead of
    /// mirroring the virtual directory structure.
    #[arg(long)]
    flat: bool,

    /// Number of worker threads.  Defaults to the number of CPU cores.
    #[arg(long, short = 'j')]
    parallel: Option<usize>,

    /// Print the match list and exit without writing anything.
    #[arg(long)]
    dry_run: bool,

    /// Overwrite existing files in the output directory.  Without this
    /// flag, existing files are skipped.
    #[arg(long)]
    overwrite: bool,

    /// Suppress progress bar; only print the final summary.
    #[arg(long, short = 'q')]
    quiet: bool,

    /// Path to a TACT key file (wowdev space-separated format: KEY_NAME KEY_VALUE).
    /// Loads additional decryption keys for encrypted BLTE blocks at runtime.
    #[arg(long)]
    tact_keys: Option<PathBuf>,
}

/// Arguments for the `probe` subcommand.
#[derive(Debug, Parser)]
struct ProbeArgs {
    /// Game installation root(s).  Pass multiple times to probe several
    /// archives in one invocation.
    #[arg(long, short = 'a', required = true)]
    archive: Vec<PathBuf>,

    /// Product UID override.  Auto-detected when omitted.
    #[arg(long)]
    product: Option<String>,

    /// Exit immediately on first failure instead of probing remaining archives.
    #[arg(long)]
    fail_fast: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Export(args) => run_export(&args),
        Command::Inspect(args) => inspect::run(&args),
        Command::Probe(args) => run_probe(&args),
    }
}

fn run_export(args: &ExportArgs) -> Result<()> {
    // ── Load runtime TACT keys ────────────────────────────────────────────
    if let Some(key_path) = &args.tact_keys {
        let n = rustydemon_lib::key_service::load_keys_from_file(key_path)
            .with_context(|| format!("loading TACT keys from {}", key_path.display()))?;
        eprintln!("  tact-keys: loaded {n} key(s) from {}", key_path.display());
    }

    // ── Open archive ──────────────────────────────────────────────────────
    let product = args.product.clone().unwrap_or_else(|| {
        detect_product(&args.archive).unwrap_or_else(|| {
            if args.archive.join("Data").join(".build.config").is_file() {
                "fenris".into()
            } else {
                "wow".into()
            }
        })
    });

    eprintln!("Opening {} (product: {product})", args.archive.display());
    let t_open = Instant::now();
    let mut casc = CascHandler::open_local(&args.archive, &product)
        .with_context(|| format!("failed to open CASC archive at {}", args.archive.display()))?;
    casc.load_builtin_paths();

    // ── Optionally apply a community listfile (WoW) ───────────────────────
    if let Some(listfile_path) = &args.listfile {
        if casc.has_builtin_paths() {
            eprintln!("  note: archive already has built-in paths; listfile will be merged");
        }
        let content = std::fs::read_to_string(listfile_path)
            .with_context(|| format!("reading listfile {}", listfile_path.display()))?;
        let fdid_map = casc.fdid_hash_snapshot();
        let (filenames, tree) = rustydemon_lib::prepare_listfile(&content, &fdid_map);
        let n = filenames.len();
        casc.apply_listfile(filenames, tree);
        eprintln!(
            "  listfile: {} path entries from {}",
            n,
            listfile_path.display()
        );
    }

    eprintln!(
        "  loaded in {:.2}s  ({} root entries, {} filenames)",
        t_open.elapsed().as_secs_f32(),
        casc.root_count(),
        casc.filename_count(),
    );

    // ── Build match list ──────────────────────────────────────────────────
    let matches: Vec<CascFile> = if let Some(fdid) = args.fdid {
        if !casc.file_exists_by_fdid(fdid) {
            return Err(anyhow!("FileDataID {fdid} not found in root manifest"));
        }
        let fdid_map = casc.fdid_hash_snapshot();
        let hash = fdid_map
            .get(&fdid)
            .copied()
            .ok_or_else(|| anyhow!("FileDataID {fdid} has no root hash"))?;
        let name = casc
            .filename_for_hash(hash)
            .unwrap_or_else(|| format!("fdid_{fdid}.bin"));
        vec![CascFile::new(name, hash, Some(fdid))]
    } else {
        let path = args.path.as_deref().unwrap();
        let tree = casc.root_folder.as_ref().ok_or_else(|| {
            anyhow!(
                "archive has no virtual file tree — for WoW, pass --listfile \
                 <path> (download one from https://github.com/wowdev/wow-listfile), \
                 or use --fdid <id> to export a single file by FileDataID"
            )
        })?;
        let hits =
            PathQuery::run(path, tree).with_context(|| format!("resolving --path {path}"))?;
        eprintln!("  matched {} files for '{}'", hits.len(), path);
        hits
    };

    if matches.is_empty() {
        eprintln!("Nothing to export.");
        return Ok(());
    }

    if args.dry_run {
        for f in &matches {
            println!("{}", f.full_path);
        }
        eprintln!("(dry run — nothing written)");
        return Ok(());
    }

    std::fs::create_dir_all(&args.output)
        .with_context(|| format!("failed to create output dir {}", args.output.display()))?;

    // ── Configure thread pool ─────────────────────────────────────────────
    if let Some(n) = args.parallel {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .ok();
    }

    // ── Export in parallel ────────────────────────────────────────────────
    let pb = if args.quiet {
        ProgressBar::hidden()
    } else {
        let bar = ProgressBar::new(matches.len() as u64);
        bar.set_style(
            ProgressStyle::with_template(
                "{bar:40.cyan/blue} {pos:>7}/{len:7} [{elapsed_precise}] {wide_msg}",
            )
            .unwrap(),
        );
        bar
    };

    let ok = AtomicUsize::new(0);
    let skipped_missing_chunk = AtomicUsize::new(0);
    let skipped_existing = AtomicUsize::new(0);
    let errors = AtomicUsize::new(0);
    let t_export = Instant::now();

    matches.par_iter().for_each(|file| {
        let dst = if args.flat {
            args.output.join(
                Path::new(&file.full_path)
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("unknown.bin")),
            )
        } else {
            args.output.join(&file.full_path)
        };

        if !args.overwrite && dst.exists() {
            skipped_existing.fetch_add(1, Ordering::Relaxed);
            pb.inc(1);
            return;
        }

        match casc.open_file_by_hash(file.hash) {
            Ok(bytes) => {
                if let Some(parent) = dst.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        errors.fetch_add(1, Ordering::Relaxed);
                        eprintln!("create dir {}: {e}", parent.display());
                        pb.inc(1);
                        return;
                    }
                }
                match std::fs::write(&dst, &bytes) {
                    Ok(()) => {
                        ok.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                        eprintln!("write {}: {e}", dst.display());
                    }
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("no file on disk") || msg.contains("IndexNotFound") {
                    skipped_missing_chunk.fetch_add(1, Ordering::Relaxed);
                } else {
                    errors.fetch_add(1, Ordering::Relaxed);
                    eprintln!("open {}: {e}", file.full_path);
                }
            }
        }
        pb.inc(1);
    });

    pb.finish_and_clear();

    let ok = ok.load(Ordering::Relaxed);
    let skipped_mc = skipped_missing_chunk.load(Ordering::Relaxed);
    let skipped_ex = skipped_existing.load(Ordering::Relaxed);
    let errs = errors.load(Ordering::Relaxed);
    eprintln!(
        "Done in {:.1}s:  exported={ok}  skipped(missing-chunk)={skipped_mc}  \
         skipped(exists)={skipped_ex}  errors={errs}",
        t_export.elapsed().as_secs_f32()
    );

    if errs > 0 {
        std::process::exit(1);
    }

    Ok(())
}

/// Auto-detect the product UID from `.build.info`, falling back to the
/// directory name for installs that don't carry one.
fn detect_product(base: &Path) -> Option<String> {
    CascConfig::detect_products(base).into_iter().next()
}

// ── probe ─────────────────────────────────────────────────────────────────────

fn run_probe(args: &ProbeArgs) -> Result<()> {
    let mut any_fail = false;

    for archive in &args.archive {
        let result = probe_one(archive, args.product.as_deref());
        match result {
            Ok(info) => {
                println!(
                    "[PASS]  {:<28}  {:.2}s  {:>7} entries  {}",
                    info.label,
                    info.elapsed_secs,
                    info.entry_count,
                    archive.display()
                );
            }
            Err(e) => {
                println!("[FAIL]  {}  —  {e:#}", archive.display());
                any_fail = true;
                if args.fail_fast {
                    break;
                }
            }
        }
    }

    if any_fail {
        std::process::exit(1);
    }
    Ok(())
}

struct ProbeInfo {
    label: String,
    elapsed_secs: f32,
    entry_count: usize,
}

fn probe_one(archive: &Path, product_override: Option<&str>) -> Result<ProbeInfo> {
    let product = product_override
        .map(str::to_owned)
        .or_else(|| detect_product(archive))
        .unwrap_or_else(|| "wow".into());

    let t = Instant::now();
    let mut casc = CascHandler::open_local(archive, &product)
        .with_context(|| format!("failed to open {}", archive.display()))?;
    casc.load_builtin_paths();

    let entry_count = casc.root_count();
    let game_type = casc.config.game_type;
    let elapsed_secs = t.elapsed().as_secs_f32();

    // Pick a canonical file to extract per game type so we exercise the full
    // read path (index lookup → data.NNN read → BLTE decode), not just open.
    let canonical: Option<&str> = match game_type {
        GameType::DiabloIIResurrected => Some("data/data/global/dataversionbuild.txt"),
        GameType::DiabloIV | GameType::DiabloIVBeta => Some("base/Default_GPU_Settings.txt"),
        GameType::StarCraft => Some("SD/rez/EstT01ED.txt"),
        // WoW and others don't have builtin paths without a listfile;
        // opening successfully + having entries is enough signal.
        _ => None,
    };

    if let Some(path) = canonical {
        let bytes = casc
            .open_file_by_name(path)
            .with_context(|| format!("extracting canonical file '{path}'"))?;
        if bytes.is_empty() {
            return Err(anyhow!("canonical file '{path}' extracted as empty"));
        }
    }

    let launcher = launcher_tag(archive);
    let game_name = game_display_name(game_type, &product);
    let label = format!("{game_name} [{launcher}]");

    Ok(ProbeInfo {
        label,
        elapsed_secs,
        entry_count,
    })
}

fn launcher_tag(path: &Path) -> &'static str {
    let s = path.to_string_lossy();
    if s.contains("steamapps") || s.contains("Steam") {
        "Steam"
    } else {
        "Battle.net"
    }
}

fn game_display_name(game_type: GameType, fallback_product: &str) -> String {
    match game_type {
        GameType::DiabloIIResurrected => "D2R".into(),
        GameType::DiabloIV => "D4".into(),
        GameType::DiabloIVBeta => "D4 Beta".into(),
        GameType::DiabloIII => "D3".into(),
        GameType::WorldOfWarcraft => "WoW".into(),
        GameType::Warcraft3Reforged => "WC3".into(),
        GameType::StarCraft => "SC1".into(),
        GameType::StarCraft2 => "SC2".into(),
        GameType::HeroesOfTheStorm => "HotS".into(),
        GameType::Hearthstone => "HS".into(),
        GameType::Overwatch => "OW2".into(),
        _ => fallback_product.to_uppercase(),
    }
}
