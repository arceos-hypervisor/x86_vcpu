use alloc::collections::VecDeque;
use axvisor_api::vmm::{VCpuId, VMId};
use bit_field::BitField;
use x86::io::outl;
use core::arch::asm;
use core::fmt::{Debug, Formatter, Result};
use core::{arch::naked_asm, mem::size_of};
use x86::controlregs::{Xcr0, xcr0 as xcr0_read, xcr0_write};
use x86::dtables::{self, DescriptorTablePointer};
use x86::msr::IA32_GS_BASE;
use x86::segmentation::SegmentSelector;
use x86_64::registers::control::{Cr0, Cr0Flags, Cr3, Cr4, Cr4Flags, EferFlags};

use axaddrspace::device::AccessWidth;
use axaddrspace::{GuestPhysAddr, GuestVirtAddr, HostPhysAddr, NestedPageFaultInfo};
use axerrno::{AxResult, ax_err, ax_err_type};
use axvcpu::{AxArchVCpu, AxVCpuExitReason, AxVCpuHal};
use tock_registers::interfaces::{Debuggable, ReadWriteable, Readable, Writeable};

use super::definitions::SvmExitCode;
use super::structs::{IOPm, MSRPm, VmcbFrame};
use super::vmcb::{NestedCtl, SvmExitInfo, VmcbCleanBits, VmcbTlbControl, set_vmcb_segment};
use crate::{ept::GuestPageWalkInfo, msr::Msr, regs::GeneralRegisters, xstate::XState};

const QEMU_EXIT_PORT: u16 = 0x604;
const QEMU_EXIT_MAGIC: u64 = 0x2000;

#[derive(PartialEq, Eq, Debug)]
pub enum VmCpuMode {
    Real,
    Protected,
    Compatibility, // IA-32E mode (CS.L = 0)
    Mode64,        // IA-32E mode (CS.L = 1)
}

/// States loaded/stored during VMLOAD/VMSAVE instructions.
///
/// VMLOAD/VMSAVE only load/store the guest version of these states from/to the
/// VMCB, but not the host version. Therefore, we need to keep track of the host
/// versions separately.
#[derive(Default)]
pub struct VmLoadSaveStates {
    /// The base address of the FS segment.
    pub fs_base: u64,
    /// The base address of the GS segment.
    pub gs_base: u64,
    /// The value of the KERNEL_GS_BASE MSR.
    pub kernel_gs_base: u64,
    /// The value of the SYSENTER_CS MSR.
    pub sysenter_cs: u64,
    /// The value of the SYSENTER_ESP MSR.
    pub sysenter_esp: u64,
    /// The value of the SYSENTER_EIP MSR.
    pub sysenter_eip: u64,
    /// The value of the STAR MSR.
    pub star: u64,
    /// The value of the LSTAR MSR.
    pub lstar: u64,
    /// The value of the CSTAR MSR.
    pub cstar: u64,
    /// The value of the SF_MASK MSR.
    pub sfmask: u64,
    /// The local descriptor table register.
    pub ldtr: u16,
    /// The task register.
    pub tr: u16,
}

// Some fields are not loaded/saved because they are not used in ArceOS and
// AxVisor, we keep their fields and loader/savers for future use.
#[allow(dead_code)]
impl VmLoadSaveStates {
    /// Save the current FS and GS (including KERNEL_GS_BASE) bases.
    #[inline(always)]
    pub fn save_fs_gs(&mut self) {
        self.fs_base = Msr::IA32_FS_BASE.read();
        self.gs_base = Msr::IA32_GS_BASE.read();
        self.kernel_gs_base = Msr::IA32_KERNEL_GSBASE.read();
    }

    /// Save the current SYSENTER MSRs.
    #[inline(always)]
    pub fn save_sysenter(&mut self) {
        self.sysenter_cs = Msr::IA32_SYSENTER_CS.read();
        self.sysenter_esp = Msr::IA32_SYSENTER_ESP.read();
        self.sysenter_eip = Msr::IA32_SYSENTER_EIP.read();
    }

