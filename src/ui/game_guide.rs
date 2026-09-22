//! Game Guide knowledge base — the AI agent's "read the manual" faculty.
//!
//! The user drops in a PDF or plain-text walkthrough (GameFAQs text dumps,
//! scanned-to-text strategy guides, Markdown notes). We extract the text,
//! split it into overlapping chunks, and build a tiny in-memory BM25-ish
//! index. Before every decision the agent retrieves the handful of chunks
//! most relevant to its current mission and screen, and those excerpts ride
//! along in the prompt.
//!
//! Design constraints that shaped this module:
//!
//!   * **No new heavy dependencies.** PDF text extraction is done here, by
//!     hand, against the raw file bytes. The only inflate primitive used is
//!     `miniz_oxide`, which is already in the dependency graph via `png`.
//!     Pulling a full PDF crate (`pdf`, `lopdf`) would add dozens of
//!     transitive crates for what is, for our purposes, "find the content
//!     streams and run the text-showing operators".
//!   * **Never block the emulator.** Parsing happens once, on the UI thread,
//!     at upload time. Retrieval is a linear scan over a few thousand chunks
//!     — microseconds — so it can run inline in the decision path.
//!   * **Graceful degradation.** A scanned image-only PDF has no text layer;
//!     we detect that and tell the user instead of silently indexing nothing.

use std::collections::HashMap;
use std::path::Path;

/// Target size of an indexed chunk, in characters.
const CHUNK_CHARS: usize = 900;
/// Overlap between adjacent chunks so a fact straddling a boundary is still
/// retrievable from both sides.
const CHUNK_OVERLAP_CHARS: usize = 150;
/// Hard ceiling on indexed text per document (~8 MB of prose). Guards against
/// a pathological upload eating all RAM.
const MAX_DOC_CHARS: usize = 8 * 1024 * 1024;
/// Refuse absurd input files outright.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Documents & chunks
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideKind {
    Pdf,
    Text,
    /// Imported from a URL (wiki walkthrough, online guide).
    Web,
}

