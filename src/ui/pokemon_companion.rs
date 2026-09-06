//! Pokémon Gen 3 Live Companion: Party Monitor, IV/EV & Shiny Tracker

use crate::gba::mmu::Mmu;
use crate::gba::Gba;
use egui::{Color32, ProgressBar, RichText, Window};

#[derive(Debug, Clone)]
pub struct DecryptedPokemon {
    pub species_id: u16,
    pub species_name: String,
    pub nickname: String,
    pub level: u8,
    pub current_hp: u16,
    pub max_hp: u16,
    pub nature: &'static str,
    pub is_shiny: bool,
    pub is_egg: bool,
    pub iv_hp: u8,
    pub iv_atk: u8,
    pub iv_def: u8,
    pub iv_spd: u8,
    pub iv_spatk: u8,
    pub iv_spdef: u8,
    pub ev_hp: u8,
    pub ev_atk: u8,
    pub ev_def: u8,
    pub ev_spd: u8,
    pub ev_spatk: u8,
    pub ev_spdef: u8,
    pub moves: [u16; 4],
    pub pp: [u8; 4],
}

#[derive(Default)]
pub struct PokemonCompanion {
    pub is_open: bool,
    pub party: Vec<DecryptedPokemon>,
}

const NATURES: [&str; 25] = [
    "Hardy", "Lonely", "Brave", "Adamant", "Naughty",
    "Bold", "Docile", "Relaxed", "Impish", "Lax",
    "Timid", "Hasty", "Serious", "Jolly", "Naive",
    "Modest", "Mild", "Quiet", "Bashful", "Rash",
    "Calm", "Gentle", "Sassy", "Careful", "Quirky",
];

// 24 permutations of Growth (0), Attacks (1), EVs (2), Misc (3)
const SUBSTRUCT_ORDERS: [[usize; 4]; 24] = [
    [0, 1, 2, 3], [0, 1, 3, 2], [0, 2, 1, 3], [0, 2, 3, 1], [0, 3, 1, 2], [0, 3, 2, 1],
    [1, 0, 2, 3], [1, 0, 3, 2], [1, 2, 0, 3], [1, 2, 3, 0], [1, 3, 0, 2], [1, 3, 2, 0],
    [2, 0, 1, 3], [2, 0, 3, 1], [2, 1, 0, 3], [2, 1, 3, 0], [2, 3, 0, 1], [2, 3, 1, 0],
    [3, 0, 1, 2], [3, 0, 2, 1], [3, 1, 0, 2], [3, 1, 2, 0], [3, 2, 0, 1], [3, 2, 1, 0],
];

impl PokemonCompanion {
    pub fn new() -> Self {
        Self {
            is_open: false,
            party: Vec::with_capacity(6),
        }
    }

    pub fn poll_party_memory(&mut self, mmu: &mut Mmu, game_code: &str) {
        self.party.clear();

        // Check if game is a compatible Gen 3 RPG cartridge: BPE, BPR, AXV, AXP, BPG
        let is_pokemon = game_code.starts_with("BPE") || game_code.starts_with("BPR")
            || game_code.starts_with("AXV") || game_code.starts_with("AXP") || game_code.starts_with("BPG");

        if !is_pokemon {
            return;
        }

        // Determine SaveBlock1 pointer address based on game code family
        let ptr_addr = if game_code.starts_with("BPE") {
            0x0300_5D8C // BPE family gSaveBlock1Ptr
        } else if game_code.starts_with("BPR") || game_code.starts_with("BPG") {
            0x0300_5008 // BPR/BPG family gSaveBlock1Ptr
        } else {
            0x0300_5D90 // AXV/AXP family gSaveBlock1Ptr
        };

        let sb1_ptr = mmu.read32(ptr_addr);
        if !(0x0200_0000..=0x0203_FFFF).contains(&sb1_ptr) {
            return;
        }

        let party_offset = if game_code.starts_with("BPE") {
            0x0234
        } else if game_code.starts_with("BPR") || game_code.starts_with("BPG") {
            0x0034
        } else {
            0x0234
        };

        let party_count = (mmu.read32(sb1_ptr + party_offset) & 0xFF) as usize;
        let count = party_count.min(6);
        let party_base = sb1_ptr + party_offset + 4;

        for slot in 0..count {
            let addr = party_base + (slot as u32) * 100;
            let mut raw = [0u8; 100];
            for i in 0..100 {
                raw[i] = mmu.read8(addr + i as u32);
            }

            if let Some(pkmn) = Self::decrypt_pokemon(&raw) {
                self.party.push(pkmn);
            }
        }
    }

