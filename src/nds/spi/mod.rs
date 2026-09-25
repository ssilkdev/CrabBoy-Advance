//! Nintendo DS SPI Subsystem
//!
//! Emulates the Serial Peripheral Interface (SPI) connected to the ARM7:
//! - Device 0: Power Management IC (PMIC)
//! - Device 1: Firmware NVRAM / Flash
//! - Device 2: TSC2046 Resistive Touchscreen Controller

pub struct SpiBus {
    pub spicnt: u16,
    pub spidata: u16,

    // Devices
    pub touch: Tsc2046,
    pub nvram: FirmwareNvram,
    pub pmic: Pmic,
}

impl Default for SpiBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SpiBus {
    pub fn new() -> Self {
        Self {
            spicnt: 0,
            spidata: 0,
            touch: Tsc2046::new(),
            nvram: FirmwareNvram::new(),
            pmic: Pmic::new(),
        }
    }

    pub fn write_cnt(&mut self, val: u16) {
        // GBATEK SPICNT: 0-1 baud, 7 busy, 8-9 device, 10 16-bit mode (unused), 11 CS hold, 14 IRQ, 15 enable
        // Clearing the hold bit alone doesn't release chip-select: that
        // happens after the next byte (the one transferred with hold=0).
        // Only disabling the bus drops CS immediately.
        let was_enabled = (self.spicnt & (1 << 15)) != 0;
        self.spicnt = val;

        if was_enabled && (val & (1 << 15)) == 0 {
            self.touch.deselect();
            self.nvram.deselect();
            self.pmic.deselect();
        }
    }

    pub fn transfer_byte(&mut self, byte: u8) -> u8 {
        if (self.spicnt & (1 << 15)) == 0 {
            return 0; // SPI disabled
        }

        let device = (self.spicnt >> 8) & 0x03;
        let resp = match device {
            0 => self.pmic.transfer(byte),
            1 => self.nvram.transfer(byte),
            2 => self.touch.transfer(byte),
            _ => 0,
        };

        if (self.spicnt & (1 << 11)) == 0 {
            // Hold bit not set: chip select immediately de-asserted
            match device {
                0 => self.pmic.deselect(),
                1 => self.nvram.deselect(),
                2 => self.touch.deselect(),
                _ => {}
            }
        }

        resp
    }
}

/// Power Management IC (Device 0)
pub struct Pmic {
    pub regs: [u8; 5],
    pub state: PmicState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PmicState {
    Command,
    DataRead(usize),
    DataWrite(usize),
}

impl Pmic {
    pub fn new() -> Self {
        Self {
            regs: [
                0x00, // 0: Control
                0x00, // 1: Battery status
                0x00, // 2: Microphone amp
                0x00, // 3: Reserved
                0x0F, // 4: Backlight control (both top and bottom screens on)
            ],
            state: PmicState::Command,
        }
    }

    pub fn deselect(&mut self) {
        self.state = PmicState::Command;
    }

    pub fn transfer(&mut self, byte: u8) -> u8 {
        match self.state {
            PmicState::Command => {
                let reg = (byte & 0x7F) as usize;
                let read = (byte & 0x80) != 0;
                if reg < self.regs.len() {
                    if read {
                        self.state = PmicState::DataRead(reg);
                    } else {
                        self.state = PmicState::DataWrite(reg);
                    }
                }
                0
            }
            PmicState::DataRead(reg) => {
                self.state = PmicState::Command;
                self.regs[reg]
            }
            PmicState::DataWrite(reg) => {
                self.regs[reg] = byte;
                self.state = PmicState::Command;
                0
            }
        }
    }
}

/// TSC2046 Resistive Touchscreen Controller (Device 2)
pub struct Tsc2046 {
    pub touch_coords: Option<(u16, u16)>,
    control_byte: u8,
    response_byte_idx: usize,
    response_bytes: [u8; 2],
}

impl Tsc2046 {
    pub fn new() -> Self {
        Self {
            touch_coords: None,
            control_byte: 0,
            response_byte_idx: 0,
            response_bytes: [0, 0],
        }
    }

    /// Screen pixel -> 12-bit ADC value (linear, matching the firmware calibration).
    pub fn adc_x(x: u16) -> u16 {
        ((x.min(255) as u32 * (3800 - 300)) / 255 + 300) as u16
    }

    pub fn adc_y(y: u16) -> u16 {
        ((y.min(191) as u32 * (3700 - 350)) / 191 + 350) as u16
    }

    pub fn set_touch(&mut self, coords: Option<(u16, u16)>) {
        self.touch_coords = coords;
    }

