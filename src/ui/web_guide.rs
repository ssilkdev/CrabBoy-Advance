//! Web guide ingestion — point the agent at an online wiki walkthrough.
//!
//! The user pastes a URL (Bulbapedia, Serebii, a GameFAQs text dump, any wiki)
//! and the agent fetches it, strips the page furniture, and indexes the prose
//! exactly like an uploaded PDF.
//!
//! Two things make real wikis harder than "download and strip tags":
//!
//!   * **Chrome dominates the byte count.** A Bulbapedia article is ~360 KB of
//!     HTML of which the walkthrough is a small fraction; nav menus, edit
//!     links, category lists and the sidebar would otherwise flood the index
//!     with junk that outranks the gameplay text. We cut to the MediaWiki
//!     content container when present and drop `script`/`style`/`nav`/etc.
//!   * **Long guides are split across sub-pages.** The obvious URL for
//!     "Pokémon Emerald walkthrough" is an INDEX: its body is a table of
//!     contents linking `/Part_1` … `/Part_21`, and contains essentially no
//!     gameplay text at all. Indexing just that page produces a knowledge base
//!     that knows chapter titles and nothing else. So we detect the index
//!     shape and follow the sub-page links.
//!
//! Transport is a `curl` subprocess, matching `updater.rs` and `ai_agent.rs`
//! (no TLS stack in the dependency graph).

use std::collections::HashSet;
use std::path::PathBuf;

/// Hard cap on a single downloaded page.
const MAX_PAGE_BYTES: usize = 12 * 1024 * 1024;
/// Upper bound on sub-pages followed for one multi-part guide.
const MAX_SUBPAGES: usize = 40;
/// A page yielding less than this much text is considered contentless.
const MIN_USEFUL_CHARS: usize = 400;

/// One fetched page of a (possibly multi-part) web guide.
#[derive(Clone, Debug)]
pub struct WebPage {
    pub url: String,
    pub title: String,
    pub text: String,
    /// Short descriptor of what this part covers, harvested from the index
    /// page's table of contents (e.g. "Route 104, Petalburg Woods, Rustboro
    /// City, Rustboro Gym"). Indexed with every chunk of the page so that
    /// location names reach chunks whose body never repeats them.
    pub hint: String,
}

/// Extracts a "Part N" -> "what it covers" map from an index page's text.
///
/// Bulbapedia renders its walkthrough contents as a two-column table, which
/// `html_to_text` flattens to a "Part 2" line followed by a line listing the
/// locations. Those location names are the most useful retrieval handles in
/// the whole guide — the body text of a chapter frequently never repeats the
/// city or gym name that a user would search for.
pub fn parse_toc_hints(index_text: &str) -> Vec<(u32, String)> {
    let lines: Vec<&str> = index_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("part ") else {
            continue;
        };
        let Ok(num) = rest.trim().parse::<u32>() else {
            continue;
        };
        // The descriptor is the next line, provided it is prose and not
        // simply the next "Part N" entry.
        if let Some(next) = lines.get(i + 1) {
            let nl = next.to_ascii_lowercase();
            if !nl.starts_with("part ") && next.len() > 10 {
                out.push((num, next.to_string()));
            }
        }
    }
    out
}

