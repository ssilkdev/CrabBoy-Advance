//! GBA Serial I/O (SIO), Multi-Player Link Cable & Wireless Adapter (RFU) Engine

use std::net::UdpSocket;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SioMode {
    Normal8Bit,
    Normal32Bit,
    MultiPlayer,
    Uart,
    GeneralPurpose,
    JoyBus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiplayerRole {
    SinglePlayer,
    Player1Host,
    Player2Client,
}

/// Serial port registers (ROADMAP M1: rewritten to the real register map
/// and read/write rules, verified against mGBA's "SIO register R/W" suite).
///
/// | Addr  | Register                                     |
/// |-------|----------------------------------------------|
/// | 0x120 | SIODATA32_L / SIOMULTI0 (`siomulti[0]`)      |
/// | 0x122 | SIODATA32_H / SIOMULTI1 (`siomulti[1]`)      |
/// | 0x124 | SIOMULTI2 (`siomulti[2]`)                    |
/// | 0x126 | SIOMULTI3 (`siomulti[3]`)                    |
/// | 0x128 | SIOCNT                                       |
/// | 0x12A | SIODATA8 / SIOMLT_SEND (`siodata8`)          |
/// | 0x134 | RCNT                                         |
/// | 0x140 | JOYCNT, 0x150-0x156 JOY_RECV/JOY_TRANS, 0x158 JOYSTAT |
///
/// (Before this, SIOCNT was at 0x124 and the multiplayer data at
/// 0x128-0x12E, so games writing SIOCNT actually wrote SIOMULTI0.)
pub struct Sio {
    pub siocnt: u16,
    /// SIODATA8 in normal mode, SIOMLT_SEND in multiplayer mode.
    pub siodata8: u16,
    /// SIOMULTI0-3; entries 0-1 double as SIODATA32 in 32-bit normal mode.
    pub siomulti: [u16; 4],
    pub rcnt: u16,
    pub joycnt: u16,
    pub joy_recv: u32,
    pub joy_trans: u32,
    pub joystat: u16,

    // Networking & Multi-Instance State
    pub role: MultiplayerRole,
    pub socket: Option<UdpSocket>,
    pub local_port: u16,
    pub peer_port: u16,
    pub is_connected: bool,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub last_ping_ms: f32,
    pub last_ping_instant: Instant,

    // Internal transfer cycle timing
    pub transfer_cycles_left: u32,
    /// Mode the running transfer was started in; completion is handled
    /// in that mode even if the game switches modes mid-transfer.
    transfer_mode: SioMode,
    pub irq_requested: bool,

    // Wireless Adapter (RFU) emulation state
    pub rfu_enabled: bool,
    pub rfu_state: u8,
    pub rfu_rx_buf: Vec<u8>,
    pub rfu_tx_buf: Vec<u8>,
}

impl Default for Sio {
    fn default() -> Self {
        Self::new()
    }
}

impl Sio {
    pub fn new() -> Self {
        Self {
            siocnt: 0,
            siodata8: 0,
            siomulti: [0; 4],
            rcnt: 0,
            joycnt: 0,
            joy_recv: 0,
            joy_trans: 0,
            joystat: 0,
            role: MultiplayerRole::SinglePlayer,
            socket: None,
            local_port: 8765,
            peer_port: 8766,
            is_connected: false,
            packets_sent: 0,
            packets_received: 0,
            last_ping_ms: 0.0,
            last_ping_instant: Instant::now(),
            transfer_cycles_left: 0,
            transfer_mode: SioMode::Normal8Bit,
            irq_requested: false,
            rfu_enabled: false,
            rfu_state: 0,
            rfu_rx_buf: Vec::with_capacity(256),
            rfu_tx_buf: Vec::with_capacity(256),
        }
    }

    pub fn set_role(&mut self, role: MultiplayerRole) {
        self.role = role;
        self.socket = None;
        self.is_connected = false;

        match role {
            MultiplayerRole::Player1Host => {
                self.local_port = 8765;
                self.peer_port = 8766;
                let _ = self.init_socket();
            }
            MultiplayerRole::Player2Client => {
                self.local_port = 8766;
                self.peer_port = 8765;
                let _ = self.init_socket();
            }
            MultiplayerRole::SinglePlayer => {}
        }
    }

    pub fn init_socket(&mut self) -> std::io::Result<()> {
        let sock = UdpSocket::bind(format!("127.0.0.1:{}", self.local_port))?;
        sock.set_nonblocking(true)?;
        self.socket = Some(sock);
        Ok(())
    }

    /// Current communication mode, from RCNT bits 14-15 and SIOCNT 12-13.
    pub fn mode(&self) -> SioMode {
        if self.rcnt & 0x8000 == 0 {
            match (self.siocnt >> 12) & 3 {
                0 => SioMode::Normal8Bit,
                1 => SioMode::Normal32Bit,
                2 => SioMode::MultiPlayer,
                _ => SioMode::Uart,
            }
        } else if self.rcnt & 0x4000 == 0 {
            SioMode::GeneralPurpose
        } else {
            SioMode::JoyBus
        }
    }

    fn player_id(&self) -> u16 {
        match self.role {
            MultiplayerRole::Player2Client => 1,
            _ => 0,
        }
    }

    pub fn read_io16(&self, offset: u32) -> u16 {
        match offset {
            0x120 | 0x122 | 0x124 | 0x126 => self.siomulti[((offset - 0x120) / 2) as usize],
            0x128 => {
                let mut cnt = self.siocnt & 0x7F8F;
                if self.transfer_cycles_left > 0 {
                    cnt |= 1 << 7; // start/busy
                }
                match self.mode() {
                    SioMode::MultiPlayer => {
                        // Bit 2: 1 = child or no link partner; bit 3: all
                        // GBAs ready; bits 4-5: this GBA's player ID.
                        let child = self.player_id() != 0 || !self.is_connected;
                        cnt |= (child as u16) << 2 | 1 << 3 | self.player_id() << 4;
                    }
                    // UART: receive FIFO empty flag.
                    SioMode::Uart => cnt |= 1 << 5,
                    _ => {}
                }
                cnt
            }
            0x12A => {
                if self.mode() == SioMode::Uart {
                    0 // receive FIFO: nothing received
                } else {
                    self.siodata8
                }
            }
            0x134 => self.rcnt,
            0x140 => self.joycnt,
            0x150 => self.joy_recv as u16,
            0x152 => (self.joy_recv >> 16) as u16,
            // JOY_TRANS reads back as 0 on hardware (mGBA suite).
            0x154 | 0x156 => 0,
            0x158 => self.joystat,
            _ => 0,
        }
    }

    pub fn write_io16(&mut self, offset: u32, val: u16) {
        match offset {
            // SIODATA32 is only writable in 32-bit normal mode; in the other
            // modes 0x120-0x126 are received data (read-only).
            0x120 | 0x122 if self.mode() == SioMode::Normal32Bit => {
                self.siomulti[((offset - 0x120) / 2) as usize] = val;
            }
            0x128 => {
                let prev = self.siocnt;
                self.siocnt = val;
                // Line levels in RCNT follow the new mode.
                self.write_io16(0x134, self.rcnt);
                // Start transfer requested (bit 7)
                if (val & (1 << 7)) != 0 && (prev & (1 << 7)) == 0 {
                    self.start_transfer();
                }
            }
            0x12A if self.mode() != SioMode::Uart => self.siodata8 = val,
            0x134 => {
                // Bits 14-15 select the mode; in GPIO mode bits 0-8 are the
                // pins and their directions. Otherwise the four data lines
                // read their idle levels with nothing connected: normal
                // mode SC/SI high, multiplayer/UART all high (pulled up),
                // JOY bus SI/SO high.
                let mode_bits = val & 0xC000;
                self.rcnt = mode_bits;
                let lines = match self.mode() {
                    SioMode::GeneralPurpose => val & 0x000F,
                    SioMode::Normal8Bit | SioMode::Normal32Bit => 0x5,
                    SioMode::MultiPlayer | SioMode::Uart => 0xF,
                    SioMode::JoyBus => 0xC,
                };
                self.rcnt |= (val & 0x01F0) | lines;
            }
            // JOYCNT: bit 6 = IRQ enable; bits 0-2 are flags cleared by
            // writing 1.
            0x140 => self.joycnt = (val & 0x40) | (self.joycnt & !(val & 7) & !0x40),
            0x154 => self.joy_trans = (self.joy_trans & 0xFFFF_0000) | val as u32,
            0x156 => self.joy_trans = (self.joy_trans & 0xFFFF) | ((val as u32) << 16),
            0x158 => self.joystat = (self.joystat & !0x30) | (val & 0x30),
            _ => {}
        }
    }

    fn start_transfer(&mut self) {
        // Multi-player transfer cycles: ~2000 cycles at 115200 bps
        self.transfer_cycles_left = 2048;
        self.transfer_mode = self.mode();

        if self.mode() == SioMode::MultiPlayer {
            // Every slot reads 0xFFFF until its GBA's data arrives; ours is
            // SIOMLT_SEND. (Previously SIOMULTI[id] itself was sent.)
            let my_id = self.player_id() as usize;
            self.siomulti = [0xFFFF; 4];
            self.siomulti[my_id] = self.siodata8;

            // If networked, broadcast my multi-player data to peer
            if let Some(ref sock) = self.socket {
                let data = self.siodata8.to_le_bytes();
                let packet = [0x53, 0x49, 0x4F, my_id as u8, data[0], data[1]]; // "SIO" + id + data
                if sock.send_to(&packet, format!("127.0.0.1:{}", self.peer_port)).is_ok() {
                    self.packets_sent += 1;
                }
            }
        }
    }

    /// Advance SIO timer and process incoming network packets
    pub fn step(&mut self, cycles: u32) -> bool {
        // Check incoming UDP packets non-blockingly
        if let Some(ref sock) = self.socket {
            let mut buf = [0u8; 16];
            while let Ok((n, _src)) = sock.recv_from(&mut buf) {
                if n >= 6 && &buf[0..3] == b"SIO" {
                    let peer_id = (buf[3] as usize) & 3;
                    let val = u16::from_le_bytes([buf[4], buf[5]]);
                    self.siomulti[peer_id] = val;
                    self.packets_received += 1;
                    self.is_connected = true;
                    self.last_ping_ms = self.last_ping_instant.elapsed().as_secs_f32() * 1000.0;
                    self.last_ping_instant = Instant::now();
                }
            }
        }

        if self.transfer_cycles_left > 0 {
            if cycles >= self.transfer_cycles_left {
                self.transfer_cycles_left = 0;
                self.siocnt &= !(1 << 7); // Clear busy flag

                // Received data with no link partner attached: the data
                // lines read 0 (matches mGBA and the suite's SIO tests).
                // With a networked peer, its value has already landed in
                // siomulti via the UDP path above.
                if !self.is_connected {
                    match self.transfer_mode {
                        SioMode::MultiPlayer => self.siomulti = [0; 4],
                        SioMode::Normal8Bit => self.siodata8 = 0,
                        SioMode::Normal32Bit => {
                            self.siomulti[0] = 0;
                            self.siomulti[1] = 0;
                        }
                        _ => {}
                    }
                }

                // Check SIO IRQ Enable (bit 14)
                if (self.siocnt & (1 << 14)) != 0 {
                    self.irq_requested = true;
                    return true;
                }
            } else {
                self.transfer_cycles_left -= cycles;
            }
        }

        false
    }
}

/// Serial port registers and in-flight transfer (ROADMAP M2). Networking
/// (role, socket, statistics) is session state, not machine state.
impl crate::gba::state::Snapshot for Sio {
    fn save(&self, w: &mut crate::gba::state::StateWriter) {
        w.u16(self.siocnt); w.u16(self.siodata8);
        for v in self.siomulti { w.u16(v); }
        w.u16(self.rcnt); w.u16(self.joycnt); w.u32(self.joy_recv); w.u32(self.joy_trans); w.u16(self.joystat);
        w.u32(self.transfer_cycles_left);
        w.u8(self.transfer_mode as u8);
        w.bool(self.irq_requested);
    }
    fn load(&mut self, r: &mut crate::gba::state::StateReader) -> Option<()> {
        self.siocnt = r.u16()?; self.siodata8 = r.u16()?;
        for v in &mut self.siomulti { *v = r.u16()?; }
        self.rcnt = r.u16()?; self.joycnt = r.u16()?; self.joy_recv = r.u32()?; self.joy_trans = r.u32()?;
        self.joystat = r.u16()?;
        self.transfer_cycles_left = r.u32()?;
        self.transfer_mode = match r.u8()? {
            0 => SioMode::Normal8Bit, 1 => SioMode::Normal32Bit, 2 => SioMode::MultiPlayer,
            3 => SioMode::Uart, 4 => SioMode::GeneralPurpose, 5 => SioMode::JoyBus, _ => return None,
        };
        self.irq_requested = r.bool()?;
        Some(())
    }
}
