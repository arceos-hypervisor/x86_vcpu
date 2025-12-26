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

cfg_if::cfg_if! {
    if #[cfg(feature = "vmx")] {
        mod vmx;
        use vmx as vender;
        pub use vmx::{VmxExitInfo, VmxExitReason, VmxInterruptInfo, VmxIoExitInfo};
        pub use vender::VmxArchVCpu;
        pub use vender::VmxArchPerCpuState;
    }else if #[cfg(feature = "svm")] {
        mod svm;
        use svm as vender;
        pub use vender::{
            SvmArchVCpu,SvmArchPerCpuState,
        };
    }
}

//
//         mod vmx;
//         use vmx as vender;
//         pub use vmx::{VmxExitInfo, VmxExitReason, VmxInterruptInfo, VmxIoExitInfo};
//
//         pub use vender::VmxArchVCpu;
//         pub use vender::VmxArchPerCpuState;
//
//
// mod svm;
// use svm as vendor;
// pub use vendor::{
//     SvmArchVCpu,
//     SvmArchPerCpuState,
// };

pub use ept::GuestPageWalkInfo;
pub use regs::GeneralRegisters;
pub use vender::has_hardware_support;