    pub fn decrypt_pokemon(raw: &[u8; 100]) -> Option<DecryptedPokemon> {
        let personality = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let ot_id = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);

        if personality == 0 && ot_id == 0 {
            return None;
        }

        // Decode Nickname (Gen 3 GBA character encoding: 0xBB='A' .. 0xD4='Z', 0xD5='a' .. 0xEE='z')
        let mut nickname = String::new();
        for &b in &raw[8..18] {
            if b == 0xFF { break; }
            if (0xBB..=0xD4).contains(&b) {
                nickname.push((b - 0xBB + b'A') as char);
            } else if (0xD5..=0xEE).contains(&b) {
                nickname.push((b - 0xD5 + b'a') as char);
            } else if (0xA1..=0xAA).contains(&b) {
                nickname.push((b - 0xA1 + b'0') as char);
            } else if b == 0x00 {
                nickname.push(' ');
            }
        }
        if nickname.is_empty() {
            nickname = "POKÉMON".to_string();
        }

        // Decrypt 48 bytes of data (4 substructs of 12 bytes each)
        let key = personality ^ ot_id;
        let mut decrypted = [0u8; 48];
        for i in 0..12 {
            let chunk = u32::from_le_bytes([
                raw[32 + i * 4],
                raw[33 + i * 4],
                raw[34 + i * 4],
                raw[35 + i * 4],
            ]);
            let dec_word = (chunk ^ key).to_le_bytes();
            decrypted[i * 4..i * 4 + 4].copy_from_slice(&dec_word);
        }

        let order_idx = ((personality % 24) as usize).min(23);
        let order = SUBSTRUCT_ORDERS[order_idx];

        // Locate substructs in decrypted buffer: Growth (0), Attacks (1), EVs (2), Misc (3)
        let get_substruct = |target: usize| -> usize {
            for (pos, &id) in order.iter().enumerate() {
                if id == target {
                    return pos * 12;
                }
            }
            0
        };

        let g_off = get_substruct(0);
        let a_off = get_substruct(1);
        let e_off = get_substruct(2);
        let m_off = get_substruct(3);

        let species_id = u16::from_le_bytes([decrypted[g_off], decrypted[g_off + 1]]);
        if species_id == 0 || species_id > 412 {
            return None;
        }

        let moves = [
            u16::from_le_bytes([decrypted[a_off], decrypted[a_off + 1]]),
            u16::from_le_bytes([decrypted[a_off + 2], decrypted[a_off + 3]]),
            u16::from_le_bytes([decrypted[a_off + 4], decrypted[a_off + 5]]),
            u16::from_le_bytes([decrypted[a_off + 6], decrypted[a_off + 7]]),
        ];
        let pp = [
            decrypted[a_off + 8],
            decrypted[a_off + 9],
            decrypted[a_off + 10],
            decrypted[a_off + 11],
        ];

        let ev_hp = decrypted[e_off];
        let ev_atk = decrypted[e_off + 1];
        let ev_def = decrypted[e_off + 2];
        let ev_spd = decrypted[e_off + 3];
        let ev_spatk = decrypted[e_off + 4];
        let ev_spdef = decrypted[e_off + 5];

        let iv_data = u32::from_le_bytes([
            decrypted[m_off + 4],
            decrypted[m_off + 5],
            decrypted[m_off + 6],
            decrypted[m_off + 7],
        ]);
        let iv_hp = (iv_data & 0x1F) as u8;
        let iv_atk = ((iv_data >> 5) & 0x1F) as u8;
        let iv_def = ((iv_data >> 10) & 0x1F) as u8;
        let iv_spd = ((iv_data >> 15) & 0x1F) as u8;
        let iv_spatk = ((iv_data >> 20) & 0x1F) as u8;
        let iv_spdef = ((iv_data >> 25) & 0x1F) as u8;
        let is_egg = ((iv_data >> 30) & 1) != 0;

        let level = raw[84];
        let current_hp = u16::from_le_bytes([raw[86], raw[87]]);
        let max_hp = u16::from_le_bytes([raw[88], raw[89]]);

        let nature = NATURES[(personality % 25) as usize];