    /// Save the current SYSCALL MSRs.
    #[inline(always)]
    pub fn save_syscall(&mut self) {
        self.star = Msr::IA32_STAR.read();
        self.lstar = Msr::IA32_LSTAR.read();
        self.cstar = Msr::IA32_CSTAR.read();
        self.sfmask = Msr::IA32_FMASK.read();
    }

    /// Save the current LDTR and TR registers.
    #[inline(always)]
    pub fn save_segs(&mut self) {
        let ldtr: u16;
        let tr: u16;

        unsafe {
            asm!(
                "sldt {0:x}",
                "str {1:x}",
                out(reg) ldtr,
                out(reg) tr,
            );
        }

        self.ldtr = ldtr;
        self.tr = tr;
    }

    /// Save all VMLOAD/VMSAVE related states.
    pub fn save_all(&mut self) {
        self.save_fs_gs();
        self.save_sysenter();
        self.save_syscall();
        self.save_segs();
    }

    /// Load the saved FS and GS (including KERNEL_GS_BASE) bases.
    #[inline(always)]
    pub fn load_fs_gs(&self) {
        unsafe {
            Msr::IA32_FS_BASE.write(self.fs_base);
            Msr::IA32_GS_BASE.write(self.gs_base);
            Msr::IA32_KERNEL_GSBASE.write(self.kernel_gs_base);
        }
    }

    /// Load the saved SYSENTER MSRs.
    #[inline(always)]
    pub fn load_sysenter(&self) {
        unsafe {
            Msr::IA32_SYSENTER_CS.write(self.sysenter_cs);
            Msr::IA32_SYSENTER_ESP.write(self.sysenter_esp);
            Msr::IA32_SYSENTER_EIP.write(self.sysenter_eip);
        }
    }

    /// Load the saved SYSCALL MSRs.
    #[inline(always)]
    pub fn load_syscall(&self) {
        unsafe {
            Msr::IA32_STAR.write(self.star);
            Msr::IA32_LSTAR.write(self.lstar);
            Msr::IA32_CSTAR.write(self.cstar);
            Msr::IA32_FMASK.write(self.sfmask);
        }
    }

    /// Load all VMLOAD/VMSAVE related states.
    #[inline(always)]
    pub fn load_segs(&self) {
        let ldtr = self.ldtr;
        let tr = self.tr;

        unsafe {
            asm!(
                "lldt {0:x}",
                "ltr {1:x}",
                in(reg) ldtr,
                in(reg) tr,
            );
        }
    }

    /// Load all VMLOAD/VMSAVE related states.
    pub fn load_all(&self) {
        self.load_fs_gs();
        self.load_sysenter();
        self.load_syscall();
        self.load_segs();
    }

    /// Create a new [`VmLoadSaveStates`] instance from current hardware states.
    pub fn new_from_hardware() -> Self {
        let mut states = Self::default();
        states.save_all();
        states
    }
}

#[repr(C)]
pub struct SvmVcpu<H: AxVCpuHal> {
    // DO NOT modify `guest_regs` and `host_stack_top` and their order unless you do know what you are doing!
    // DO NOT add anything before or between them unless you do know what you are doing!
    guest_regs: GeneralRegisters,
    #[allow(dead_code)] // actually used in asm!
    host_stack_top: u64,
    launched: bool,
    vmcb: VmcbFrame<H>,
    load_save_states: VmLoadSaveStates,
    iopm: IOPm<H>,
    msrpm: MSRPm<H>,
    pending_events: VecDeque<(u8, Option<u32>)>,
    xstate: XState,
    entry: Option<GuestPhysAddr>,
    npt_root: Option<HostPhysAddr>,
    // is_host: bool, temporary removed because we don't care about type 1.5 now
}

impl<H: AxVCpuHal> SvmVcpu<H> {
    /// Create a new [`SvmVcpu`].
    pub fn new() -> AxResult<Self> {
        let vcpu = Self {
            guest_regs: GeneralRegisters::default(),
            host_stack_top: 0,
            launched: false,
            vmcb: VmcbFrame::new()?,
            load_save_states: VmLoadSaveStates::default(),
            iopm: IOPm::passthrough_all()?,
            msrpm: MSRPm::passthrough_all()?,
            pending_events: VecDeque::with_capacity(8),
            xstate: XState::new(),
            entry: None,
            npt_root: None,
            // is_host: false,
        };
        info!("[HV] created SvmVcpu(vmcb: {:#x})", vcpu.vmcb.phys_addr());
        Ok(vcpu)
    }