impl GuideKind {
    pub fn label(&self) -> &'static str {
        match self {
            GuideKind::Pdf => "PDF",
            GuideKind::Text => "TEXT",
            GuideKind::Web => "WEB",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GuideDoc {
    pub id: usize,
    pub name: String,
    pub kind: GuideKind,
    pub chars: usize,
    pub chunks: usize,
    /// User can mute a document without deleting it.
    pub enabled: bool,
    /// Where it came from, for web imports. Shown in the UI and used to
    /// re-fetch on refresh.
    pub source_url: Option<String>,
}

#[derive(Clone, Debug)]
struct Chunk {
    doc_id: usize,
    /// 1-based ordinal within its document, used for citations.
    ordinal: usize,
    text: String,
    /// Term -> frequency within this chunk.
    terms: HashMap<String, u32>,
    len_tokens: u32,
    /// Lowercased text, kept for cheap exact-phrase boosting.
    lower: String,
}

/// One retrieved excerpt handed to the model.
#[derive(Clone, Debug)]
pub struct Excerpt {
    pub doc_name: String,
    pub ordinal: usize,
    pub text: String,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct GuideLibrary {
    pub docs: Vec<GuideDoc>,
    chunks: Vec<Chunk>,
    /// Document frequency per term, across all chunks.
    df: HashMap<String, u32>,
    avg_len: f32,
    next_id: usize,
    /// Most recent excerpts served, for display in the UI.
    pub last_citations: Vec<String>,
}

/// Expands a query with domain synonyms before retrieval.
///
/// The user asks in goal language ("the first trainer badge"); the guide is
/// written in world language ("Rustboro Gym", "Roxanne", "Stone Badge"). No
/// amount of BM25 tuning bridges that, because the target words are simply
/// absent from the query. Ordinal gym/badge references are the common case
/// worth handling, so they get mapped to the terms a walkthrough actually
/// uses. Unknown queries pass through untouched.
#[cfg(test)]
fn expand_query(query: &str) -> String {
    expand_query_against(query, &HashMap::new())
}

/// Query expansion filtered by what the index actually contains.
///
/// `df` is the document-frequency table. An expansion term is only added when
/// it appears somewhere in the indexed guide, which makes the whole mechanism
/// self-limiting: a Zelda or Metroid walkthrough contains no "Roxanne", so the
/// Pokémon vocabulary silently drops out and those queries behave exactly as
/// they did before. Nothing here can degrade a guide it doesn't apply to.
fn expand_query_against(query: &str, df: &HashMap<String, u32>) -> String {
    let q = query.to_lowercase();
    let mut extra: Vec<&str> = Vec::new();

    // "first badge" / "1st gym" / "gym 1" -> ordinal index.
    const ORDINALS: [(&str, usize); 16] = [
        ("first", 1),
        ("1st", 1),
        ("second", 2),
        ("2nd", 2),
        ("third", 3),
        ("3rd", 3),
        ("fourth", 4),
        ("4th", 4),
        ("fifth", 5),
        ("5th", 5),
        ("sixth", 6),
        ("6th", 6),
        ("seventh", 7),
        ("7th", 7),
        ("eighth", 8),
        ("8th", 8),
    ];

    let mentions_gym = q.contains("gym") || q.contains("badge") || q.contains("leader");
    if mentions_gym {
        // Generic gym vocabulary helps every such query.
        extra.push("gym leader badge");
        for (word, idx) in ORDINALS {
            if q.contains(word) {
                if let Some(terms) = HOENN_GYMS.get(idx - 1) {
                    extra.push(terms);
                }
            }
        }
    }
    if q.contains("elite four") || q.contains("champion") || q.contains("league") {
        extra.push("Sidney Phoebe Glacia Drake Wallace Pokémon League Ever Grande");
    }

    if extra.is_empty() {
        return query.to_string();
    }

    // Keep only expansion words the guide actually uses. An empty `df` (the
    // plain `expand_query` entry point) disables filtering.
    let kept: Vec<String> = if df.is_empty() {
        extra.iter().map(|s| s.to_string()).collect()
    } else {
        extra
            .iter()
            .flat_map(|group| tokenize(group))
            .filter(|t| df.contains_key(t))
            .collect()
    };

    if kept.is_empty() {
        query.to_string()
    } else {
        format!("{} {}", query, kept.join(" "))
    }
}

/// Gym leader / city / badge triples in league order for Hoenn (the ROM this
/// emulator targets most). Used only to expand ordinal queries.
const HOENN_GYMS: [&str; 8] = [
    "Roxanne Rustboro Stone Badge Rock",
    "Brawly Dewford Knuckle Badge Fighting",
    "Wattson Mauville Dynamo Badge Electric",
    "Flannery Lavaridge Heat Badge Fire",
    "Norman Petalburg Balance Badge Normal",
    "Winona Fortree Feather Badge Flying",
    "Tate Liza Mossdeep Mind Badge Psychic",
    "Juan Sootopolis Rain Badge Water",
];

impl GuideLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.iter().all(|d| !d.enabled) || self.chunks.is_empty()
    }

    pub fn total_chunks(&self) -> usize {
        self.chunks.len()
    }

    pub fn total_chars(&self) -> usize {
        self.docs.iter().map(|d| d.chars).sum()
    }

    /// Loads a guide from disk. Dispatches on extension, with content sniffing
    /// as a fallback (a `.dat` that starts with `%PDF` is still a PDF).
    pub fn add_file(&mut self, path: &Path) -> Result<usize, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("cannot stat file: {}", e))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(format!(
                "file is {} MB — too large to index (limit {} MB)",
                meta.len() / (1024 * 1024),
                MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read file: {}", e))?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        let ext = path
            .extension()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        let is_pdf = bytes.starts_with(b"%PDF") || ext == "pdf";

        let (text, kind) = if is_pdf {
            (extract_pdf_text(&bytes)?, GuideKind::Pdf)
        } else {
            (decode_text(&bytes)?, GuideKind::Text)
        };

        self.add_text(name, kind, &text)
    }

    /// Indexes already-extracted text. Exposed for tests and for future
    /// sources (clipboard paste, URL fetch).
    pub fn add_text(
        &mut self,
        name: String,
        kind: GuideKind,
        raw: &str,
    ) -> Result<usize, String> {
        self.add_text_with_context(name, kind, raw, "")
    }

    /// Indexes text where every chunk is tagged with a shared context line.
    ///
    /// `context` is prepended to each chunk before tokenizing, so terms that
    /// describe the whole section stay retrievable from any chunk of it. This
    /// matters for large multi-part web guides: the body of "Rustboro Gym"
    /// often never repeats the words "Rustboro Gym" (they live in a heading or
    /// a table of contents), so a chunk about beating Roxanne is unreachable
    /// by the location name without this. The context is also what makes a
    /// vague query like "first trainer badge" land on the right chapter.
    pub fn add_text_with_context(
        &mut self,
        name: String,
        kind: GuideKind,
        raw: &str,
        context: &str,
    ) -> Result<usize, String> {
        let text = normalize_text(raw);
        if text.trim().chars().count() < 200 {
            return Err(match kind {
                GuideKind::Pdf => "no usable text layer found in this PDF (it is probably a \
                                   scan of images). Run OCR on it first, or upload a .txt guide."
                    .to_string(),
                GuideKind::Text => "file contains almost no text".to_string(),
                GuideKind::Web => "page contains almost no text".to_string(),
            });
        }
        let text: String = if text.chars().count() > MAX_DOC_CHARS {
            text.chars().take(MAX_DOC_CHARS).collect()
        } else {
            text
        };

        let doc_id = self.next_id;
        self.next_id += 1;

        let pieces = chunk_text(&text);
        let mut added = 0usize;
        let ctx = context.trim();
        for (i, piece) in pieces.into_iter().enumerate() {
            // Index the context alongside the chunk, but keep it OUT of the
            // stored text: the excerpt shown to the model should read as the
            // guide wrote it, while the term index gets the extra handles.
            let terms = if ctx.is_empty() {
                tokenize_counts(&piece)
            } else {
                tokenize_counts(&format!("{}\n{}", ctx, piece))
            };
            if terms.is_empty() {
                continue;
            }
            let len_tokens = terms.values().sum::<u32>();
            for term in terms.keys() {
                *self.df.entry(term.clone()).or_insert(0) += 1;
            }
            self.chunks.push(Chunk {
                doc_id,
                ordinal: i + 1,
                lower: piece.to_lowercase(),
                text: piece,
                terms,
                len_tokens,
            });
            added += 1;
        }

        if added == 0 {
            return Err("guide produced no indexable chunks".to_string());
        }

        self.docs.push(GuideDoc {
            id: doc_id,
            name,
            kind,
            chars: text.chars().count(),
            chunks: added,
            enabled: true,
            source_url: None,
        });
        self.recompute_avg_len();
        Ok(doc_id)
    }

    /// Indexes a multi-section document (a web guide's pages) as ONE logical
    /// document, where each section carries its own retrieval context.
    ///
    /// Sections are `(title, hint, text)`. Chunk ordinals run continuously
    /// across the whole document so citations stay unique, and each section's
    /// title+hint is indexed with every one of its chunks.
    pub fn add_sections(
        &mut self,
        name: String,
        kind: GuideKind,
        source_url: Option<String>,
        sections: &[(String, String, String)],
    ) -> Result<usize, String> {
        let doc_id = self.next_id;
        let mut ordinal = 0usize;
        let mut total_chars = 0usize;
        let mut staged: Vec<Chunk> = Vec::new();

        for (title, hint, raw) in sections {
            let text = normalize_text(raw);
            if text.trim().chars().count() < 100 {
                continue;
            }
            let text: String = if text.chars().count() > MAX_DOC_CHARS {
                text.chars().take(MAX_DOC_CHARS).collect()
            } else {
                text
            };
            total_chars += text.chars().count();

            let ctx = format!("{} {}", title, hint);
            let ctx = ctx.trim();
            for piece in chunk_text(&text) {
                let terms = tokenize_counts(&format!("{}\n{}", ctx, piece));
                if terms.is_empty() {
                    continue;
                }
                ordinal += 1;
                staged.push(Chunk {
                    doc_id,
                    ordinal,
                    lower: piece.to_lowercase(),
                    text: piece,
                    terms,
                    len_tokens: 0,
                });
            }
        }

        if staged.is_empty() {
            return Err("guide produced no indexable chunks".to_string());
        }
        if total_chars > MAX_DOC_CHARS * 40 {
            return Err("guide is implausibly large; refusing to index".to_string());
        }

        // Commit: only now mutate shared index state, so a rejected import
        // cannot leave half its terms in the document-frequency table.
        self.next_id += 1;
        let added = staged.len();
        for mut c in staged {
            c.len_tokens = c.terms.values().sum::<u32>();
            for term in c.terms.keys() {
                *self.df.entry(term.clone()).or_insert(0) += 1;
            }
            self.chunks.push(c);
        }

        self.docs.push(GuideDoc {
            id: doc_id,
            name,
            kind,
            chars: total_chars,
            chunks: added,
            enabled: true,
            source_url,
        });
        // BM25 length normalisation reads this; forgetting it silently skews
        // every future score.
        self.recompute_avg_len();
        Ok(doc_id)
    }

    pub fn remove(&mut self, doc_id: usize) {
        self.docs.retain(|d| d.id != doc_id);
        let removed: Vec<Chunk> = {
            let (keep, drop): (Vec<Chunk>, Vec<Chunk>) = self
                .chunks
                .drain(..)
                .partition(|c| c.doc_id != doc_id);
            self.chunks = keep;
            drop
        };
        for c in removed {
            for term in c.terms.keys() {
                if let Some(v) = self.df.get_mut(term) {
                    *v = v.saturating_sub(1);
                }
            }
        }
        self.df.retain(|_, v| *v > 0);
        self.recompute_avg_len();
    }

    pub fn clear(&mut self) {
        self.docs.clear();
        self.chunks.clear();
        self.df.clear();
        self.avg_len = 0.0;
        self.last_citations.clear();
    }

    fn recompute_avg_len(&mut self) {
        if self.chunks.is_empty() {
            self.avg_len = 0.0;
            return;
        }
        let total: u64 = self.chunks.iter().map(|c| c.len_tokens as u64).sum();
        self.avg_len = total as f32 / self.chunks.len() as f32;
    }

    fn doc_name(&self, doc_id: usize) -> &str {
        self.docs
            .iter()
            .find(|d| d.id == doc_id)
            .map(|d| d.name.as_str())
            .unwrap_or("guide")
    }

    fn doc_enabled(&self, doc_id: usize) -> bool {
        self.docs
            .iter()
            .find(|d| d.id == doc_id)
            .map(|d| d.enabled)
            .unwrap_or(false)
    }

    /// BM25 retrieval with an exact-phrase bonus.
    ///
    /// `query` is free text (the mission, plus the agent's last observation).
    /// Returns at most `top_k` excerpts, best first.
    pub fn retrieve(&self, query: &str, top_k: usize) -> Vec<Excerpt> {
        if self.chunks.is_empty() || top_k == 0 {
            return Vec::new();
        }
        let q_terms = tokenize(&expand_query_against(query, &self.df));
        if q_terms.is_empty() {
            return Vec::new();
        }
        let q_lower = query.to_lowercase();
        let phrases = phrase_candidates(&q_lower);

        let n = self.chunks.len() as f32;
        const K1: f32 = 1.4;
        const B: f32 = 0.72;

        let mut scored: Vec<(f32, usize)> = Vec::with_capacity(self.chunks.len());
        for (idx, chunk) in self.chunks.iter().enumerate() {
            if !self.doc_enabled(chunk.doc_id) {
                continue;
            }
            let mut score = 0.0f32;
            let mut matched_idf = 0.0f32;
            let mut total_idf = 0.0f32;
            for term in &q_terms {
                let df = *self.df.get(term).unwrap_or(&1) as f32;
                // Robertson/Sparck-Jones IDF, floored so that a term present
                // in most chunks contributes ~0 rather than a negative score.
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln().max(0.0);
                total_idf += idf;
                let Some(&tf) = chunk.terms.get(term) else {
                    continue;
                };
                matched_idf += idf;
                let tf = tf as f32;
                let norm = 1.0 - B + B * (chunk.len_tokens as f32 / self.avg_len.max(1.0));
                score += idf * (tf * (K1 + 1.0)) / (tf + K1 * norm);
            }

            // Coverage weighting.
            //
            // Plain BM25 sums per-term contributions, so a long chunk that
            // mentions ONE common query word many times outranks a chunk that
            // matches most of the query once each. On a 500-chunk walkthrough
            // that is the difference between "complete the first trainer
            // badge" returning the Elite Four (many hits on "complete" and
            // "trainer") and returning the first gym. Scaling by the share of
            // query information actually present rewards breadth of match.
            if total_idf > 0.0 {
                let coverage = (matched_idf / total_idf).clamp(0.0, 1.0);
                score *= 0.35 + 0.65 * coverage;
            }
            // Exact multi-word hits are far stronger evidence than a bag of
            // words: "rustboro gym" beating two unrelated mentions matters.
            for p in &phrases {
                if chunk.lower.contains(p.as_str()) {
                    score += 2.5 * (p.split_whitespace().count() as f32);
                }
            }
            if score > 0.0 {
                scored.push((score, idx));
            }
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        scored
            .into_iter()
            .map(|(score, idx)| {
                let c = &self.chunks[idx];
                Excerpt {
                    doc_name: self.doc_name(c.doc_id).to_string(),
                    ordinal: c.ordinal,
                    text: c.text.clone(),
                    score,
                }
            })
            .collect()
    }

    /// Retrieval formatted for injection into the prompt, capped at
    /// `char_budget` characters so a long guide cannot blow the context window.
    /// Also records citations for the UI. Returns `None` when nothing matched.
    pub fn context_block(&mut self, query: &str, top_k: usize, char_budget: usize) -> Option<String> {
        let hits = self.retrieve(query, top_k);
        if hits.is_empty() {
            self.last_citations.clear();
            return None;
        }
        let mut out = String::from(
            "GAME GUIDE EXCERPTS (retrieved from the walkthrough the user uploaded; \
             treat as authoritative game knowledge, but trust the screenshot over the \
             guide when they disagree):\n",
        );
        let mut cites = Vec::new();
        for h in hits {
            let header = format!("\n--- [{} #{}] ---\n", h.doc_name, h.ordinal);
            if out.len() + header.len() + h.text.len() > char_budget {
                let room = char_budget.saturating_sub(out.len() + header.len());
                if room < 200 {
                    break;
                }
                out.push_str(&header);
                out.extend(h.text.chars().take(room));
                out.push('…');
                cites.push(format!("{} #{}", h.doc_name, h.ordinal));
                break;
            }
            out.push_str(&header);
            out.push_str(&h.text);
            cites.push(format!("{} #{}", h.doc_name, h.ordinal));
        }
        self.last_citations = cites;
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// Tokenization & chunking
// ---------------------------------------------------------------------------

/// Words that carry no retrieval signal in walkthrough prose.
const STOPWORDS: &[&str] = &[
    "the", "and", "you", "your", "for", "with", "that", "this", "from", "have", "will", "are",
    "can", "get", "not", "but", "all", "one", "its", "it's", "into", "out", "how", "when", "then",
    "there", "here", "what", "which", "they", "them", "has", "was", "were", "been", "just", "also",
    "any", "some", "more", "than", "use", "used", "using", "very", "like", "make", "made", "after",
    "before", "once", "now", "way", "back", "over", "down", "each", "two", "new",
];

fn is_stopword(w: &str) -> bool {
    STOPWORDS.contains(&w)
}

/// Lowercased alphanumeric words, stopwords and 1-char noise removed.
pub fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                cur.push(lc);
            }
        } else if !cur.is_empty() {
            if cur.len() > 1 && !is_stopword(&cur) {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.len() > 1 && !is_stopword(&cur) {
        out.push(cur);
    }
    out
}

fn tokenize_counts(s: &str) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    for t in tokenize(s) {
        *map.entry(t).or_insert(0) += 1;
    }
    map
}

/// Contiguous 2- and 3-word spans of the query, used for phrase boosting.
fn phrase_candidates(lower_query: &str) -> Vec<String> {
    let words: Vec<&str> = lower_query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2 && !is_stopword(w))
        .collect();
    let mut out = Vec::new();
    for w in words.windows(2) {
        out.push(w.join(" "));
    }
    for w in words.windows(3) {
        out.push(w.join(" "));
    }
    out.truncate(48);
    out
}

/// Collapses the ragged whitespace that PDF extraction produces while keeping
/// paragraph structure.
fn normalize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0usize;
    for line in s.lines() {
        let trimmed = line.trim_end();
        let squeezed: String = {
            // Collapse internal runs of spaces/tabs; ASCII-art tables in text
            // guides otherwise dominate the token counts.
            let mut acc = String::with_capacity(trimmed.len());
            let mut last_ws = false;
            for ch in trimmed.chars() {
                let ws = ch.is_whitespace();
                if ws {
                    if !last_ws {
                        acc.push(' ');
                    }
                } else if ch.is_control() {
                    // drop
                } else {
                    acc.push(ch);
                }
                last_ws = ws;
            }
            acc.trim().to_string()
        };
        if squeezed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(&squeezed);
            out.push('\n');
        }
    }
    out
}