/// Pulls the trailing number out of a sub-page URL ("…/Part_12" -> 12).
fn url_part_number(url: &str) -> Option<u32> {
    let tail = url.rsplit('/').next()?;
    let digits: String = tail.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Result of importing a URL.
#[derive(Clone, Debug)]
pub struct WebGuide {
    /// Title of the entry page, used to name the indexed document.
    pub title: String,
    pub root_url: String,
    pub pages: Vec<WebPage>,
}

impl WebGuide {
    pub fn total_chars(&self) -> usize {
        self.pages.iter().map(|p| p.text.chars().count()).sum()
    }

    /// Concatenates every page into one document, each section headed by its
    /// page title so retrieval hits carry their chapter with them.
    pub fn combined_text(&self) -> String {
        let mut out = String::new();
        for p in &self.pages {
            out.push_str("\n\n=== ");
            out.push_str(&p.title);
            out.push_str(" ===\n");
            out.push_str(&p.text);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

fn curl_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(system_root) = std::env::var("SystemRoot") {
            let candidate = PathBuf::from(system_root).join("System32").join("curl.exe");
            if candidate.exists() {
                return candidate;
            }
        }
        PathBuf::from("curl.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("curl")
    }
}

fn silence_console(_cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        _cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Downloads a URL as text. Follows redirects; sends a real User-Agent because
/// several wikis (Bulbapedia among them) refuse the default curl agent.
pub fn fetch_url(url: &str, timeout_secs: u32) -> Result<String, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("URL must start with http:// or https://".to_string());
    }

    let mut cmd = std::process::Command::new(curl_path());
    cmd.args([
        "-sS",
        "-L",
        "--compressed",
        "--max-time",
        &timeout_secs.to_string(),
        "--max-filesize",
        &MAX_PAGE_BYTES.to_string(),
        "-A",
        "Mozilla/5.0 (compatible; CrabBoyAdvance/0.5; +game-guide-import)",
        "-w",
        "\n__CRABBOY_HTTP_STATUS__%{http_code}",
        url,
    ]);
    silence_console(&mut cmd);

    let out = cmd
        .output()
        .map_err(|e| format!("curl failed to start: {}", e))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "fetch failed: {}",
            if err.trim().is_empty() {
                "network error".to_string()
            } else {
                err.trim().to_string()
            }
        ));
    }

    let body = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, status) = match body.rfind("__CRABBOY_HTTP_STATUS__") {
        Some(idx) => {
            let status = body[idx + "__CRABBOY_HTTP_STATUS__".len()..]
                .trim()
                .parse::<u32>()
                .unwrap_or(0);
            (body[..idx].to_string(), status)
        }
        None => (body, 0),
    };

    if status >= 400 {
        return Err(match status {
            403 => "the site refused the request (403) — it may block automated access".to_string(),
            404 => "page not found (404) — check the URL".to_string(),
            429 => "rate limited by the site (429) — wait a moment and retry".to_string(),
            s => format!("server returned HTTP {}", s),
        });
    }
    if body.trim().is_empty() {
        return Err("the server returned an empty page".to_string());
    }
    Ok(body)
}

// ---------------------------------------------------------------------------
// HTML -> text
// ---------------------------------------------------------------------------

/// Elements whose entire subtree is discarded.
const DROP_TAGS: [&str; 8] = [
    "script", "style", "nav", "footer", "header", "noscript", "svg", "form",
];

/// Extracts readable text from an HTML page.
///
/// Deliberately a lexical pass rather than a DOM parse: adding `scraper`/
/// `html5ever` would pull a large dependency tree for what amounts to "drop
/// these subtrees, keep the text, remember where the block boundaries were".
pub fn html_to_text(html: &str) -> String {
    // Prefer the MediaWiki article body when present; it excludes the sidebar,
    // the nav rail and the category footer in one step.
    let scoped = scope_to_content(html);

    let bytes = scoped.as_bytes();
    let mut out = String::with_capacity(scoped.len() / 4);
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Comment?
            if scoped[i..].starts_with("<!--") {
                match scoped[i..].find("-->") {
                    Some(end) => {
                        i += end + 3;
                        continue;
                    }
                    None => break,
                }
            }

            let Some(tag_end) = find_byte(bytes, b'>', i) else {
                break;
            };
            let raw_tag = &scoped[i + 1..tag_end];
            let name = tag_name(raw_tag);

            if let Some(drop) = DROP_TAGS.iter().find(|d| **d == name) {
                // Skip to the matching close tag; if absent, drop the rest.
                let close = format!("</{}", drop);
                match find_ci(&scoped, &close, tag_end) {
                    Some(c) => {
                        i = find_byte(bytes, b'>', c).map(|e| e + 1).unwrap_or(bytes.len());
                        continue;
                    }
                    None => break,
                }
            }

            // Block-level elements become line breaks so paragraphs survive.
            if is_block_tag(name) && !out.ends_with('\n') {
                out.push('\n');
            }
            // Table cells become spaced fields rather than glued words.
            if matches!(name, "td" | "th") && !out.ends_with(' ') && !out.ends_with('\n') {
                out.push(' ');
            }

            i = tag_end + 1;
            continue;
        }

        // Text run up to the next tag.
        let start = i;
        while i < bytes.len() && bytes[i] != b'<' {
            i += 1;
        }
        let chunk = &scoped[start..i];
        push_decoded(chunk, &mut out);
    }

    tidy_whitespace(&out)
}

