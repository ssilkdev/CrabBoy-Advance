//! SIO Link Cable & Wireless Adapter Networking Dialog

use crate::gba::mmu::sio::MultiplayerRole;
use crate::gba::Gba;
use egui::{Color32, RichText, Window};

#[derive(Default)]
pub struct LinkDialog {
    pub is_open: bool,
}

impl LinkDialog {
    pub fn new() -> Self {
        Self { is_open: false }
    }

    pub fn show(&mut self, ctx: &egui::Context, gba: &mut Gba, toast: &mut Option<String>) {
        if !self.is_open {
            return;
        }

        let mut open = self.is_open;
        Window::new("🔗 Link Cable & SIO Multiplayer")
            .open(&mut open)
            .default_width(450.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading("Multi-Player Link Cable & Wireless Adapter");
                ui.label(RichText::new("Connect 2 local emulator windows via UDP loopback (127.0.0.1) for Pokémon trading and battles.").weak().small());
                ui.separator();

                ui.label(RichText::new("Multiplayer Instance Role:").strong());
                let mut current_role = gba.mmu.sio.role;
                let mut role_changed = false;

                if ui.radio_value(&mut current_role, MultiplayerRole::SinglePlayer, "Standalone (Single Player)").clicked() {
                    role_changed = true;
                }
                if ui.radio_value(&mut current_role, MultiplayerRole::Player1Host, "Player 1 (Master / Host - Port 8765)").clicked() {
                    role_changed = true;
                }
                if ui.radio_value(&mut current_role, MultiplayerRole::Player2Client, "Player 2 (Slave / Client - Port 8766)").clicked() {
                    role_changed = true;
                }

                if role_changed {
                    gba.mmu.sio.set_role(current_role);
                    *toast = Some(format!("Multiplayer Role set to {:?}", current_role));
                }

                ui.separator();
                ui.heading("Connection Status");

                if gba.mmu.sio.role != MultiplayerRole::SinglePlayer {
                    let (status_text, color) = if gba.mmu.sio.is_connected {
                        (format!("🟢 Connected to Peer (Ping: {:.1}ms)", gba.mmu.sio.last_ping_ms), Color32::GREEN)
                    } else {
                        ("🟡 Waiting for Peer on 127.0.0.1...".to_string(), Color32::YELLOW)
                    };

                    ui.label(RichText::new(status_text).color(color).strong());
                    ui.label(format!("Local Socket: 127.0.0.1:{}", gba.mmu.sio.local_port));
                    ui.label(format!("Target Peer: 127.0.0.1:{}", gba.mmu.sio.peer_port));
                    ui.label(format!("Packets Sent: {} | Received: {}", gba.mmu.sio.packets_sent, gba.mmu.sio.packets_received));

                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("📡 Reconnect Socket").clicked() {
                            let _ = gba.mmu.sio.init_socket();
                            *toast = Some("Reinitialized socket".to_string());
                        }
                    });
                } else {
                    ui.label(RichText::new("⚪ Networking inactive (Single Player mode active)").weak());
                }

                ui.separator();
                ui.label(RichText::new("GBA SIO Hardware State:").strong());
                let siocnt = gba.mmu.sio.siocnt;
                let mode_str = match (siocnt >> 12) & 3 {
                    0 => "8-bit / 32-bit Normal",
                    1 => "Multi-Player Link Cable",
                    2 => "UART",
                    _ => "JOY BUS",
                };
                ui.label(format!("SIOCNT: 0x{:04X} ({})", siocnt, mode_str));
                ui.label(format!("RCNT: 0x{:04X}", gba.mmu.sio.rcnt));
                ui.label(format!("Multi-player Data: [P0: 0x{:04X}, P1: 0x{:04X}, P2: 0x{:04X}, P3: 0x{:04X}]",
                    gba.mmu.sio.siomulti[0], gba.mmu.sio.siomulti[1], gba.mmu.sio.siomulti[2], gba.mmu.sio.siomulti[3]));
            });
        self.is_open = open;
    }
}