/// Splits on paragraph boundaries, packing into ~`CHUNK_CHARS` pieces with a
/// small overlap so facts spanning a boundary stay retrievable.
fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();

    let flush = |cur: &mut String, chunks: &mut Vec<String>| {
        let t = cur.trim();
        if t.chars().count() >= 40 {
            chunks.push(t.to_string());
        }
        cur.clear();
    };

    for para in text.split('\n') {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        // A single monster paragraph (common in PDF extraction) is hard-split.
        if para.chars().count() > CHUNK_CHARS * 2 {
            flush(&mut cur, &mut chunks);
            let chars: Vec<char> = para.chars().collect();
            let mut start = 0usize;
            while start < chars.len() {
                let end = (start + CHUNK_CHARS).min(chars.len());
                chunks.push(chars[start..end].iter().collect());
                if end == chars.len() {
                    break;
                }
                start = end.saturating_sub(CHUNK_OVERLAP_CHARS);
            }
            continue;
        }
        if cur.chars().count() + para.chars().count() > CHUNK_CHARS && !cur.is_empty() {
            // Carry the tail of this chunk into the next one as overlap.
            let tail: String = {
                let cs: Vec<char> = cur.chars().collect();
                let start = cs.len().saturating_sub(CHUNK_OVERLAP_CHARS);
                cs[start..].iter().collect()
            };
            flush(&mut cur, &mut chunks);
            cur.push_str(tail.trim_start());
            cur.push('\n');
        }
        cur.push_str(para);
        cur.push('\n');
    }
    flush(&mut cur, &mut chunks);
    chunks
}

// ---------------------------------------------------------------------------
// Plain-text decoding
// ---------------------------------------------------------------------------

