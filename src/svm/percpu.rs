//! AMD-SVM per-CPU enable/disable logic
//!
//! References (AMD APM v2 *System Programming*):
//! https://www.amd.com/content/dam/amd/en/documents/processor-tech-docs/programmer-references/24593.pdf
//!   • § 15.4 Enabling SVM
//!
//! Summary of procedure (compared to Intel VMX):
//!  1.  check if the CPU supports SVM
//!  2. Allocate Host-Save Area (HSAVE) and write its physical address to `MSR_VM_HSAVE_PA`
//!  3. Set `EFER.SVME` (bit 12) to enable SVM mode
//!  4. Clearing `EFER.SVME` disables SVM (no need for VMXON/VMXOFF equivalents)

use axerrno::{ax_err, ax_err_type, AxResult};
use axvcpu::{AxArchPerCpu, AxVCpuHal};
use memory_addr::PAGE_SIZE_4K as PAGE_SIZE;
use raw_cpuid::CpuId;
use x86_64::registers::control::EferFlags;

use crate::frame::PhysFrame;
use crate::msr::Msr;
use crate::svm::has_hardware_support;


/// Per-core state for AMD-SVM

// (AMD64 APM Vol.2, Section 15.30.4)
//The 64-bit read/write VM_HSAVE_PA MSR holds the physical address of a 4KB block of memory where VMRUN saves host state
pub struct SvmPerCpuState<H: AxVCpuHal> {
    hsave_page: PhysFrame<H>,
}

impl<H: AxVCpuHal> AxArchPerCpu for SvmPerCpuState<H> {
    fn new(_cpu_id: usize) -> AxResult<Self> {
        Ok(Self {
            hsave_page: unsafe { PhysFrame::uninit() },
        })
    }

    /// Returns true if SVM is enabled on this core (EFER.SVME == 1)
    fn is_enabled(&self) -> bool {
        let efer = Msr::IA32_EFER.read();
        EferFlags::from_bits_truncate(efer).contains(EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE)
    }

    fn hardware_enable(&mut self) -> AxResult {
        if !has_hardware_support() {
            return ax_err!(Unsupported, "CPU does not support AMD-SVM");
        }
        if self.is_enabled() {
            return ax_err!(ResourceBusy, "SVM already enabled");
        }

        // Enable XSAVE/XRSTOR.
        super::vcpu::XState::enable_xsave();

        // Allocate & register Host-Save Area
        self.hsave_page = PhysFrame::alloc_zero()?;
        let hsave_pa = self.hsave_page.start_paddr().as_usize() as u64;
        unsafe { Msr::VM_HSAVE_PA.write(hsave_pa); }


        //Set EFER.SVME to enable SVM
        let mut efer = EferFlags::from_bits_truncate(Msr::IA32_EFER.read());
        efer.insert(EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE); // bit 12
        unsafe { Msr::IA32_EFER.write(efer.bits()); }

        info!("[AxVM] SVM enabled (HSAVE @ {:#x}).", hsave_pa);
        Ok(())
    }


    fn hardware_disable(&mut self) -> AxResult {
        if !self.is_enabled() {
            return ax_err!(BadState, "SVM is not enabled");
        }
        unsafe {
        // 1) Clear SVME bit
        let mut efer = EferFlags::from_bits_truncate(Msr::IA32_EFER.read());
        efer.remove(EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE);
        Msr::IA32_EFER.write(efer.bits());

        // 2) Clear HSAVE pointer
        Msr::VM_HSAVE_PA.write(0);
    }
        info!("[AxVM] SVM disabled.");
        Ok(())
    }
}