    pub fn is_touching(&self) -> bool {
        self.touch_coords.is_some()
    }

    pub fn deselect(&mut self) {
        self.response_byte_idx = 0;
    }

    pub fn transfer(&mut self, byte: u8) -> u8 {
        if self.response_byte_idx > 0 && self.response_byte_idx <= 2 {
            let out = self.response_bytes[self.response_byte_idx - 1];
            self.response_byte_idx += 1;
            return out;
        }

        // New command byte
        self.control_byte = byte;
        let channel = (byte >> 4) & 0x07;

        let adc_val = match self.touch_coords {
            Some((x, y)) => {
                match channel {
                    // Y-Position
                    0b001 => Self::adc_y(y),
                    // X-Position
                    0b101 => Self::adc_x(x),
                    // Z1 Pressure
                    0b011 => 0x0700,
                    // Z2 Pressure
                    0b100 => 0x0200,
                    // Battery / Temperature
                    0b010 => 0x0F00,
                    _ => 0,
                }
            }
            None => match channel {
                // Not touched: high impedance / release
                0b011 | 0b100 => 0x0000,
                _ => 0x0FFF,
            },
        };

        // 12-bit result formatted across 2 bytes:
        // Byte 1: (adc_val >> 5) & 0x7F
        // Byte 2: (adc_val & 0x1F) << 3
        self.response_bytes[0] = ((adc_val >> 5) & 0x7F) as u8;
        self.response_bytes[1] = ((adc_val & 0x1F) << 3) as u8;
        self.response_byte_idx = 1;

        0
    }
}

/// Firmware FLASH (Device 1): ST M45PE20-style serial flash
/// (GBATEK "DS Firmware Serial Flash Memory"). Replies are shifted out on
/// the bytes that follow a command, so e.g. RDSR returns the status on the
/// second byte and keeps repeating it while chip-select is held.
pub struct FirmwareNvram {
    pub memory: Vec<u8>,
    cmd: u8,
    addr: u32,
    state: NvramState,
    write_enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NvramState {
    Idle,
    Addr(u8),
    Dummy,
    Data,
}

impl FirmwareNvram {
    pub const SIZE: usize = 256 * 1024; // 256 KB standard NDS firmware flash

    pub fn new() -> Self {
        let mut memory = vec![0xFF; Self::SIZE];
        Self::init_default_settings(&mut memory);

        Self {
            memory,
            cmd: 0,
            addr: 0,
            state: NvramState::Idle,
            write_enabled: false,
        }
    }

    /// Build a minimal firmware image in the layout GBATEK documents
    /// ("DS Firmware Header", "DS Firmware Wifi Calibration Data",
    /// "DS Firmware User Settings"): header with the user-settings offset,
    /// Wi-Fi config with a valid CRC, and two CRC-checked user-settings
    /// copies at 3FE00h/3FF00h (the one with the higher update counter wins).
    fn init_default_settings(mem: &mut [u8]) {
        let size = mem.len();
        if size < 0x2_0000 {
            return;
        }
        let put16 = |m: &mut [u8], o: usize, v: u16| m[o..o + 2].copy_from_slice(&v.to_le_bytes());

        // --- Header (000h..029h)
        mem[0x08..0x0C].copy_from_slice(b"MACP"); // firmware identifier
        mem[0x1D] = 0xFF; // console type: original NDS
        put16(mem, 0x20, ((size - 0x200) / 8) as u16); // user settings offset / 8
        put16(mem, 0x22, 0x7EC0);
        put16(mem, 0x24, 0x7E40);

        // --- Wi-Fi calibration/config (02Ah..1FFh)
        let cfg_len = 0x138usize;
        mem[0x2C..0x2C + cfg_len].fill(0);
        put16(mem, 0x2C, cfg_len as u16);
        mem[0x2F] = 0x00; // firmware version (original DS)
        mem[0x36..0x3C].copy_from_slice(&[0x00, 0x09, 0xBF, 0x12, 0x34, 0x56]); // MAC
        put16(mem, 0x3C, 0x3FFE); // channels 1..13 enabled
        let crc = crc16(0, &mem[0x2C..0x2C + cfg_len]);
        put16(mem, 0x2A, crc);

        // --- User settings, two copies
        let mut us = [0u8; 0x100];
        us[0x74..].fill(0xFF);
        put16(&mut us, 0x00, 5); // version
        us[0x02] = 11; // favourite colour (blue-ish)
        us[0x03] = 1; // birthday month
        us[0x04] = 1; // birthday day
        let name: Vec<u16> = "CrabBoy".encode_utf16().collect();
        for (i, c) in name.iter().enumerate() {
            put16(&mut us, 0x06 + i * 2, *c);
        }
        put16(&mut us, 0x1A, name.len() as u16);
        // Touch calibration from the two points Tsc2046 maps linearly.
        let (x1, y1, x2, y2) = (32u16, 32u16, 224u16, 160u16);
        put16(&mut us, 0x58, Tsc2046::adc_x(x1));
        put16(&mut us, 0x5A, Tsc2046::adc_y(y1));
        us[0x5C] = x1 as u8;
        us[0x5D] = y1 as u8;
        put16(&mut us, 0x5E, Tsc2046::adc_x(x2));
        put16(&mut us, 0x60, Tsc2046::adc_y(y2));
        us[0x62] = x2 as u8;
        us[0x63] = y2 as u8;
        // Language English, max backlight, "settings set" flags.
        put16(&mut us, 0x64, 0xFC00 | 0x0030 | 1);
        for (copy, counter) in [(size - 0x200, 0u16), (size - 0x100, 1u16)] {
            put16(&mut us, 0x70, counter);
            let crc = crc16(0xFFFF, &us[..0x70]);
            put16(&mut us, 0x72, crc);
            mem[copy..copy + 0x100].copy_from_slice(&us);
        }
    }

