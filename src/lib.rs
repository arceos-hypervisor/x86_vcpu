#![no_std]
#![feature(doc_cfg)]
#![cfg(target_arch = "x86_64")]
#![doc = include_str!("../README.md")]

#[macro_use]
extern crate log;

extern crate alloc;

#[cfg(test)]
mod test_utils;

pub(crate) mod msr;
#[macro_use]
pub(crate) mod regs;
mod ept;
pub(crate) mod mem;

cfg_if::cfg_if! {
    if #[cfg(feature = "vmx")] {
        mod vmx;
        use vmx as vender;
        pub use vmx::{VmxExitInfo, VmxExitReason, VmxInterruptInfo, VmxIoExitInfo};

        pub use vender::VmxArchVCpu;
        pub use vender::VmxArchPerCpuState;
    }
}

pub use ept::GuestPageWalkInfo;
pub use regs::GeneralRegisters;
pub use vender::has_hardware_support;

pub type HostVirtAddr = usize;
pub type HostPhysAddr = usize;

/// Hardware abstraction layer for memory management.
pub trait Hal {
    /// Allocates a frame and returns its host physical address. The
    ///
    /// # Returns
    ///
    /// * `Option<HostPhysAddr>` - Some containing the physical address of the allocated frame, or None if allocation fails.
    fn alloc_frame() -> Option<HostPhysAddr>;

    /// Deallocates a frame given its physical address.
    ///
    /// # Parameters
    ///
    /// * `paddr` - The physical address of the frame to deallocate.
    fn dealloc_frame(paddr: HostPhysAddr);

    /// Converts a host physical address to a host virtual address.
    ///
    /// # Parameters
    ///
    /// * `paddr` - The physical address to convert.
    ///
    /// # Returns
    ///
    /// * `HostVirtAddr` - The corresponding virtual address.
    fn phys_to_virt(paddr: HostPhysAddr) -> HostVirtAddr;

    /// Converts a host virtual address to a host physical address.
    ///
    /// # Parameters
    ///
    /// * `vaddr` - The virtual address to convert.
    ///
    /// # Returns
    ///
    /// * `HostPhysAddr` - The corresponding physical address.
    fn virt_to_phys(vaddr: HostVirtAddr) -> HostPhysAddr;
}
