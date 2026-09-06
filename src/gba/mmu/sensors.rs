//! Cartridge Hardware Sensors: Solar Sensor, Gyro/Tilt & Rumble Pak

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorType {
    None,
    Solar,
    GyroTilt,
    Rumble,
}

pub struct CartridgeSensors {
    pub sensor_type: SensorType,

    // Solar Sensor State
    pub sunlight_level: u8, // 0 (Darkness) to 10 (Maximum Sun)
    pub auto_diurnal_cycle: bool,
    solar_counter: u8,
    solar_clock: bool,
    solar_reset: bool,

    // Gyro / Accelerometer State
    pub tilt_x: f32, // -1.0 to 1.0 (-90 deg to +90 deg)
    pub tilt_y: f32,
    gyro_angle: u16, // 0x0000 - 0x0FFF (0x0800 is center)

    // Rumble Motor State
    pub rumble_active: bool,
    pub rumble_strength: f32,
}

impl Default for CartridgeSensors {
    fn default() -> Self {
        Self::new()
    }
}

impl CartridgeSensors {
    pub fn new() -> Self {
        Self {
            sensor_type: SensorType::None,
            sunlight_level: 6, // Default moderate sunlight
            auto_diurnal_cycle: false,
            solar_counter: 0,
            solar_clock: false,
            solar_reset: false,
            tilt_x: 0.0,
            tilt_y: 0.0,
            gyro_angle: 0x0800,
            rumble_active: false,
            rumble_strength: 0.8,
        }
    }

    /// Automatically detects sensor type from ROM Game Code or Title
    pub fn detect_from_cartridge(&mut self, game_code: &str, title: &str) {
        if game_code.starts_with("U3I") || game_code.starts_with("U32") || game_code.starts_with("U33") || title.contains("BOKTAI") {
            self.sensor_type = SensorType::Solar;
        } else if game_code.starts_with("RZW") || title.contains("WARIOWARE T") {
            self.sensor_type = SensorType::GyroTilt;
        } else if game_code.starts_with("KYG") || title.contains("YOSHI") {
            self.sensor_type = SensorType::GyroTilt;
        } else if game_code.starts_with("V49") || title.contains("DRILL") || title.contains("PINBALL") {
            self.sensor_type = SensorType::Rumble;
        } else {
            self.sensor_type = SensorType::None;
        }
    }

    pub fn set_sunlight(&mut self, level: u8) {
        self.sunlight_level = level.clamp(0, 10);
    }

    pub fn set_tilt(&mut self, x: f32, y: f32) {
        self.tilt_x = x.clamp(-1.0, 1.0);
        self.tilt_y = y.clamp(-1.0, 1.0);
        let raw = ((self.tilt_x * 1024.0) + 2048.0).clamp(0.0, 4095.0) as u16;
        self.gyro_angle = raw;
    }

    /// Intercepts reads to GPIO registers at 0x080000C4..C8
    pub fn read_gpio(&self, addr: u32) -> u8 {
        match addr & 0xFF {
            0xC4 => {
                // GPIO Data
                let mut data = 0u8;
                match self.sensor_type {
                    SensorType::Solar => {
                        // Photodiode counter data output bit (bit 1)
                        if self.solar_counter > 0 && (self.solar_counter & 1) != 0 {
                            data |= 1 << 1;
                        }
                    }
                    SensorType::GyroTilt => {
                        // Bit 0/1 serial stream of 12-bit angle
                        if (self.gyro_angle & 1) != 0 {
                            data |= 1 << 1;
                        }
                    }
                    SensorType::Rumble => {
                        if self.rumble_active {
                            data |= 1 << 3;
                        }
                    }
                    _ => {}
                }
                data
            }
            0xC6 => 0x0F, // Direction (in/out)
            0xC8 => 0x01, // GPIO Enable
            _ => 0,
        }
    }

    /// Intercepts writes to GPIO registers at 0x080000C4..C8
    pub fn write_gpio(&mut self, addr: u32, val: u8) {
        match addr & 0xFF {
            0xC4 => {
                match self.sensor_type {
                    SensorType::Solar => {
                        let clk = (val & (1 << 0)) != 0;
                        let rst = (val & (1 << 2)) != 0;

                        if rst {
                            // Reset counter: initialize count based on sunlight level
                            self.solar_counter = self.sunlight_level * 18;
                        } else if clk && !self.solar_clock {
                            // Rising edge of SCK: decrement / shift counter
                            if self.solar_counter > 0 {
                                self.solar_counter -= 1;
                            }
                        }
                        self.solar_clock = clk;
                        self.solar_reset = rst;
                    }
                    SensorType::Rumble => {
                        // Drill Dozer rumble bit is GPIO pin 3 (1 << 3)
                        self.rumble_active = (val & (1 << 3)) != 0;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
