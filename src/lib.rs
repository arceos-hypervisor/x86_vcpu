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
mod instruction_emulator;
mod page_table;

cfg_if::cfg_if! {
    if #[cfg(feature = "vmx")] {
        mod vmx;
        use vmx as vender;
        pub use vmx::{VmxExitInfo, VmxExitReason, VmxInterruptInfo, VmxIoExitInfo};

        pub use vender::VmxArchVCpu;
        pub use vender::VmxArchPerCpuState;
    }
}

use axaddrspace::GuestPhysAddr;
pub use ept::GuestPageWalkInfo;
use memory_addr::PhysAddr;
pub use regs::GeneralRegisters;
pub use vender::has_hardware_support;

/// Legacy function for backward compatibility
/// Use GuestMemoryAccess for new code
pub fn translate_to_phys(addr: GuestPhysAddr) -> Option<PhysAddr> {
    axvisor_api::guest_memory::translate_to_phys(axvisor_api::vmm::current_vm_id(), axvisor_api::vmm::current_vcpu_id(), addr)
}