    /// Latest valid user-settings copy (for the RAM copy at 027FFC80h).
    pub fn user_settings(&self) -> &[u8] {
        let size = self.memory.len();
        let a = &self.memory[size - 0x200..size - 0x100];
        let b = &self.memory[size - 0x100..];
        let counter = |m: &[u8]| u16::from_le_bytes([m[0x70], m[0x71]]) & 0x7F;
        if (counter(b).wrapping_sub(counter(a)) & 0x7F) == 1 { b } else { a }
    }

    pub fn deselect(&mut self) {
        // Write/erase commands complete (and clear WEL) when CS goes high.
        if self.state != NvramState::Idle && matches!(self.cmd, 0x02 | 0x0A | 0xDB | 0xD8) {
            self.write_enabled = false;
        }
        self.state = NvramState::Idle;
    }

    pub fn transfer(&mut self, byte: u8) -> u8 {
        let size = self.memory.len() as u32;
        match self.state {
            NvramState::Idle => {
                self.cmd = byte;
                self.addr = 0;
                match byte {
                    0x06 => self.write_enabled = true,
                    0x04 => self.write_enabled = false,
                    0x03 | 0x0B | 0x02 | 0x0A | 0xDB | 0xD8 => self.state = NvramState::Addr(3),
                    0x05 | 0x9F => self.state = NvramState::Data,
                    _ => {}
                }
                0xFF
            }
            NvramState::Addr(left) => {
                self.addr = (self.addr << 8) | byte as u32;
                if left > 1 {
                    self.state = NvramState::Addr(left - 1);
                    return 0xFF;
                }
                self.addr %= size;
                self.state = if self.cmd == 0x0B { NvramState::Dummy } else { NvramState::Data };
                if self.write_enabled {
                    let a = self.addr as usize;
                    match self.cmd {
                        0xDB => self.memory[a & !0xFF..(a & !0xFF) + 0x100].fill(0xFF),
                        0xD8 => {
                            let s = a & !0xFFFF;
                            let e = (s + 0x1_0000).min(self.memory.len());
                            self.memory[s..e].fill(0xFF);
                        }
                        _ => {}
                    }
                }
                0xFF
            }
            NvramState::Dummy => {
                self.state = NvramState::Data;
                0xFF
            }
            NvramState::Data => match self.cmd {
                0x05 => (self.write_enabled as u8) << 1, // WIP=0, WEL
                0x9F => {
                    let id = [0x20u8, 0x40, 0x12]; // ST M45PE20
                    let v = id.get(self.addr as usize).copied().unwrap_or(0xFF);
                    self.addr += 1;
                    v
                }
                0x03 | 0x0B => {
                    let v = self.memory[self.addr as usize];
                    self.addr = (self.addr + 1) % size;
                    v
                }
                0x02 | 0x0A if self.write_enabled => {
                    let a = self.addr as usize;
                    // 0Ah page write replaces, 02h page program only clears bits.
                    self.memory[a] = if self.cmd == 0x02 { self.memory[a] & byte } else { byte };
                    self.addr = (self.addr & !0xFF) | ((self.addr + 1) & 0xFF);
                    0xFF
                }
                _ => 0xFF,
            },
        }
    }
}

/// CRC-16 as used by the DS firmware (reflected polynomial A001h).
pub fn crc16(init: u16, data: &[u8]) -> u16 {
    let mut crc = init;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}
