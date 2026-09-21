# 🤖 AI Agent Player

Hand the controller to an AI and watch it play. The agent screenshots the live
GBA framebuffer, asks a multimodal model what to press, and drives the emulated
keypad — while a spectator panel shows you what it sees, what it wants, and
which buttons it's holding.

Open it with **`Ctrl+A`** or **Tools ➔ 🤖 AI Agent Player**.

Or start it directly from the command line — useful for streaming and kiosk setups:

```bash
# Autostart the agent against a local vision server
crabboy-advance game.gba --ai-play

# Point it somewhere else, and keep emulating while the model thinks
crabboy-advance game.gba --ai-play \
    --ai-endpoint http://127.0.0.1:8081/v1/chat/completions \
    --ai-no-pause

# No server at all
crabboy-advance game.gba --ai-play --ai-brain heuristic
```

| Flag | Meaning |
| :--- | :--- |
| `--ai-play` | Start the agent immediately on launch (also opens the spectator panel) |
| `--ai-endpoint <URL>` | OpenAI-compatible vision endpoint |
| `--ai-model <NAME>` | Model name sent in the request |
| `--ai-brain <KIND>` | `vision` (default) or `heuristic` |
| `--ai-objective <TEXT>` | Standing objective handed to the agent |
| `--ai-no-pause` | Keep emulating while the model thinks |

---

## How it works

```
 PPU framebuffer (240x160 u32)
        │  nearest-neighbour upscale (default 3x -> 720x480)
        ▼
    PNG encode ──► base64 ──► data:image/png;base64,...
        │
        ▼
  worker thread ──► curl POST /v1/chat/completions ──► vision model
        │                                                   │
        │                        {"observation","goal","actions":[…]}
        ▼                                                   │
   mpsc channel ◄───────────────────────────────────────────┘
        │
        ▼
  action queue ──► Mmu::keypad.set_key_state() ──► Gba::run_frame()
```

The emulator thread **never blocks on the network**. Inference runs on a
dedicated worker; the UI collects finished plans through an `mpsc` channel with
`try_recv`. If the model takes 40 seconds, the UI stays at 60 FPS and the HUD
shows a live "AI THINKING (12.4s)" counter.

Transport is a `curl` subprocess, matching the existing pattern in
`updater.rs` — no TLS stack is pulled into the dependency graph. The image
rides in a temp file rather than argv, since a base64 PNG blows past `ARG_MAX`.

## The plan format

The model is asked to reply with only this JSON:

```json
{
  "observation": "Title screen with the legendary Pokemon is displayed.",
  "goal": "Start the game.",
  "actions": [
    {"buttons": ["START"], "frames": 5},
    {"buttons": [], "frames": 40}
  ]
}
```

- Valid buttons: `A B START SELECT UP DOWN LEFT RIGHT L R`.
- `"buttons": []` is a wait. Multiple buttons are held simultaneously.
- `frames` is in 60ths of a second — a menu tap is 3-6, walking is 20-60.

The parser is deliberately forgiving, because local models are messy. It
survives markdown fences, leading prose, `<think>…</think>` reasoning traces,
`{"button": "A"}` singular form, `"duration"` instead of `"frames"`, and
aliases like `dpad_up` / `L1` / `east`. A plan with zero usable actions is
rejected rather than silently turning into a no-op.

## Brains

**Vision LLM** — any OpenAI-compatible endpoint.

| Backend | Endpoint |
| :--- | :--- |
| llama.cpp (`--mmproj`) | `http://127.0.0.1:8080/v1/chat/completions` |
| Ollama | `http://127.0.0.1:11434/v1/chat/completions` |
| vLLM | `http://127.0.0.1:8000/v1/chat/completions` |
| OpenAI | `https://api.openai.com/v1/chat/completions` + API key |

For the local vision stack in this repo's parent directory:

```bash
./run-llama-server-vision.sh      # llama.cpp + mmproj projector on :8080
```

> The model **must** have an image projector loaded. A text-only server will
> reject or ignore the image part. Use **Test Connection** in the dialog to
> confirm the endpoint is reachable and see which model answers.

**Heuristic autopilot** — a deterministic offline driver (xorshift64\*) that
needs no server. It explores, mashes through dialogs, and detects when the
screen stops changing. Useful when the model host is down, and as a baseline.

## Watching it play

The right-hand spectator panel streams, live:

- **Status** — Idle / Thinking (with elapsed time) / Playing / Error
- **Controller** — all ten GBA buttons, lit as the agent holds them
- **Sees** — the model's one-line observation of the current screen
- **Wants** — its current goal
- **Stats** — decisions, inputs executed, last latency, which brain answered
- **Input trace** — scrollback of every action it has taken

A badge over the game viewport mirrors the state so it's visible in fullscreen
and in recordings.