    /// Set the new [`SvmVcpu`] context from guest OS.
    // pub fn setup(&mut self, npt_root: HostPhysAddr, entry: GuestPhysAddr) -> AxResult {
    //     self.setup_vmcb(entry, npt_root)?;
    //     Ok(())
    // }

    /// No operation is needed for SVM binding.
    ///
    /// Unlike VMX which requires VMCS to be loaded via VMPTRLD,
    /// SVM uses the `VMRUN` instruction and takes the VMCB physical address
    /// from the `RAX` register at the moment of execution.
    ///
    /// Since `RAX` is a volatile register and may be clobbered during normal execution,
    /// it is unsafe to set `RAX` earlier and rely on it later.
    /// Therefore, the correct place to set `RAX` is right before `VMRUN`,
    /// inside the actual launch/resume assembly code.
    ///
    /// This function is kept for interface consistency but performs no action.
    pub fn bind_to_current_processor(&self) -> AxResult {
        Ok(())
    }

    /// No operation is needed for SVM unbinding.
    ///
    /// SVM does not maintain a per-CPU binding state like VMX (e.g., via VMPTRLD).
    /// Once `VMEXIT` occurs, the VCPU state is saved to the VMCB, and no
    /// unbinding step is required.
    ///
    /// This function is kept for interface compatibility.
    pub fn unbind_from_current_processor(&self) -> AxResult {
        Ok(())
    }

    pub fn get_cpu_mode(&self) -> VmCpuMode {
        let vmcb = &mut unsafe { self.vmcb.as_vmcb() }.state;

        let ia32_efer = vmcb.efer.get();
        let cs_attr = vmcb.cs.attr.get();
        let cr0 = vmcb.cr0.get();

        if (ia32_efer & (1 << 10)) != 0 {
            if (cs_attr & (1 << 13)) != 0 {
                // CS.L = 1
                VmCpuMode::Mode64
            } else {
                VmCpuMode::Compatibility
            }
        } else if (cr0 & (1 << 0)) != 0 {
            // CR0.PE = 1
            VmCpuMode::Protected
        } else {
            VmCpuMode::Real
        }
    }

    pub fn inner_run(&mut self) -> Option<SvmExitInfo> {
        // Inject pending events
        if self.launched {
            self.inject_pending_events().unwrap();
        }

        // Run guest
        self.load_guest_xstate();

        unsafe {
            self.svm_run();
        }

        self.load_host_xstate();

        // Handle vm-exits
        let exit_info = self.exit_info().unwrap();
        panic!("VM exit: {:#x?}", exit_info);

        match self.builtin_vmexit_handler(&exit_info) {
            Some(result) => {
                if result.is_err() {
                    panic!(
                        "VmxVcpu failed to handle a VM-exit that should be handled by itself: {:?}, error {:?}, vcpu: {:#x?}",
                        exit_info.exit_info_1,
                        result.unwrap_err(),
                        self
                    );
                }
                None
            }
            None => Some(exit_info),
        }
    }

    pub fn exit_info(&self) -> AxResult<SvmExitInfo> {
        unsafe { self.vmcb.as_vmcb().exit_info() }
    }

    pub fn raw_interrupt_exit_info(&self) -> AxResult<u32> {
        todo!()
    }

    // pub fn interrupt_exit_info(&self) -> AxResult<SvmInterruptInfo> {
    //     todo!()
    // }

    // pub fn io_exit_info(&self) -> AxResult<svm::SvmIoExitInfo> {
    //     todo!()
    // }

    pub fn nested_page_fault_info(&self) -> AxResult<NestedPageFaultInfo> {
        todo!()
    }

    pub fn regs(&self) -> &GeneralRegisters {
        &self.guest_regs
    }

    pub fn regs_mut(&mut self) -> &mut GeneralRegisters {
        &mut self.guest_regs
    }

