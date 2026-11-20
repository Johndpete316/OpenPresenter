//! `pp6tool`: command-line tools for `.pro6` libraries.

mod render_cmd;
mod report;
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
