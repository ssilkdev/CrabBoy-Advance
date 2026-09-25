//! NDS CPU Subsystems (ARM946E-S & ARM7TDMI)

pub mod arm7;
pub mod arm9;
pub mod executor;

pub use arm7::{step_arm7, Arm7Tdmi};
pub use arm9::{step_arm9, Arm946eS};