    pub fn stack_pointer(&self) -> usize {
        todo!()
    }

    pub fn set_stack_pointer(&mut self, rsp: usize) {
        todo!()
    }

    pub fn gla2gva(&self, guest_rip: GuestVirtAddr) -> GuestVirtAddr {
        let vmcb = unsafe { self.vmcb.as_vmcb() };
        let cpu_mode = self.get_cpu_mode();
        let seg_base = if cpu_mode == VmCpuMode::Mode64 {
            0
        } else {
            vmcb.state.cs.base.get()
        };
        guest_rip + seg_base as usize
    }

    pub fn get_ptw_info(&self) -> GuestPageWalkInfo {
        todo!()
    }

    pub fn rip(&self) -> usize {
        todo!()
    }

    pub fn cs(&self) -> u16 {
        todo!()
    }

    pub fn advance_rip(&mut self, instr_len: u8) -> AxResult {
        todo!()
    }

    pub fn queue_event(&mut self, vector: u8, err_code: Option<u32>) {
        todo!()
    }

    pub fn set_interrupt_window(&mut self, enable: bool) -> AxResult {
        todo!()
    }

    pub fn set_io_intercept_of_range(&mut self, port_base: u32, count: u32, intercept: bool) {
        todo!()
    }

    pub fn set_msr_intercept_of_range(&mut self, msr: u32, intercept: bool) {
        todo!()
    }
}

// Implementation of private methods
impl<H: AxVCpuHal> SvmVcpu<H> {
    #[allow(dead_code)]
    fn setup_io_bitmap(&mut self) -> AxResult {
        todo!()
    }

    #[allow(dead_code)]
    fn setup_msr_bitmap(&mut self) -> AxResult {
        todo!()
    }

    fn setup_vmcb(&mut self, entry: GuestPhysAddr, npt_root: HostPhysAddr) -> AxResult {
        // Commented out because not implemented yet
        // self.setup_io_bitmap()?;
        // self.setup_msr_bitmap()?;
        self.setup_vmcb_guest(entry)?;
        self.setup_vmcb_control(npt_root, true)
    }

    fn setup_vmcb_guest(&mut self, entry: GuestPhysAddr) -> AxResult {
        info!("[AxVM] Setting up VMCB for guest at {:#x}", entry);
        let cr0_val: Cr0Flags =
            Cr0Flags::NOT_WRITE_THROUGH | Cr0Flags::CACHE_DISABLE | Cr0Flags::EXTENSION_TYPE;
        self.set_cr(0, cr0_val.bits())?;
        self.set_cr(4, 0)?;

        let st = &mut unsafe { self.vmcb.as_vmcb() }.state;

        macro_rules! seg {
            ($seg:ident, $attr:expr) => {
                set_vmcb_segment(&mut st.$seg, 0, $attr);
            };
        }

        // CS: P S CODE READ (bit 7, 4, 3, 1) = 0x9a
        // seg!(cs, 0x9b);
        // st.cs.selector.set(0xf000);
        // st.cs.base.set(0xffff0000);
        // st.cs.limit.set(0xffff);
        // st.cs.attr.set(0x9b);
        st.cs.selector.set(0);
        st.cs.base.set(0);
        st.cs.limit.set(0xffff);
        st.cs.attr.set(0x9b);

        // DS ~ SS: P S WRITE (bit 7, 4, 1) = 0x92
        seg!(ds, 0x93);
        seg!(es, 0x93);
        seg!(fs, 0x93);
        seg!(gs, 0x93);
        seg!(ss, 0x93);
        seg!(ldtr, 0x82);
        seg!(tr, 0x8b);

        // GDTR / IDTR
        st.gdtr.base.set(0);
        st.gdtr.limit.set(0xffff);
        st.idtr.base.set(0);
        st.idtr.limit.set(0xffff);

        // 关键寄存器与指针
        st.cr3.set(0);
        st.dr7.set(0x400);
        st.rsp.set(0);
        st.rip.set(entry.as_usize() as u64);
        st.rflags.set(0x2); // bit1 必须为 1
        st.dr6.set(0xffff0ff0);

        // SYSENTER MSRs
        // st.sysenter_cs.set(0);
        // st.sysenter_esp.set(0);
        // st.sysenter_eip.set(0);

        // MSR / PAT / EFER
        st.efer
            .set(0 | EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE.bits()); // 必须置 SVME 位
        st.g_pat.set(Msr::IA32_PAT.read());

        // st.cpl.set(0);
        // st.star.set(0);
        // st.lstar.set(0);
        // st.cstar.set(0);
        // st.sfmask.set(0);
        // st.kernel_gs_base.set(Msr::IA32_KERNEL_GSBASE.read());
        // st.rax.set(0); // hypervisor 返回值

        Ok(())
    }