/// Decodes a text file as UTF-8, falling back to Latin-1 (GameFAQs guides are
/// frequently CP1252) and stripping a BOM.
pub fn decode_text(bytes: &[u8]) -> Result<String, String> {
    let body = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let be = bytes.starts_with(&[0xFE, 0xFF]);
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|p| {
                if be {
                    u16::from_be_bytes([p[0], p[1]])
                } else {
                    u16::from_le_bytes([p[0], p[1]])
                }
            })
            .collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    match std::str::from_utf8(body) {
        Ok(s) => Ok(s.to_string()),
        Err(_) => Ok(body.iter().map(|&b| b as char).collect()),
    }
}

// ---------------------------------------------------------------------------
// PDF text extraction
// ---------------------------------------------------------------------------

/// Character-code -> text mapping recovered from the PDF's `/ToUnicode` CMaps.
///
/// Modern PDF producers embed *subset* fonts addressed by glyph ID, so the
/// bytes inside `Tj`/`TJ` strings are NOT text — `(\0&\05\0$)` is glyph ids
/// 0x26,0x35,0x24, not "&5$". The `/ToUnicode` CMap is the authoritative
/// translation back to characters, and every conforming producer that claims
/// extractable text emits one.
#[derive(Default)]
struct ToUnicode {
    map: HashMap<u32, String>,
    /// True when the file's CMaps use 2-byte codes (Identity-H and friends).
    two_byte: bool,
}

impl ToUnicode {
    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Decodes a raw PDF string. `hex_source` marks strings written as
    /// `<...>`, which are the ones that carry 2-byte CIDs.
    fn decode(&self, raw: &[u8], hex_source: bool) -> String {
        // A UTF-16BE BOM is self-describing; honour it before anything else.
        if raw.len() >= 2 && raw[0] == 0xFE && raw[1] == 0xFF {
            let units: Vec<u16> = raw[2..]
                .chunks_exact(2)
                .map(|p| u16::from_be_bytes([p[0], p[1]]))
                .collect();
            return String::from_utf16_lossy(&units);
        }

        let looks_two_byte = raw.len() >= 2
            && raw.len() % 2 == 0
            && (self.two_byte || hex_source)
            && raw.chunks_exact(2).filter(|p| p[0] == 0).count() * 2 >= raw.len() / 2;

        if looks_two_byte {
            let mut out = String::new();
            let mut mapped = 0usize;
            let total = raw.len() / 2;
            for p in raw.chunks_exact(2) {
                let code = u16::from_be_bytes([p[0], p[1]]) as u32;
                if let Some(s) = self.map.get(&code) {
                    out.push_str(s);
                    mapped += 1;
                } else if let Some(c) = standard_subset_glyph(code) {
                    out.push(c);
                } else if let Some(c) = char::from_u32(code) {
                    out.push(c);
                }
            }
            // If essentially nothing resolved through the CMap we still return
            // the heuristic decoding above; it is strictly better than the
            // control-character soup a raw byte cast produces.
            let _ = (mapped, total);
            return out;
        }

        let mut out = String::new();
        for &b in raw {
            match self.map.get(&(b as u32)) {
                Some(s) => out.push_str(s),
                None => out.push(b as char),
            }
        }
        out
    }
}

/// Fallback for subset fonts with no usable `/ToUnicode`.
///
/// TrueType subsets are overwhelmingly emitted in the standard Macintosh
/// glyph order, where GID 3 is `space` and the printable ASCII range follows
/// contiguously — so `char = gid + 29` over GIDs 3..=95. Applying this beats
/// emitting NUL-laced garbage; it is only ever reached when the producer
/// omitted the CMap.
fn standard_subset_glyph(gid: u32) -> Option<char> {
    if (3..=95).contains(&gid) {
        char::from_u32(gid + 29)
    } else {
        None
    }
}

/// Harvests every `/ToUnicode` CMap in the file into one merged table.
///
/// Resolving which CMap belongs to which `/Fx` in which content stream would
/// require walking the page tree and the indirect-object graph. For retrieval
/// purposes the union is sufficient: subset fonts within a single document
/// are produced by one tool from one glyph order and agree on shared codes.
///
/// Per-resource-name tables. Merging every CMap in the file into one table is
/// WRONG in practice: a document that mixes a body font with an icon/emoji
/// font has the two disagreeing about the same glyph id, and the merge makes
/// every "e" come out as an emoji. Keying by the `/Fx` name the content stream
/// actually selects with `Tf` keeps the faces apart.
#[derive(Default)]
struct FontCmaps {
    /// Resource name (`F1`, `TT0`, …) -> its decoding table.
    by_name: HashMap<String, ToUnicode>,
    /// Union of everything, used when a stream shows text without a `Tf`
    /// we could resolve (rare, but it beats emitting glyph ids).
    fallback: ToUnicode,
}

impl FontCmaps {
    fn select(&self, name: Option<&str>) -> &ToUnicode {
        name.and_then(|n| self.by_name.get(n))
            .unwrap_or(&self.fallback)
    }

    fn is_empty(&self) -> bool {
        self.by_name.is_empty() && self.fallback.is_empty()
    }
}

/// Builds the per-font decoding tables.
///
/// Two lexical passes, no page-tree walk:
///   1. every `N 0 obj` that is a `/Type /Font` gets its `/ToUnicode` CMap
///      parsed and stored under its object number;
///   2. every `/Font << /F1 5 0 R … >>` resource dictionary binds a name to
///      one of those objects.
fn collect_font_cmaps(bytes: &[u8]) -> FontCmaps {
    let mut out = FontCmaps::default();
    let mut by_obj: HashMap<u32, ToUnicode> = HashMap::new();

    // Pass 1: font objects -> CMap.
    let mut i = 0usize;
    let mut scanned = 0usize;
    while let Some(pos) = find_bytes(bytes, b"/ToUnicode", i) {
        i = pos + 10;
        scanned += 1;
        if scanned > 2048 {
            break;
        }
        let tail = &bytes[i..(i + 32).min(bytes.len())];
        let Some(cmap_obj) = parse_leading_ref(tail) else {
            continue;
        };
        // Which font object contains this /ToUnicode key?
        let Some(owner) = enclosing_object_number(bytes, pos) else {
            continue;
        };
        if by_obj.contains_key(&owner) {
            continue;
        }
        let Some(stream) = fetch_object_stream(bytes, cmap_obj) else {
            continue;
        };
        let mut tu = ToUnicode::default();
        parse_cmap_into(&stream, &mut tu);
        if !tu.is_empty() {
            for (k, v) in &tu.map {
                out.fallback.map.entry(*k).or_insert_with(|| v.clone());
            }
            out.fallback.two_byte |= tu.two_byte;
            by_obj.insert(owner, tu);
        }
    }

    // Pass 2: resource names -> font objects.
    let mut j = 0usize;
    let mut dicts = 0usize;
    while let Some(pos) = find_bytes(bytes, b"/Font", j) {
        j = pos + 5;
        dicts += 1;
        if dicts > 4096 {
            break;
        }
        // Only a resource dictionary has `/Font <<`; `/Type /Font` does not.
        let mut k = j;
        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
            k += 1;
        }
        if !(bytes.get(k) == Some(&b'<') && bytes.get(k + 1) == Some(&b'<')) {
            continue;
        }
        let end = skip_dict(bytes, k).min(bytes.len());
        for (name, obj) in parse_name_ref_pairs(&bytes[k..end]) {
            if let Some(tu) = by_obj.get(&obj) {
                out.by_name.entry(name).or_insert_with(|| ToUnicode {
                    map: tu.map.clone(),
                    two_byte: tu.two_byte,
                });
            }
        }
    }

    // Some producers inline the CMap without a resolvable reference; sweep any
    // stream that self-identifies as one into the fallback table.
    if out.is_empty() {
        let mut m = 0usize;
        while let Some(pos) = find_bytes(bytes, b"beginbfchar", m) {
            m = pos + 11;
            let start = rfind_bytes(bytes, b"begincmap", pos).unwrap_or(0);
            parse_cmap_into(&bytes[start..(pos + 4096).min(bytes.len())], &mut out.fallback);
            if out.fallback.map.len() > 4096 {
                break;
            }
        }
    }
    out
}

