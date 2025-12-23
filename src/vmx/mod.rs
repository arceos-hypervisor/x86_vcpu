mod definitions;
mod instructions;
mod percpu;
mod structs;
mod vcpu;
mod vmcs;

use self::structs::VmxBasic;

pub use self::definitions::VmxRawExitReason;
pub use self::percpu::VmxPerCpuState as VmxArchPerCpuState;
pub use self::vcpu::VmxVcpu as VmxArchVCpu;
pub use self::vmcs::{VmxExitInfo, VmxInterruptInfo, VmxIoExitInfo};

// 导出自定义错误类型
pub use crate::{Result, VmxError};

/// Return if current platform support virtualization extension.
pub fn has_hardware_support() -> bool {
    if let Some(feature) = raw_cpuid::CpuId::new().get_feature_info() {
        feature.has_vmx()
    } else {
        false
    }
}

pub fn read_vmcs_revision_id() -> u32 {
    VmxBasic::read().revision_id
}

fn as_axerr(err: x86::vmx::VmFail) -> VmxError {
    use x86::vmx::VmFail;
    match err {
        VmFail::VmFailValid => VmxError::VmxInstructionError(alloc::string::String::from(
            vmcs::instruction_error().as_str(),
        )),
        VmFail::VmFailInvalid => VmxError::InvalidVmcsPtr,
    }
}
