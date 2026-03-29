mod definitions;
mod instructions;
mod percpu;
mod vcpu;
mod vmcb;

pub fn has_hardware_support() -> bool {
    raw_cpuid::CpuId::new().get_svm_info().is_some()
}
