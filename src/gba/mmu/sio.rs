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

pub struct Sio {
    pub siodata32_l: u16,
    pub siodata32_h: u16,
    pub siocnt: u16,
    pub siodata8: u16,
    pub siomulti: [u16; 4],
    pub rcnt: u16,

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
            siodata32_l: 0,
            siodata32_h: 0,
            siocnt: 0,
            siodata8: 0,
            siomulti: [0xFFFF; 4],
            rcnt: 0,
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

    pub fn read_io16(&self, offset: u32) -> u16 {
        match offset {
            0x120 => self.siodata32_l,
            0x122 => self.siodata32_h,
            0x124 => {
                let mut cnt = self.siocnt;
                if self.transfer_cycles_left > 0 {
                    cnt |= 1 << 7; // Busy
                }
                // Assign player ID bits 4-5 based on role
                let player_id = match self.role {
                    MultiplayerRole::Player2Client => 1,
                    _ => 0,
                };
                (cnt & !(3 << 4)) | (player_id << 4)
            }
            0x126 => self.siodata8,
            0x128 => self.siomulti[0],
            0x12A => self.siomulti[1],
            0x12C => self.siomulti[2],
            0x12E => self.siomulti[3],
            0x134 => self.rcnt,
            _ => 0,
        }
    }

    pub fn write_io16(&mut self, offset: u32, val: u16) {
        match offset {
            0x120 => self.siodata32_l = val,
            0x122 => self.siodata32_h = val,
            0x124 => {
                let prev = self.siocnt;
                self.siocnt = val;

                // Start transfer requested (bit 7)
                if (val & (1 << 7)) != 0 && (prev & (1 << 7)) == 0 {
                    self.start_transfer();
                }
            }
            0x126 => self.siodata8 = val,
            0x128 => self.siomulti[0] = val,
            0x12A => self.siomulti[1] = val,
            0x12C => self.siomulti[2] = val,
            0x12E => self.siomulti[3] = val,
            0x134 => self.rcnt = val,
            _ => {}
        }
    }

    fn start_transfer(&mut self) {
        // Multi-player transfer cycles: ~2000 cycles at 115200 bps
        self.transfer_cycles_left = 2048;

        // If networked, broadcast my multi-player data to peer
        if let Some(ref sock) = self.socket {
            let my_id = match self.role {
                MultiplayerRole::Player2Client => 1usize,
                _ => 0usize,
            };
            let data = self.siomulti[my_id].to_le_bytes();
            let packet = [0x53, 0x49, 0x4F, my_id as u8, data[0], data[1]]; // "SIO" + id + data
            if sock.send_to(&packet, format!("127.0.0.1:{}", self.peer_port)).is_ok() {
                self.packets_sent += 1;
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