/// Walks backwards from `pos` to the `N G obj` header that encloses it.
fn enclosing_object_number(bytes: &[u8], pos: usize) -> Option<u32> {
    let start = rfind_bytes(bytes, b" obj", pos)?;
    // Back over "<gen> " and then the object number.
    let mut i = start;
    let mut seen_gen = false;
    while i > 0 {
        i -= 1;
        if bytes[i].is_ascii_digit() {
            continue;
        }
        if bytes[i] == b' ' && !seen_gen {
            seen_gen = true;
            continue;
        }
        break;
    }
    let slice = &bytes[i..start];
    let s = String::from_utf8_lossy(slice);
    s.split_whitespace().next()?.parse::<u32>().ok()
}

/// Extracts `/Name N 0 R` pairs from a dictionary body.
fn parse_name_ref_pairs(dict: &[u8]) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < dict.len() {
        if dict[i] != b'/' {
            i += 1;
            continue;
        }
        let ns = i + 1;
        let mut ne = ns;
        while ne < dict.len() && !is_pdf_delim(dict[ne]) {
            ne += 1;
        }
        let name = String::from_utf8_lossy(&dict[ns..ne]).to_string();
        i = ne;
        let tail = &dict[ne..(ne + 32).min(dict.len())];
        if let Some(obj) = parse_leading_ref(tail) {
            if !name.is_empty() {
                out.push((name, obj));
            }
        }
    }
    out
}

/// Parses `N G R` at the start of a slice, returning `N`.
///
/// The `R` may be followed immediately by a delimiter (`/ToUnicode 1430 0 R>>`
/// is legal and common), so tokens are cut at PDF delimiters rather than at
/// whitespace alone — splitting on whitespace yields `"R>>"` and silently
/// rejects every font in the file.
fn parse_leading_ref(tail: &[u8]) -> Option<u32> {
    let mut toks: Vec<&[u8]> = Vec::new();
    let mut i = 0usize;
    while i < tail.len() && toks.len() < 3 {
        while i < tail.len() && tail[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < tail.len() && !is_pdf_delim(tail[i]) {
            i += 1;
        }
        if i == start {
            break; // hit a delimiter where a token was expected
        }
        toks.push(&tail[start..i]);
    }
    if toks.len() < 3 || toks[2] != b"R" {
        return None;
    }
    let num: u32 = std::str::from_utf8(toks[0]).ok()?.parse().ok()?;
    std::str::from_utf8(toks[1]).ok()?.parse::<u32>().ok()?;
    Some(num)
}

/// Finds `N 0 obj ... stream ... endstream` and returns the inflated payload.
fn fetch_object_stream(bytes: &[u8], obj_num: u32) -> Option<Vec<u8>> {
    let needle = format!("{} 0 obj", obj_num);
    let mut from = 0usize;
    loop {
        let pos = find_bytes(bytes, needle.as_bytes(), from)?;
        // Guard against matching "112 0 obj" when looking for "12 0 obj".
        let ok_left = pos == 0 || !bytes[pos - 1].is_ascii_digit();
        from = pos + needle.len();
        if !ok_left {
            continue;
        }
        let limit = (pos + 4 * 1024 * 1024).min(bytes.len());
        let region = &bytes[pos..limit];
        let s_rel = find_bytes(region, b"stream", 0)?;
        let e_rel = find_bytes(region, b"endstream", s_rel)?;
        let dict = &region[..s_rel];
        let mut data = pos + s_rel + 6;
        if bytes.get(data) == Some(&b'\r') {
            data += 1;
        }
        if bytes.get(data) == Some(&b'\n') {
            data += 1;
        }
        let raw = &bytes[data..pos + e_rel];
        return if contains_bytes(dict, b"/FlateDecode") {
            inflate(raw)
        } else {
            Some(raw.to_vec())
        };
    }
}

/// Reads `beginbfchar`/`beginbfrange` blocks out of a CMap program.
fn parse_cmap_into(cmap: &[u8], tu: &mut ToUnicode) {
    let text: String = String::from_utf8_lossy(cmap).into_owned();

    // bfchar: <src> <dst>
    let mut rest = text.as_str();
    while let Some(start) = rest.find("beginbfchar") {
        let body_start = start + "beginbfchar".len();
        let end = rest[body_start..]
            .find("endbfchar")
            .map(|e| body_start + e)
            .unwrap_or(rest.len());
        for line in rest[body_start..end].lines() {
            let toks = hex_tokens(line);
            if toks.len() >= 2 {
                if let Some(code) = hex_to_code(&toks[0], tu) {
                    let val = hex_to_string(&toks[1]);
                    if !val.is_empty() {
                        tu.map.entry(code).or_insert(val);
                    }
                }
            }
        }
        rest = &rest[end..];
        if rest.len() < 10 {
            break;
        }
        rest = &rest[9.min(rest.len())..];
    }

    // bfrange: <lo> <hi> <dst>  |  <lo> <hi> [<d1> <d2> ...]
    let mut rest = text.as_str();
    while let Some(start) = rest.find("beginbfrange") {
        let body_start = start + "beginbfrange".len();
        let end = rest[body_start..]
            .find("endbfrange")
            .map(|e| body_start + e)
            .unwrap_or(rest.len());
        for line in rest[body_start..end].lines() {
            let toks = hex_tokens(line);
            if line.contains('[') {
                if toks.len() >= 3 {
                    let (Some(lo), Some(_hi)) =
                        (hex_to_code(&toks[0], tu), hex_to_code(&toks[1], tu))
                    else {
                        continue;
                    };
                    for (k, t) in toks[2..].iter().enumerate() {
                        let val = hex_to_string(t);
                        if !val.is_empty() {
                            tu.map.entry(lo + k as u32).or_insert(val);
                        }
                    }
                }
            } else if toks.len() >= 3 {
                let (Some(lo), Some(hi)) = (hex_to_code(&toks[0], tu), hex_to_code(&toks[1], tu))
                else {
                    continue;
                };
                if hi < lo || hi - lo > 65_535 {
                    continue;
                }
                let base = hex_to_scalars(&toks[2]);
                let Some(&first) = base.first() else { continue };
                for off in 0..=(hi - lo) {
                    let mut s = String::new();
                    // Only the last scalar increments across a bfrange.
                    for (idx, &u) in base.iter().enumerate() {
                        let v = if idx == base.len() - 1 {
                            u.wrapping_add(off)
                        } else {
                            u
                        };
                        if let Some(c) = char::from_u32(v) {
                            s.push(c);
                        }
                    }
                    let _ = first;
                    if !s.is_empty() {
                        tu.map.entry(lo + off).or_insert(s);
                    }
                }
            }
        }
        rest = &rest[end..];
        if rest.len() < 11 {
            break;
        }
        rest = &rest[10.min(rest.len())..];
    }
}

/// Extracts the `<....>` groups on a CMap line.
fn hex_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for ch in line.chars() {
        match ch {
            '<' => cur = Some(String::new()),
            '>' => {
                if let Some(s) = cur.take() {
                    out.push(s);
                }
            }
            c if c.is_ascii_hexdigit() => {
                if let Some(s) = cur.as_mut() {
                    s.push(c);
                }
            }
            _ => {}
        }
    }
    out
}

/// A source code in a CMap; its digit count tells us the code width.
fn hex_to_code(tok: &str, tu: &mut ToUnicode) -> Option<u32> {
    if tok.is_empty() || tok.len() > 8 {
        return None;
    }
    if tok.len() >= 4 {
        tu.two_byte = true;
    }
    u32::from_str_radix(tok, 16).ok()
}

