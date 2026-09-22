//! AI Agent Player — autonomous vision-driven gameplay.
//!
//! The agent observes the live GBA framebuffer, asks a multimodal LLM what to
//! press, and drives `Mmu::keypad` while the user watches. The emulator thread
//! NEVER blocks on the network: inference runs on a worker thread and results
//! are collected through a channel.
//!
//! Transport is a `curl` subprocess, matching the pattern already established
//! in `updater.rs` (no TLS stack pulled into the dependency graph). The wire
//! format is OpenAI-compatible `/v1/chat/completions` with an `image_url`
//! content part, which is what llama.cpp's server speaks when started with an
//! `--mmproj` projector (see `run-llama-server-vision.sh`).

use crate::gba::keypad::Key;
use crate::gba::Gba;
use crate::gba::{SCREEN_HEIGHT, SCREEN_WIDTH};
use crate::ui::game_guide::{GuideKind, GuideLibrary};

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Canonical GBA button order. Index positions match `Key as usize` and the
/// `[bool; 10]` layout already used by the TAS engine and bezel renderer.
pub const KEY_ORDER: [Key; 10] = [
    Key::A,
    Key::B,
    Key::Select,
    Key::Start,
    Key::Right,
    Key::Left,
    Key::Up,
    Key::Down,
    Key::R,
    Key::L,
];

pub const KEY_NAMES: [&str; 10] = [
    "A", "B", "SELECT", "START", "RIGHT", "LEFT", "UP", "DOWN", "R", "L",
];

/// Maps a model-supplied button name to an index into [`KEY_ORDER`].
/// Accepts common aliases so a slightly off-script model still drives the pad.
pub fn button_index(name: &str) -> Option<usize> {
    let n = name.trim().to_ascii_uppercase();
    let n = n.trim_start_matches("BUTTON_").trim_start_matches("KEY_");
    match n {
        "A" => Some(0),
        "B" => Some(1),
        "SELECT" => Some(2),
        "START" => Some(3),
        "RIGHT" | "DPAD_RIGHT" | "EAST" => Some(4),
        "LEFT" | "DPAD_LEFT" | "WEST" => Some(5),
        "UP" | "DPAD_UP" | "NORTH" => Some(6),
        "DOWN" | "DPAD_DOWN" | "SOUTH" => Some(7),
        "R" | "RB" | "R1" | "SHOULDER_R" => Some(8),
        "L" | "LB" | "L1" | "SHOULDER_L" => Some(9),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Brain {
    /// Multimodal LLM over an OpenAI-compatible HTTP endpoint.
    VisionModel,
    /// Offline rule-based driver. Requires no server; used as a fallback and
    /// to exercise the whole pipeline when the model host is down.
    Heuristic,
}

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub brain: Brain,
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    /// Objective handed to the model as standing context.
    pub objective: String,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Seconds before a single inference request is abandoned.
    pub request_timeout_secs: u32,
    /// Nearest-neighbour upscale applied to the 240x160 frame before encoding.
    /// Small frames confuse vision encoders; 3x (720x480) is a good default.
    pub image_scale: usize,
    /// Freeze emulation during inference. Essential for a slow local model on
    /// turn-based games; disable for a continuous "live" spectator feel.
    pub pause_while_thinking: bool,
    /// Minimum emulated frames between two consecutive decisions.
    pub min_frames_between_decisions: u32,
    /// Hard ceiling on how long one action may hold a button.
    pub max_action_frames: u32,
    /// Let the human press buttons at the same time as the agent.
    pub allow_human_coop: bool,
    /// Append every decision to `recordings/ai_session_*.jsonl`.
    pub log_transcript: bool,
    /// How many guide excerpts to retrieve per decision.
    pub guide_excerpts: usize,
    /// Character ceiling on the retrieved guide block in the prompt.
    pub guide_char_budget: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            brain: Brain::VisionModel,
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".to_string(),
            model: "local-vision".to_string(),
            api_key: String::new(),
            objective: "Play the game competently: get through menus, explore, \
                        and make forward progress."
                .to_string(),
            temperature: 0.4,
            // Reasoning models spend most of their budget in `reasoning_content`
            // before emitting the answer; 400 truncates them mid-thought and
            // yields an empty `content`. Measured: ~260 tokens for a simple
            // dialog screen on Qwen3-27B, so 1200 leaves real headroom.
            max_tokens: 1200,
            request_timeout_secs: 120,
            image_scale: 3,
            pause_while_thinking: true,
            min_frames_between_decisions: 0,
            max_action_frames: 120,
            allow_human_coop: false,
            log_transcript: true,
            // Four excerpts at ~900 chars each plus headers fits comfortably
            // inside a 4k-token prompt alongside the image and still gives the
            // model a real section of the walkthrough to work from.
            guide_excerpts: 4,
            guide_char_budget: 4000,
        }
    }
}

