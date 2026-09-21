//! AI Agent control dialog and live spectator panel.
//!
//! Two surfaces:
//!   * `show_dialog` — configuration: brain, endpoint, objective, pacing.
//!   * `show_panel`  — the always-visible spectator side panel showing what the
//!     agent sees, wants, and is pressing, so the user can watch it play.

use super::ai_agent::{AgentStatus, AiAgent, Brain, PlanSource, KEY_NAMES};
use egui::{Color32, RichText, Window};

#[derive(Default)]
pub struct AiAgentDialog {
    pub is_open: bool,
    pub show_panel: bool,
}

impl AiAgentDialog {
    pub fn new() -> Self {
        Self {
            is_open: false,
            show_panel: true,
        }
    }

    fn status_color(status: &AgentStatus) -> Color32 {
        match status {
            AgentStatus::Disabled => Color32::GRAY,
            AgentStatus::Idle => Color32::LIGHT_BLUE,
            AgentStatus::Thinking => Color32::from_rgb(255, 200, 60),
            AgentStatus::Acting => Color32::from_rgb(90, 220, 110),
            AgentStatus::Error(_) => Color32::from_rgb(255, 110, 110),
        }
    }

    pub fn show_dialog(
        &mut self,
        ctx: &egui::Context,
        agent: &mut AiAgent,
        rom_name: &str,
        toast: &mut Option<String>,
    ) {
        if !self.is_open {
            return;
        }
        let mut open = self.is_open;

        Window::new("🤖 AI Agent Player")
            .open(&mut open)
            .default_width(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Let an AI Agent play while you watch");
                ui.label(
                    RichText::new(
                        "The agent sees the live GBA screen, decides which buttons to press, \
                         and drives the emulated keypad. Inference runs on a worker thread — \
                         the emulator never stalls waiting on the model.",
                    )
                    .weak()
                    .small(),
                );
                ui.separator();

                // --- Master switch -------------------------------------
                ui.horizontal(|ui| {
                    let running = agent.enabled;
                    let (label, fill) = if running {
                        ("⏹ Stop Agent", Color32::from_rgb(180, 60, 60))
                    } else {
                        ("▶ Start Agent", Color32::from_rgb(40, 130, 70))
                    };
                    if ui
                        .add(egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(fill))
                        .clicked()
                    {
                        if running {
                            agent.stop();
                            *toast = Some("🤖 AI Agent stopped — controls returned to you".to_string());
                        } else {
                            agent.start(rom_name);
                            *toast = Some("🤖 AI Agent is now playing".to_string());
                        }
                    }

                    ui.add_space(8.0);
                    let st = agent.status.clone();
                    ui.label(
                        RichText::new(st.label())
                            .color(Self::status_color(&st))
                            .strong(),
                    );
                });

                ui.add_space(4.0);
                ui.checkbox(&mut self.show_panel, "Show live spectator panel");

                ui.separator();

                // --- Brain selection -----------------------------------
                ui.label(RichText::new("Agent Brain").strong());
                ui.radio_value(
                    &mut agent.config.brain,
                    Brain::VisionModel,
                    "Vision LLM (OpenAI-compatible server — llama.cpp, Ollama, vLLM, OpenAI)",
                );
                ui.radio_value(
                    &mut agent.config.brain,
                    Brain::Heuristic,
                    "Offline heuristic autopilot (no server required)",
                );

                if agent.config.brain == Brain::VisionModel {
                    ui.indent("vision_cfg", |ui| {
                        egui::Grid::new("ai_endpoint_grid")
                            .num_columns(2)
                            .spacing([8.0, 6.0])
                            .show(ui, |ui| {
                                ui.label("Endpoint:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut agent.config.endpoint)
                                        .desired_width(340.0)
                                        .hint_text("http://127.0.0.1:8080/v1/chat/completions"),
                                );
                                ui.end_row();

                                ui.label("Model:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut agent.config.model)
                                        .desired_width(340.0)
                                        .hint_text("local-vision"),
                                );
                                ui.end_row();

                                ui.label("API key:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut agent.config.api_key)
                                        .password(true)
                                        .desired_width(340.0)
                                        .hint_text("(blank for a local server)"),
                                );
                                ui.end_row();
                            });

                        ui.horizontal(|ui| {
                            if ui.button("🔌 Test Connection").clicked() {
                                agent.probe_endpoint();
                                *toast = Some("Probing AI endpoint...".to_string());
                            }
                            match &agent.last_probe {
                                Some(Ok(model)) => {
                                    ui.label(
                                        RichText::new(format!("✅ reachable — {}", model))
                                            .color(Color32::LIGHT_GREEN)
                                            .small(),
                                    );
                                }
                                Some(Err(e)) => {
                                    ui.label(
                                        RichText::new(format!("❌ {}", e))
                                            .color(Color32::LIGHT_RED)
                                            .small(),
                                    );
                                }
                                None => {
                                    ui.label(RichText::new("not tested").weak().small());
                                }
                            }
                        });

                        ui.label(
                            RichText::new(
                                "Local vision server: run ./run-llama-server-vision.sh (llama.cpp \
                                 with --mmproj). A text-only model will be rejected — the agent \
                                 needs the image projector loaded.",
                            )
                            .weak()
                            .small(),
                        );
                    });
                }

                ui.separator();

                // --- Objective -----------------------------------------
                ui.label(RichText::new("Objective given to the agent").strong());
                ui.add(
                    egui::TextEdit::multiline(&mut agent.config.objective)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );

                ui.separator();

                // --- Behaviour -----------------------------------------
                ui.collapsing("⚙ Behaviour & Tuning", |ui| {
                    ui.checkbox(
                        &mut agent.config.pause_while_thinking,
                        "Pause emulation while the agent is thinking (recommended for slow local models)",
                    );
                    ui.checkbox(
                        &mut agent.config.allow_human_coop,
                        "Co-op mode: your inputs are merged with the agent's",
                    );
                    ui.checkbox(
                        &mut agent.config.log_transcript,
                        "Log every decision to recordings/ai_session_*.jsonl",
                    );

                    ui.add(
                        egui::Slider::new(&mut agent.config.image_scale, 1..=6)
                            .text("Screenshot upscale sent to the model"),
                    );
                    ui.add(
                        egui::Slider::new(&mut agent.config.max_action_frames, 1..=600)
                            .text("Max frames per single action"),
                    );
                    ui.add(
                        egui::Slider::new(&mut agent.config.min_frames_between_decisions, 0..=600)
                            .text("Min frames between decisions"),
                    );
                    ui.add(
                        egui::Slider::new(&mut agent.config.temperature, 0.0..=1.5)
                            .text("Temperature"),
                    );
                    ui.add(
                        egui::Slider::new(&mut agent.config.max_tokens, 64..=4096)
                            .text("Max response tokens"),
                    );
                    ui.label(
                        RichText::new(
                            "Reasoning models (Qwen3, DeepSeek-R1) spend most of their budget \
                             thinking before answering — below ~800 they get cut off mid-thought \
                             and return nothing usable.",
                        )
                        .weak()
                        .small(),
                    );
                    ui.add(
                        egui::Slider::new(&mut agent.config.request_timeout_secs, 10..=600)
                            .text("Request timeout (s)"),
                    );
                });

                if let Some(path) = agent.transcript_display() {
                    ui.label(RichText::new(format!("📝 Transcript: {}", path)).weak().small());
                }
            });

