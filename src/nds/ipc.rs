//! Inter-Processor Communication (IPC) for Nintendo DS
//!
//! Synchronizes the ARM9 and ARM7 cores via IPCSYNC registers and
//! two 16-entry 32-bit hardware FIFOs.

use std::collections::VecDeque;

const FIFO_CAPACITY: usize = 16;

#[derive(Debug, Clone, Default)]
pub struct IpcSync {
    /// 4 bits written by ARM9, read by ARM7
    pub arm9_to_arm7: u8,
    /// 4 bits written by ARM7, read by ARM9
    pub arm7_to_arm9: u8,
    /// ARM9 enables IRQ when ARM7 writes to IPCSYNC
    pub arm9_irq_enable: bool,
    /// ARM7 enables IRQ when ARM9 writes to IPCSYNC
    pub arm7_irq_enable: bool,
    /// Pending IRQ to ARM9
    pub irq_to_arm9: bool,
    /// Pending IRQ to ARM7
    pub irq_to_arm7: bool,
}

impl IpcSync {
    pub fn read_arm9(&self) -> u16 {
        let mut val = (self.arm7_to_arm9 as u16) & 0x0F;
        val |= ((self.arm9_to_arm7 as u16) & 0x0F) << 8;
        if self.arm9_irq_enable {
            val |= 1 << 14;
        }
        val
    }

    pub fn write_arm9(&mut self, val: u16) {
        self.arm9_to_arm7 = ((val >> 8) & 0x0F) as u8;
        self.arm9_irq_enable = (val & (1 << 14)) != 0;
        if (val & (1 << 13)) != 0 && self.arm7_irq_enable {
            self.irq_to_arm7 = true;
        }
    }

    pub fn read_arm7(&self) -> u16 {
        let mut val = (self.arm9_to_arm7 as u16) & 0x0F;
        val |= ((self.arm7_to_arm9 as u16) & 0x0F) << 8;
        if self.arm7_irq_enable {
            val |= 1 << 14;
        }
        val
    }

    pub fn write_arm7(&mut self, val: u16) {
        self.arm7_to_arm9 = ((val >> 8) & 0x0F) as u8;
        self.arm7_irq_enable = (val & (1 << 14)) != 0;
        if (val & (1 << 13)) != 0 && self.arm9_irq_enable {
            self.irq_to_arm9 = true;
        }
    }
}

/// IRQ bit raised when a CPU's send FIFO becomes empty.
pub const IRQ_IPC_SEND_EMPTY: u32 = 1 << 17;
/// IRQ bit raised when a CPU's receive FIFO becomes not-empty.
pub const IRQ_IPC_RECV_NOT_EMPTY: u32 = 1 << 18;

/// One direction-pair of IPCFIFOCNT state, as seen by one CPU.
#[derive(Debug, Clone, Default)]
pub struct FifoSide {
    pub send_empty_irq: bool,
    pub recv_not_empty_irq: bool,
    pub error: bool,
    pub enable: bool,
    /// Last word successfully read (returned again on underflow).
    pub last_read: u32,
}

/// The two 16-word IPC FIFOs (GBATEK "DS Inter Process Communication").
/// `pending_arm9`/`pending_arm7` collect IF bits for the bus to raise.
#[derive(Debug, Clone)]
pub struct IpcFifo {
    pub queue_9_to_7: VecDeque<u32>,
    pub queue_7_to_9: VecDeque<u32>,
    pub arm9: FifoSide,
    pub arm7: FifoSide,
    pub pending_arm9: u32,
    pub pending_arm7: u32,
}

impl Default for IpcFifo {
    fn default() -> Self {
        Self {
            queue_9_to_7: VecDeque::with_capacity(FIFO_CAPACITY),
            queue_7_to_9: VecDeque::with_capacity(FIFO_CAPACITY),
            arm9: FifoSide::default(),
            arm7: FifoSide::default(),
            pending_arm9: 0,
            pending_arm7: 0,
        }
    }
}

fn cnt_value(side: &FifoSide, send: &VecDeque<u32>, recv: &VecDeque<u32>) -> u16 {
    let mut val = 0u16;
    if send.is_empty() { val |= 1 << 0; }
    if send.len() >= FIFO_CAPACITY { val |= 1 << 1; }
    if side.send_empty_irq { val |= 1 << 2; }
    if recv.is_empty() { val |= 1 << 8; }
    if recv.len() >= FIFO_CAPACITY { val |= 1 << 9; }
    if side.recv_not_empty_irq { val |= 1 << 10; }
    if side.error { val |= 1 << 14; }
    if side.enable { val |= 1 << 15; }
    val
}