impl AgentConfig {
    /// Derives the server's health-probe URL from the chat endpoint.
    pub fn health_url(&self) -> String {
        match self.endpoint.find("/v1/") {
            Some(idx) => format!("{}/v1/models", &self.endpoint[..idx]),
            None => self.endpoint.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Plans & actions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanSource {
    Model,
    Heuristic,
}

#[derive(Clone, Debug)]
pub struct AgentAction {
    /// Indices into [`KEY_ORDER`]; empty means "hold nothing" (a wait).
    pub buttons: Vec<usize>,
    pub frames: u32,
}

impl AgentAction {
    pub fn label(&self) -> String {
        if self.buttons.is_empty() {
            format!("WAIT {}f", self.frames)
        } else {
            let names: Vec<&str> = self.buttons.iter().map(|&i| KEY_NAMES[i]).collect();
            format!("{} {}f", names.join("+"), self.frames)
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentPlan {
    pub observation: String,
    pub goal: String,
    pub actions: Vec<AgentAction>,
    pub latency_ms: u64,
    pub source: PlanSource,
    /// Free-text note addressed to the user, shown in the chat box.
    pub say: String,
    /// Model's self-report on the active mission: progress notes and whether
    /// it considers the mission finished.
    pub mission_progress: String,
    pub mission_complete: bool,
}

/// One inference request handed to the worker thread.
struct AgentRequest {
    png: Vec<u8>,
    context: String,
    config: AgentConfig,
    /// Guide excerpts retrieved for this decision, already formatted.
    guide_context: Option<String>,
}

enum AgentResponse {
    Plan(AgentPlan),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentStatus {
    Disabled,
    Idle,
    Thinking,
    Acting,
    Error(String),
}

impl AgentStatus {
    pub fn label(&self) -> String {
        match self {
            AgentStatus::Disabled => "Disabled".to_string(),
            AgentStatus::Idle => "Idle".to_string(),
            AgentStatus::Thinking => "Thinking…".to_string(),
            AgentStatus::Acting => "Playing".to_string(),
            AgentStatus::Error(e) => format!("Error: {}", e),
        }
    }
}

// ---------------------------------------------------------------------------
// Missions & chat
// ---------------------------------------------------------------------------

/// Who said a line in the chat transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    /// The human, issuing an instruction.
    User,
    /// The agent, reporting what it sees/plans.
    Agent,
    /// Emulator/system notice (mission accepted, completed, errors).
    System,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    /// Emulated frame the line was produced at.
    pub frame: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionState {
    Active,
    Done,
    Failed,
    Cancelled,
}

/// A user instruction such as "Complete the first trainer badge".
///
/// A mission is stronger than the standing objective: while one is active it
/// is the agent's primary goal, it drives guide retrieval, and the model is
/// asked to report when it is finished so the UI can close it out.
#[derive(Clone, Debug)]
pub struct Mission {
    pub text: String,
    pub state: MissionState,
    pub started_frame: u64,
    pub decisions: u32,
    /// Model's own running notes on progress, refreshed each decision.
    pub progress: String,
}

impl Mission {
    pub fn new(text: String, frame: u64) -> Self {
        Self {
            text,
            state: MissionState::Active,
            started_frame: frame,
            decisions: 0,
            progress: String::new(),
        }
    }
}

/// Progress of a background web-guide import.
#[derive(Clone, Debug)]
pub enum WebImportProgress {
    Fetching { done: usize, total: usize, label: String },
    Done(Box<crate::ui::web_guide::WebGuide>),
    Failed(String),
}

/// Handle to an in-flight web-guide import.
pub struct WebImport {
    pub url: String,
    pub done: usize,
    pub total: usize,
    pub label: String,
    rx: std::sync::mpsc::Receiver<WebImportProgress>,
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

pub struct AiAgent {
    pub enabled: bool,
    pub config: AgentConfig,
    pub status: AgentStatus,

    /// Reasoning trace rendered in the spectator panel.
    pub last_observation: String,
    pub last_goal: String,
    pub last_latency_ms: u64,
    pub last_source: Option<PlanSource>,
    pub decisions: u64,
    pub actions_executed: u64,
    /// Newest-first ring of executed action labels.
    pub action_log: VecDeque<String>,

    queue: VecDeque<AgentAction>,
    current: Option<(AgentAction, u32)>, // (action, frames remaining)
    awaiting_response: bool,
    frames_since_decision: u32,
    request_started: Option<Instant>,

    /// Screen-stall detection: repeated identical frames mean the last input
    /// did nothing, which is worth telling the model about.
    last_frame_hash: u64,
    stalled_decisions: u32,

    tx: Option<Sender<AgentRequest>>,
    rx: Option<Receiver<AgentResponse>>,
    transcript_path: Option<PathBuf>,
    heuristic_state: u64,

    /// Server reachability, filled in by `probe_endpoint`.
    pub last_probe: Option<Result<String, String>>,
    probe_result: Arc<Mutex<Option<Result<String, String>>>>,
    probe_in_flight: bool,

    // -- Missions, chat and guide knowledge ------------------------------
    /// Uploaded walkthroughs, indexed for retrieval.
    pub guides: GuideLibrary,
    /// In-flight web-guide download, if any.
    pub web_import: Option<WebImport>,
    /// Oldest-first conversation with the user.
    pub chat: Vec<ChatMessage>,
    /// The instruction currently being pursued, if any.
    pub mission: Option<Mission>,
    /// Missions already closed out, newest last.
    pub mission_history: Vec<Mission>,
    /// Guide excerpt citations used for the most recent decision.
    pub last_guide_citations: Vec<String>,
}

impl Default for AiAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl AiAgent {
    pub fn new() -> Self {
        Self {
            enabled: false,
            config: AgentConfig::default(),
            status: AgentStatus::Disabled,
            last_observation: String::new(),
            last_goal: String::new(),
            last_latency_ms: 0,
            last_source: None,
            decisions: 0,
            actions_executed: 0,
            action_log: VecDeque::with_capacity(64),
            queue: VecDeque::new(),
            current: None,
            awaiting_response: false,
            frames_since_decision: 0,
            request_started: None,
            last_frame_hash: 0,
            stalled_decisions: 0,
            tx: None,
            rx: None,
            transcript_path: None,
            heuristic_state: 0x2545_F491_4F6C_DD1D,
            last_probe: None,
            probe_result: Arc::new(Mutex::new(None)),
            probe_in_flight: false,
            guides: GuideLibrary::new(),
            web_import: None,
            chat: Vec::new(),
            mission: None,
            mission_history: Vec::new(),
            last_guide_citations: Vec::new(),
        }
    }

    // -- Chat & missions ---------------------------------------------------

    fn push_chat(&mut self, role: ChatRole, text: impl Into<String>, frame: u64) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        self.chat.push(ChatMessage {
            role,
            text,
            frame,
        });
        // The chat is a rolling window: it is rendered every frame and fed
        // (in part) back into the prompt, so it must not grow without bound.
        while self.chat.len() > 200 {
            self.chat.remove(0);
        }
    }

    /// Records a system notice in the chat.
    pub fn system_note(&mut self, text: impl Into<String>, frame: u64) {
        self.push_chat(ChatRole::System, text, frame);
    }

    /// Accepts a user instruction such as "Complete the first trainer badge"
    /// and makes it the agent's active mission.
    ///
    /// Any in-flight plan is dropped so the new instruction takes effect on
    /// the very next decision instead of after the old queue drains.
    pub fn submit_instruction(&mut self, text: &str, frame: u64) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.push_chat(ChatRole::User, text, frame);

        if let Some(prev) = self.mission.take() {
            if prev.state == MissionState::Active {
                let mut prev = prev;
                prev.state = MissionState::Cancelled;
                self.push_chat(
                    ChatRole::System,
                    format!("Superseded previous mission: {}", prev.text),
                    frame,
                );
                self.mission_history.push(prev);
            }
        }

        self.mission = Some(Mission::new(text.to_string(), frame));
        self.push_chat(ChatRole::System, format!("Mission accepted: {}", text), frame);

        // Re-plan immediately against the new instruction.
        self.queue.clear();
        self.current = None;
        self.frames_since_decision = u32::MAX;

        if !self.enabled {
            self.push_chat(
                ChatRole::System,
                "Agent is stopped — press ▶ Start Agent to begin work on this mission.",
                frame,
            );
        }
    }

    pub fn cancel_mission(&mut self, frame: u64) {
        if let Some(mut m) = self.mission.take() {
            m.state = MissionState::Cancelled;
            self.push_chat(ChatRole::System, format!("Mission cancelled: {}", m.text), frame);
            self.mission_history.push(m);
            self.queue.clear();
            self.current = None;
        }
    }

    pub fn mission_label(&self) -> Option<String> {
        self.mission.as_ref().map(|m| m.text.clone())
    }

    /// Starts a background fetch of a web guide.
    ///
    /// Runs on a worker thread: a multi-part wiki walkthrough is ~22 HTTP
    /// requests, which would freeze the emulator for seconds if done inline.
    pub fn start_web_guide_import(&mut self, url: String, follow_subpages: bool, max_pages: usize) {
        if self.web_import.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let timeout = self.config.request_timeout_secs.max(10);
        let fetch_url = url.clone();
        std::thread::Builder::new()
            .name("guide-import".into())
            .spawn(move || {
                let tx2 = tx.clone();
                let res = crate::ui::web_guide::import_web_guide(
                    &fetch_url,
                    timeout,
                    follow_subpages,
                    max_pages,
                    |done, total, label| {
                        let _ = tx2.send(WebImportProgress::Fetching {
                            done,
                            total,
                            label: label.to_string(),
                        });
                    },
                );
                let _ = tx.send(match res {
                    Ok(g) => WebImportProgress::Done(Box::new(g)),
                    Err(e) => WebImportProgress::Failed(e),
                });
            })
            .ok();

        self.web_import = Some(WebImport {
            url,
            done: 0,
            total: 1,
            label: "Fetching…".to_string(),
            rx,
        });
    }

    /// Drains import progress. Returns a user-facing message when it finishes.
    pub fn poll_web_import(&mut self, frame: u64) -> Option<Result<String, String>> {
        let Some(imp) = &mut self.web_import else {
            return None;
        };
        let mut finished: Option<Result<String, String>> = None;

        loop {
            match imp.rx.try_recv() {
                Ok(WebImportProgress::Fetching { done, total, label }) => {
                    imp.done = done;
                    imp.total = total;
                    imp.label = label;
                }
                Ok(WebImportProgress::Done(guide)) => {
                    finished = Some(self.load_web_guide(*guide, frame));
                    break;
                }
                Ok(WebImportProgress::Failed(e)) => {
                    finished = Some(Err(e));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = Some(Err("import worker stopped unexpectedly".to_string()));
                    break;
                }
            }
        }

        if let Some(res) = finished {
            self.web_import = None;
            if let Err(e) = &res {
                self.push_chat(
                    ChatRole::System,
                    format!("Guide import failed: {}", e),
                    frame,
                );
            }
            return Some(res);
        }
        None
    }

    /// Indexes an already-fetched web guide.
    ///
    /// Fetching happens on a worker thread (see `start_web_guide_import`);
    /// this is the cheap indexing half that runs on the UI thread.
    pub fn load_web_guide(
        &mut self,
        guide: crate::ui::web_guide::WebGuide,
        frame: u64,
    ) -> Result<String, String> {
        let sections: Vec<(String, String, String)> = guide
            .pages
            .iter()
            .map(|p| (p.title.clone(), p.hint.clone(), p.text.clone()))
            .collect();
        let page_count = sections.len();

        let id = self.guides.add_sections(
            guide.title.clone(),
            GuideKind::Web,
            Some(guide.root_url.clone()),
            &sections,
        )?;
        let doc = self
            .guides
            .docs
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| "guide vanished after indexing".to_string())?;

        let msg = format!(
            "Learned guide “{}” from the web ({} page{}, {} chars, {} sections indexed). \
             I'll consult it while playing.",
            doc.name,
            page_count,
            if page_count == 1 { "" } else { "s" },
            doc.chars,
            doc.chunks
        );
        self.push_chat(ChatRole::System, msg.clone(), frame);
        Ok(msg)
    }

    /// Loads a PDF/text walkthrough into the knowledge base.
    pub fn load_guide(&mut self, path: &std::path::Path, frame: u64) -> Result<String, String> {
        let id = self.guides.add_file(path)?;
        let doc = self
            .guides
            .docs
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| "guide vanished after indexing".to_string())?;
        let msg = format!(
            "Learned guide “{}” ({}, {} chars, {} sections indexed). I'll consult it while playing.",
            doc.name,
            doc.kind.label(),
            doc.chars,
            doc.chunks
        );
        self.push_chat(ChatRole::System, msg.clone(), frame);
        Ok(msg)
    }

    pub fn forget_guide(&mut self, doc_id: usize, frame: u64) {
        let name = self
            .guides
            .docs
            .iter()
            .find(|d| d.id == doc_id)
            .map(|d| d.name.clone());
        self.guides.remove(doc_id);
        if let Some(n) = name {
            self.push_chat(ChatRole::System, format!("Forgot guide “{}”.", n), frame);
        }
    }

    pub fn is_thinking(&self) -> bool {
        self.awaiting_response
    }

    pub fn current_action_label(&self) -> Option<String> {
        self.current.as_ref().map(|(a, rem)| {
            if a.buttons.is_empty() {
                format!("WAIT ({} frames left)", rem)
            } else {
                let names: Vec<&str> = a.buttons.iter().map(|&i| KEY_NAMES[i]).collect();
                format!("{} ({} frames left)", names.join("+"), rem)
            }
        })
    }

    /// Buttons the agent is holding this frame, as a `[bool; 10]` in
    /// [`KEY_ORDER`] layout (matches the bezel/TAS convention).
    pub fn held_buttons(&self) -> [bool; 10] {
        let mut held = [false; 10];
        if let Some((action, _)) = &self.current {
            for &b in &action.buttons {
                held[b] = true;
            }
        }
        held
    }

    pub fn start(&mut self, rom_name: &str) {
        if self.enabled {
            return;
        }
        self.enabled = true;
        self.status = AgentStatus::Idle;
        self.queue.clear();
        self.current = None;
        self.awaiting_response = false;
        self.frames_since_decision = u32::MAX; // decide immediately
        self.ensure_worker();
        if self.config.log_transcript {
            self.transcript_path = Some(new_transcript_path(rom_name));
        }
    }

    pub fn stop(&mut self) {
        self.enabled = false;
        self.status = AgentStatus::Disabled;
        self.queue.clear();
        self.current = None;
        // A request may still be in flight; its response is discarded on arrival.
    }

    fn ensure_worker(&mut self) {
        if self.tx.is_some() {
            return;
        }
        let (req_tx, req_rx) = mpsc::channel::<AgentRequest>();
        let (resp_tx, resp_rx) = mpsc::channel::<AgentResponse>();

        std::thread::Builder::new()
            .name("ai-agent-inference".to_string())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    let started = Instant::now();
                    let result = run_inference(&req);
                    let latency_ms = started.elapsed().as_millis() as u64;
                    let msg = match result {
                        Ok(mut plan) => {
                            plan.latency_ms = latency_ms;
                            AgentResponse::Plan(plan)
                        }
                        Err(e) => AgentResponse::Error(e),
                    };
                    if resp_tx.send(msg).is_err() {
                        break; // UI gone
                    }
                }
            })
            .ok();

        self.tx = Some(req_tx);
        self.rx = Some(resp_rx);
    }

    /// Writes the agent's held buttons into the emulated keypad. Called once
    /// per UI update, before frames are stepped.
    pub fn apply_inputs(&self, gba: &mut Gba) {
        if !self.enabled {
            return;
        }
        let held = self.held_buttons();
        for (i, key) in KEY_ORDER.iter().enumerate() {
            if self.config.allow_human_coop {
                // OR the agent's intent over whatever the human is pressing.
                if held[i] {
                    gba.mmu.keypad.set_key_state(*key, true);
                }
            } else {
                gba.mmu.keypad.set_key_state(*key, held[i]);
            }
        }
    }

    /// Advances the action cursor by the number of frames actually emulated.
    pub fn tick(&mut self, frames_run: u32) {
        if !self.enabled || frames_run == 0 {
            return;
        }
        self.frames_since_decision = self.frames_since_decision.saturating_add(frames_run);

        let mut remaining = frames_run;
        while remaining > 0 {
            match &mut self.current {
                Some((_, left)) => {
                    let consumed = remaining.min(*left);
                    *left -= consumed;
                    remaining -= consumed;
                    if *left == 0 {
                        self.current = None;
                        self.actions_executed += 1;
                    }
                }
                None => {
                    match self.queue.pop_front() {
                        Some(action) => {
                            let frames = action.frames.max(1);
                            self.push_log(action.label());
                            self.current = Some((action, frames));
                        }
                        None => break,
                    }
                }
            }
        }

        if self.current.is_none() && self.queue.is_empty() && !self.awaiting_response {
            if !matches!(self.status, AgentStatus::Error(_)) {
                self.status = AgentStatus::Idle;
            }
        } else if self.current.is_some() && !matches!(self.status, AgentStatus::Error(_)) {
            // An Error is sticky: it must survive the backoff wait action that
            // gets queued on failure, otherwise the "AI OFFLINE" indicator
            // flickers away the moment the user needs to see it. It is cleared
            // only by a successfully accepted plan.
            self.status = AgentStatus::Acting;
        }
    }

    fn push_log(&mut self, label: String) {
        self.action_log.push_front(label);
        while self.action_log.len() > 40 {
            self.action_log.pop_back();
        }
    }

    /// Issues a new decision request when the plan is exhausted.
    pub fn maybe_request(&mut self, gba: &Gba, rom_name: &str) {
        if !self.enabled || self.awaiting_response {
            return;
        }
        if self.current.is_some() || !self.queue.is_empty() {
            return;
        }
        if self.frames_since_decision < self.config.min_frames_between_decisions {
            return;
        }

        let fb = gba.get_framebuffer();
        let hash = frame_hash(fb);
        let stalled = hash == self.last_frame_hash;
        if stalled {
            self.stalled_decisions = self.stalled_decisions.saturating_add(1);
        } else {
            self.stalled_decisions = 0;
        }
        self.last_frame_hash = hash;

        if self.config.brain == Brain::Heuristic {
            let plan = self.heuristic_plan(stalled);
            self.accept_plan(plan, gba.frame_counter);
            return;
        }

        let png = match encode_png(fb, self.config.image_scale) {
            Ok(p) => p,
            Err(e) => {
                self.status = AgentStatus::Error(format!("frame encode failed: {}", e));
                return;
            }
        };

        let context = self.build_context(rom_name, gba.frame_counter, stalled);

        // Retrieve guide knowledge for THIS decision. The query blends the
        // mission (what the user asked for) with the last observation (where
        // we actually are), so retrieval tracks progress instead of returning
        // the same opening section forever.
        let guide_context = if self.guides.is_empty() {
            self.last_guide_citations.clear();
            None
        } else {
            let mut query = String::new();
            if let Some(m) = &self.mission {
                query.push_str(&m.text);
                query.push(' ');
                query.push_str(&m.progress);
                query.push(' ');
            }
            query.push_str(&self.config.objective);
            query.push(' ');
            query.push_str(&self.last_observation);
            query.push(' ');
            query.push_str(&self.last_goal);
            let block = self.guides.context_block(
                &query,
                self.config.guide_excerpts,
                self.config.guide_char_budget,
            );
            self.last_guide_citations = self.guides.last_citations.clone();
            block
        };

        let req = AgentRequest {
            png,
            context,
            config: self.config.clone(),
            guide_context,
        };

        self.ensure_worker();
        if let Some(tx) = &self.tx {
            if tx.send(req).is_ok() {
                self.awaiting_response = true;
                self.request_started = Some(Instant::now());
                self.status = AgentStatus::Thinking;
                self.frames_since_decision = 0;
            } else {
                self.status = AgentStatus::Error("inference worker died".to_string());
                self.tx = None;
                self.rx = None;
            }
        }
    }

    fn build_context(&self, rom_name: &str, frame: u64, stalled: bool) -> String {
        let mut s = String::new();
        s.push_str(&format!("Game: {}\n", rom_name));
        s.push_str(&format!("Emulated frame: {}\n", frame));
        s.push_str(&format!("Your standing objective: {}\n", self.config.objective));

        // The active mission outranks the standing objective.
        if let Some(m) = &self.mission {
            s.push_str(&format!(
                "\nCURRENT MISSION FROM THE USER (this is your priority): {}\n",
                m.text
            ));
            s.push_str(&format!(
                "You have spent {} decisions and {} emulated frames on it so far.\n",
                m.decisions,
                frame.saturating_sub(m.started_frame)
            ));
            if !m.progress.is_empty() {
                s.push_str(&format!("Your own progress notes: {}\n", m.progress));
            }
            s.push_str(
                "Report progress in \"mission_progress\" every turn, and set \
                 \"mission_complete\": true ONLY when the mission is genuinely \
                 finished and you can see the evidence on screen.\n",
            );
        }

        // Recent conversation so the model can answer follow-up instructions.
        let recent_chat: Vec<String> = self
            .chat
            .iter()
            .rev()
            .take(8)
            .filter(|m| m.role != ChatRole::System)
            .map(|m| {
                let who = match m.role {
                    ChatRole::User => "USER",
                    ChatRole::Agent => "YOU",
                    ChatRole::System => "SYSTEM",
                };
                format!("{}: {}", who, m.text)
            })
            .collect();
        if !recent_chat.is_empty() {
            s.push_str("\nRecent conversation (newest first):\n");
            for line in recent_chat {
                s.push_str(&format!("  {}\n", line));
            }
        }

        if !self.last_goal.is_empty() {
            s.push_str(&format!("Your previous goal: {}\n", self.last_goal));
        }
        if !self.action_log.is_empty() {
            let recent: Vec<String> = self.action_log.iter().take(8).cloned().collect();
            s.push_str(&format!(
                "Your last inputs (most recent first): {}\n",
                recent.join(", ")
            ));
        }
        if stalled {
            s.push_str(&format!(
                "WARNING: the screen is pixel-identical to your last decision \
                 ({} times in a row). Your previous input had no visible effect. \
                 Try a different button — if a dialog is open press A, if you are \
                 on a title screen press START, otherwise move in a direction.\n",
                self.stalled_decisions + 1
            ));
        }
        s.push_str("\nLook at the screenshot and decide the next inputs.");
        s
    }

    /// Runs ONE decision synchronously against the configured model, including
    /// guide retrieval, and applies the resulting plan.
    ///
    /// This is the same code path as the async `maybe_request`/`poll` pair,
    /// minus the worker thread, so it is what headless runs and integration
    /// tests use to exercise the real model without racing the UI loop. It
    /// BLOCKS for up to `request_timeout_secs`; never call it from the render
    /// loop of a running emulator.
    pub fn decide_now(&mut self, gba: &Gba, rom_name: &str) -> Result<AgentPlan, String> {
        let fb = gba.get_framebuffer();
        let png = encode_png(fb, self.config.image_scale)
            .map_err(|e| format!("frame encode failed: {}", e))?;
        let context = self.build_context(rom_name, gba.frame_counter, false);

        let guide_context = if self.guides.is_empty() {
            None
        } else {
            let mut query = String::new();
            if let Some(m) = &self.mission {
                query.push_str(&m.text);
                query.push(' ');
                query.push_str(&m.progress);
                query.push(' ');
            }
            query.push_str(&self.config.objective);
            query.push(' ');
            query.push_str(&self.last_observation);
            let block = self.guides.context_block(
                &query,
                self.config.guide_excerpts,
                self.config.guide_char_budget,
            );
            self.last_guide_citations = self.guides.last_citations.clone();
            block
        };

        let req = AgentRequest {
            png,
            context,
            config: self.config.clone(),
            guide_context,
        };
        let started = Instant::now();
        let mut plan = run_inference(&req)?;
        plan.latency_ms = started.elapsed().as_millis() as u64;
        self.accept_plan(plan.clone(), gba.frame_counter);
        Ok(plan)
    }

    /// Drains worker results. Call once per UI update.
    pub fn poll(&mut self, frame_counter: u64) {
        // Drain into a local buffer first: handling a message needs `&mut self`,
        // which cannot coexist with the borrow of `self.rx` held by the loop.
        let mut messages = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(msg) => messages.push(msg),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.rx = None;
            self.tx = None;
        }

        for msg in messages {
            match msg {
                AgentResponse::Plan(plan) => {
                    self.awaiting_response = false;
                    self.request_started = None;
                    if self.enabled {
                        self.accept_plan(plan, frame_counter);
                    }
                }
                AgentResponse::Error(e) => {
                    self.awaiting_response = false;
                    self.request_started = None;
                    if self.enabled {
                        self.status = AgentStatus::Error(e);
                        // Keep the emulator alive rather than hammering a dead
                        // endpoint: back off before the next attempt.
                        self.frames_since_decision = 0;
                        self.queue.push_back(AgentAction {
                            buttons: vec![],
                            frames: 120,
                        });
                    }
                }
            }
        }

        // Surface async probe results.
        let probe = self.probe_result.lock().ok().and_then(|mut g| g.take());
        if let Some(p) = probe {
            self.probe_in_flight = false;
            self.last_probe = Some(p);
        }
    }