    fn setup_vmcb_control(&mut self, npt_root: HostPhysAddr, is_guest: bool) -> AxResult {
        let ct = &mut unsafe { self.vmcb.as_vmcb() }.control; // control-area 速记别名
        // ────────────────────────────────────────────────────────
        // 1) 基本运行环境：Nested Paging / ASID / Clean Bits / TLB
        // ────────────────────────────────────────────────────────

        // ① 开启 Nested Paging（AMD 对应 Intel 的 EPT）
        //    → set bit 0 of NESTED_CTL
        ct.nested_ctl.modify(NestedCtl::NP_ENABLE::SET);

        // ② guest ASID：NPT 使用的 TLB 标签
        ct.guest_asid.set(1);

        // ③ 嵌套 CR3（NPT root PA）
        ct.nested_cr3.set(npt_root.as_usize() as u64);

        // ④ Clean-Bits：0 = “全部脏” ⇒ 第一次 VMRUN 必定重新加载 save-area
        ct.clean_bits.set(0);

        // ⑤ TLB Control：0 = NONE, 1 = FLUSH-ASID, 3 = FLUSH-ALL
        ct.tlb_control
            .modify(VmcbTlbControl::CONTROL::FlushGuestTlb);

        ct.int_control.set(1 << 24); // V_INTR_MASKING_MASK

        // ────────────────────────────────────────────────────────
        // 2) 选择要拦截的指令 / 事件
        //    （相当于 VMX 的 Pin-based / Primary / Secondary CTLS）
        // ────────────────────────────────────────────────────────

        use super::definitions::SvmIntercept; // 你自己定义的枚举

        for intc in &[
            SvmIntercept::NMI,      // 非屏蔽中断
            SvmIntercept::CPUID,    // CPUID 指令
            SvmIntercept::SHUTDOWN, // HLT 时 Triple-Fault
            SvmIntercept::VMRUN,    // 来宾企图再次 VMRUN
            SvmIntercept::VMMCALL,  // Hypercall
            SvmIntercept::VMLOAD,
            SvmIntercept::VMSAVE,
            SvmIntercept::STGI,   // 设置全局中断
            SvmIntercept::CLGI,   // 清除全局中断
            SvmIntercept::SKINIT, // 安全启动
        ] {
            ct.set_intercept(*intc);
        }

        ct.iopm_base_pa.set(self.iopm.phys_addr().as_usize() as u64);
        ct.msrpm_base_pa
            .set(self.msrpm.phys_addr().as_usize() as u64);

        Ok(())
    }
    // 如果你用 bitfield 方式，也可以：
    // ct.intercept_vector3.modify(InterceptVec3::NMI::SET + InterceptVec3::VINTR::SET);

    fn get_paging_level(&self) -> usize {
        todo!()
    }
}
// Implementaton for type1.5 hypervisor
// #[cfg(feature = "type1_5")]
impl<H: AxVCpuHal> SvmVcpu<H> {
    pub fn set_cr(&mut self, cr_idx: usize, val: u64) -> AxResult {
        let vmcb = unsafe { self.vmcb.as_vmcb() };
        info!("Setting CR{} to {:#x}", cr_idx, val);

        match cr_idx {
            0 => vmcb.state.cr0.set(val),
            3 => vmcb.state.cr3.set(val),
            4 => vmcb.state.cr4.set(val),
            _ => return ax_err!(InvalidInput, format_args!("Unsupported CR{}", cr_idx)),
        }

        Ok(())
    }
    #[allow(dead_code)]
    fn cr(&self, cr_idx: usize) -> usize {
        let mut vmcb = unsafe { self.vmcb.as_vmcb() };
        (|| -> AxResult<usize> {
            Ok(match cr_idx {
                0 => vmcb.state.cr0.get() as usize,
                3 => vmcb.state.cr3.get() as usize,
                4 => vmcb.state.cr4.get() as usize,
                _ => unreachable!(),
            })
        })()
        .expect("Failed to read guest control register")
    }
}