## Tuning

| Setting | Default | Notes |
| :--- | :--- | :--- |
| Pause while thinking | on | Freezes emulation during inference. Essential for slow local models on turn-based games; turn off for a continuous live feel. |
| Co-op mode | off | Your inputs are OR'd with the agent's instead of being overridden. |
| Screenshot upscale | 3x | Vision encoders struggle with a raw 240x160 image. |
| Max frames per action | 120 | Hard ceiling, so a model returning `"frames": 99999` can't run away. |
| Min frames between decisions | 0 | Raise to rate-limit a metered API. |
| Log transcript | on | Appends JSONL to `recordings/ai_session_*.jsonl`. |
| Max response tokens | 1200 | **Reasoning models need headroom** — see below. |
| Request timeout | 120s | A 27B local model takes 2-9s per decision; leave slack. |

### Reasoning models (Qwen3, DeepSeek-R1, o1-style)

These split their output: the chain of thought goes to `reasoning_content`,
and only the final answer to `content`. Two consequences, both handled:

- **Budget too low ⇒ empty answer.** If the model runs out of tokens mid-thought
  the server returns `finish_reason: "length"` with `content: ""`. That looks
  like a broken server but is just a small `max_tokens`. The agent detects this
  exact case and reports *"model ran out of tokens while reasoning
  (max_tokens=N)"* instead of a generic parse failure. Measured on Qwen3-27B: a
  simple dialog screen costs ~260 completion tokens, so the default is **1200**.
- **Answer in the wrong field.** Some servers put everything in
  `reasoning_content`. The agent falls back to parsing a plan out of it rather
  than failing.

Verified against llama.cpp + `mmproj-Qwen3.8-27B-...-F16.gguf` reading real
Pokémon Emerald frames — it read `PRESS START` off the title screen and the
partially-rendered dialog text `Hi! Sorry to keep you wa`, and chose the
correct button in both cases (2.4-8.7s per decision).

### Stall detection

If the framebuffer is pixel-identical to the previous decision, the agent tells
the model its last input had no visible effect and suggests trying something
else. This breaks the classic loop of a model pressing `A` forever at a screen
that needs `START`.

### Failure handling

A refused connection, a timeout, or an unparseable reply sets a **sticky**
error status (it survives the backoff wait, so the "AI OFFLINE" badge doesn't
flicker away) and queues a 2-second wait instead of hammering a dead endpoint.
Emulation keeps running throughout, and no button is left stuck down.

## Testing

```bash
cargo test                         # unit + integration (no network needed)
cargo test --test ai_agent_tests   # e2e against a real in-process HTTP server
cargo test --test ai_agent_ui_tests  # headless egui render of dialog + panel
```

`tests/ai_agent_tests.rs` stands up a real `TcpListener` speaking the OpenAI
wire format, asserts the request actually carries a base64 PNG, and verifies
the resulting keypad bits — including simultaneous `A+RIGHT`, recovery from a
dead endpoint, reasoning-model response shapes, and that stopping the agent
releases the pad.

**Parser totality.** The release profile sets `panic = "abort"`, so a panic on
the inference worker would kill the entire emulator, not just the agent. Model
output is untrusted input, so `parse_plan` is required to be *total*: every
input returns either a plan or an `Err`, never a panic. This is pinned by a
hostile-input test (unterminated strings, 5000-deep brace nesting, integer
overflow in `frames`, multi-byte UTF-8 at truncation boundaries, bidi control
characters) plus a deterministic 20,000-case fuzz pass over mutated and random
inputs. Any accepted plan is additionally checked for its invariants —
non-empty actions, `frames >= 1`, button indices in range.

To watch it drive a real ROM end-to-end:

```bash
./scripts/demo_ai_agent.sh "/path/to/game.gba"
```

That starts `scripts/mock_vision_server.py` (which saves every screenshot the
agent sends, so you can confirm what the model actually saw), runs the agent,
and prints where the frames and transcript landed.

Against a genuine vision server:

```bash
GBA_TEST_ROM="/path/to/game.gba" \
AI_AGENT_ENDPOINT="http://127.0.0.1:8080/v1/chat/completions" \
cargo test --release --test ai_agent_live -- --ignored --nocapture
```

## Files

| File | Role |
| :--- | :--- |
| `src/ui/ai_agent.rs` | Agent core: config, plan parsing, action queue, worker thread, PNG/base64, HTTP |
| `src/ui/ai_agent_dialog.rs` | Config dialog + live spectator side panel |
| `src/ui/mod.rs` | Integration into the frame loop, menu, hotkey, HUD badge |
| `scripts/mock_vision_server.py` | Mock OpenAI-compatible vision endpoint |
| `scripts/demo_ai_agent.sh` | One-command end-to-end demo |