fn hex_to_scalars(tok: &str) -> Vec<u32> {
    let units: Vec<u16> = tok
        .as_bytes()
        .chunks(4)
        .filter(|c| c.len() == 4)
        .filter_map(|c| u16::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
        .collect();
    String::from_utf16_lossy(&units).chars().map(|c| c as u32).collect()
}

fn hex_to_string(tok: &str) -> String {
    let units: Vec<u16> = tok
        .as_bytes()
        .chunks(4)
        .filter(|c| c.len() == 4)
        .filter_map(|c| u16::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
        .collect();
    String::from_utf16_lossy(&units)
        .replace('\u{0}', "")
        .to_string()
}

/// Extracts the text layer from a PDF by locating content streams, inflating
/// the FlateDecode ones, and running the text-showing operators. Character
/// codes are translated through the file's `/ToUnicode` CMaps.
///
/// This is deliberately a *lexical* extractor, not a full PDF parser: it does
/// not resolve the page tree. For strategy guides — where the goal is "get the
/// prose into a retrieval index" — reading every content stream in file order
/// is equivalent and vastly simpler.
pub fn extract_pdf_text(bytes: &[u8]) -> Result<String, String> {
    if !bytes.starts_with(b"%PDF") && find_bytes(bytes, b"%PDF", 0).is_none() {
        return Err("not a PDF file (missing %PDF header)".to_string());
    }

    let fonts = collect_font_cmaps(bytes);
    let mut out = String::new();
    let mut i = 0usize;
    let mut streams_seen = 0usize;
    let mut encrypted_hint = find_bytes(bytes, b"/Encrypt", 0).is_some();

    while let Some(pos) = find_bytes(bytes, b"stream", i) {
        // Guard: `endstream` also contains "stream"; never treat it as a start.
        if pos >= 3 && &bytes[pos - 3..pos] == b"end" {
            i = pos + 6;
            continue;
        }
        let dict_start = rfind_bytes(bytes, b"<<", pos).unwrap_or(pos.saturating_sub(512));
        let dict = &bytes[dict_start..pos];

        let mut data_start = pos + b"stream".len();
        if bytes.get(data_start) == Some(&b'\r') {
            data_start += 1;
        }
        if bytes.get(data_start) == Some(&b'\n') {
            data_start += 1;
        }
        let Some(end) = find_bytes(bytes, b"endstream", data_start) else {
            break;
        };
        i = end + b"endstream".len();
        streams_seen += 1;
        if streams_seen > 20_000 {
            break;
        }

        // Skip anything that is obviously not a content stream.
        if contains_bytes(dict, b"/Image")
            || contains_bytes(dict, b"/DCTDecode")
            || contains_bytes(dict, b"/JPXDecode")
            || contains_bytes(dict, b"/CCITTFaxDecode")
            || contains_bytes(dict, b"/XRef")
            || contains_bytes(dict, b"/FontFile")
        {
            continue;
        }

        let raw = &bytes[data_start..end];
        let decoded: Vec<u8> = if contains_bytes(dict, b"/FlateDecode") {
            match inflate(raw) {
                Some(d) => d,
                None => continue,
            }
        } else if contains_bytes(dict, b"/Filter") {
            // LZW/RunLength/ASCII85 etc. — unsupported, skip quietly.
            continue;
        } else {
            raw.to_vec()
        };

        // Only content streams have text-showing operators. This one check
        // filters out xref streams, metadata and binary junk that would
        // otherwise inject garbage into the index.
        if !(contains_bytes(&decoded, b"BT") || contains_bytes(&decoded, b"Tj") || contains_bytes(&decoded, b"TJ"))
        {
            continue;
        }
        encrypted_hint = false;
        extract_content_text(&decoded, &fonts, &mut out);
        out.push('\n');

        if out.len() > MAX_DOC_CHARS * 2 {
            break;
        }
    }

    if out.trim().is_empty() {
        if encrypted_hint {
            return Err(
                "PDF appears to be encrypted/password-protected; text cannot be extracted"
                    .to_string(),
            );
        }
        return Err(
            "no text layer found in this PDF (likely a scanned image). OCR it first, \
             or upload a .txt walkthrough."
                .to_string(),
        );
    }
    Ok(out)
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    // Streams are normally zlib-wrapped; a few writers emit raw deflate.
    if let Ok(v) = miniz_oxide::inflate::decompress_to_vec_zlib(data) {
        return Some(v);
    }
    if let Ok(v) = miniz_oxide::inflate::decompress_to_vec(data) {
        return Some(v);
    }
    // Truncated/over-long streams: retry tolerantly, keeping what inflated.
    use miniz_oxide::inflate::TINFLStatus;
    let r = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, MAX_DOC_CHARS * 2);
    match r {
        Ok(v) => Some(v),
        Err(e) if e.status == TINFLStatus::HasMoreOutput && !e.output.is_empty() => Some(e.output),
        Err(e) if !e.output.is_empty() => Some(e.output),
        Err(_) => None,
    }
}

/// Runs the text-showing operators of a decoded content stream.
///
/// Spacing and line breaks are derived from the **text matrix**, not from the
/// mere presence of a `Td`/`Tm`. Many PDF producers position every single
/// glyph with its own `Tm`, so treating each one as a separator shreds words
/// into "T h i s". Instead we track the pen position: a downward jump starts a
/// new line, and a horizontal gap wider than roughly a space advances inserts
/// one space.
#[allow(unused_assignments)]
fn extract_content_text(content: &[u8], fonts: &FontCmaps, out: &mut String) {
    let n = content.len();
    let mut i = 0usize;
    let mut pending = String::new();
    let mut nums: Vec<f64> = Vec::new();
    let mut in_array = false;

    // Text state.
    let mut font_name: Option<String> = None;
    // Most recent `/Name` operand seen; promoted to `font_name` by `Tf`.
    let mut last_name: Option<String> = None;
    let mut font_size = 10.0f64;
    let mut leading = 0.0f64;
    // Pen position of the current line start and of the pen itself.
    let mut line_x = 0.0f64;
    let mut line_y = 0.0f64;
    let mut pen_x = 0.0f64;
    let mut pen_y = 0.0f64;
    // Where the previously emitted glyph run ended.
    let mut last_x = f64::NAN;
    let mut last_y = f64::NAN;
    let mut have_pos = false;

    // Emits `pending` at the current pen position, inserting a newline or a
    // space first if the geometry calls for one. The macro necessarily writes
    // pen/position state that some expansion sites then immediately overwrite
    // (e.g. `BT` resets the matrix right after flushing), which rustc reports
    // as dead stores per expansion; they are correct at the sites that DO read
    // them, so the lint is silenced for this function only.
    macro_rules! flush_text {
        () => {
            if !pending.is_empty() {
                if have_pos {
                    let dy = last_y - pen_y;
                    let dx = pen_x - last_x;
                    let size = font_size.abs().max(1.0);
                    if dy.abs() > size * 0.5 {
                        // New baseline: a line break.
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                    } else if dx > size * 0.65 {
                        // Gap wide enough to be an inter-word space.
                        if !out.ends_with(' ') && !out.ends_with('\n') {
                            out.push(' ');
                        }
                    } else if dx < -size * 0.5 {
                        // Pen jumped backwards without changing line: a new
                        // column or a wrapped line in a table.
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                    }
                }
                out.push_str(&pending);
                // Estimate the advance: 0.5em per glyph is close enough for
                // proportional body text and only feeds the gap heuristic.
                let advance = pending.chars().count() as f64 * font_size.abs() * 0.5;
                last_x = pen_x + advance;
                last_y = pen_y;
                pen_x = last_x;
                have_pos = true;
                pending.clear();
            }
        };
    }

    while i < n {
        let c = content[i];
        match c {
            b'(' => {
                let (raw, next) = parse_literal_string(content, i);
                pending.push_str(&fonts.select(font_name.as_deref()).decode(&raw, false));
                i = next;
            }
            b'<' => {
                if content.get(i + 1) == Some(&b'<') {
                    // Inline dictionary (e.g. BDC properties): skip it wholesale.
                    i = skip_dict(content, i);
                } else {
                    let (raw, next) = parse_hex_string(content, i);
                    pending.push_str(&fonts.select(font_name.as_deref()).decode(&raw, true));
                    i = next;
                }
            }
            b'/' => {
                // A name operand. Record it, but do NOT treat it as the font
                // yet: `/GS3 gs`, `/Span <</MCID 0>> BDC` and friends are also
                // names, and binding them as the font would send every glyph
                // through the wrong CMap. Only `Tf` promotes a name to font.
                let ns = i + 1;
                let mut ne = ns;
                while ne < n && !is_pdf_delim(content[ne]) {
                    ne += 1;
                }
                last_name = Some(String::from_utf8_lossy(&content[ns..ne]).to_string());
                i = ne.max(i + 1);
            }
            b'[' => {
                in_array = true;
                nums.clear();
                i += 1;
            }
            b']' => {
                in_array = false;
                i += 1;
            }
            b'%' => {
                while i < n && content[i] != b'\n' {
                    i += 1;
                }
            }
            b')' | b'>' | b'{' | b'}' => i += 1,
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                let start = i;
                while i < n && !is_pdf_delim(content[i]) {
                    i += 1;
                }
                if i == start {
                    i += 1;
                    continue;
                }
                let tok = &content[start..i];

                if let Some(v) = parse_number(tok) {
                    if in_array {
                        // Inside a TJ array a large negative kern is a word
                        // space (units are 1/1000 em).
                        if v <= -180.0 && !pending.ends_with(' ') && !pending.is_empty() {
                            pending.push(' ');
                        }
                    } else {
                        nums.push(v);
                    }
                    continue;
                }

                match tok {
                    b"Tj" | b"TJ" => {
                        flush_text!();
                        nums.clear();
                    }
                    b"'" => {
                        // Next line, then show.
                        flush_text!();
                        line_y -= leading;
                        pen_x = line_x;
                        pen_y = line_y;
                        flush_text!();
                        nums.clear();
                    }
                    b"\"" => {
                        flush_text!();
                        line_y -= leading;
                        pen_x = line_x;
                        pen_y = line_y;
                        flush_text!();
                        nums.clear();
                    }
                    b"Tf" => {
                        // `/F1 12 Tf` — promote the pending name to the font.
                        flush_text!();
                        if last_name.is_some() {
                            font_name = last_name.clone();
                        }
                        if let Some(&sz) = nums.last() {
                            font_size = sz;
                        }
                        nums.clear();
                    }
                    b"TL" => {
                        if let Some(&l) = nums.last() {
                            leading = l;
                        }
                        nums.clear();
                    }
                    b"Td" => {
                        flush_text!();
                        if nums.len() >= 2 {
                            let ty = nums[nums.len() - 1];
                            let tx = nums[nums.len() - 2];
                            line_x += tx;
                            line_y += ty;
                            pen_x = line_x;
                            pen_y = line_y;
                        }
                        nums.clear();
                    }
                    b"TD" => {
                        flush_text!();
                        if nums.len() >= 2 {
                            let ty = nums[nums.len() - 1];
                            let tx = nums[nums.len() - 2];
                            leading = -ty;
                            line_x += tx;
                            line_y += ty;
                            pen_x = line_x;
                            pen_y = line_y;
                        }
                        nums.clear();
                    }
                    b"Tm" => {
                        flush_text!();
                        // `a b c d e f Tm` — e,f are the translation.
                        if nums.len() >= 6 {
                            line_x = nums[nums.len() - 2];
                            line_y = nums[nums.len() - 1];
                            pen_x = line_x;
                            pen_y = line_y;
                        }
                        nums.clear();
                    }
                    b"T*" => {
                        flush_text!();
                        line_y -= leading;
                        pen_x = line_x;
                        pen_y = line_y;
                        nums.clear();
                    }
                    b"BT" => {
                        flush_text!();
                        line_x = 0.0;
                        line_y = 0.0;
                        pen_x = 0.0;
                        pen_y = 0.0;
                        have_pos = false;
                        nums.clear();
                    }
                    b"ET" => {
                        flush_text!();
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                        have_pos = false;
                        nums.clear();
                    }
                    _ => {
                        nums.clear();
                    }
                }
            }
        }
    }
    flush_text!();
}

