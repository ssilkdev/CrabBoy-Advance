# 🦀 CrabBoy Advance 5.1.0 — Game Boy Core & AI Game Guides

This release adds a second emulated system and teaches the AI agent to read.

---

## 🎮 Game Boy / Game Boy Color support

CrabBoy Advance is no longer GBA-only. A full DMG/CGB core (CPU, PPU, APU,
MMU, cartridge and MBC handling) sits behind a shared `EmuCore` abstraction,
so the existing frontend drives either system without branching at every call
site.

Rewind, save states and the save manager route through the active core, which
means those features work unchanged for Game Boy titles.

## 🤖 AI agent: game guides

The agent can now consult a walkthrough instead of playing blind.

**Upload a guide** — a PDF or `.txt` walkthrough is parsed, chunked and
indexed. Retrieved sections are fed to the model with every decision, and the
sections consulted are shown in the UI.

**Import an online wiki** — paste a URL (Bulbapedia, Serebii, any wiki) and
the guide is fetched and indexed the same way. Multi-part walkthroughs are
detected and followed automatically: the usual "walkthrough" URL is only a
table of contents, so the importer discovers and fetches the individual
chapter pages, then strips nav, sidebar and category chrome.

**Give it instructions** — a chat box (**Ctrl+G**, "AI Coach") accepts orders
like *"Complete the first trainer badge"*. The instruction becomes a mission
that outranks the standing objective, is carried in every prompt, and is
reported on with each decision until the agent can see it is done.

The PDF text extractor is written in-tree; the only new dependency is
`miniz_oxide`, which was already present transitively.

## 🔎 Retrieval that answers real questions

Guides speak world language ("Roxanne", "Rustboro Gym", "Stone Badge") while
players speak goal language ("the first badge"). Plain BM25 handled that
badly — *"Complete the first trainer badge"* returned an Elite Four passage,
because one long chunk repeating a common word outranked the chunk matching
most of the query.

Two fixes, measured on a landmark benchmark over the live Bulbapedia Emerald
walkthrough (563 indexed sections): **4/6 → 6/6 queries correct**.

- **Coverage weighting** — scores scale with the share of the query actually
  matched, rewarding breadth over repetition.
- **Query expansion** — ordinal gym/badge references map to the terms guides
  really use. The expansion is filtered by the index's own vocabulary, so a
  guide for a different game is provably unaffected.

## 🐞 Fixes

- **Typing no longer reaches the game.** Keyboard input drove the emulated
  keypad even while a text field had focus, so typing an instruction mashed
  A/B/START into the running game. Gamepad input is unaffected.
- **Local model compatibility.** Guide context is appended to the single
  system message; Qwen-family chat templates reject a second system turn
  outright, which broke guide-assisted play on exactly the local llama.cpp
  setups this feature targets.

---

## 📦 Assets

| File | Platform |
| --- | --- |
| `crabboy-advance-linux-x86_64` | Linux x86_64 (raw binary, used by the in-app updater) |
| `crabboy-advance-v5.1.0-linux-x86_64.tar.gz` | Linux x86_64 (portable bundle) |
| `SHA256SUMS.txt` | Checksums for every asset above |

The in-app updater verifies downloads against `SHA256SUMS.txt` before
installing, so that file ships with every release.

### Verifying a download

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
```

### Running the portable bundle

```bash
tar -xzf crabboy-advance-v5.1.0-linux-x86_64.tar.gz
./crabboy-advance-linux-x86_64
```

Optionally register the app with your desktop environment:

```bash
./packaging/linux/install.sh
```
