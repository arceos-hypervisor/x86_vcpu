// Copyright 2025 The Axvisor Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![no_std]
#![feature(doc_cfg)]
#![feature(concat_idents)]
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
pub(crate) mod xstate;

#[cfg(all(feature = "vmx", feature = "svm"))]
compile_error!("Features 'vmx' and 'svm' are mutually exclusive. Please enable only one of them.");

cfg_if::cfg_if! {
    if #[cfg(feature = "vmx")] {
        mod vmx;
        use vmx as vendor;
        // pub use vmx::{VmxExitInfo, VmxExitReason, VmxInterruptInfo, VmxIoExitInfo};
        pub use vendor::VmxArchVCpu as X86ArchVCpu;
        pub use vendor::VmxArchPerCpuState as X86ArchPerCpuState;
    } else if #[cfg(feature = "svm")] {
        mod svm;
        use svm as vendor;
        pub use vendor::{
            SvmArchVCpu as X86ArchVCpu, SvmArchPerCpuState as X86ArchPerCpuState,
        };
    }
}

pub use ept::GuestPageWalkInfo;
pub use regs::GeneralRegisters;
pub use vendor::has_hardware_support;