        self.is_open = open;
    }

    /// Live spectator panel: the user's window into the agent's head.
    pub fn show_panel(&mut self, ctx: &egui::Context, agent: &AiAgent) {
        if !self.show_panel || !agent.enabled {
            return;
        }

        egui::SidePanel::right("ai_spectator_panel")
            .resizable(true)
            .default_width(290.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("🤖 AI Agent").color(Color32::from_rgb(120, 200, 255)));
                });

                let st = agent.status.clone();
                ui.label(
                    RichText::new(st.label())
                        .color(Self::status_color(&st))
                        .strong(),
                );

                if let Some(elapsed) = agent.thinking_elapsed() {
                    ui.label(
                        RichText::new(format!("⏳ waiting {:.1}s on model", elapsed.as_secs_f32()))
                            .weak()
                            .small(),
                    );
                }

                ui.separator();

                // Currently held buttons, as a live pad readout.
                ui.label(RichText::new("Controller").strong().small());
                let held = agent.held_buttons();
                ui.horizontal_wrapped(|ui| {
                    for (i, name) in KEY_NAMES.iter().enumerate() {
                        let (fg, bg) = if held[i] {
                            (Color32::BLACK, Color32::from_rgb(120, 230, 140))
                        } else {
                            (Color32::from_gray(140), Color32::from_gray(45))
                        };
                        let label = RichText::new(format!(" {} ", name)).color(fg).monospace().small();
                        ui.add(egui::Button::new(label).fill(bg).frame(true));
                    }
                });

                if let Some(action) = agent.current_action_label() {
                    ui.label(
                        RichText::new(format!("▶ {}", action))
                            .color(Color32::from_rgb(160, 220, 255))
                            .small(),
                    );
                }

                ui.separator();

                ui.label(RichText::new("Sees").strong().small());
                ui.label(
                    RichText::new(if agent.last_observation.is_empty() {
                        "(no observation yet)"
                    } else {
                        &agent.last_observation
                    })
                    .small(),
                );

                ui.add_space(4.0);
                ui.label(RichText::new("Wants").strong().small());
                ui.label(
                    RichText::new(if agent.last_goal.is_empty() {
                        "(no goal yet)"
                    } else {
                        &agent.last_goal
                    })
                    .color(Color32::from_rgb(200, 230, 170))
                    .small(),
                );

                ui.separator();

                egui::Grid::new("ai_stats_grid")
                    .num_columns(2)
                    .spacing([6.0, 2.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Decisions:").weak().small());
                        ui.label(RichText::new(agent.decisions.to_string()).monospace().small());
                        ui.end_row();

                        ui.label(RichText::new("Inputs sent:").weak().small());
                        ui.label(
                            RichText::new(agent.actions_executed.to_string())
                                .monospace()
                                .small(),
                        );
                        ui.end_row();

                        ui.label(RichText::new("Last latency:").weak().small());
                        ui.label(
                            RichText::new(format!("{} ms", agent.last_latency_ms))
                                .monospace()
                                .small(),
                        );
                        ui.end_row();

                        ui.label(RichText::new("Brain:").weak().small());
                        let brain = match agent.last_source {
                            Some(PlanSource::Model) => "vision model",
                            Some(PlanSource::Heuristic) => "heuristic",
                            None => "—",
                        };
                        ui.label(RichText::new(brain).monospace().small());
                        ui.end_row();
                    });

                ui.separator();
                ui.label(RichText::new("Input trace").strong().small());
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for entry in agent.action_log.iter() {
                            ui.label(RichText::new(entry).monospace().small().weak());
                        }
                    });
            });
    }
}
