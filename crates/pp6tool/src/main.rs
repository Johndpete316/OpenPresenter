//! `pp6tool`: command-line tools for `.pro6` libraries.

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
    }
    Ok(ExitCode::SUCCESS)
}
