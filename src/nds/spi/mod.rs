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
        // Bit 15: Enable, Bits 8-9: Device Select, Bit 10: Hold CS, Bits 0-1: Baud
        let prev_hold = (self.spicnt & (1 << 10)) != 0;
        let new_hold = (val & (1 << 10)) != 0;
        self.spicnt = val;

        if prev_hold && !new_hold {
            // Chip select was released; deselect active device
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

        if (self.spicnt & (1 << 10)) == 0 {
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
                let x_clamped = x.min(255) as u32;
                let y_clamped = y.min(191) as u32;
                match channel {
                    // Y-Position
                    0b001 => (((y_clamped * (3700 - 350)) / 191) + 350) as u16,
                    // X-Position
                    0b101 => (((x_clamped * (3800 - 300)) / 255) + 300) as u16,
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

/// Firmware NVRAM Flash Memory (Device 1)
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
    Reading,
    Writing,
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

    /// Sets up authentic NDS firmware user settings block at 0x3FE00
    fn init_default_settings(mem: &mut [u8]) {
        let offset = 0x3FE00;
        if offset + 0x200 > mem.len() {
            return;
        }

        // Header / version
        mem[offset] = 0x05;
        mem[offset + 1] = 0x00;

        // User Name: "CrabBoy" in UTF-16LE
        let name = "CrabBoy";
        let mut name_offset = offset + 0x04;
        for c in name.encode_utf16() {
            let bytes = c.to_le_bytes();
            mem[name_offset] = bytes[0];
            mem[name_offset + 1] = bytes[1];
            name_offset += 2;
        }

        // Color (0 = Gray, 1 = Brown, ..., 5 = Blue)
        mem[offset + 0x1A] = 5;

        // Language: English (1)
        mem[offset + 0x1C] = 1;

        // Touch calibration ADC points:
        // Point 1: (300, 350) -> screen (0, 0)
        // Point 2: (3800, 3700) -> screen (255, 191)
        let calib_offset = offset + 0x20;
        mem[calib_offset..calib_offset + 2].copy_from_slice(&300u16.to_le_bytes());
        mem[calib_offset + 2..calib_offset + 4].copy_from_slice(&350u16.to_le_bytes());
        mem[calib_offset + 4] = 0;
        mem[calib_offset + 5] = 0;

        mem[calib_offset + 6..calib_offset + 8].copy_from_slice(&3800u16.to_le_bytes());
        mem[calib_offset + 8..calib_offset + 10].copy_from_slice(&3700u16.to_le_bytes());
        mem[calib_offset + 10] = 255;
        mem[calib_offset + 11] = 191;
    }

    pub fn deselect(&mut self) {
        self.state = NvramState::Idle;
    }

    pub fn transfer(&mut self, byte: u8) -> u8 {
        match self.state {
            NvramState::Idle => {
                self.cmd = byte;
                match byte {
                    0x06 => {
                        // WREN
                        self.write_enabled = true;
                        0
                    }
                    0x04 => {
                        // WRDI
                        self.write_enabled = false;
                        0
                    }
                    0x05 => {
                        // RDSR: Bit 1 is WEL (Write Enable Latch)
                        if self.write_enabled { 0x02 } else { 0x00 }
                    }
                    0x03 => {
                        // READ
                        self.addr = 0;
                        self.state = NvramState::Addr(3);
                        0
                    }
                    0x02 => {
                        // WRITE
                        self.addr = 0;
                        self.state = NvramState::Addr(3);
                        0
                    }
                    _ => 0,
                }
            }
            NvramState::Addr(bytes_left) => {
                self.addr = (self.addr << 8) | (byte as u32);
                if bytes_left == 1 {
                    if self.cmd == 0x03 {
                        self.state = NvramState::Reading;
                    } else if self.cmd == 0x02 {
                        self.state = NvramState::Writing;
                    } else {
                        self.state = NvramState::Idle;
                    }
                } else {
                    self.state = NvramState::Addr(bytes_left - 1);
                }
                0
            }
            NvramState::Reading => {
                let val = if (self.addr as usize) < self.memory.len() {
                    self.memory[self.addr as usize]
                } else {
                    0xFF
                };
                self.addr = (self.addr + 1) & (Self::SIZE as u32 - 1);
                val
            }
            NvramState::Writing => {
                if self.write_enabled && (self.addr as usize) < self.memory.len() {
                    self.memory[self.addr as usize] = byte;
                }
                self.addr = (self.addr + 1) & (Self::SIZE as u32 - 1);
                0
            }
        }
    }
}