/// Narrows the HTML to the article body when a known content container exists.
fn scope_to_content(html: &str) -> String {
    for marker in [
        "mw-parser-output",
        "id=\"mw-content-text\"",
        "id=\"content\"",
        "<article",
        "role=\"main\"",
    ] {
        if let Some(pos) = find_ci(html, marker, 0) {
            // Start at the element containing the marker.
            let start = html[..pos].rfind('<').unwrap_or(pos);
            // Stop before the category/navigation footer MediaWiki appends.
            let tail = &html[start..];
            let end = ["id=\"catlinks\"", "printfooter", "id=\"footer\""]
                .iter()
                .filter_map(|m| find_ci(tail, m, 0))
                .min()
                .unwrap_or(tail.len());
            let slice = &tail[..end];
            // Guard against matching a stray marker inside an attribute or
            // a tiny fragment. Kept low: a short-but-real article (a stub
            // wiki page) must still be scoped, otherwise the nav and category
            // chrome leaks in and outweighs the actual content.
            if slice.len() > 80 {
                return slice.to_string();
            }
        }
    }
    html.to_string()
}

fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "br"
            | "tr"
            | "li"
            | "ul"
            | "ol"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "section"
            | "table"
            | "blockquote"
            | "pre"
            | "dd"
            | "dt"
            | "dl"
            | "hr"
    )
}

/// Lowercased element name from raw tag text (`/div class=x` -> `div`).
fn tag_name(raw: &str) -> &str {
    let t = raw.strip_prefix('/').unwrap_or(raw);
    let end = t
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(t.len());
    &t[..end]
}

fn find_byte(hay: &[u8], needle: u8, from: usize) -> Option<usize> {
    hay.get(from..)?.iter().position(|&b| b == needle).map(|p| p + from)
}

/// Case-insensitive substring search over ASCII markers.
fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    let h = hay[from..].to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    h.find(&n).map(|p| p + from)
}

/// Appends text, resolving the HTML entities that appear in wiki prose.
fn push_decoded(chunk: &str, out: &mut String) {
    let mut rest = chunk;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" | "#160" => Some(' '),
            "mdash" | "#8212" => Some('—'),
            "ndash" | "#8211" => Some('–'),
            "hellip" => Some('…'),
            "eacute" => Some('é'),
            "times" => Some('×'),
            _ => {
                if let Some(num) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                    u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
                } else if let Some(num) = ent.strip_prefix('#') {
                    num.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
}

/// Collapses the whitespace storm that tag-stripping leaves behind.
fn tidy_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0usize;
    for line in s.lines() {
        let mut compact = String::with_capacity(line.len());
        let mut last_space = false;
        for ch in line.chars() {
            if ch.is_whitespace() {
                if !last_space {
                    compact.push(' ');
                }
                last_space = true;
            } else {
                compact.push(ch);
                last_space = false;
            }
        }
        let trimmed = compact.trim();
        if trimmed.is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}

/// Reads `<title>` (minus the site suffix) or the first `<h1>`.
pub fn page_title(html: &str, fallback: &str) -> String {
    let raw = find_ci(html, "<title", 0)
        .and_then(|s| find_byte(html.as_bytes(), b'>', s).map(|o| o + 1))
        .and_then(|start| find_ci(html, "</title", start).map(|end| &html[start..end]))
        .or_else(|| {
            find_ci(html, "<h1", 0)
                .and_then(|s| find_byte(html.as_bytes(), b'>', s).map(|o| o + 1))
                .and_then(|start| find_ci(html, "</h1", start).map(|end| &html[start..end]))
        });

    let Some(raw) = raw else {
        return fallback.to_string();
    };
    let mut title = String::new();
    // The <h1> path can still contain markup (e.g. <i>) — strip it.
    let mut depth = 0;
    for ch in raw.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = (depth as i32 - 1).max(0) as usize,
            c if depth == 0 => title.push(c),
            _ => {}
        }
    }
    let mut decoded = String::new();
    push_decoded(&title, &mut decoded);
    // "Walkthrough:Pokémon Emerald - Bulbapedia, the ..." -> drop site suffix.
    let cleaned = decoded
        .split(" - ")
        .next()
        .unwrap_or(&decoded)
        .split(" | ")
        .next()
        .unwrap_or(&decoded)
        .trim()
        .to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

