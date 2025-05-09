#![no_std]
#![feature(doc_cfg)]
#![feature(concat_idents)]
#![feature(naked_functions)]
#![doc = include_str!("../README.md")]

#[macro_use]
extern crate log;

extern crate alloc;

pub(crate) mod msr;
#[macro_use]
pub(crate) mod regs;
mod ept;
mod frame;
mod page_table;

mod context;
mod segmentation;
// mod tables;
mod xstate;

cfg_if::cfg_if! {
    if #[cfg(feature = "vmx")] {
        mod vmx;
        use vmx as vender;
        pub use vmx::{VmxExitInfo, VmxExitReason, VmxInterruptInfo, VmxIoExitInfo};

        pub use vender::VmxArchVCpu;
        pub use vender::VmxArchPerCpuState;
    }
}

pub use context::LinuxContext;
pub use regs::GeneralRegisters;
pub use vender::has_hardware_support;
