mod definitions;   // SvmExitCode / Intercept 位
mod instructions;  // vmrun / vmload / vmsave / stgi / clgi / invlpga
mod percpu;        // SvmPerCpuState（EFER.SVME & HSAVE）
mod structs;       // IOPm / MSRPm / Vmcb 封装
mod vcpu;          // SvmVcpu（核心逻辑）
mod vmcb;
mod flags;
mod frame;
// VMCB 读写 & VMEXIT 解码

pub use self::definitions::{SvmExitCode,SvmIntercept};
pub use self::percpu::SvmPerCpuState as SvmArchPerCpuState;
pub use self::vcpu::SvmVcpu       as SvmArchVCpu;


pub fn has_hardware_support() -> bool {
    if let Some(ext) = raw_cpuid::CpuId::new().get_extended_processor_and_feature_identifiers()
    {
        ext.has_svm()
    } else {
        false
    }
}

/* 额外：SVM 没有 VMX-instruction-error / VMCS revision id 概念，
 * 如需调试可在 vmcb::exit_code() 中直接查看 SvmExitCode。*/