// ---------------------------------------------------------------------------
// Multi-part guide discovery
// ---------------------------------------------------------------------------

/// Finds sub-page links that look like continuations of `base_url`.
///
/// Matches hrefs that extend the entry page's own path — `/Part_1`, `/Page-2`,
/// `/Chapter_3` — which is exactly the MediaWiki convention Bulbapedia uses to
/// split a long walkthrough. Anything pointing elsewhere on the site (nav,
/// categories, unrelated articles) is ignored.
pub fn discover_subpages(html: &str, base_url: &str) -> Vec<String> {
    let Some((origin, base_path)) = split_origin_path(base_url) else {
        return Vec::new();
    };
    let base_path = base_path.trim_end_matches('/');
    if base_path.is_empty() {
        return Vec::new();
    }
    let base_lc = base_path.to_ascii_lowercase();

    let mut seen: HashSet<String> = HashSet::new();
    let mut found: Vec<(u32, String)> = Vec::new();

    for raw in hrefs(html) {
        // Normalise to an absolute URL on the same origin.
        let abs = if raw.starts_with("http://") || raw.starts_with("https://") {
            if !raw.to_ascii_lowercase().starts_with(&origin.to_ascii_lowercase()) {
                continue;
            }
            raw.clone()
        } else if raw.starts_with('/') {
            format!("{}{}", origin, raw)
        } else {
            continue;
        };

        let Some((_, path)) = split_origin_path(&abs) else {
            continue;
        };
        let path_lc = path.to_ascii_lowercase();

        // Must be a strict extension of the entry path.
        let Some(suffix) = path_lc.strip_prefix(&base_lc) else {
            continue;
        };
        if suffix.is_empty() || !suffix.starts_with('/') {
            continue;
        }
        let tail = &suffix[1..];
        // One level deep only, and no query/fragment pages.
        if tail.contains('/') || tail.contains('?') || tail.contains('#') || tail.is_empty() {
            continue;
        }
        // Order by trailing number when present ("part_10" after "part_2").
        let num: u32 = tail
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(u32::MAX);

        let clean = abs.split('#').next().unwrap_or(&abs).to_string();
        if seen.insert(clean.to_ascii_lowercase()) {
            found.push((num, clean));
        }
        if found.len() >= MAX_SUBPAGES {
            break;
        }
    }

    found.sort_by_key(|(n, url)| (*n, url.clone()));
    found.into_iter().map(|(_, u)| u).collect()
}

fn split_origin_path(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("https://")
        .map(|r| ("https://", r))
        .or_else(|| url.strip_prefix("http://").map(|r| ("http://", r)))?;
    let (scheme, after) = rest;
    let slash = after.find('/')?;
    let host = &after[..slash];
    let path = &after[slash..];
    let path = path.split('#').next().unwrap_or(path);
    Some((format!("{}{}", scheme, host), path.to_string()))
}

