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

#[derive(Debug, Clone)]
pub struct IpcFifo {
    pub queue_9_to_7: VecDeque<u32>,
    pub queue_7_to_9: VecDeque<u32>,

    pub arm9_send_empty_irq: bool,
    pub arm9_recv_not_empty_irq: bool,
    pub arm9_error: bool,
    pub arm9_enable: bool,

    pub arm7_send_empty_irq: bool,
    pub arm7_recv_not_empty_irq: bool,
    pub arm7_error: bool,
    pub arm7_enable: bool,

    pub irq_to_arm9: bool,
    pub irq_to_arm7: bool,
}

impl Default for IpcFifo {
    fn default() -> Self {
        Self {
            queue_9_to_7: VecDeque::with_capacity(FIFO_CAPACITY),
            queue_7_to_9: VecDeque::with_capacity(FIFO_CAPACITY),
            arm9_send_empty_irq: false,
            arm9_recv_not_empty_irq: false,
            arm9_error: false,
            arm9_enable: false,
            arm7_send_empty_irq: false,
            arm7_recv_not_empty_irq: false,
            arm7_error: false,
            arm7_enable: false,
            irq_to_arm9: false,
            irq_to_arm7: false,
        }
    }
}

impl IpcFifo {
    pub fn read_cnt_arm9(&self) -> u16 {
        let mut val = 0u16;
        if self.queue_9_to_7.is_empty() {
            val |= 1 << 0;
        }
        if self.queue_9_to_7.len() >= FIFO_CAPACITY {
            val |= 1 << 1;
        }
        if self.arm9_send_empty_irq {
            val |= 1 << 2;
        }
        if self.queue_7_to_9.is_empty() {
            val |= 1 << 8;
        }
        if self.queue_7_to_9.len() >= FIFO_CAPACITY {
            val |= 1 << 9;
        }
        if self.arm9_recv_not_empty_irq {
            val |= 1 << 10;
        }
        if self.arm9_error {
            val |= 1 << 14;
        }
        if self.arm9_enable {
            val |= 1 << 15;
        }
        val
    }

    pub fn write_cnt_arm9(&mut self, val: u16) {
        self.arm9_send_empty_irq = (val & (1 << 2)) != 0;
        if (val & (1 << 3)) != 0 {
            self.queue_9_to_7.clear();
        }
        self.arm9_recv_not_empty_irq = (val & (1 << 10)) != 0;
        if (val & (1 << 14)) != 0 {
            self.arm9_error = false;
        }
        self.arm9_enable = (val & (1 << 15)) != 0;
    }

    pub fn write_data_arm9(&mut self, data: u32) {
        if !self.arm9_enable {
            return;
        }
        if self.queue_9_to_7.len() < FIFO_CAPACITY {
            self.queue_9_to_7.push_back(data);
            if self.arm7_recv_not_empty_irq {
                self.irq_to_arm7 = true;
            }
        } else {
            self.arm9_error = true;
        }
    }

    pub fn read_data_arm9(&mut self) -> u32 {
        if !self.arm9_enable {
            return 0;
        }
        if let Some(val) = self.queue_7_to_9.pop_front() {
            if self.queue_7_to_9.is_empty() && self.arm7_send_empty_irq {
                self.irq_to_arm7 = true;
            }
            val
        } else {
            self.arm9_error = true;
            0
        }
    }

    pub fn read_cnt_arm7(&self) -> u16 {
        let mut val = 0u16;
        if self.queue_7_to_9.is_empty() {
            val |= 1 << 0;
        }
        if self.queue_7_to_9.len() >= FIFO_CAPACITY {
            val |= 1 << 1;
        }
        if self.arm7_send_empty_irq {
            val |= 1 << 2;
        }
        if self.queue_9_to_7.is_empty() {
            val |= 1 << 8;
        }
        if self.queue_9_to_7.len() >= FIFO_CAPACITY {
            val |= 1 << 9;
        }
        if self.arm7_recv_not_empty_irq {
            val |= 1 << 10;
        }
        if self.arm7_error {
            val |= 1 << 14;
        }
        if self.arm7_enable {
            val |= 1 << 15;
        }
        val
    }

    pub fn write_cnt_arm7(&mut self, val: u16) {
        self.arm7_send_empty_irq = (val & (1 << 2)) != 0;
        if (val & (1 << 3)) != 0 {
            self.queue_7_to_9.clear();
        }
        self.arm7_recv_not_empty_irq = (val & (1 << 10)) != 0;
        if (val & (1 << 14)) != 0 {
            self.arm7_error = false;
        }
        self.arm7_enable = (val & (1 << 15)) != 0;
    }

    pub fn write_data_arm7(&mut self, data: u32) {
        if !self.arm7_enable {
            return;
        }
        if self.queue_7_to_9.len() < FIFO_CAPACITY {
            self.queue_7_to_9.push_back(data);
            if self.arm9_recv_not_empty_irq {
                self.irq_to_arm9 = true;
            }
        } else {
            self.arm7_error = true;
        }
    }

    pub fn read_data_arm7(&mut self) -> u32 {
        if !self.arm7_enable {
            return 0;
        }
        if let Some(val) = self.queue_9_to_7.pop_front() {
            if self.queue_9_to_7.is_empty() && self.arm9_send_empty_irq {
                self.irq_to_arm9 = true;
            }
            val
        } else {
            self.arm7_error = true;
            0
        }
    }
}

/// Unified IPC controller containing IPCSYNC and the two FIFOs.
#[derive(Debug, Clone, Default)]
pub struct Ipc {
    pub sync: IpcSync,
    pub fifo: IpcFifo,
}