impl<H: AxVCpuHal> SvmVcpu<H> {
    //  unsafe extern "C" fn svm_run(&mut self) -> usize {
    //     let vmcb_phy = self.vmcb.phys_addr().as_usize() as u64;
    //
    //      unsafe {
    //         naked_asm!(
    //             save_regs_to_stack!(),
    //             // "clgi",                                // 清除中断，确保 SVM 运行不中断
    //             "mov    [rdi + {host_stack_size}], rsp", // save current RSP to Vcpu::host_stack_top
    //             "mov    rsp, rdi",                      // set RSP to guest regs area
    //             restore_regs_from_stack!(),            // restore guest status
    //             "mov rax,{vmcb}",
    //             "vmload rax",
    //             "vmrun rax",
    //             "jmp {failed}",
    //             host_stack_size = const size_of::<GeneralRegisters>(),
    //             failed = sym Self::svm_entry_failed,
    //             vmcb = in(reg) vmcb_phy,  // 正确绑定 vmcb 变量
    //             // options(noreturn),
    //         );
    //     }
    //      0
    // }

    /// Operations immediately before VMRUN instruction. This includes:
    ///
    /// 1. Disabling interrupts (CLGI)
    /// 2. Syncing RAX from guest_regs to VMCB
    /// 3. Saving host FS/GS related states
    /// 4. `VMLOAD`ing the VMCB
    #[inline(always)]
    fn before_vmrun(&mut self) {
        unsafe {
            asm!("clgi");
        }

        unsafe { self.vmcb.as_vmcb().state.rax.set(self.regs().rax) };

        self.load_save_states.save_fs_gs();

        unsafe {
            asm!(
                "vmload rax",
                in("rax") self.vmcb.phys_addr().as_usize() as u64,
            );
        }
    }

    /// Operations immediately after VMRUN instruction. This includes:
    ///
    /// 1. `VMSAVE`ing the VMCB
    /// 2. Restoring host FS/GS related states
    /// 3. Syncing RAX from VMCB to guest_regs
    /// 4. Enabling interrupts (STGI)
    #[inline(always)]
    fn after_vmrun(&mut self) {
        let vmcb = self.vmcb.phys_addr().as_usize() as u64;

        unsafe {
            asm!(
                "vmsave rax",
                in("rax") vmcb,
            );
        }

        self.load_save_states.load_fs_gs();

        self.regs_mut().rax = unsafe { self.vmcb.as_vmcb().state.rax.get() };

        unsafe {
            asm!("stgi");
        }
    }

    pub unsafe fn svm_run(&mut self) {
        let self_addr = self as *mut Self as u64;
        let vmcb = self.vmcb.phys_addr().as_usize() as u64;

        self.before_vmrun();

        unsafe {
            asm!(
                save_regs_to_stack!(norax),             // Save host gpr except RAX, which holds vmcb pa
                "mov [rdi + {host_stack_top}], rsp",    // Save current RSP to Vcpu::host_stack_top
                "mov rsp, rdi",                         // Set RSP to guest_regs area
                restore_regs_from_stack!(norax),        // Restore guest status except RAX
                "vmrun rax",                            // Let's go!
                save_regs_to_stack!(norax),             // Save guest gpr except RAX
                "mov rdi, rsp",                         // Regain the pointer to VCpu struct
                "mov rsp, [rdi + {host_stack_top}]",    // Restore host RSP from Vcpu::host_stack_top
                restore_regs_from_stack!(norax),        // Restore host gpr except RAX
                host_stack_top = const size_of::<GeneralRegisters>(),
                in("rax") vmcb,
                in("rdi") self_addr,
            )
        }

        self.after_vmrun();
    }