fn hrefs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(pos) = find_ci(html, "href=", i) {
        i = pos + 5;
        let rest = &html[i..];
        let mut chars = rest.char_indices();
        let Some((_, quote)) = chars.next() else { break };
        let (start, end_char) = match quote {
            '"' => (1, '"'),
            '\'' => (1, '\''),
            _ => (0, ' '),
        };
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c == end_char || c == '>' || c == '\n')
            .unwrap_or(0);
        if end > 0 {
            out.push(tail[..end].trim().to_string());
        }
        if out.len() > 4000 {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Fetches a URL and, when it turns out to be an index page for a multi-part
/// guide, follows its sub-pages too.
///
/// `progress` is called with (done, total, label) so the UI can show movement
/// during what may be a 20-page download.
pub fn import_web_guide(
    url: &str,
    timeout_secs: u32,
    follow_subpages: bool,
    max_pages: usize,
    mut progress: impl FnMut(usize, usize, &str),
) -> Result<WebGuide, String> {
    progress(0, 1, "Fetching page…");
    let root_html = fetch_url(url, timeout_secs)?;
    let root_title = page_title(&root_html, url);
    let root_text = html_to_text(&root_html);

    let mut pages: Vec<WebPage> = Vec::new();
    let mut subpages: Vec<String> = Vec::new();

    if follow_subpages {
        subpages = discover_subpages(&root_html, url);
        subpages.truncate(max_pages.max(1));
    }

    // The entry page is worth keeping only if it has real prose. A pure
    // table-of-contents index contributes nothing but chapter titles, which
    // would pollute retrieval with headings that match every query.
    let root_is_thin = root_text.chars().count() < MIN_USEFUL_CHARS * 4;
    // Harvest the table of contents BEFORE deciding whether to keep the index
    // page: even a contentless index carries the part->locations map that
    // makes the chapters findable by name.
    let toc = parse_toc_hints(&root_text);

    if !(root_is_thin && !subpages.is_empty()) {
        pages.push(WebPage {
            url: url.to_string(),
            title: root_title.clone(),
            text: root_text,
            hint: String::new(),
        });
    }

    let total = subpages.len() + 1;
    for (idx, sub) in subpages.iter().enumerate() {
        progress(idx + 1, total, sub);
        // One failed chapter must not abort a 20-part import.
        let Ok(html) = fetch_url(sub, timeout_secs) else {
            continue;
        };
        let text = html_to_text(&html);
        if text.chars().count() < MIN_USEFUL_CHARS {
            continue;
        }
        let title = page_title(&html, sub);
        let hint = url_part_number(sub)
            .and_then(|n| toc.iter().find(|(tn, _)| *tn == n))
            .map(|(_, d)| d.clone())
            .unwrap_or_default();
        pages.push(WebPage {
            url: sub.clone(),
            title,
            text,
            hint,
        });
    }

    if pages.is_empty() {
        return Err(
            "no readable text found at that URL (the page may be JavaScript-rendered \
             or blocked). Try a different guide URL, or save the page as text and \
             upload the file."
                .to_string(),
        );
    }

    Ok(WebGuide {
        title: root_title,
        root_url: url.to_string(),
        pages,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_decodes_entities() {
        let html = "<div><p>Roxanne &amp; Brawly</p><p>Caf&eacute; &#65;&#x42;</p></div>";
        let text = html_to_text(html);
        assert!(text.contains("Roxanne & Brawly"), "got {:?}", text);
        assert!(text.contains("Café AB"), "got {:?}", text);
    }

    #[test]
    fn drops_script_style_and_nav_subtrees() {
        let html = "<nav>MENU LINKS</nav><script>var x=1;</script>\
                    <style>.a{color:red}</style><p>Real walkthrough text</p>\
                    <footer>FOOTER JUNK</footer>";
        let text = html_to_text(html);
        assert!(text.contains("Real walkthrough text"));
        assert!(!text.contains("MENU LINKS"), "nav leaked: {:?}", text);
        assert!(!text.contains("var x"), "script leaked: {:?}", text);
        assert!(!text.contains("color:red"), "style leaked: {:?}", text);
        assert!(!text.contains("FOOTER JUNK"), "footer leaked: {:?}", text);
    }

    #[test]
    fn block_tags_become_line_breaks() {
        let text = html_to_text("<p>First line</p><p>Second line</p>");
        assert!(
            text.contains("First line\nSecond line"),
            "paragraphs glued: {:?}",
            text
        );
    }

    #[test]
    fn scopes_to_mediawiki_content_when_present() {
        let html = format!(
            "<html><body><div id=\"mw-navigation\">SIDEBAR JUNK {}</div>\
             <div class=\"mw-parser-output\"><p>{}</p></div>\
             <div id=\"catlinks\">CATEGORY JUNK</div></body></html>",
            "x".repeat(600),
            "The Stone Badge is held by Roxanne. ".repeat(30)
        );
        let text = html_to_text(&html);
        assert!(text.contains("Stone Badge"));
        assert!(!text.contains("SIDEBAR JUNK"), "sidebar leaked");
        assert!(!text.contains("CATEGORY JUNK"), "catlinks leaked");
    }

    #[test]
    fn title_extraction_drops_site_suffix() {
        let html = "<title>Walkthrough:Pokémon Emerald - Bulbapedia, the community-driven encyclopedia</title>";
        assert_eq!(page_title(html, "fallback"), "Walkthrough:Pokémon Emerald");
        assert_eq!(page_title("<html></html>", "fallback"), "fallback");
    }

    #[test]
    fn discovers_and_orders_numbered_subpages() {
        let base = "https://wiki.example.org/wiki/Walkthrough:Game";
        let html = r#"
            <a href="/wiki/Walkthrough:Game/Part_2">Part 2</a>
            <a href="/wiki/Walkthrough:Game/Part_10">Part 10</a>
            <a href="/wiki/Walkthrough:Game/Part_1">Part 1</a>
            <a href="/wiki/Some_Other_Article">Unrelated</a>
            <a href="/wiki/Walkthrough:Game/Part_1/Deeper">Too deep</a>
            <a href="https://elsewhere.test/wiki/Walkthrough:Game/Part_3">Offsite</a>
        "#;
        let subs = discover_subpages(html, base);
        assert_eq!(
            subs,
            vec![
                "https://wiki.example.org/wiki/Walkthrough:Game/Part_1".to_string(),
                "https://wiki.example.org/wiki/Walkthrough:Game/Part_2".to_string(),
                "https://wiki.example.org/wiki/Walkthrough:Game/Part_10".to_string(),
            ],
            "numeric ordering or filtering wrong"
        );
    }

    #[test]
    fn subpage_discovery_deduplicates() {
        let base = "https://w.test/wiki/G";
        let html = r#"<a href="/wiki/G/Part_1">a</a><a href="/wiki/G/Part_1">b</a>
                      <a href="/wiki/G/Part_1#section">c</a>"#;
        assert_eq!(discover_subpages(html, base).len(), 1);
    }

    #[test]
    fn rejects_non_http_urls() {
        assert!(fetch_url("file:///etc/passwd", 5).is_err());
        assert!(fetch_url("ftp://example.com/x", 5).is_err());
    }

    #[test]
    fn combined_text_labels_each_part() {
        let g = WebGuide {
            title: "Guide".into(),
            root_url: "https://x.test/g".into(),
            pages: vec![
                WebPage {
                    url: "https://x.test/g/1".into(),
                    title: "Part 1".into(),
                    hint: String::new(),
                    text: "Littleroot Town".into(),
                },
                WebPage {
                    url: "https://x.test/g/2".into(),
                    title: "Part 2".into(),
                    hint: String::new(),
                    text: "Rustboro Gym".into(),
                },
            ],
        };
        let c = g.combined_text();
        assert!(c.contains("=== Part 1 ==="));
        assert!(c.contains("=== Part 2 ==="));
        assert!(c.contains("Rustboro Gym"));
    }
}