/// Apply an IPCFIFOCNT write; returns IF bits for the writing CPU.
fn cnt_write(side: &mut FifoSide, send: &mut VecDeque<u32>, recv: &VecDeque<u32>, val: u16) -> u32 {
    let mut irq = 0;
    let send_irq = (val & (1 << 2)) != 0;
    let recv_irq = (val & (1 << 10)) != 0;
    if (val & (1 << 3)) != 0 {
        send.clear();
    }
    // Enabling an IRQ while its condition already holds fires it.
    if send_irq && !side.send_empty_irq && send.is_empty() {
        irq |= IRQ_IPC_SEND_EMPTY;
    }
    if recv_irq && !side.recv_not_empty_irq && !recv.is_empty() {
        irq |= IRQ_IPC_RECV_NOT_EMPTY;
    }
    side.send_empty_irq = send_irq;
    side.recv_not_empty_irq = recv_irq;
    if (val & (1 << 14)) != 0 {
        side.error = false; // acknowledge by writing 1
    }
    side.enable = (val & (1 << 15)) != 0;
    irq
}

impl IpcFifo {
    pub fn read_cnt_arm9(&self) -> u16 {
        cnt_value(&self.arm9, &self.queue_9_to_7, &self.queue_7_to_9)
    }

    pub fn read_cnt_arm7(&self) -> u16 {
        cnt_value(&self.arm7, &self.queue_7_to_9, &self.queue_9_to_7)
    }

    pub fn write_cnt_arm9(&mut self, val: u16) {
        let was_empty = self.queue_9_to_7.is_empty();
        self.pending_arm9 |= cnt_write(&mut self.arm9, &mut self.queue_9_to_7, &self.queue_7_to_9, val);
        if !was_empty && self.queue_9_to_7.is_empty() && self.arm9.send_empty_irq {
            self.pending_arm9 |= IRQ_IPC_SEND_EMPTY;
        }
    }

    pub fn write_cnt_arm7(&mut self, val: u16) {
        let was_empty = self.queue_7_to_9.is_empty();
        self.pending_arm7 |= cnt_write(&mut self.arm7, &mut self.queue_7_to_9, &self.queue_9_to_7, val);
        if !was_empty && self.queue_7_to_9.is_empty() && self.arm7.send_empty_irq {
            self.pending_arm7 |= IRQ_IPC_SEND_EMPTY;
        }
    }

    pub fn write_data_arm9(&mut self, data: u32) {
        if !self.arm9.enable {
            return;
        }
        if self.queue_9_to_7.len() < FIFO_CAPACITY {
            let was_empty = self.queue_9_to_7.is_empty();
            self.queue_9_to_7.push_back(data);
            if was_empty && self.arm7.recv_not_empty_irq {
                self.pending_arm7 |= IRQ_IPC_RECV_NOT_EMPTY;
            }
        } else {
            self.arm9.error = true;
        }
    }

    pub fn write_data_arm7(&mut self, data: u32) {
        if !self.arm7.enable {
            return;
        }
        if self.queue_7_to_9.len() < FIFO_CAPACITY {
            let was_empty = self.queue_7_to_9.is_empty();
            self.queue_7_to_9.push_back(data);
            if was_empty && self.arm9.recv_not_empty_irq {
                self.pending_arm9 |= IRQ_IPC_RECV_NOT_EMPTY;
            }
        } else {
            self.arm7.error = true;
        }
    }

    pub fn read_data_arm9(&mut self) -> u32 {
        if !self.arm9.enable {
            // Disabled: peek the oldest word without removing it.
            return self.queue_7_to_9.front().copied().unwrap_or(self.arm9.last_read);
        }
        match self.queue_7_to_9.pop_front() {
            Some(val) => {
                self.arm9.last_read = val;
                if self.queue_7_to_9.is_empty() && self.arm7.send_empty_irq {
                    self.pending_arm7 |= IRQ_IPC_SEND_EMPTY;
                }
                val
            }
            None => {
                self.arm9.error = true;
                self.arm9.last_read
            }
        }
    }

    pub fn read_data_arm7(&mut self) -> u32 {
        if !self.arm7.enable {
            return self.queue_9_to_7.front().copied().unwrap_or(self.arm7.last_read);
        }
        match self.queue_9_to_7.pop_front() {
            Some(val) => {
                self.arm7.last_read = val;
                if self.queue_9_to_7.is_empty() && self.arm9.send_empty_irq {
                    self.pending_arm9 |= IRQ_IPC_SEND_EMPTY;
                }
                val
            }
            None => {
                self.arm7.error = true;
                self.arm7.last_read
            }
        }
    }
}

/// Unified IPC controller containing IPCSYNC and the two FIFOs.
#[derive(Debug, Clone, Default)]
pub struct Ipc {
    pub sync: IpcSync,
    pub fifo: IpcFifo,
}