fn is_pdf_delim(c: u8) -> bool {
    c.is_ascii_whitespace()
        || matches!(
            c,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

fn parse_number(tok: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(tok).ok()?;
    if !s
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+')
    {
        return None;
    }
    if !s.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<f64>().ok()
}

/// Skips a `<< ... >>` dictionary, honouring nesting. Returns the index past it.
fn skip_dict(content: &[u8], start: usize) -> usize {
    let mut depth = 0usize;
    let mut i = start;
    while i < content.len() {
        if content[i] == b'<' && content.get(i + 1) == Some(&b'<') {
            depth += 1;
            i += 2;
        } else if content[i] == b'>' && content.get(i + 1) == Some(&b'>') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else if content[i] == b'(' {
            let (_, next) = parse_literal_string(content, i);
            i = next;
        } else {
            i += 1;
        }
    }
    content.len()
}

/// Parses a `( ... )` literal string starting at `start` (which must be `(`).
/// Returns the RAW bytes (escapes resolved, no charset assumed) and the index
/// just past the closing paren. Interpretation is the CMap's job.
fn parse_literal_string(content: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut out: Vec<u8> = Vec::new();
    let mut i = start + 1;
    let mut depth = 1usize;
    let n = content.len();
    while i < n {
        let c = content[i];
        match c {
            b'\\' => {
                i += 1;
                let Some(&e) = content.get(i) else { break };
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'(' => out.push(b'('),
                    b')' => out.push(b')'),
                    b'\\' => out.push(b'\\'),
                    b'\n' => {} // line continuation
                    b'\r' => {
                        if content.get(i + 1) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    b'0'..=b'7' => {
                        // Up to three octal digits.
                        let mut val = (e - b'0') as u32;
                        let mut k = 0;
                        while k < 2 {
                            match content.get(i + 1) {
                                Some(&d @ b'0'..=b'7') => {
                                    val = val * 8 + (d - b'0') as u32;
                                    i += 1;
                                    k += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push((val & 0xFF) as u8);
                    }
                    other => out.push(other),
                }
                i += 1;
            }
            b'(' => {
                depth += 1;
                out.push(b'(');
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return (out, i);
                }
                out.push(b')');
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    (out, n)
}

/// Parses a `< ... >` hex string into raw bytes.
fn parse_hex_string(content: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut nibbles = Vec::new();
    let mut i = start + 1;
    while i < content.len() && content[i] != b'>' {
        let c = content[i];
        if let Some(v) = (c as char).to_digit(16) {
            nibbles.push(v as u8);
        }
        i += 1;
    }
    if i < content.len() {
        i += 1; // past '>'
    }
    if nibbles.len() % 2 == 1 {
        nibbles.push(0); // odd trailing nibble is padded with 0 per spec
    }
    let raw: Vec<u8> = nibbles.chunks_exact(2).map(|p| (p[0] << 4) | p[1]).collect();
    (raw, i)
}

// ---------------------------------------------------------------------------
// Byte-slice search helpers
// ---------------------------------------------------------------------------

fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn rfind_bytes(haystack: &[u8], needle: &[u8], before: usize) -> Option<usize> {
    let end = before.min(haystack.len());
    if needle.len() > end {
        return None;
    }
    haystack[..end]
        .windows(needle.len())
        .rposition(|w| w == needle)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    find_bytes(haystack, needle, 0).is_some()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_expansion_only_adds_terms_the_guide_contains() {
        // A non-Pokémon guide must be completely unaffected by the Hoenn gym
        // vocabulary: the expansion is filtered by the document-frequency
        // table, so unknown words are dropped before they can skew scoring.
        let mut zelda = GuideLibrary::new();
        zelda
            .add_text(
                "Zelda".to_string(),
                GuideKind::Text,
                &"The first dungeon is the Deku Tree. Defeat Queen Gohma to earn the \
                  Kokiri Emerald and open the path onward. "
                    .repeat(12),
            )
            .expect("index");

        let expanded = expand_query_against("beat the first gym badge", &zelda.df);
        for leaked in ["roxanne", "rustboro", "brawly", "wattson"] {
            assert!(
                !expanded.to_lowercase().contains(leaked),
                "Pokémon term {:?} leaked into a Zelda query: {:?}",
                leaked,
                expanded
            );
        }

        // And with a matching guide the expansion does fire.
        let mut hoenn = GuideLibrary::new();
        hoenn
            .add_text(
                "Emerald".to_string(),
                GuideKind::Text,
                &"Roxanne leads the Rustboro Gym and awards the Stone Badge to trainers \
                  who defeat her Rock-type team. "
                    .repeat(12),
            )
            .expect("index");
        let expanded = expand_query_against("beat the first gym badge", &hoenn.df);
        assert!(
            expanded.to_lowercase().contains("roxanne"),
            "expansion did not fire on a matching guide: {:?}",
            expanded
        );
    }

    #[test]
    fn expansion_is_a_noop_for_unrelated_queries() {
        assert_eq!(expand_query("where is the hidden cave"), "where is the hidden cave");
    }

    #[test]
    fn literal_string_escapes() {
        let (raw, n) = parse_literal_string(b"(Hi \\(there\\)\\101)", 0);
        let tu = ToUnicode::default();
        assert_eq!(tu.decode(&raw, false), "Hi (there)A");
        assert_eq!(n, 18);
    }

    #[test]
    fn hex_string_utf16() {
        let tu = ToUnicode::default();
        let (raw, _) = parse_hex_string(b"<FEFF00480049>", 0);
        assert_eq!(tu.decode(&raw, true), "HI");
        let (raw2, _) = parse_hex_string(b"<48656C6C6F>", 0);
        assert_eq!(tu.decode(&raw2, true), "Hello");
    }

    #[test]
    fn cmap_bfchar_and_bfrange_decode_glyph_ids() {
        // A subset font addressed by glyph id, exactly like a real guide PDF.
        let cmap = b"/CIDInit /ProcSet findresource begin
begincmap
2 beginbfchar
<0026> <0043>
<0035> <0052>
endbfchar
1 beginbfrange
<0044> <0046> <0061>
endbfrange
endcmap";
        let mut tu = ToUnicode::default();
        parse_cmap_into(cmap, &mut tu);
        assert!(tu.two_byte);
        assert_eq!(tu.map.get(&0x26).map(String::as_str), Some("C"));
        assert_eq!(tu.map.get(&0x35).map(String::as_str), Some("R"));
        // bfrange 0x44..0x46 -> 'a','b','c'
        assert_eq!(tu.map.get(&0x44).map(String::as_str), Some("a"));
        assert_eq!(tu.map.get(&0x45).map(String::as_str), Some("b"));
        assert_eq!(tu.map.get(&0x46).map(String::as_str), Some("c"));

        let (raw, _) = parse_hex_string(b"<00260044004500460035>", 0);
        assert_eq!(tu.decode(&raw, true), "CabcR");
    }

    #[test]
    fn subset_glyph_fallback_beats_garbage() {
        // No CMap at all: GID order fallback must still yield readable ASCII.
        let tu = ToUnicode {
            map: HashMap::new(),
            two_byte: true,
        };
        // 'G'=0x47 -> gid 0x2A; 'Y'=0x59 -> gid 0x3C; space -> gid 3
        let raw = vec![0x00, 0x2A, 0x00, 0x03, 0x00, 0x3C];
        assert_eq!(tu.decode(&raw, true), "G Y");
    }

    #[test]
    fn content_stream_operators() {
        let stream = b"BT /F1 12 Tf 72 700 Td (Rustboro Gym) Tj 0 -14 Td [(Beat )-300(Roxanne)] TJ ET";
        let mut out = String::new();
        extract_content_text(stream, &FontCmaps::default(), &mut out);
        assert!(out.contains("Rustboro Gym"), "got {:?}", out);
        assert!(out.contains("Beat Roxanne"), "got {:?}", out);
    }

    #[test]
    fn retrieval_ranks_relevant_chunk_first() {
        let mut lib = GuideLibrary::new();
        let text = "\
CHAPTER ONE: PETALBURG WOODS
Catch a Shroomish here. Team Aqua grunt fight near the exit for a Great Ball.
Nothing else of note in this small forest area between routes.

CHAPTER TWO: RUSTBORO GYM
The first gym badge is the Stone Badge held by Leader Roxanne.
She uses Rock-type Pokemon: Geodude level 12, Geodude level 12, and Nosepass level 15.
Use a Grass or Water type such as Marshtomp or Lombre to sweep her team quickly.
Enter the gym in northern Rustboro City, defeat the two Youngster trainers, then Roxanne.
Winning awards the Stone Badge and TM39 Rock Tomb.

CHAPTER THREE: DEWFORD TOWN
Take the boat from Mr Briney. Brawly holds the Knuckle Badge and uses Fighting types.
";
        lib.add_text("emerald.txt".into(), GuideKind::Text, text).unwrap();
        let hits = lib.retrieve("complete the first trainer badge", 3);
        assert!(!hits.is_empty());
        assert!(
            hits[0].text.contains("Roxanne"),
            "expected Roxanne chunk first, got: {}",
            hits[0].text
        );
    }

    #[test]
    fn context_block_respects_budget() {
        let mut lib = GuideLibrary::new();
        let body = "Roxanne Stone Badge rock type gym leader Rustboro walkthrough section. ".repeat(200);
        lib.add_text("g.txt".into(), GuideKind::Text, &body).unwrap();
        let block = lib.context_block("Roxanne Stone Badge", 4, 1200).unwrap();
        assert!(block.len() <= 1300, "block was {} chars", block.len());
        assert!(!lib.last_citations.is_empty());
    }

    #[test]
    fn remove_document_drops_its_chunks() {
        let mut lib = GuideLibrary::new();
        let id = lib
            .add_text(
                "a.txt".into(),
                GuideKind::Text,
                &"Roxanne holds the Stone Badge in Rustboro City gym. ".repeat(40),
            )
            .unwrap();
        assert!(lib.total_chunks() > 0);
        lib.remove(id);
        assert_eq!(lib.total_chunks(), 0);
        assert!(lib.retrieve("Roxanne", 3).is_empty());
    }

    #[test]
    fn disabled_document_is_not_retrieved() {
        let mut lib = GuideLibrary::new();
        lib.add_text(
            "a.txt".into(),
            GuideKind::Text,
            &"Roxanne holds the Stone Badge in Rustboro City gym. ".repeat(40),
        )
        .unwrap();
        assert!(!lib.retrieve("Roxanne", 3).is_empty());
        lib.docs[0].enabled = false;
        assert!(lib.retrieve("Roxanne", 3).is_empty());
    }

    #[test]
    fn short_or_empty_input_is_rejected() {
        let mut lib = GuideLibrary::new();
        assert!(lib.add_text("x.txt".into(), GuideKind::Text, "too short").is_err());
    }
}