    fn allow_interrupt(&self) -> bool {
        todo!()
    }

    fn inject_pending_events(&mut self) -> AxResult {
        todo!()
    }

    fn builtin_vmexit_handler(&mut self, exit_info: &SvmExitInfo) -> Option<AxResult> {
        let exit_code = match exit_info.exit_code {
            Ok(code) => code,
            Err(code) => {
                error!("Unknown #VMEXIT exit code: {:#x}", code);
                panic!("wrong code");
            }
        };

        match exit_code {
            SvmExitCode::CPUID => Some(self.handle_cpuid()),
            _ => None,
        }

        //
        // let res = match exit_code {
        //     SvmExitCode::EXCP(vec) => self.handle_exception(vec, &exit_info),
        //     SvmExitCode::NMI => self.handle_nmi(),
        //     SvmExitCode::CPUID => self.handle_cpuid(),
        //     SvmExitCode::VMMCALL => self.handle_hypercall(),
        //     SvmExitCode::NPF => self.handle_nested_page_fault(&exit_info),
        //     SvmExitCode::MSR => match exit_info.exit_info_1 {
        //         0 => self.handle_msr_read(),
        //         1 => self.handle_msr_write(),
        //         _ => panic!("MSR can't handle"),
        //     },
        //     SvmExitCode::SHUTDOWN => {
        //         error!("#VMEXIT(SHUTDOWN): {:#x?}", exit_info);
        //         self.cpu_data.vcpu.inject_fault()?;
        //         Ok(())
        //     }
        //     _ => panic!("code can't handle"),
        // };
    }

    fn handle_svm_preemption_timer(&mut self) -> AxResult {
        todo!()
    }

    fn handle_cr(&mut self) -> AxResult {
        todo!()
    }

    fn handle_cpuid(&mut self) -> AxResult {
        todo!()
    }

    fn handle_xsetbv(&mut self) -> AxResult {
        todo!()
    }

    fn load_guest_xstate(&mut self) {
        self.xstate.switch_to_guest();
    }

    fn load_host_xstate(&mut self) {
        self.xstate.switch_to_host();
    }
}

impl<H: AxVCpuHal> Drop for SvmVcpu<H> {
    fn drop(&mut self) {
        todo!()
    }
}

impl<H: AxVCpuHal> core::fmt::Debug for SvmVcpu<H> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        todo!()
    }
}

impl<H: AxVCpuHal> AxArchVCpu for SvmVcpu<H> {
    type CreateConfig = ();
    type SetupConfig = ();

    fn new(vm_id: VMId, vcpu_id: VCpuId, config: Self::CreateConfig) -> AxResult<Self> {
        Self::new()
    }

    fn set_entry(&mut self, entry: GuestPhysAddr) -> AxResult {
        self.entry = Some(entry);
        Ok(())
    }

    fn set_ept_root(&mut self, ept_root: HostPhysAddr) -> AxResult {
        self.npt_root = Some(ept_root);
        Ok(())
    }

    fn setup(&mut self, _config: Self::SetupConfig) -> AxResult {
        self.setup_vmcb(self.entry.unwrap(), self.npt_root.unwrap())
    }

    fn run(&mut self) -> AxResult<AxVCpuExitReason> {
        match self.inner_run() {
            Some(exit_info) => {
                warn!("VMX unsupported VM-Exit: {:#x?}", exit_info.exit_info_1);
                warn!("VCpu {:#x?}", self);
                Ok(AxVCpuExitReason::Halt)
            }
            _ => Ok(AxVCpuExitReason::Halt),
        }
    }

    fn bind(&mut self) -> AxResult {
        self.bind_to_current_processor()
    }

    fn unbind(&mut self) -> AxResult {
        self.launched = false;
        self.unbind_from_current_processor()
    }

    fn set_gpr(&mut self, reg: usize, val: usize) {
        self.regs_mut().set_reg_of_index(reg as u8, val as u64);
    }

    fn inject_interrupt(&mut self, vector: usize) -> AxResult {
        todo!()
    }

    fn set_return_value(&mut self, val: usize) {
        self.regs_mut().rax = val as u64;
    }
}
