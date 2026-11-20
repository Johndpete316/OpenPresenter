#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real PP6 files. They aren't in the repository (church content); put `.pro6`
//! files in `pp6-examples/` at the workspace root to run these checks.

use std::path::PathBuf;

fn corpus() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../pp6-examples");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "pro6"))
        .collect();
    v.sort();
    v
}

#[test]
fn real_files_parse_round_trip_and_rtf_reads() {
    let files = corpus();
    if files.is_empty() {
        eprintln!("pp6-examples not present; skipping");
        return;
    }
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        let doc = pro6::load(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert_eq!(
            pro6::write_string(&doc),
            src,
            "{} must write back byte-identical",
            f.display()
        );
        for t in doc.text_elements() {
            let rtf = t
                .rtf_doc(&doc.path)
                .unwrap_or_else(|e| panic!("{}: {e}", f.display()))
                .expect("RTFData present");
            assert!(!rtf.fonts.is_empty(), "{}: font table", f.display());
            assert!(
                !rtf.unhandled_words.contains_key("rquote"),
                "apostrophes are decoded"
            );
        }
    }
}