    fn accept_plan(&mut self, plan: AgentPlan, frame_counter: u64) {
        self.decisions += 1;
        self.last_observation = plan.observation.clone();
        self.last_goal = plan.goal.clone();
        self.last_latency_ms = plan.latency_ms;
        self.last_source = Some(plan.source);

        // Surface the model's note to the user, and fold its self-report into
        // the active mission.
        if !plan.say.is_empty() {
            self.push_chat(ChatRole::Agent, plan.say.clone(), frame_counter);
        }
        let mut completed: Option<String> = None;
        if let Some(m) = &mut self.mission {
            m.decisions = m.decisions.saturating_add(1);
            if !plan.mission_progress.is_empty() {
                m.progress = plan.mission_progress.clone();
            }
            if plan.mission_complete {
                m.state = MissionState::Done;
                completed = Some(m.text.clone());
            }
        }
        if let Some(text) = completed {
            // Completion is terminal: retire the mission so the agent does not
            // keep re-planning against a goal it has already met.
            if let Some(done) = self.mission.take() {
                self.mission_history.push(done);
            }
            self.push_chat(
                ChatRole::System,
                format!("✅ Mission complete: {}", text),
                frame_counter,
            );
        }
        if matches!(self.status, AgentStatus::Error(_)) {
            self.status = AgentStatus::Acting;
        }

        if self.config.log_transcript {
            self.write_transcript(&plan, frame_counter);
        }

        let cap = self.config.max_action_frames.max(1);
        for mut a in plan.actions {
            a.frames = a.frames.clamp(1, cap);
            self.queue.push_back(a);
        }
        if self.queue.is_empty() {
            // Never let a malformed plan stall the agent forever.
            self.queue.push_back(AgentAction {
                buttons: vec![],
                frames: 30,
            });
        }
        self.status = AgentStatus::Acting;
    }

