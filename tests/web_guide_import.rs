#![cfg(feature = "desktop")]
//! Web-guide import: URL -> stripped text -> indexed knowledge base.
//!
//! The offline tests use synthetic HTML shaped like a real MediaWiki page and
//! run everywhere. The live tests hit Bulbapedia and are `#[ignore]`d; run
//! them with:
//!
//! ```text
//! cargo test --test web_guide_import -- --ignored --nocapture
//! ```

use gba_simulator::ui::ai_agent::AiAgent;
use gba_simulator::ui::game_guide::GuideKind;
use gba_simulator::ui::web_guide::*;

const LIVE_URL: &str = "https://bulbapedia.bulbagarden.net/wiki/Walkthrough:Pok%C3%A9mon_Emerald";

/// Builds a MediaWiki-shaped page with `body` as the article text.
fn wiki_page(title: &str, body: &str) -> String {
    format!(
        "<html><head><title>{t} - TestWiki, the free encyclopedia</title></head>\
         <body><nav id=\"mw-navigation\">Main page Random page Special pages {pad}</nav>\
         <div id=\"mw-content-text\"><div class=\"mw-parser-output\">{b}</div></div>\
         <div id=\"catlinks\">Categories: Walkthroughs</div>\
         <footer>Privacy policy About Disclaimers</footer></body></html>",
        t = title,
        b = body,
        pad = "nav filler ".repeat(60),
    )
}

// ---------------------------------------------------------------------------
// Offline
// ---------------------------------------------------------------------------

#[test]
fn extracts_article_text_and_discards_chrome() {
    let body = "<h2>Rustboro Gym</h2><p>The Gym Leader is Roxanne, who uses Rock-type \
                Pokémon. Defeating her earns the Stone Badge.</p>"
        .to_string();
    let html = wiki_page("Walkthrough:Test/Part 2", &body);
    let text = html_to_text(&html);

    assert!(text.contains("Roxanne"), "article text lost: {:?}", text);
    assert!(text.contains("Stone Badge"));
    assert!(!text.contains("Random page"), "nav chrome leaked: {:?}", text);
    assert!(!text.contains("Privacy policy"), "footer leaked");
    assert!(!text.contains("Categories:"), "catlinks leaked");
}

#[test]
fn page_title_strips_the_wiki_suffix() {
    let html = wiki_page("Walkthrough:Test/Part 2", "<p>body</p>");
    assert_eq!(page_title(&html, "fallback"), "Walkthrough:Test/Part 2");
}

#[test]
fn toc_hints_map_parts_to_locations() {
    // Shaped like html_to_text output for Bulbapedia's contents table.
    let index_text = "Contents\n\
                      Main Storyline\n\
                      Part 1\n\
                      Introduction, Littleroot Town, Route 101\n\
                      Part 2\n\
                      Route 104, Petalburg Woods, Rustboro City, Rustboro Gym\n\
                      Part 3\n\
                      Dewford Town, Dewford Gym, Granite Cave\n";
    let hints = parse_toc_hints(index_text);

    assert_eq!(hints.len(), 3, "got {:?}", hints);
    assert_eq!(hints[1].0, 2);
    assert!(hints[1].1.contains("Rustboro Gym"), "got {:?}", hints[1].1);
}

#[test]
fn subpage_discovery_is_scoped_ordered_and_deduped() {
    let base = "https://wiki.test/wiki/Walkthrough:Game";
    let html = r#"
        <a href="/wiki/Walkthrough:Game/Part_10">10</a>
        <a href="/wiki/Walkthrough:Game/Part_2">2</a>
        <a href="/wiki/Walkthrough:Game/Part_2">dup</a>
        <a href="/wiki/Walkthrough:Game/Part_1">1</a>
        <a href="/wiki/Unrelated_Article">no</a>
        <a href="/wiki/Walkthrough:Game/Part_1/Sub">too deep</a>
        <a href="https://other.test/wiki/Walkthrough:Game/Part_5">offsite</a>
    "#;
    let subs = discover_subpages(html, base);
    assert_eq!(
        subs,
        vec![
            "https://wiki.test/wiki/Walkthrough:Game/Part_1".to_string(),
            "https://wiki.test/wiki/Walkthrough:Game/Part_2".to_string(),
            "https://wiki.test/wiki/Walkthrough:Game/Part_10".to_string(),
        ]
    );
}

#[test]
fn rejects_non_http_schemes() {
    // Guards against a pasted path turning into a local file read.
    for bad in ["file:///etc/passwd", "ftp://x.test/g", "javascript:alert(1)"] {
        assert!(fetch_url(bad, 5).is_err(), "{} was accepted", bad);
    }
}

