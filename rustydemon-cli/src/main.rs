//! Rusty Demon CLI — headless batch exporter for CASC archives.
//!
//! Reads a local CASC installation (Battle.net or Steam), walks a virtual
//! subdirectory, and writes matching files to a host directory.  Intended
//! for scripting and server-side extraction; the GUI binary remains the
//! way to browse interactively.
//!
//! ## Example
//!
//! ```text
//! rustydemon-cli \
//!     --archive "/home/deck/.steam/steam/steamapps/common/Diablo IV" \
//!     --path base/meta/Sound \
//!     --output ./out
//! ```

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use rustydemon_lib::{CascConfig, CascFile, CascHandler, PathQuery};

/// Rusty Demon CLI — batch export files from a CASC archive.
#[derive(Debug, Parser)]
#[command(name = "rustydemon-cli", version, about)]
struct Args {
    /// Game installation root (the directory that contains `.build.info`
    /// for Battle.net installs, or `Data/.build.config` for Steam installs).
    #[arg(long, short = 'a')]
    archive: PathBuf,

    /// What to export.  Three forms, auto-detected:
    ///
    /// 1. Literal folder, exported recursively: `base/meta/Sound`
    /// 2. Literal file, exported alone: `Interface/Icons/INV_Sword_04.blp`
    /// 3. Glob (contains `*`, `?`, or `{...}`) matched against full virtual
    ///    paths.  `**/` is auto-prepended unless the pattern already starts
    ///    with `/` or `**/`.  Examples:
    ///    `"sylvanas*.wmo"` (anywhere),
    ///    `"textures/*.tex"` (any textures dir),
    ///    `"**/cinematics/*.vid"` (literal anchor).
    #[arg(long, short = 'p')]
    path: String,

    /// Host directory to write files into.  Will be created if missing.
    #[arg(long, short = 'o')]
    output: PathBuf,

    /// Product UID (e.g. `fenris`, `wow`).  Auto-detected from `.build.info`
    /// if omitted.
    #[arg(long)]
    product: Option<String>,

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
}

fn main() -> Result<()> {
    let args = Args::parse();

    // ── Open archive ──────────────────────────────────────────────────────
    // Detection order: --product flag → .build.info → Steam D4 fallback
    // (fenris, the only static-container game we ship support for). If we
    // guess wrong the handler's own auto-detection catches it anyway, but
    // a good guess keeps the log line honest.
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
    eprintln!(
        "  loaded in {:.2}s  ({} root entries, {} filenames)",
        t_open.elapsed().as_secs_f32(),
        casc.root_count(),
        casc.filename_count(),
    );

    let tree = casc
        .root_folder
        .as_ref()
        .ok_or_else(|| anyhow!("archive has no virtual file tree (listfile may be missing)"))?;

    // ── Build match list ──────────────────────────────────────────────────
    let matches: Vec<CascFile> = PathQuery::run(&args.path, tree)
        .with_context(|| format!("resolving --path {}", args.path))?;

    eprintln!("  matched {} files for '{}'", matches.len(), args.path);

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
            .ok(); // ignore "already initialised"
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
                // Non-installed content (Steam chunks on disk) reports as a
                // "no file on disk" index error from the static container —
                // treat that as "skipped" rather than a hard failure, since
                // it's the expected behaviour for files outside the locally
                // installed chunks.
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