        // Shiny calculation: (otId_low ^ otId_high ^ pers_low ^ pers_high) < 8
        let p_low = (personality & 0xFFFF) as u16;
        let p_high = (personality >> 16) as u16;
        let o_low = (ot_id & 0xFFFF) as u16;
        let o_high = (ot_id >> 16) as u16;
        let is_shiny = (p_low ^ p_high ^ o_low ^ o_high) < 8;

        Some(DecryptedPokemon {
            species_id,
            species_name: format!("No.{:03}", species_id),
            nickname,
            level,
            current_hp,
            max_hp: max_hp.max(1),
            nature,
            is_shiny,
            is_egg,
            iv_hp,
            iv_atk,
            iv_def,
            iv_spd,
            iv_spatk,
            iv_spdef,
            ev_hp,
            ev_atk,
            ev_def,
            ev_spd,
            ev_spatk,
            ev_spdef,
            moves,
            pp,
        })
    }

    pub fn show(&mut self, ctx: &egui::Context, _gba: &Gba) {
        if !self.is_open {
            return;
        }

        let mut open = self.is_open;
        Window::new("🐾 Pokémon Gen 3 Companion")
            .open(&mut open)
            .default_width(520.0)
            .default_height(550.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Trainer Party & Stat Inspector");
                ui.label(RichText::new("Live memory decryption of active party Pokémon in EWRAM.").weak().small());
                ui.separator();

                if self.party.is_empty() {
                    ui.label(RichText::new("No active Pokémon party found in memory.").italics());
                    ui.label(RichText::new("Ensure a compatible Gen 3 cartridge is loaded with an active save game.").weak().small());
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (i, p) in self.party.iter().enumerate() {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("{}. {}", i + 1, p.nickname)).strong().size(15.0));
                                    ui.label(RichText::new(format!("({})", p.species_name)).weak());
                                    ui.label(RichText::new(format!("Lv.{}", p.level)).color(Color32::LIGHT_BLUE).strong());
                                    ui.label(RichText::new(format!("Nature: {}", p.nature)).color(Color32::LIGHT_YELLOW));

                                    if p.is_shiny {
                                        ui.label(RichText::new("✨ SHINY!").color(Color32::from_rgb(255, 215, 0)).strong());
                                    }
                                });

                                // HP Bar
                                let hp_ratio = (p.current_hp as f32 / p.max_hp as f32).clamp(0.0, 1.0);
                                let hp_color = if hp_ratio > 0.5 {
                                    Color32::from_rgb(50, 205, 50)
                                } else if hp_ratio > 0.2 {
                                    Color32::from_rgb(255, 165, 0)
                                } else {
                                    Color32::from_rgb(255, 50, 50)
                                };

                                ui.horizontal(|ui| {
                                    ui.label("HP:");
                                    ui.add(ProgressBar::new(hp_ratio).fill(hp_color).text(format!("{}/{}", p.current_hp, p.max_hp)));
                                });

                                // IVs & EVs Grid
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("IVs:").strong());
                                    ui.label(RichText::new(format!("HP:{:2} Atk:{:2} Def:{:2} SpA:{:2} SpD:{:2} Spe:{:2}",
                                        p.iv_hp, p.iv_atk, p.iv_def, p.iv_spatk, p.iv_spdef, p.iv_spd)).monospace().color(Color32::LIGHT_GREEN));
                                    let iv_total = p.iv_hp + p.iv_atk + p.iv_def + p.iv_spatk + p.iv_spdef + p.iv_spd;
                                    ui.label(RichText::new(format!("({}/186)", iv_total)).weak().small());
                                });

                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("EVs:").strong());
                                    ui.label(RichText::new(format!("HP:{:3} Atk:{:3} Def:{:3} SpA:{:3} SpD:{:3} Spe:{:3}",
                                        p.ev_hp, p.ev_atk, p.ev_def, p.ev_spatk, p.ev_spdef, p.ev_spd)).monospace().color(Color32::from_rgb(180, 180, 255)));
                                    let ev_total = (p.ev_hp as u32) + (p.ev_atk as u32) + (p.ev_def as u32) + (p.ev_spatk as u32) + (p.ev_spdef as u32) + (p.ev_spd as u32);
                                    ui.label(RichText::new(format!("({}/510)", ev_total)).weak().small());
                                });
                            });
                        }
                    });
                }
            });
        self.is_open = open;
    }
}