    fn write_transcript(&self, plan: &AgentPlan, frame_counter: u64) {
        let Some(path) = &self.transcript_path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let actions: Vec<serde_json::Value> = plan
            .actions
            .iter()
            .map(|a| {
                serde_json::json!({
                    "buttons": a.buttons.iter().map(|&i| KEY_NAMES[i]).collect::<Vec<_>>(),
                    "frames": a.frames,
                })
            })
            .collect();
        let record = serde_json::json!({
            "decision": self.decisions,
            "frame": frame_counter,
            "latency_ms": plan.latency_ms,
            "source": match plan.source { PlanSource::Model => "model", PlanSource::Heuristic => "heuristic" },
            "observation": plan.observation,
            "goal": plan.goal,
            "say": plan.say,
            "mission": self.mission.as_ref().map(|m| m.text.clone()),
            "mission_progress": plan.mission_progress,
            "mission_complete": plan.mission_complete,
            "guide_citations": self.last_guide_citations,
            "actions": actions,
        });
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{}", record);
        }
    }

    pub fn transcript_display(&self) -> Option<String> {
        self.transcript_path
            .as_ref()
            .map(|p| p.display().to_string())
    }

    /// Fire-and-forget reachability check against the configured server.
    pub fn probe_endpoint(&mut self) {
        if self.probe_in_flight {
            return;
        }
        self.probe_in_flight = true;
        let url = self.config.health_url();
        let slot = Arc::clone(&self.probe_result);
        std::thread::spawn(move || {
            let res = http_get(&url, 8);
            if let Ok(mut g) = slot.lock() {
                *g = Some(res.map(|body| {
                    let model = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|v| {
                            v["data"][0]["id"]
                                .as_str()
                                .or_else(|| v["models"][0]["name"].as_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| "unknown model".to_string());
                    model
                }));
            }
        });
    }

    // -- Offline heuristic brain ------------------------------------------

    fn next_rand(&mut self) -> u64 {
        // xorshift64*: deterministic, no dependency, good enough to vary inputs.
        let mut x = self.heuristic_state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.heuristic_state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn heuristic_plan(&mut self, stalled: bool) -> AgentPlan {
        let r = self.next_rand();
        let actions = if stalled {
            // Stuck: alternate the two "advance anything" buttons.
            let which = (r % 2) as usize;
            vec![
                AgentAction {
                    buttons: vec![if which == 0 { 0 } else { 3 }],
                    frames: 4,
                },
                AgentAction {
                    buttons: vec![],
                    frames: 12,
                },
            ]
        } else {
            let dir = 4 + (r % 4) as usize; // RIGHT/LEFT/UP/DOWN
            let walk = 20 + (r >> 8) % 30;
            vec![
                AgentAction {
                    buttons: vec![dir],
                    frames: walk as u32,
                },
                AgentAction {
                    buttons: vec![0],
                    frames: 4,
                },
                AgentAction {
                    buttons: vec![],
                    frames: 8,
                },
            ]
        };
        AgentPlan {
            observation: if stalled {
                "Screen unchanged since last decision; trying an advance button.".to_string()
            } else {
                "Offline heuristic driver: exploring and interacting.".to_string()
            },
            goal: "Make forward progress without a vision model.".to_string(),
            actions,
            latency_ms: 0,
            source: PlanSource::Heuristic,
            // The heuristic brain is blind: it cannot read a guide, chat, or
            // judge a mission, so it never speaks and never claims completion.
            say: String::new(),
            mission_progress: String::new(),
            mission_complete: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Inference transport
// ---------------------------------------------------------------------------

const SYSTEM_PROMPT: &str = "You are an expert Game Boy Advance player controlling a real emulator. \
You receive a screenshot of the current 240x160 GBA screen (upscaled). \
Decide the next few controller inputs.

Reply with ONLY a JSON object, no prose and no markdown fences:
{
  \"observation\": \"<what is on screen right now, one sentence>\",
  \"goal\": \"<what you are trying to do next, one sentence>\",
  \"say\": \"<a short message to the user; keep it brief or empty>\",
  \"mission_progress\": \"<progress on the user's mission, one sentence>\",
  \"mission_complete\": false,
  \"actions\": [ {\"buttons\": [\"A\"], \"frames\": 4}, {\"buttons\": [], \"frames\": 20} ]
}

Rules:
- Valid buttons: A, B, START, SELECT, UP, DOWN, LEFT, RIGHT, L, R.
- \"buttons\": [] means hold nothing (a wait). Multiple buttons are pressed simultaneously.
- \"frames\" is how long to hold, in 60ths of a second. A menu tap is 3-6 frames. Walking is 20-60.
- Return 1 to 6 actions. Always end movement with a short wait so the screen can update.
- Never invent buttons and never return an empty actions list.
- When the user has given you a MISSION, it outranks the standing objective. Work
  towards it step by step and keep \"mission_progress\" honest about where you are.
- Set \"mission_complete\": true ONLY when the screen shows the mission is actually
  done (for example the badge/item is obtained). Never claim completion speculatively.
- If GAME GUIDE EXCERPTS are supplied, they come from a walkthrough the user
  uploaded. Use them for names, locations, level requirements and route order.
  The screenshot is ground truth: if the guide disagrees with what you see,
  believe the screen and say so in \"say\".";

fn run_inference(req: &AgentRequest) -> Result<AgentPlan, String> {
    let data_url = format!("data:image/png;base64,{}", base64_encode(&req.png));

    // Guide excerpts are appended to the SINGLE system message rather than
    // sent as a second one. Many chat templates (Qwen's among them) hard-fail
    // with "System message must be at the beginning" when a second system turn
    // appears, so a separate message makes the request unusable on exactly the
    // local servers this feature targets. A labelled section inside the one
    // system prompt keeps the retrieved text just as clearly separated from
    // the live observation.
    let system_content = match &req.guide_context {
        Some(guide) => format!("{}\n\n{}", SYSTEM_PROMPT, guide),
        None => SYSTEM_PROMPT.to_string(),
    };

    let messages = vec![
        serde_json::json!({ "role": "system", "content": system_content }),
        serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": req.context },
                { "type": "image_url", "image_url": { "url": data_url } }
            ]
        }),
    ];

    let payload = serde_json::json!({
        "model": req.config.model,
        "temperature": req.config.temperature,
        "max_tokens": req.config.max_tokens,
        "stream": false,
        "messages": messages
    });

    let body = serde_json::to_vec(&payload).map_err(|e| format!("payload encode: {}", e))?;
    let raw = http_post_json(&req.config.endpoint, &body, &req.config.api_key, req.config.request_timeout_secs)?;

    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad JSON from server: {} ({})", e, truncate(&raw, 160)))?;

    if let Some(err) = v["error"]["message"].as_str() {
        return Err(format!("server error: {}", truncate(err, 200)));
    }

    let choice = &v["choices"][0];
    let message = &choice["message"];
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("");

    // Reasoning models (Qwen3, DeepSeek-R1, o1-style) split their output: the
    // chain of thought goes to `reasoning_content` and only the final answer
    // to `content`. If the token budget runs out mid-thought, `content` comes
    // back EMPTY with finish_reason=length — which looks like a broken server
    // but is really just too small a max_tokens.
    let content = message["content"].as_str().unwrap_or("");
    let reasoning = message["reasoning_content"].as_str().unwrap_or("");

    if content.trim().is_empty() {
        if finish_reason == "length" {
            return Err(format!(
                "model ran out of tokens while reasoning (max_tokens={}). \
                 Raise 'Max response tokens' — reasoning models need ~1200+.",
                req.config.max_tokens
            ));
        }
        // Some servers put the whole answer in reasoning_content. Try to
        // recover a plan from it before giving up.
        if !reasoning.trim().is_empty() {
            if let Ok(plan) = parse_plan(reasoning) {
                return Ok(plan);
            }
            return Err(format!(
                "model returned only reasoning, no answer: {}",
                truncate(reasoning, 160)
            ));
        }
        return Err(format!("empty response from model ({})", truncate(&raw, 160)));
    }

    parse_plan(content)
}

/// Extracts a plan from model output, tolerating markdown fences, leading
/// chatter, and `<think>` blocks emitted by reasoning models.
pub fn parse_plan(content: &str) -> Result<AgentPlan, String> {
    let cleaned = strip_reasoning(content);
    let json_slice = extract_json_object(&cleaned)
        .ok_or_else(|| format!("no JSON object in model output: {}", truncate(&cleaned, 160)))?;

    let v: serde_json::Value = serde_json::from_str(&json_slice)
        .map_err(|e| format!("unparseable plan JSON: {} ({})", e, truncate(&json_slice, 160)))?;

    let observation = v["observation"].as_str().unwrap_or("").trim().to_string();
    let goal = v["goal"].as_str().unwrap_or("").trim().to_string();

    let mut actions = Vec::new();
    if let Some(arr) = v["actions"].as_array() {
        for item in arr.iter().take(8) {
            // "buttons" may be an array, a bare string, or absent (a wait).
            let mut buttons = Vec::new();
            match &item["buttons"] {
                serde_json::Value::Array(list) => {
                    for b in list {
                        if let Some(name) = b.as_str() {
                            if let Some(idx) = button_index(name) {
                                if !buttons.contains(&idx) {
                                    buttons.push(idx);
                                }
                            }
                        }
                    }
                }
                serde_json::Value::String(s) => {
                    if let Some(idx) = button_index(s) {
                        buttons.push(idx);
                    }
                }
                _ => {
                    // Some models emit {"button": "A"} instead.
                    if let Some(s) = item["button"].as_str() {
                        if let Some(idx) = button_index(s) {
                            buttons.push(idx);
                        }
                    }
                }
            }

            let frames = item["frames"]
                .as_u64()
                .or_else(|| item["duration"].as_u64())
                .unwrap_or(6) as u32;

            actions.push(AgentAction {
                buttons,
                frames: frames.max(1),
            });
        }
    }

    if actions.is_empty() {
        return Err(format!(
            "plan contained no usable actions: {}",
            truncate(&json_slice, 160)
        ));
    }

    let say = v["say"]
        .as_str()
        .or_else(|| v["message"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let mission_progress = v["mission_progress"]
        .as_str()
        .or_else(|| v["progress"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    // Accept both a bool and the strings models like to emit instead.
    let mission_complete = v["mission_complete"].as_bool().unwrap_or_else(|| {
        matches!(
            v["mission_complete"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "true" | "yes" | "done" | "complete" | "completed"
        )
    });

    Ok(AgentPlan {
        observation,
        goal,
        actions,
        latency_ms: 0,
        source: PlanSource::Model,
        say,
        mission_progress,
        mission_complete,
    })
}

fn strip_reasoning(s: &str) -> String {
    // Drop <think>...</think> spans (Qwen-style reasoning traces).
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Returns the first balanced `{...}` span, ignoring braces inside strings.
fn extract_json_object(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        let cut: String = t.chars().take(max).collect();
        format!("{}…", cut)
    }
}

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

fn http_get(url: &str, timeout_secs: u32) -> Result<String, String> {
    let mut cmd = std::process::Command::new(curl_path());
    cmd.args([
        "-s",
        "-S",
        "--max-time",
        &timeout_secs.to_string(),
        url,
    ]);
    silence_console(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("curl failed to start: {}", e))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "cannot reach {} ({})",
            url,
            truncate(if err.is_empty() { "connection refused" } else { &err }, 120)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// POSTs a JSON body. The payload carries a base64 image and can be hundreds of
/// kilobytes, so it goes through a temp file rather than argv.
fn http_post_json(url: &str, body: &[u8], api_key: &str, timeout_secs: u32) -> Result<String, String> {
    let tmp = std::env::temp_dir().join(format!(
        "crabboy_ai_req_{}_{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::write(&tmp, body).map_err(|e| format!("temp payload write failed: {}", e))?;

    let mut cmd = std::process::Command::new(curl_path());
    cmd.args([
        "-s",
        "-S",
        "--max-time",
        &timeout_secs.to_string(),
        "-H",
        "Content-Type: application/json",
        "-H",
        "Expect:",
    ]);
    if !api_key.trim().is_empty() {
        cmd.arg("-H")
            .arg(format!("Authorization: Bearer {}", api_key.trim()));
    }
    cmd.arg("--data-binary")
        .arg(format!("@{}", tmp.display()))
        .arg(url);
    silence_console(&mut cmd);

    let out = cmd.output();
    let _ = std::fs::remove_file(&tmp);

    let out = out.map_err(|e| format!("curl failed to start: {}", e))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let detail = if err.trim().is_empty() {
            "connection refused — is the vision server running?".to_string()
        } else {
            truncate(&err, 160)
        };
        return Err(format!("{} ({})", detail, url));
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        return Err(format!("empty response from {}", url));
    }
    Ok(text)
}

// ---------------------------------------------------------------------------
// Frame encoding
// ---------------------------------------------------------------------------

fn frame_hash(fb: &[u32; SCREEN_WIDTH * SCREEN_HEIGHT]) -> u64 {
    // FNV-1a over a strided sample: full-buffer hashing every decision is
    // wasted work when we only need "did anything visibly change".
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for i in (0..fb.len()).step_by(7) {
        h ^= fb[i] as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Nearest-neighbour upscales the framebuffer and encodes it as PNG in memory.
pub fn encode_png(
    fb: &[u32; SCREEN_WIDTH * SCREEN_HEIGHT],
    scale: usize,
) -> Result<Vec<u8>, String> {
    let scale = scale.clamp(1, 6);
    let w = SCREEN_WIDTH * scale;
    let h = SCREEN_HEIGHT * scale;
    let mut rgb = Vec::with_capacity(w * h * 3);

    for y in 0..h {
        let sy = y / scale;
        for x in 0..w {
            let sx = x / scale;
            let p = fb[sy * SCREEN_WIDTH + sx];
            rgb.push((p & 0xFF) as u8);
            rgb.push(((p >> 8) & 0xFF) as u8);
            rgb.push(((p >> 16) & 0xFF) as u8);
        }
    }

    let mut out = Vec::with_capacity(w * h);
    {
        let encoder = image::codecs::png::PngEncoder::new(&mut out);
        image::ImageEncoder::write_image(
            encoder,
            &rgb,
            w as u32,
            h as u32,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding. Avoids taking a dependency for ~20 lines.
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;

        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn new_transcript_path(rom_name: &str) -> PathBuf {
    let safe: String = rom_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    PathBuf::from("recordings").join(format!("ai_session_{}_{}.jsonl", safe, ts))
}

/// How long the in-flight request has been running, for the "Thinking…" HUD.
impl AiAgent {
    pub fn thinking_elapsed(&self) -> Option<Duration> {
        self.request_started.map(|t| t.elapsed())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn parses_a_clean_plan() {
        let plan = parse_plan(
            r#"{"observation":"Title screen","goal":"Start the game","actions":[{"buttons":["START"],"frames":5},{"buttons":[],"frames":30}]}"#,
        )
        .expect("should parse");
        assert_eq!(plan.observation, "Title screen");
        assert_eq!(plan.actions.len(), 2);
        assert_eq!(plan.actions[0].buttons, vec![3]); // START
        assert_eq!(plan.actions[0].frames, 5);
        assert!(plan.actions[1].buttons.is_empty());
    }

    #[test]
    fn tolerates_fences_reasoning_and_prose() {
        let messy = "<think>The screen shows a menu, I should press A.</think>\n\
                     Sure! Here is my plan:\n\
                     ```json\n\
                     {\"observation\":\"Dialog box\",\"goal\":\"Advance text\",\
                      \"actions\":[{\"buttons\":[\"A\"],\"frames\":4}]}\n\
                     ```\n";
        let plan = parse_plan(messy).expect("should parse through the noise");
        assert_eq!(plan.goal, "Advance text");
        assert_eq!(plan.actions[0].buttons, vec![0]);
    }

    #[test]
    fn accepts_alias_shapes_and_combined_buttons() {
        let plan = parse_plan(
            r#"{"actions":[{"button":"dpad_up","duration":12},{"buttons":["B","Right"],"frames":9}]}"#,
        )
        .expect("aliases should resolve");
        assert_eq!(plan.actions[0].buttons, vec![6]); // UP
        assert_eq!(plan.actions[0].frames, 12);
        assert_eq!(plan.actions[1].buttons, vec![1, 4]); // B + RIGHT
    }

    #[test]
    fn rejects_plans_with_no_actions() {
        assert!(parse_plan(r#"{"observation":"nothing","actions":[]}"#).is_err());
        assert!(parse_plan("I refuse to answer in JSON.").is_err());
    }

    #[test]
    fn action_queue_consumes_exactly_the_frames_requested() {
        let mut agent = AiAgent::new();
        agent.config.brain = Brain::Heuristic;
        agent.config.log_transcript = false;
        agent.enabled = true;
        agent.queue.push_back(AgentAction { buttons: vec![0], frames: 3 });
        agent.queue.push_back(AgentAction { buttons: vec![4], frames: 2 });

        agent.tick(1);
        assert_eq!(agent.held_buttons()[0], true, "A held on first frame");
        agent.tick(2);
        // 3 frames consumed -> first action retired, second not yet started.
        assert_eq!(agent.actions_executed, 1);
        agent.tick(1);
        assert!(agent.held_buttons()[4], "RIGHT now held");
        agent.tick(1);
        assert_eq!(agent.actions_executed, 2);
        assert_eq!(agent.held_buttons(), [false; 10], "pad released when idle");
    }

    #[test]
    fn png_encoder_emits_a_valid_signature_and_scales() {
        let fb = [0x00FF_8040u32; SCREEN_WIDTH * SCREEN_HEIGHT];
        let png = encode_png(&fb, 3).expect("encode");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR width/height are big-endian at offsets 16 and 20.
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert_eq!((w, h), (720, 480));
    }

    #[test]
    fn health_url_is_derived_from_the_chat_endpoint() {
        let mut cfg = AgentConfig::default();
        cfg.endpoint = "http://127.0.0.1:8080/v1/chat/completions".to_string();
        assert_eq!(cfg.health_url(), "http://127.0.0.1:8080/v1/models");
    }

    #[test]
    fn default_token_budget_fits_a_reasoning_model() {
        // Measured against Qwen3-27B via llama.cpp: a simple dialog screen
        // costs ~260 completion tokens including the reasoning trace. A budget
        // near 400 truncates mid-thought and returns an EMPTY content field.
        assert!(
            AgentConfig::default().max_tokens >= 800,
            "default max_tokens must leave room for a reasoning trace"
        );
    }

    #[test]
    fn plan_parses_from_a_real_qwen3_vision_reply() {
        // Verbatim content from the local llama.cpp vision server reading a
        // real Pokemon Emerald dialog frame.
        let content = r#"{"observation":"A character (a scientist/professor) is shown on a yellow oval with a dialogue box at the bottom displaying the partially cut-off text 'Hi! Sorry to keep you wa', indicating an in-progress conversation that needs to be advanced.","goal":"Advance the dialogue by pressing A to continue the conversation with the character.","actions":[{"buttons":["A"],"frames":6}]}"#;
        let plan = parse_plan(content).expect("real model output must parse");
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].buttons, vec![0]); // A
        assert_eq!(plan.actions[0].frames, 6);
        assert!(plan.observation.contains("Sorry to keep you wa"));
    }
}
