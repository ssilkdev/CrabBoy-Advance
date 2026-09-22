//! Real-world PDF ingestion for the AI game-guide knowledge base.
//!
//! Uses the shipped user manual as a known-good PDF fixture: it is a
//! FlateDecode, text-layer PDF produced by a normal renderer, which is exactly
//! the shape a downloaded strategy guide has.

use gba_simulator::ui::game_guide::{extract_pdf_text, GuideKind, GuideLibrary};
use std::path::Path;

fn manual_path() -> Option<&'static Path> {
    let p = Path::new("CrabBoy_Advance_User_Manual.pdf");
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

#[test]
fn extracts_text_from_a_real_pdf() {
    let Some(path) = manual_path() else {
        eprintln!("manual PDF fixture absent; skipping");
        return;
    };
    let bytes = std::fs::read(path).expect("read manual pdf");
    let text = extract_pdf_text(&bytes).expect("extract text layer");

    assert!(
        text.len() > 2000,
        "expected a substantial text layer, got {} chars",
        text.len()
    );
    let lower = text.to_lowercase();
    assert!(
        lower.contains("crabboy"),
        "extracted text should mention the product name; first 400 chars: {:?}",
        &text.chars().take(400).collect::<String>()
    );

    // The extractor must not emit raw binary: a text layer that is mostly
    // control bytes means the content-stream filter failed.
    let control = text.chars().filter(|c| c.is_control() && *c != '\n' && *c != '\r' && *c != '\t').count();
    assert!(
        (control as f32) < text.len() as f32 * 0.02,
        "too many control chars ({} of {}) — binary leaked into the text layer",
        control,
        text.len()
    );
}

#[test]
fn indexes_a_real_pdf_and_retrieves_from_it() {
    let Some(path) = manual_path() else {
        eprintln!("manual PDF fixture absent; skipping");
        return;
    };
    let mut lib = GuideLibrary::new();
    let id = lib.add_file(path).expect("index manual pdf");

    let doc = lib.docs.iter().find(|d| d.id == id).expect("doc recorded");
    assert_eq!(doc.kind, GuideKind::Pdf);
    assert!(doc.chunks > 1, "expected multiple chunks, got {}", doc.chunks);

    let hits = lib.retrieve("emulator controls and buttons", 4);
    assert!(!hits.is_empty(), "retrieval returned nothing from a real guide");
    assert!(hits[0].score > 0.0);

    let block = lib
        .context_block("emulator controls and buttons", 3, 4000)
        .expect("context block");
    assert!(block.contains("GAME GUIDE EXCERPTS"));
    assert!(block.len() <= 4200);
    assert!(!lib.last_citations.is_empty());
}

#[test]
fn extracted_text_is_word_clean_not_letter_soup() {
    // Regression guard for the two failure modes real PDFs trigger:
    //   * per-glyph `Tm` positioning shredding words into "T h i s";
    //   * merging every font's /ToUnicode into one table, which makes a
    //     document's icon font hijack the body font's glyph ids.
    let Some(path) = manual_path() else {
        eprintln!("manual PDF fixture absent; skipping");
        return;
    };
    let bytes = std::fs::read(path).expect("read manual pdf");
    let text = extract_pdf_text(&bytes).expect("extract text layer");

    let words: Vec<&str> = text.split_whitespace().collect();
    assert!(words.len() > 1500, "only {} words extracted", words.len());

    // Letter-soup detector: stray single alphabetic "words".
    let singles = words
        .iter()
        .filter(|w| w.chars().count() == 1 && w.chars().all(|c| c.is_alphabetic()))
        .count();
    let ratio = singles as f32 / words.len() as f32;
    assert!(
        ratio < 0.08,
        "text looks shredded into single letters: {}/{} = {:.3}",
        singles,
        words.len(),
        ratio
    );

    // Wrong-CMap detector: body prose must not be peppered with pictographs.
    let pictographs = text
        .chars()
        .filter(|&c| ('\u{1F300}'..='\u{1FAFF}').contains(&c))
        .count();
    assert!(
        pictographs < 20,
        "{} pictographs in the body text — a decorative font's CMap is \
         being applied to body glyphs",
        pictographs
    );

    // Real multi-word phrases must survive intact.
    let lower = text.to_lowercase();
    for phrase in ["game boy advance", "save state"] {
        assert!(
            lower.contains(phrase),
            "expected phrase {:?} in extracted text",
            phrase
        );
    }
}

#[test]
fn rejects_non_pdf_bytes() {
    let err = extract_pdf_text(b"this is clearly not a pdf file at all").unwrap_err();
    assert!(err.to_lowercase().contains("pdf"), "got: {}", err);
}

#[test]
fn indexes_a_plain_text_walkthrough() {
    let dir = std::env::temp_dir().join("crabboy_guide_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("walkthrough.txt");
    let body = "\
=============================
 POKEMON EMERALD WALKTHROUGH
=============================

SECTION 4: RUSTBORO CITY GYM
The first badge in the game is the Stone Badge, held by Gym Leader Roxanne.
Her team is Geodude (Lv12), Geodude (Lv12) and Nosepass (Lv15), all Rock types.
Bring a Grass or Water Pokemon. Marshtomp learns Mud Shot and sweeps the gym.
The gym is in the north-west of Rustboro City. Two Youngster trainers guard the
path to Roxanne. Beating her awards the Stone Badge and TM39 Rock Tomb.

SECTION 5: ROUTE 116 AND RUSTURF TUNNEL
After the badge, chase the Team Aqua grunt who steals the Devon Goods.
";
    std::fs::write(&path, body).unwrap();

    let mut lib = GuideLibrary::new();
    let id = lib.add_file(&path).expect("index txt guide");
    assert_eq!(lib.docs.iter().find(|d| d.id == id).unwrap().kind, GuideKind::Text);

    let hits = lib.retrieve("Complete the first trainer badge", 3);
    assert!(!hits.is_empty());
    assert!(
        hits[0].text.to_lowercase().contains("roxanne"),
        "expected the gym section first, got: {}",
        hits[0].text
    );

    let _ = std::fs::remove_file(&path);
}