#[test]
fn indexed_web_guide_is_retrievable_and_labelled() {
    let guide = WebGuide {
        title: "Walkthrough:Test".to_string(),
        root_url: "https://wiki.test/wiki/Walkthrough:Test".to_string(),
        pages: vec![
            WebPage {
                url: "https://wiki.test/wiki/Walkthrough:Test/Part_1".to_string(),
                title: "Walkthrough:Test/Part 1".to_string(),
                hint: "Littleroot Town, Route 101".to_string(),
                text: "You receive your first Pokémon from Professor Birch on Route 101. \
                       Head north through the tall grass toward Oldale Town and keep healing \
                       at the Pokémon Center whenever your team is worn down."
                    .repeat(6),
            },
            WebPage {
                url: "https://wiki.test/wiki/Walkthrough:Test/Part_2".to_string(),
                title: "Walkthrough:Test/Part 2".to_string(),
                hint: "Rustboro City, Rustboro Gym".to_string(),
                text: "Roxanne leads the Gym here and specialises in Rock-type Pokémon. \
                       Grass and Water moves are very effective. Beating her awards the \
                       Stone Badge and a TM."
                    .repeat(6),
            },
        ],
    };

    let mut agent = AiAgent::new();
    let msg = agent.load_web_guide(guide, 7).expect("index web guide");
    assert!(msg.contains("from the web"), "unexpected notice: {}", msg);

    let doc = &agent.guides.docs[0];
    assert_eq!(doc.kind, GuideKind::Web);
    assert_eq!(
        doc.source_url.as_deref(),
        Some("https://wiki.test/wiki/Walkthrough:Test")
    );

    // Retrieval by content...
    let hits = agent.guides.retrieve("Roxanne rock type gym", 2);
    assert!(!hits.is_empty());
    assert!(hits[0].text.contains("Roxanne"), "got {:?}", hits[0].text);

    // ...and by the section hint, whose words never appear in the body text.
    // This is what makes a chapter findable by the location it covers.
    let by_hint = agent.guides.retrieve("Rustboro City", 2);
    assert!(
        by_hint.iter().any(|h| h.text.contains("Stone Badge")),
        "hint-based retrieval failed: {:?}",
        by_hint.iter().map(|h| &h.text).collect::<Vec<_>>()
    );
}

#[test]
fn section_text_excludes_the_injected_context() {
    // The hint is indexed but must not be spliced into the excerpt the model
    // reads, or the guide appears to say things it never said.
    let guide = WebGuide {
        title: "G".to_string(),
        root_url: "https://w.test/g".to_string(),
        pages: vec![WebPage {
            url: "https://w.test/g/Part_1".to_string(),
            title: "G/Part 1".to_string(),
            hint: "UNIQUEHINTTOKEN".to_string(),
            text: "Ordinary walkthrough prose about crossing the bridge and entering the cave."
                .repeat(8),
        }],
    };
    let mut agent = AiAgent::new();
    agent.load_web_guide(guide, 0).unwrap();

    let hits = agent.guides.retrieve("UNIQUEHINTTOKEN", 1);
    assert!(!hits.is_empty(), "hint should be searchable");
    assert!(
        !hits[0].text.contains("UNIQUEHINTTOKEN"),
        "hint leaked into excerpt text: {:?}",
        hits[0].text
    );
}

#[test]
fn failed_import_leaves_the_library_untouched() {
    let mut agent = AiAgent::new();
    let empty = WebGuide {
        title: "Nothing".to_string(),
        root_url: "https://w.test/none".to_string(),
        pages: vec![WebPage {
            url: "https://w.test/none".to_string(),
            title: "Nothing".to_string(),
            hint: String::new(),
            text: "too short".to_string(),
        }],
    };
    assert!(agent.load_web_guide(empty, 0).is_err());
    assert!(agent.guides.docs.is_empty());
    assert!(agent.guides.is_empty());
}

// ---------------------------------------------------------------------------
// Live (network)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires network access to bulbapedia.bulbagarden.net"]
fn live_bulbapedia_index_lists_every_part() {
    let html = fetch_url(LIVE_URL, 30).expect("fetch index");
    let subs = discover_subpages(&html, LIVE_URL);
    println!("discovered {} sub-pages", subs.len());
    assert!(
        subs.len() >= 15,
        "expected a multi-part walkthrough, got {:?}",
        subs
    );
    assert!(subs[0].ends_with("Part_1"), "ordering wrong: {:?}", &subs[..3]);
}

#[test]
#[ignore = "requires network access to bulbapedia.bulbagarden.net"]
fn live_bulbapedia_import_answers_real_questions() {
    let guide = import_web_guide(LIVE_URL, 30, true, 25, |d, t, _| {
        if d > 0 && d % 5 == 0 {
            println!("  fetched {}/{}", d, t);
        }
    })
    .expect("import");

    println!(
        "imported {:?}: {} pages, {} chars",
        guide.title,
        guide.pages.len(),
        guide.total_chars()
    );
    assert!(guide.pages.len() >= 15, "too few pages imported");
    assert!(guide.total_chars() > 100_000, "suspiciously little text");

    // Sub-page hints must have been harvested from the index TOC.
    assert!(
        guide.pages.iter().any(|p| p.hint.contains("Rustboro")),
        "TOC hints were not attached to the parts"
    );

    let mut agent = AiAgent::new();
    agent.load_web_guide(guide, 0).expect("index");

    // Goal-language questions must land on the right chapter even though the
    // guide never uses the words "first trainer badge".
    let cases: [(&str, &[&str]); 4] = [
        (
            "Complete the first trainer badge",
            &["roxanne", "rustboro", "stone badge"],
        ),
        (
            "how do I get the second gym badge",
            &["brawly", "dewford", "knuckle badge"],
        ),
        ("how do I beat the Elite Four", &["sidney", "phoebe", "glacia", "drake"]),
        ("beat the Mauville gym", &["wattson", "mauville", "dynamo"]),
    ];

    for (query, expected) in cases {
        let hits = agent.guides.retrieve(query, 3);
        let blob = hits
            .iter()
            .map(|h| h.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        let matched = expected.iter().any(|e| blob.contains(e));
        println!("{} {:?}", if matched { "PASS" } else { "FAIL" }, query);
        assert!(
            matched,
            "query {:?} retrieved nothing about {:?}\nfirst hit: {}",
            query,
            expected,
            hits.first()
                .map(|h| h.text.chars().take(200).collect::<String>())
                .unwrap_or_else(|| "<no hits>".into())
        );
    }
}
