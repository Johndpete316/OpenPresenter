//! `pp6tool`: command-line tools for `.pro6` libraries.

mod convert_cmd;
mod render_cmd;
mod report;
mod rescale_cmd;
mod roundtrip;
mod walk;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use tracing::level_filters::LevelFilter;

#[derive(Parser)]
#[command(version, about = "Tools for ProPresenter 6 .pro6 libraries")]
struct Cli {
    /// More logging on stderr (-v info, -vv debug).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inventory every .pro6 file under a directory: parse status, canvas sizes,
    /// fonts, groups, element/attribute names (flagging ones not in the spec),
    /// and files that use media cues.
    Report {
        /// Directory to scan recursively.
        dir: PathBuf,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Load, save and compare every .pro6 file under a directory. Exits non-zero
    /// if any file changes meaningfully or can't be read back.
    Roundtrip {
        /// Directory to scan recursively.
        dir: PathBuf,
        /// Also save each rewritten file here, at the same relative path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Render every slide of a .pro6 file to PNG (for visual comparison with PP6).
    Render {
        file: PathBuf,
        /// Directory for the PNGs (created if missing).
        #[arg(long)]
        out: PathBuf,
        /// Output size WIDTHxHEIGHT (default: the document's canvas size).
        #[arg(long, value_parser = render_cmd::parse_size)]
        size: Option<(u32, u32)>,
        /// Extra font directory (repeatable).
        #[arg(long = "fonts")]
        font_dirs: Vec<PathBuf>,
        /// Use only the --fonts directories, not the fonts installed on this PC.
        #[arg(long)]
        fonts_only: bool,
    },
    /// Change the canvas resolution of presentations (dry run unless --out or --in-place).
    Rescale {
        /// Target canvas WIDTHxHEIGHT, e.g. 1024x768.
        #[arg(long, value_parser = render_cmd::parse_size)]
        to: (u32, u32),
        /// Files and/or folders (searched recursively).
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Fill the new canvas when the aspect ratio differs (default: fit and centre).
        #[arg(long)]
        stretch: bool,
        /// Write rescaled copies here (mirrors folder structure).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Replace the originals (each is backed up to .pro6.bak first).
        #[arg(long)]
        in_place: bool,
        /// Render before/after PNGs of every slide into this folder.
        #[arg(long)]
        preview: Option<PathBuf>,
        /// Keep WinFlowData/WinFontData (default: drop them for rescaled text).
        #[arg(long)]
        keep_win_payloads: bool,
        /// Extra font directory for --preview (repeatable).
        #[arg(long = "fonts")]
        font_dirs: Vec<PathBuf>,
    },
    /// Convert a .pro6 to the native .opres format (checked by converting back), or a
    /// .opres back to .pro6. Never overwrites a file.
    Convert {
        file: PathBuf,
        /// Output file (default: next to the input with the other extension).
        #[arg(long)]
        out: Option<PathBuf>,
        /// .opres → .pro6: leave out slides marked incompatible (default: write them
        /// without the parts .pro6 can't hold).
        #[arg(long)]
        skip_incompatible: bool,
    },
}

fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    let level = match cli.verbose {
        0 => LevelFilter::ERROR,
        1 => LevelFilter::INFO,
        _ => LevelFilter::DEBUG,
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(level)
        .init();

    match cli.command {
        Command::Report { dir, json } => {
            let started = Instant::now();
            let report = report::build(&dir)?;
            let elapsed = started.elapsed();
            tracing::info!(
                files = report.summary.files_total,
                elapsed_ms = elapsed.as_secs_f64() * 1000.0,
                "report complete"
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!(
                    "{}",
                    report::render_text(&report, &dir.display().to_string())
                );
            }
            eprintln!(
                "parsed {} file(s) in {:.1} ms",
                report.summary.files_total,
                elapsed.as_secs_f64() * 1000.0
            );
        }
        Command::Roundtrip { dir, out } => {
            let started = Instant::now();
            let results = roundtrip::run(&dir, out.as_deref())?;
            print!(
                "{}",
                roundtrip::render_text(&results, &dir.display().to_string())
            );
            eprintln!(
                "round-tripped {} file(s) in {:.1} ms",
                results.len(),
                started.elapsed().as_secs_f64() * 1000.0
            );
            if results.iter().any(|r| r.outcome.is_failure()) {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Rescale {
            to,
            inputs,
            stretch,
            out,
            in_place,
            preview,
            keep_win_payloads,
            font_dirs,
        } => {
            let dry_run = out.is_none() && !in_place;
            let opts = rescale_cmd::Options {
                to,
                inputs,
                stretch,
                out,
                in_place,
                preview,
                keep_win_payloads,
                font_dirs,
            };
            let results = rescale_cmd::run(&opts)?;
            print!("{}", rescale_cmd::render_text(&results, dry_run));
            if results.iter().any(|r| r.error.is_some()) {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Convert {
            file,
            out,
            skip_incompatible,
        } => {
            print!(
                "{}",
                convert_cmd::run(&convert_cmd::Options {
                    file,
                    out,
                    skip_incompatible,
                })?
            );
        }
        Command::Render {
            file,
            out,
            size,
            font_dirs,
            fonts_only,
        } => {
            let started = Instant::now();
            let opts = render_cmd::Options {
                file,
                out,
                size,
                font_dirs,
                fonts_only,
            };
            let results = render_cmd::run(&opts)?;
            print!("{}", render_cmd::render_text(&results));
            let failed = results.iter().filter(|r| r.error.is_some()).count();
            eprintln!(
                "rendered {} of {} slide(s) in {:.1} ms (including font loading)",
                results.len() - failed,
                results.len(),
                started.elapsed().as_secs_f64() * 1000.0
            );
            if failed > 0 {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
