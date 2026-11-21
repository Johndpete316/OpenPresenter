//! `pp6tool convert`: `.pro6` → `.opres` (checked by converting back), or a `.opres`
//! back to `.pro6`. For testing the native format; never overwrites a file.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

pub struct Options {
    pub file: PathBuf,
    pub out: Option<PathBuf>,
    pub skip_incompatible: bool,
}

pub fn run(opts: &Options) -> anyhow::Result<String> {
    let to_native = doc::is_pro6(&opts.file);
    if !to_native && !doc::is_native(&opts.file) {
        bail!("{} is neither a .pro6 nor a .opres file", opts.file.display());
    }
    let out = opts.out.clone().unwrap_or_else(|| {
        opts.file
            .with_extension(if to_native { doc::NATIVE_EXT } else { "pro6" })
    });
    if out.exists() {
        bail!("{} already exists; choose another --out", out.display());
    }
    // Exactly the given file, not a .opres saved next to it.
    let d = doc::load(&opts.file).map_err(|e| anyhow::anyhow!(e.user_message()))?;
    if to_native {
        let json = doc::file::to_json(&d);
        // Check before writing: the .opres must export back to the same .pro6.
        let back = doc::file::from_json(&json, &out).map_err(|e| anyhow::anyhow!(e.user_message()))?;
        let original = pro6::write_string(d.pro6.as_deref().context("no source")?);
        let exported = pro6::write_string(
            &doc::to_pro6(&back).map_err(|e| anyhow::anyhow!(e.user_message()))?,
        );
        if exported != original {
            bail!("converting back to .pro6 doesn't give the original; nothing written");
        }
        write(&out, &json)?;
        Ok(format!(
            "{} → {} ({} slides; converts back to an identical .pro6)\n",
            opts.file.display(),
            out.display(),
            d.slides().count()
        ))
    } else {
        let choice = if opts.skip_incompatible {
            doc::ExportChoice::SkipIncompatible
        } else {
            doc::ExportChoice::DropUnsupported
        };
        let (p, report) =
            doc::export_pro6(&d, choice).map_err(|e| anyhow::anyhow!(e.user_message()))?;
        write(&out, pro6::write_string(&p).as_bytes())?;
        let mut s = format!("{} → {}\n", opts.file.display(), out.display());
        for l in &report.left_out {
            let what = if opts.skip_incompatible {
                "left out"
            } else {
                "written without its OpenPresent-only parts"
            };
            s.push_str(&format!("  {l}: {what}\n"));
        }
        Ok(s)
    }
}

fn write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    doc::file::write_atomic(path, bytes).with_context(|| format!("couldn't write {}", path.display()))
}
