use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt::{Debug, Formatter, Result};
use core::{arch::naked_asm, mem::size_of};

use bit_field::BitField;
use raw_cpuid::CpuId;
use x86::bits64::vmx;
use x86::controlregs::Xcr0;
use x86::dtables::{self, DescriptorTablePointer};
use x86::segmentation::SegmentSelector;
use x86_64::VirtAddr;
use x86_64::registers::control::{Cr0, Cr0Flags, Cr3, Cr4, Cr4Flags, EferFlags};

use page_table_entry::x86_64::X64PTE;
use page_table_multiarch::{PageSize, PagingHandler, PagingResult};

use axaddrspace::EPTTranslator;
use axaddrspace::{GuestPhysAddr, GuestVirtAddr, HostPhysAddr, MappingFlags, NestedPageFaultInfo};
use axerrno::{AxResult, ax_err, ax_err_type};
use axvcpu::{AccessWidth, AxArchVCpu, AxVCpuExitReason, AxVCpuHal, AxVcpuAccessGuestState};

use super::VmxExitInfo;
use super::as_axerr;
use super::definitions::VmxExitReason;
use super::read_vmcs_revision_id;
use super::structs::{EptpList, IOBitmap, MsrBitmap, VmxRegion};
use super::vmcs::{
    self, VmcsControl32, VmcsControl64, VmcsControlNW, VmcsGuest16, VmcsGuest32, VmcsGuest64,
    VmcsGuestNW, VmcsHost16, VmcsHost32, VmcsHost64, VmcsHostNW, exit_qualification,
    interrupt_exit_info,
};
use crate::LinuxContext;
use crate::page_table::GuestPageTable64;
use crate::page_table::GuestPageWalkInfo;
use crate::segmentation::{Segment, SegmentAccessRights};
use crate::xstate::XState;
use crate::{msr::Msr, regs::GeneralRegisters};

const VMX_PREEMPTION_TIMER_SET_VALUE: u32 = 1_000_000;

const QEMU_EXIT_PORT: u16 = 0x604;
const QEMU_EXIT_MAGIC: u64 = 0x2000;

#[derive(PartialEq, Eq, Debug)]
pub enum VmCpuMode {
    Real,
    Protected,
    Compatibility, // IA-32E mode (CS.L = 0)
    Mode64,        // IA-32E mode (CS.L = 1)
}

const MSR_IA32_EFER_LMA_BIT: u64 = 1 << 10;
const CR0_PE: usize = 1 << 0;

/// A virtual CPU within a guest.
#[repr(C)]
pub struct VmxVcpu<H: AxVCpuHal> {
    // DO NOT modify `guest_regs` and `host_stack_top` and their order unless you do know what you are doing!
    // DO NOT add anything before or between them unless you do know what you are doing!
    guest_regs: GeneralRegisters,
    host_stack_top: u64,
    launched: bool,
    vmcs: VmxRegion<H>,
    io_bitmap: IOBitmap<H>,
    msr_bitmap: MsrBitmap<H>,
    eptp_list: EptpList<H>,

    pending_events: VecDeque<(u8, Option<u32>)>,
    // xstate: XState,
    xstate: XState,
    entry: Option<GuestPhysAddr>,
    ept_root: Option<HostPhysAddr>,

    id: usize,
}

impl<H: AxVCpuHal> VmxVcpu<H> {
    /// Create a new [`VmxVcpu`].
    pub fn new(id: usize) -> AxResult<Self> {
        let vcpu = Self {
            guest_regs: GeneralRegisters::default(),
            host_stack_top: 0,
            launched: false,
            vmcs: VmxRegion::new(read_vmcs_revision_id(), false)?,
            io_bitmap: IOBitmap::passthrough_all()?,
            msr_bitmap: MsrBitmap::passthrough_all()?,
            eptp_list: EptpList::new()?,
            pending_events: VecDeque::with_capacity(8),
            xstate: XState::new(),
            entry: None,
            ept_root: None,
            id,
        };
        debug!("[HV] created VmxVcpu(vmcs: {:#x})", vcpu.vmcs.phys_addr(),);
        Ok(vcpu)
    }

    // /// Get the identifier of this [`VmxVcpu`].
    // pub fn vcpu_id(&self) -> usize {
    //     get_current_vcpu::<Self>().unwrap().id()
    // }

    /// Bind this [`VmxVcpu`] to current logical processor.
    pub fn bind_to_current_processor(&self) -> AxResult {
        trace!(
            "VmxVcpu bind to current processor vmcs @ {:#x}",
            self.vmcs.phys_addr()
        );
        unsafe {
            vmx::vmptrld(self.vmcs.phys_addr().as_usize() as u64).map_err(as_axerr)?;
        }
        self.setup_vmcs_host()?;
        Ok(())
    }

    /// Unbind this [`VmxVcpu`] from current logical processor.
    pub fn unbind_from_current_processor(&self) -> AxResult {
        trace!(
            "VmxVcpu unbind from current processor vmcs @ {:#x}",
            self.vmcs.phys_addr()
        );

        unsafe {
            vmx::vmclear(self.vmcs.phys_addr().as_usize() as u64).map_err(as_axerr)?;
        }
        Ok(())
    }

    /// Get CPU mode of the guest.
    pub fn get_cpu_mode(&self) -> VmCpuMode {
        let ia32_efer = Msr::IA32_EFER.read();
        let cs_access_right = VmcsGuest32::CS_ACCESS_RIGHTS.read().unwrap();
        let cr0 = VmcsGuestNW::CR0.read().unwrap();
        if (ia32_efer & MSR_IA32_EFER_LMA_BIT) != 0 {
            if (cs_access_right & 0x2000) != 0 {
                // CS.L = 1
                VmCpuMode::Mode64
            } else {
                VmCpuMode::Compatibility
            }
        } else if (cr0 & CR0_PE) != 0 {
            VmCpuMode::Protected
        } else {
            VmCpuMode::Real
        }
    }

    /// Run the guest. It returns when a vm-exit happens and returns the vm-exit if it cannot be handled by this [`VmxVcpu`] itself.
    pub fn inner_run(&mut self) -> Option<VmxExitInfo> {
        // Inject pending events
        if self.launched {
            self.inject_pending_events().unwrap();
        }

        // Run guest
        self.load_guest_xstate();
        unsafe {
            if self.launched {
                self.vmx_resume();
            } else {
                self.launched = true;
                VmcsHostNW::RSP
                    .write(&self.host_stack_top as *const _ as usize)
                    .unwrap();

                self.vmx_launch();
            }
        }
        self.load_host_xstate();

        // Handle vm-exits
        let exit_info = self.exit_info().unwrap();
        trace!("VM exit: {:#x?}", exit_info);

        match self.builtin_vmexit_handler(&exit_info) {
            Some(result) => {
                if result.is_err() {
                    panic!(
                        "VmxVcpu failed to handle a VM-exit that should be handled by itself: {:?}, error {:?}, vcpu: {:#x?}",
                        exit_info.exit_reason,
                        result.unwrap_err(),
                        self
                    );
                }

                None
            }
            None => Some(exit_info),
        }
    }

    /// Basic information about VM exits.
    pub fn exit_info(&self) -> AxResult<vmcs::VmxExitInfo> {
        vmcs::exit_info()
    }

    /// Raw information for VM Exits Due to Vectored Events, See SDM 25.9.2
    pub fn raw_interrupt_exit_info(&self) -> AxResult<u32> {
        vmcs::raw_interrupt_exit_info()
    }

    /// Information for VM exits due to external interrupts.
    pub fn interrupt_exit_info(&self) -> AxResult<vmcs::VmxInterruptInfo> {
        vmcs::interrupt_exit_info()
    }

    /// Information for VM exits due to I/O instructions.
    pub fn io_exit_info(&self) -> AxResult<vmcs::VmxIoExitInfo> {
        vmcs::io_exit_info()
    }

    /// Information for VM exits due to nested page table faults (EPT violation).
    pub fn nested_page_fault_info(&self) -> AxResult<NestedPageFaultInfo> {
        vmcs::ept_violation_info()
    }

    /// Guest general-purpose registers.
    pub fn regs(&self) -> &GeneralRegisters {
        &self.guest_regs
    }

    /// Mutable reference of guest general-purpose registers.
    pub fn regs_mut(&mut self) -> &mut GeneralRegisters {
        &mut self.guest_regs
    }

    /// Guest stack pointer. (`RSP`)
    pub fn stack_pointer(&self) -> usize {
        VmcsGuestNW::RSP.read().unwrap()
    }

    /// Set guest stack pointer. (`RSP`)
    pub fn set_stack_pointer(&mut self, rsp: usize) {
        VmcsGuestNW::RSP.write(rsp).unwrap()
    }

    /// Get Translate guest page table info
    pub fn get_pagetable_walk_info(&self) -> GuestPageWalkInfo {
        let cr3 = VmcsGuestNW::CR3.read().unwrap();
        let level = self.get_paging_level();
        let is_write_access = false;
        let is_inst_fetch = false;
        let is_user_mode_access = ((VmcsGuest32::SS_ACCESS_RIGHTS.read().unwrap() >> 5) & 0x3) == 3;
        let mut pse = true;
        let mut nxe =
            (VmcsGuest64::IA32_EFER.read().unwrap() & EferFlags::NO_EXECUTE_ENABLE.bits()) != 0;
        let wp = (VmcsGuestNW::CR0.read().unwrap() & Cr0Flags::WRITE_PROTECT.bits() as usize) != 0;
        let is_smap_on = (VmcsGuestNW::CR4.read().unwrap()
            & Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION.bits() as usize)
            != 0;
        let is_smep_on = (VmcsGuestNW::CR4.read().unwrap()
            & Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION.bits() as usize)
            != 0;
        let width: u32;
        if level == 4 || level == 3 {
            width = 9;
        } else if level == 2 {
            width = 10;
            pse = VmcsGuestNW::CR4.read().unwrap() & Cr4Flags::PAGE_SIZE_EXTENSION.bits() as usize
                != 0;
            nxe = false;
        } else {
            width = 0;
        }
        GuestPageWalkInfo {
            cr3,
            level,
            width,
            is_user_mode_access,
            is_write_access,
            is_inst_fetch,
            pse,
            wp,
            nxe,
            is_smap_on,
            is_smep_on,
        }
    }

    /// Guest rip. (`RIP`)
    pub fn rip(&self) -> usize {
        VmcsGuestNW::RIP.read().unwrap()
    }

    /// Guest cs. (`cs`)
    pub fn cs(&self) -> u16 {
        VmcsGuest16::CS_SELECTOR.read().unwrap()
    }

    /// Advance guest `RIP` by `instr_len` bytes.
    pub fn advance_rip(&mut self, instr_len: u8) -> AxResult {
        VmcsGuestNW::RIP.write(VmcsGuestNW::RIP.read()? + instr_len as usize)
    }

    /// Add a virtual interrupt or exception to the pending events list,
    /// and try to inject it before later VM entries.
    pub fn queue_event(&mut self, vector: u8, err_code: Option<u32>) {
        self.pending_events.push_back((vector, err_code));
    }

    /// If enable, a VM exit occurs at the beginning of any instruction if
    /// `RFLAGS.IF` = 1 and there are no other blocking of interrupts.
    /// (see SDM, Vol. 3C, Section 24.4.2)
    pub fn set_interrupt_window(&mut self, enable: bool) -> AxResult {
        let mut ctrl = VmcsControl32::PRIMARY_PROCBASED_EXEC_CONTROLS.read()?;
        let bits = vmcs::controls::PrimaryControls::INTERRUPT_WINDOW_EXITING.bits();
        if enable {
            ctrl |= bits
        } else {
            ctrl &= !bits
        }
        VmcsControl32::PRIMARY_PROCBASED_EXEC_CONTROLS.write(ctrl)?;
        Ok(())
    }

    /// Set I/O intercept by modifying I/O bitmap.
    pub fn set_io_intercept_of_range(&mut self, port_base: u32, count: u32, intercept: bool) {
        self.io_bitmap
            .set_intercept_of_range(port_base, count, intercept)
    }

    /// Set msr intercept by modifying msr bitmap.
    /// Todo: distinguish read and write.
    pub fn set_msr_intercept_of_range(&mut self, msr: u32, intercept: bool) {
        self.msr_bitmap.set_read_intercept(msr, intercept);
        self.msr_bitmap.set_write_intercept(msr, intercept);
    }

    pub fn read_guest_memory(&self, gva: GuestVirtAddr, len: usize) -> AxResult<Vec<u8>> {
        debug!("read_guest_memory @{:?} len: {}", gva, len);

        let mut content = Vec::with_capacity(len as usize);

        let mut remained_size = len;
        let mut addr = gva;

        while remained_size > 0 {
            let (gpa, _flags, page_size) = self.guest_page_table_query(gva).map_err(|e| {
                warn!(
                    "Failed to query guest page table, GVA {:?} err {:?}",
                    gva, e
                );
                ax_err_type!(BadAddress)
            })?;
            let pgoff = page_size.align_offset(addr.into());
            let read_size = (page_size as usize - pgoff).min(remained_size);
            addr += read_size;
            remained_size -= read_size;

            if let Some((hpa, _flags, _pgsize)) = H::EPTTranslator::guest_phys_to_host_phys(gpa) {
                let hva_ptr = H::PagingHandler::phys_to_virt(hpa).as_ptr();
                for i in 0..read_size {
                    content.push(unsafe { hva_ptr.add(i).read() });
                }
            } else {
                return ax_err!(BadAddress);
            }
        }
        debug!("read_guest_memory @{:?} content: {:x?}", gva, content);
        Ok(content)
    }

    pub fn decode_instruction(&self, rip: GuestVirtAddr, instr_len: usize) -> AxResult {
        use alloc::string::String;
        use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};

        let bytes = self.read_guest_memory(rip, instr_len)?;
        let mut decoder = Decoder::with_ip(
            64,
            bytes.as_slice(),
            rip.as_usize() as _,
            DecoderOptions::NONE,
        );
        let instr = decoder.decode();
        let mut output = String::new();
        let mut formattor = IntelFormatter::new();
        formattor.format(&instr, &mut output);

        debug!("Decoded instruction @Intel formatter: {}", output);
        Ok(())
    }
}

// Implementation of private methods
impl<H: AxVCpuHal> VmxVcpu<H> {
    fn setup_io_bitmap(&mut self) -> AxResult {
        // By default, I/O bitmap is set as `intercept_all`.
        // Todo: these should be combined with emulated pio device management,
        // in `modules/axvm/src/device/x86_64/mod.rs` somehow.
        let io_to_be_intercepted = [
            // // UART
            // // 0x3f8..0x3f8 + 8, // COM1
            // // We need to intercepted the access to COM2 ports.
            // // Because we want to reserve this port for host Linux.
            // 0x2f8..0x2f8 + 8, // COM2
            // // 0x3e8..0x3e8 + 8, // COM3
            // // 0x2e8..0x2e8 + 8, // COM4
            // // Virual PIC
            // 0x20..0x20 + 2, // PIC1
            // 0xa0..0xa0 + 2, // PIC2
            // // Debug Port
            // // 0x80..0x80 + 1,   // Debug Port
            // //
            // 0x92..0x92 + 1, // system_control_a
            // 0x61..0x61 + 1, // system_control_b
            // // RTC
            // 0x70..0x70 + 2, // CMOS
            // 0x40..0x40 + 4, // PIT
            // // 0xf0..0xf0 + 2,   // ports about fpu
            // // 0x3d4..0x3d4 + 2, // ports about vga
            // 0x87..0x87 + 1,   // port about dma
            // 0x60..0x60 + 1,   // ports about ps/2 controller
            // 0x64..0x64 + 1,   // ports about ps/2 controller
            // 0xcf8..0xcf8 + 8, // PCI

            // QEMU exit port
            QEMU_EXIT_PORT..QEMU_EXIT_PORT + 1, // QEMU exit port
        ];
        for port_range in io_to_be_intercepted {
            self.io_bitmap.set_intercept_of_range(
                port_range.start as _,
                port_range.count() as u32,
                true,
            );
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn setup_msr_bitmap(&mut self) -> AxResult {
        // Intercept IA32_APIC_BASE MSR accesses
        // let msr = x86::msr::IA32_APIC_BASE;
        // self.msr_bitmap.set_read_intercept(msr, true);
        // self.msr_bitmap.set_write_intercept(msr, true);

        // This is strange, guest Linux's access to `IA32_UMWAIT_CONTROL` will cause an exception.
        // But if we intercept it, it seems okay.
        const IA32_UMWAIT_CONTROL: u32 = 0xe1;
        self.msr_bitmap
            .set_write_intercept(IA32_UMWAIT_CONTROL, true);
        self.msr_bitmap
            .set_read_intercept(IA32_UMWAIT_CONTROL, true);

        // Intercept all x2APIC MSR accesses
        // for msr in 0x800..=0x83f {
        //     self.msr_bitmap.set_read_intercept(msr, true);
        //     self.msr_bitmap.set_write_intercept(msr, true);
        // }
        Ok(())
    }

    fn setup_vmcs(
        &mut self,
        ept_root: HostPhysAddr,
        entry: Option<GuestPhysAddr>,
        ctx: Option<LinuxContext>,
    ) -> AxResult {
        let mut is_guest = true;

        let paddr = self.vmcs.phys_addr().as_usize() as u64;
        unsafe {
            vmx::vmclear(paddr).map_err(as_axerr)?;
        }
        self.bind_to_current_processor()?;

        if let Some(ctx) = ctx {
            is_guest = false;
            self.setup_vmcs_guest_from_ctx(ctx)?;
        } else {
            self.setup_vmcs_guest(entry.ok_or_else(|| {
                error!("VmxVcpu::setup_vmcs: entry is None");
                ax_err_type!(InvalidInput)
            })?)?;
        }

        self.setup_vmcs_control(ept_root, is_guest)?;
        self.unbind_from_current_processor()?;
        Ok(())
    }

    fn setup_vmcs_host(&self) -> AxResult {
        VmcsHost64::IA32_PAT.write(Msr::IA32_PAT.read())?;
        VmcsHost64::IA32_EFER.write(Msr::IA32_EFER.read())?;

        VmcsHostNW::CR0.write(Cr0::read_raw() as _)?;
        VmcsHostNW::CR3.write(Cr3::read_raw().0.start_address().as_u64() as _)?;
        VmcsHostNW::CR4.write(Cr4::read_raw() as _)?;

        VmcsHost16::ES_SELECTOR.write(x86::segmentation::es().bits())?;
        VmcsHost16::CS_SELECTOR.write(x86::segmentation::cs().bits())?;
        VmcsHost16::SS_SELECTOR.write(x86::segmentation::ss().bits())?;
        VmcsHost16::DS_SELECTOR.write(x86::segmentation::ds().bits())?;
        VmcsHost16::FS_SELECTOR.write(x86::segmentation::fs().bits())?;
        VmcsHost16::GS_SELECTOR.write(x86::segmentation::gs().bits())?;
        VmcsHostNW::FS_BASE.write(Msr::IA32_FS_BASE.read() as _)?;
        VmcsHostNW::GS_BASE.write(Msr::IA32_GS_BASE.read() as _)?;

        let tr = unsafe { x86::task::tr() };
        let mut gdtp = DescriptorTablePointer::<u64>::default();
        let mut idtp = DescriptorTablePointer::<u64>::default();
        unsafe {
            dtables::sgdt(&mut gdtp);
            dtables::sidt(&mut idtp);
        }
        VmcsHost16::TR_SELECTOR.write(tr.bits())?;
        VmcsHostNW::TR_BASE.write(get_tr_base(tr, &gdtp) as _)?;
        VmcsHostNW::GDTR_BASE.write(gdtp.base as _)?;
        VmcsHostNW::IDTR_BASE.write(idtp.base as _)?;
        VmcsHostNW::RIP.write(Self::vmx_exit as usize)?;

        VmcsHostNW::IA32_SYSENTER_ESP.write(0)?;
        VmcsHostNW::IA32_SYSENTER_EIP.write(0)?;
        VmcsHost32::IA32_SYSENTER_CS.write(0)?;

        Ok(())
    }

    /// Indeed, this function can be combined with `setup_vmcs_guest`,
    /// to avoid complexity and minimize the modification,
    /// we just keep them separated.
    fn setup_vmcs_guest_from_ctx(&mut self, host_ctx: LinuxContext) -> AxResult {
        let linux = host_ctx;

        self.set_cr(0, linux.cr0.bits());
        self.set_cr(4, linux.cr4.bits());
        self.set_cr(3, linux.cr3);

        macro_rules! set_guest_segment {
            ($seg: expr, $reg: ident) => {{
                use VmcsGuest16::*;
                use VmcsGuest32::*;
                use VmcsGuestNW::*;
                concat_idents!($reg, _SELECTOR).write($seg.selector.bits())?;
                concat_idents!($reg, _BASE).write($seg.base as _)?;
                concat_idents!($reg, _LIMIT).write($seg.limit)?;
                concat_idents!($reg, _ACCESS_RIGHTS).write($seg.access_rights.bits())?;
            }};
        }

        set_guest_segment!(linux.es, ES);
        set_guest_segment!(linux.cs, CS);
        set_guest_segment!(linux.ss, SS);
        set_guest_segment!(linux.ds, DS);
        set_guest_segment!(linux.fs, FS);
        set_guest_segment!(linux.gs, GS);
        set_guest_segment!(linux.tss, TR);
        set_guest_segment!(Segment::invalid(), LDTR);

        VmcsGuestNW::GDTR_BASE.write(linux.gdt.base.as_u64() as _)?;
        VmcsGuest32::GDTR_LIMIT.write(linux.gdt.limit as _)?;
        VmcsGuestNW::IDTR_BASE.write(linux.idt.base.as_u64() as _)?;
        VmcsGuest32::IDTR_LIMIT.write(linux.idt.limit as _)?;

        VmcsGuestNW::RSP.write(linux.rsp as _)?;
        VmcsGuestNW::RIP.write(linux.rip as _)?;
        VmcsGuestNW::RFLAGS.write(0x2)?;

        VmcsGuest32::IA32_SYSENTER_CS.write(linux.ia32_sysenter_cs as _)?;
        VmcsGuestNW::IA32_SYSENTER_ESP.write(linux.ia32_sysenter_esp as _)?;
        VmcsGuestNW::IA32_SYSENTER_EIP.write(linux.ia32_sysenter_eip as _)?;

        VmcsGuestNW::DR7.write(0x400)?;
        VmcsGuest64::IA32_DEBUGCTL.write(0)?;

        VmcsGuest32::ACTIVITY_STATE.write(0)?;
        VmcsGuest32::INTERRUPTIBILITY_STATE.write(0)?;
        VmcsGuestNW::PENDING_DBG_EXCEPTIONS.write(0)?;

        VmcsGuest64::LINK_PTR.write(u64::MAX)?;
        VmcsGuest32::VMX_PREEMPTION_TIMER_VALUE.write(0)?;

        VmcsGuest64::IA32_PAT.write(linux.pat)?;
        VmcsGuest64::IA32_EFER.write(linux.efer.bits())?;

        Ok(())
    }

    fn setup_vmcs_guest(&mut self, entry: GuestPhysAddr) -> AxResult {
        let cr0_val: Cr0Flags =
            Cr0Flags::NOT_WRITE_THROUGH | Cr0Flags::CACHE_DISABLE | Cr0Flags::EXTENSION_TYPE;
        self.set_cr(0, cr0_val.bits());
        self.set_cr(4, 0);

        macro_rules! set_guest_segment {
            ($seg: ident, $access_rights: expr) => {{
                use VmcsGuest16::*;
                use VmcsGuest32::*;
                use VmcsGuestNW::*;
                concat_idents!($seg, _SELECTOR).write(0)?;
                concat_idents!($seg, _BASE).write(0)?;
                concat_idents!($seg, _LIMIT).write(0xffff)?;
                concat_idents!($seg, _ACCESS_RIGHTS).write($access_rights)?;
            }};
        }

        set_guest_segment!(ES, 0x93); // 16-bit, present, data, read/write, accessed
        set_guest_segment!(CS, 0x9b); // 16-bit, present, code, exec/read, accessed
        set_guest_segment!(SS, 0x93);
        set_guest_segment!(DS, 0x93);
        set_guest_segment!(FS, 0x93);
        set_guest_segment!(GS, 0x93);
        set_guest_segment!(TR, 0x8b); // present, system, 32-bit TSS busy
        set_guest_segment!(LDTR, 0x82); // present, system, LDT

        VmcsGuestNW::GDTR_BASE.write(0)?;
        VmcsGuest32::GDTR_LIMIT.write(0xffff)?;
        VmcsGuestNW::IDTR_BASE.write(0)?;
        VmcsGuest32::IDTR_LIMIT.write(0xffff)?;

        VmcsGuestNW::CR3.write(0)?;
        VmcsGuestNW::DR7.write(0x400)?;
        VmcsGuestNW::RSP.write(0)?;
        VmcsGuestNW::RIP.write(entry.as_usize())?;
        VmcsGuestNW::RFLAGS.write(0x2)?;
        VmcsGuestNW::PENDING_DBG_EXCEPTIONS.write(0)?;
        VmcsGuestNW::IA32_SYSENTER_ESP.write(0)?;
        VmcsGuestNW::IA32_SYSENTER_EIP.write(0)?;
        VmcsGuest32::IA32_SYSENTER_CS.write(0)?;

        VmcsGuest32::INTERRUPTIBILITY_STATE.write(0)?;
        VmcsGuest32::ACTIVITY_STATE.write(0)?;

        VmcsGuest32::VMX_PREEMPTION_TIMER_VALUE.write(VMX_PREEMPTION_TIMER_SET_VALUE)?;

        VmcsGuest64::LINK_PTR.write(u64::MAX)?; // SDM Vol. 3C, Section 24.4.2
        VmcsGuest64::IA32_DEBUGCTL.write(0)?;
        VmcsGuest64::IA32_PAT.write(Msr::IA32_PAT.read())?;
        VmcsGuest64::IA32_EFER.write(0)?;
        Ok(())
    }

    fn setup_vmcs_control(&mut self, ept_root: HostPhysAddr, is_guest: bool) -> AxResult {
        // Intercept NMI and external interrupts.
        use super::vmcs::controls::*;
        use PinbasedControls as PinCtrl;
        let raw_cpuid = CpuId::new();

        vmcs::set_control(
            VmcsControl32::PINBASED_EXEC_CONTROLS,
            Msr::IA32_VMX_TRUE_PINBASED_CTLS,
            Msr::IA32_VMX_PINBASED_CTLS.read() as u32,
            // (PinCtrl::NMI_EXITING | PinCtrl::EXTERNAL_INTERRUPT_EXITING).bits(),
            // (PinCtrl::NMI_EXITING | PinCtrl::VMX_PREEMPTION_TIMER).bits(),
            PinCtrl::NMI_EXITING.bits(),
            0,
        )?;

        // Intercept all I/O instructions, use MSR bitmaps, activate secondary controls,
        // disable CR3 load/store interception.
        use PrimaryControls as CpuCtrl;
        vmcs::set_control(
            VmcsControl32::PRIMARY_PROCBASED_EXEC_CONTROLS,
            Msr::IA32_VMX_TRUE_PROCBASED_CTLS,
            Msr::IA32_VMX_PROCBASED_CTLS.read() as u32,
            (CpuCtrl::USE_IO_BITMAPS | CpuCtrl::USE_MSR_BITMAPS | CpuCtrl::SECONDARY_CONTROLS)
                .bits(),
            (CpuCtrl::CR3_LOAD_EXITING
                | CpuCtrl::CR3_STORE_EXITING
                | CpuCtrl::CR8_LOAD_EXITING
                | CpuCtrl::CR8_STORE_EXITING)
                .bits(),
        )?;

        // Enable EPT, RDTSCP, INVPCID, and unrestricted guest.
        use SecondaryControls as CpuCtrl2;
        let mut val =
            CpuCtrl2::ENABLE_EPT | CpuCtrl2::UNRESTRICTED_GUEST | CpuCtrl2::ENABLE_VM_FUNCTIONS;

        if let Some(features) = raw_cpuid.get_extended_processor_and_feature_identifiers() {
            if features.has_rdtscp() {
                val |= CpuCtrl2::ENABLE_RDTSCP;
            }
        }

        if let Some(features) = raw_cpuid.get_extended_feature_info() {
            if features.has_invpcid() {
                val |= CpuCtrl2::ENABLE_INVPCID;
            }
            if features.has_waitpkg() {
                val |= CpuCtrl2::ENABLE_USER_WAIT_PAUSE;
            }
        }

        if let Some(features) = raw_cpuid.get_extended_state_info() {
            if features.has_xsaves_xrstors() {
                val |= CpuCtrl2::ENABLE_XSAVES_XRSTORS;
            }
        }

        vmcs::set_control(
            VmcsControl32::SECONDARY_PROCBASED_EXEC_CONTROLS,
            Msr::IA32_VMX_PROCBASED_CTLS2,
            Msr::IA32_VMX_PROCBASED_CTLS2.read() as u32,
            val.bits(),
            0,
        )?;

        // Switch to 64-bit host, acknowledge interrupt info, switch IA32_PAT/IA32_EFER on VM exit.
        use ExitControls as ExitCtrl;
        vmcs::set_control(
            VmcsControl32::VMEXIT_CONTROLS,
            Msr::IA32_VMX_TRUE_EXIT_CTLS,
            Msr::IA32_VMX_EXIT_CTLS.read() as u32,
            (ExitCtrl::HOST_ADDRESS_SPACE_SIZE
                | ExitCtrl::ACK_INTERRUPT_ON_EXIT
                | ExitCtrl::SAVE_IA32_PAT
                | ExitCtrl::LOAD_IA32_PAT
                | ExitCtrl::SAVE_IA32_EFER
                | ExitCtrl::LOAD_IA32_EFER)
                .bits(),
            0,
        )?;

        let mut val = EntryCtrl::LOAD_IA32_PAT | EntryCtrl::LOAD_IA32_EFER;

        if !is_guest {
            // IA-32e mode guest
            // On processors that support Intel 64 architecture, this control determines whether the logical processor is in IA-32e mode after VM entry.
            // Its value is loaded into IA32_EFER.LMA as part of VM entry.
            val |= EntryCtrl::IA32E_MODE_GUEST;
        }

        // Load guest IA32_PAT/IA32_EFER on VM entry.
        use EntryControls as EntryCtrl;
        vmcs::set_control(
            VmcsControl32::VMENTRY_CONTROLS,
            Msr::IA32_VMX_TRUE_ENTRY_CTLS,
            Msr::IA32_VMX_ENTRY_CTLS.read() as u32,
            val.bits(),
            0,
        )?;

        vmcs::set_ept_pointer(ept_root)?;

        // No MSR switches if hypervisor doesn't use and there is only one vCPU.
        VmcsControl32::VMEXIT_MSR_STORE_COUNT.write(0)?;
        VmcsControl32::VMEXIT_MSR_LOAD_COUNT.write(0)?;
        VmcsControl32::VMENTRY_MSR_LOAD_COUNT.write(0)?;

        // TODO: figure out why we mask it.
        VmcsControlNW::CR4_GUEST_HOST_MASK.write(0)?;
        VmcsControl32::CR3_TARGET_COUNT.write(0)?;

        // 25.6.14 VM-Function Controls
        // Table 25-10. Definitions of VM-Function Controls
        // Bit 0: EPTP switching
        VmcsControl64::VM_FUNCTION_CONTROLS.write(0b1)?;

        VmcsControl64::EPTP_LIST_ADDR.write(self.eptp_list.phys_addr().as_usize() as _)?;

        // Pass-through exceptions (except #UD(6)), don't use I/O bitmap, set MSR bitmaps.
        let exception_bitmap: u32 = 1 << 6;

        self.setup_io_bitmap()?;

        VmcsControl32::EXCEPTION_BITMAP.write(exception_bitmap)?;
        VmcsControl64::IO_BITMAP_A_ADDR.write(self.io_bitmap.phys_addr().0.as_usize() as _)?;
        VmcsControl64::IO_BITMAP_B_ADDR.write(self.io_bitmap.phys_addr().1.as_usize() as _)?;
        VmcsControl64::MSR_BITMAPS_ADDR.write(self.msr_bitmap.phys_addr().as_usize() as _)?;
        Ok(())
    }

    fn load_vmcs_guest(&self, linux: &mut LinuxContext) -> AxResult {
        linux.rip = VmcsGuestNW::RIP.read()? as _;
        linux.rsp = VmcsGuestNW::RSP.read()? as _;
        linux.cr0 = Cr0Flags::from_bits_truncate(VmcsGuestNW::CR0.read()? as _);
        linux.cr3 = VmcsGuestNW::CR3.read()? as _;
        linux.cr4 = Cr4Flags::from_bits_truncate(VmcsGuestNW::CR4.read()? as _);

        linux.es.selector = SegmentSelector::from_raw(VmcsGuest16::ES_SELECTOR.read()?);

        linux.cs.selector = SegmentSelector::from_raw(VmcsGuest16::CS_SELECTOR.read()?);
        // CS:
        // If the Type is 9 or 11 (non-conforming code segment), the DPL must equal the DPL in the access-rights field for SS.
        linux.cs.access_rights =
            SegmentAccessRights::from_bits_truncate(VmcsGuest32::CS_ACCESS_RIGHTS.read()?);
        linux.ss.selector = SegmentSelector::from_raw(VmcsGuest16::SS_SELECTOR.read()?);
        linux.ss.access_rights =
            SegmentAccessRights::from_bits_truncate(VmcsGuest32::SS_ACCESS_RIGHTS.read()?);

        linux.ds.selector = SegmentSelector::from_raw(VmcsGuest16::DS_SELECTOR.read()?);
        linux.fs.selector = SegmentSelector::from_raw(VmcsGuest16::FS_SELECTOR.read()?);
        linux.fs.base = VmcsGuestNW::FS_BASE.read()? as _;
        linux.gs.selector = SegmentSelector::from_raw(VmcsGuest16::GS_SELECTOR.read()?);
        linux.gs.base = VmcsGuestNW::GS_BASE.read()? as _;
        linux.tss.selector = SegmentSelector::from_raw(VmcsGuest16::TR_SELECTOR.read()?);

        linux.gdt.base = VirtAddr::new(VmcsGuestNW::GDTR_BASE.read()? as _);
        linux.gdt.limit = VmcsGuest32::GDTR_LIMIT.read()? as _;
        linux.idt.base = VirtAddr::new(VmcsGuestNW::IDTR_BASE.read()? as _);
        linux.idt.limit = VmcsGuest32::IDTR_LIMIT.read()? as _;

        linux.ia32_sysenter_cs = VmcsGuest32::IA32_SYSENTER_CS.read()? as _; // 0x174
        linux.ia32_sysenter_esp = VmcsGuestNW::IA32_SYSENTER_ESP.read()? as _; // 0x178
        linux.ia32_sysenter_eip = VmcsGuestNW::IA32_SYSENTER_EIP.read()? as _; // 0x17a

        linux.load_guest_regs(self.regs());
        Ok(())
    }

    fn get_paging_level(&self) -> usize {
        let mut level: u32 = 0; // non-paging
        let cr0 = VmcsGuestNW::CR0.read().unwrap();
        let cr4 = VmcsGuestNW::CR4.read().unwrap();
        let efer = VmcsGuest64::IA32_EFER.read().unwrap();
        // paging is enabled
        if cr0 & Cr0Flags::PAGING.bits() as usize != 0 {
            if cr4 & Cr4Flags::PHYSICAL_ADDRESS_EXTENSION.bits() as usize != 0 {
                // is long mode
                if efer & EferFlags::LONG_MODE_ACTIVE.bits() != 0 {
                    level = 4;
                } else {
                    level = 3;
                }
            } else {
                level = 2;
            }
        }
        level as usize
    }

    /// Translate guest virtual addr to linear addr
    fn gva_to_linear_addr(&self, vaddr: GuestVirtAddr) -> GuestVirtAddr {
        let cpu_mode = self.get_cpu_mode();
        let seg_base = if cpu_mode == VmCpuMode::Mode64 {
            0
        } else {
            VmcsGuestNW::CS_BASE.read().unwrap()
        };
        vaddr + seg_base
    }

    pub fn guest_page_table_query(
        &self,
        gva: GuestVirtAddr,
    ) -> PagingResult<(GuestPhysAddr, MappingFlags, PageSize)> {
        let addr = self.gva_to_linear_addr(gva);

        // debug!("guest_page_table_query: gva {:?} linear {:?}", gva, addr);

        let guest_ptw_info = self.get_pagetable_walk_info();
        let guest_page_table: GuestPageTable64<X64PTE, H::PagingHandler, H::EPTTranslator> =
            GuestPageTable64::construct(&guest_ptw_info);

        guest_page_table.query(addr)
    }
}

// Implementaton for type1.5 hypervisor
// #[cfg(feature = "type1_5")]
impl<H: AxVCpuHal> VmxVcpu<H> {
    fn set_cr(&mut self, cr_idx: usize, val: u64) {
        (|| -> AxResult {
            // debug!("set guest CR{} to val {:#x}", cr_idx, val);
            match cr_idx {
                0 => {
                    // Retrieve/validate restrictions on CR0
                    //
                    // In addition to what the VMX MSRs tell us, make sure that
                    // - NW and CD are kept off as they are not updated on VM exit and we
                    //   don't want them enabled for performance reasons while in root mode
                    // - PE and PG can be freely chosen (by the guest) because we demand
                    //   unrestricted guest mode support anyway
                    // - ET is ignored
                    let must0 = Msr::IA32_VMX_CR0_FIXED1.read()
                        & !(Cr0Flags::NOT_WRITE_THROUGH | Cr0Flags::CACHE_DISABLE).bits();
                    let must1 = Msr::IA32_VMX_CR0_FIXED0.read()
                        & !(Cr0Flags::PAGING | Cr0Flags::PROTECTED_MODE_ENABLE).bits();
                    VmcsGuestNW::CR0.write(((val & must0) | must1) as _)?;
                    VmcsControlNW::CR0_READ_SHADOW.write(val as _)?;
                    VmcsControlNW::CR0_GUEST_HOST_MASK.write((must1 | !must0) as _)?;
                }
                3 => VmcsGuestNW::CR3.write(val as _)?,
                4 => {
                    // Retrieve/validate restrictions on CR4
                    let must0 = Msr::IA32_VMX_CR4_FIXED1.read();
                    let must1 = Msr::IA32_VMX_CR4_FIXED0.read();
                    let val = val | Cr4Flags::VIRTUAL_MACHINE_EXTENSIONS.bits();
                    VmcsGuestNW::CR4.write(((val & must0) | must1) as _)?;
                    VmcsControlNW::CR4_READ_SHADOW.write(val as _)?;
                    VmcsControlNW::CR4_GUEST_HOST_MASK.write((must1 | !must0) as _)?;
                }
                _ => unreachable!(),
            };
            Ok(())
        })()
        .expect("Failed to write guest control register")
    }

    #[allow(dead_code)]
    fn cr(&self, cr_idx: usize) -> usize {
        (|| -> AxResult<usize> {
            Ok(match cr_idx {
                0 => VmcsGuestNW::CR0.read()?,
                3 => VmcsGuestNW::CR3.read()?,
                4 => {
                    let host_mask = VmcsControlNW::CR4_GUEST_HOST_MASK.read()?;
                    (VmcsControlNW::CR4_READ_SHADOW.read()? & host_mask)
                        | (VmcsGuestNW::CR4.read()? & !host_mask)
                }
                _ => unreachable!(),
            })
        })()
        .expect("Failed to read guest control register")
    }
}

/// Get ready then vmlaunch or vmresume.
macro_rules! vmx_entry_with {
    ($instr:literal) => {
        unsafe {
            naked_asm!(
                save_regs_to_stack!(),                  // save host status
                "mov    [rdi + {host_stack_size}], rsp", // save current RSP to Vcpu::host_stack_top
                "mov    rsp, rdi",                      // set RSP to guest regs area
                restore_regs_from_stack!(),             // restore guest status
                $instr,                                 // let's go!
                "jmp    {failed}",
                host_stack_size = const size_of::<GeneralRegisters>(),
                failed = sym Self::vmx_entry_failed,
                // options(noreturn),
            )
        }
    }
}

impl<H: AxVCpuHal> VmxVcpu<H> {
    #[naked]
    /// Enter guest with vmlaunch.
    ///
    /// `#[naked]` is essential here, without it the rust compiler will think `&mut self` is not used and won't give us correct %rdi.
    ///
    /// This function itself never returns, but [`Self::vmx_exit`] will do the return for this.
    ///
    /// The return value is a dummy value.
    unsafe extern "C" fn vmx_launch(&mut self) -> usize {
        vmx_entry_with!("vmlaunch")
    }

    #[naked]
    /// Enter guest with vmresume.
    ///
    /// See [`Self::vmx_launch`] for detail.
    unsafe extern "C" fn vmx_resume(&mut self) -> usize {
        vmx_entry_with!("vmresume")
    }

    #[naked]
    /// Return after vm-exit.
    ///
    /// The return value is a dummy value.
    unsafe extern "C" fn vmx_exit(&mut self) -> usize {
        unsafe {
            naked_asm!(
                save_regs_to_stack!(),                  // save guest status
                "mov    rsp, [rsp + {host_stack_top}]", // set RSP to Vcpu::host_stack_top
                restore_regs_from_stack!(),             // restore host status
                "ret",
                host_stack_top = const size_of::<GeneralRegisters>(),
            );
        }
    }

    fn vmx_entry_failed() -> ! {
        panic!("{}", vmcs::instruction_error().as_str())
    }

    /// Whether the guest interrupts are blocked. (SDM Vol. 3C, Section 24.4.2, Table 24-3)
    fn allow_interrupt(&self) -> bool {
        let rflags = VmcsGuestNW::RFLAGS.read().unwrap();
        let block_state = VmcsGuest32::INTERRUPTIBILITY_STATE.read().unwrap();
        rflags as u64 & x86_64::registers::rflags::RFlags::INTERRUPT_FLAG.bits() != 0
            && block_state == 0
    }

    /// Try to inject a pending event before next VM entry.
    fn inject_pending_events(&mut self) -> AxResult {
        if let Some(event) = self.pending_events.front() {
            // debug!(
            //     "inject_pending_events vector {:#x} allow_int {}",
            //     event.0,
            //     self.allow_interrupt()
            // );
            if event.0 < 32 || self.allow_interrupt() {
                // if it's an exception, or an interrupt that is not blocked, inject it directly.
                vmcs::inject_event(event.0, event.1)?;
                self.pending_events.pop_front();
            } else {
                // interrupts are blocked, enable interrupt-window exiting.
                self.set_interrupt_window(true)?;
            }
        }
        Ok(())
    }

    /// Handle vm-exits than can and should be handled by [`VmxVcpu`] itself.
    ///
    /// Return the result or None if the vm-exit was not handled.
    fn builtin_vmexit_handler(&mut self, exit_info: &VmxExitInfo) -> Option<AxResult> {
        // Following vm-exits are handled here:
        // - interrupt window: turn off interrupt window;
        // - xsetbv: set guest xcr;
        // - cr access: just panic;
        match exit_info.exit_reason {
            VmxExitReason::INTERRUPT_WINDOW => Some(self.set_interrupt_window(false)),
            VmxExitReason::PREEMPTION_TIMER => Some(self.handle_vmx_preemption_timer()),
            VmxExitReason::XSETBV => Some(self.handle_xsetbv()),
            VmxExitReason::CR_ACCESS => Some(self.handle_cr()),
            VmxExitReason::CPUID => Some(self.handle_cpuid()),
            VmxExitReason::EXCEPTION_NMI => Some(self.handle_exception_nmi(exit_info)),
            _ => None,
        }
    }

    fn handle_vmx_preemption_timer(&mut self) -> AxResult {
        /*
        The VMX-preemption timer counts down at rate proportional to that of the timestamp counter (TSC).
        Specifically, the timer counts down by 1 every time bit X in the TSC changes due to a TSC increment.
        The value of X is in the range 0–31 and can be determined by consulting the VMX capability MSR IA32_VMX_MISC (see Appendix A.6).
         */
        VmcsGuest32::VMX_PREEMPTION_TIMER_VALUE.write(VMX_PREEMPTION_TIMER_SET_VALUE)?;
        Ok(())
    }

    fn handle_exception_nmi(&mut self, exit_info: &VmxExitInfo) -> AxResult {
        let intr_info = interrupt_exit_info()?;
        info!(
            "VM exit: Exception or NMI @ RIP({:#x}, {}): {:#x?}",
            exit_info.guest_rip, exit_info.exit_instruction_length, intr_info
        );

        self.decode_instruction(
            GuestVirtAddr::from_usize(exit_info.guest_rip),
            exit_info.exit_instruction_length as _,
        )?;

        const NON_MASKABLE_INTERRUPT: u8 = 2;

        match intr_info.vector {
            // ExceptionType::NonMaskableInterrupt
            NON_MASKABLE_INTERRUPT => unsafe {
                core::arch::asm!("int {}", const NON_MASKABLE_INTERRUPT)
            },
            v => panic!("Unhandled Guest Exception: #{:#x}", v),
        }
        Ok(())
    }

    #[allow(clippy::single_match)]
    fn handle_cr(&mut self) -> AxResult {
        const VM_EXIT_INSTR_LEN_MV_TO_CR: u8 = 3;

        let cr_access_info = vmcs::cr_access_info()?;

        let reg = cr_access_info.gpr;
        let cr = cr_access_info.cr_number;

        match cr_access_info.access_type {
            /* move to cr */
            0 => {
                let val = if reg == 4 {
                    self.stack_pointer() as u64
                } else {
                    self.guest_regs.get_reg_of_index(reg)
                };
                if cr == 0 || cr == 4 {
                    self.advance_rip(VM_EXIT_INSTR_LEN_MV_TO_CR)?;
                    /* TODO: check for #GP reasons */
                    self.set_cr(cr as usize, val);

                    if cr == 0 && Cr0Flags::from_bits_truncate(val).contains(Cr0Flags::PAGING) {
                        vmcs::update_efer()?;
                    }
                    return Ok(());
                }
            }
            _ => {}
        };

        panic!(
            "Guest's access to cr not allowed: {:#x?}, {:#x?}",
            self, cr_access_info
        );
    }

    fn handle_cpuid(&mut self) -> AxResult {
        use raw_cpuid::{CpuIdResult, cpuid};

        const VM_EXIT_INSTR_LEN_CPUID: u8 = 2;
        const LEAF_FEATURE_INFO: u32 = 0x1;
        const LEAF_STRUCTURED_EXTENDED_FEATURE_FLAGS_ENUMERATION: u32 = 0x7;
        const LEAF_PROCESSOR_EXTENDED_STATE_ENUMERATION: u32 = 0xd;
        const EAX_FREQUENCY_INFO: u32 = 0x16;
        const LEAF_HYPERVISOR_INFO: u32 = 0x4000_0000;
        const LEAF_HYPERVISOR_FEATURE: u32 = 0x4000_0001;
        const VENDOR_STR: &[u8; 12] = b"RVMRVMRVMRVM";
        let vendor_regs = unsafe { &*(VENDOR_STR.as_ptr() as *const [u32; 3]) };

        let regs_clone = self.regs_mut().clone();
        let function = regs_clone.rax as u32;
        let res = match function {
            LEAF_FEATURE_INFO => {
                const FEATURE_VMX: u32 = 1 << 5;
                const FEATURE_HYPERVISOR: u32 = 1 << 31;
                const FEATURE_MCE: u32 = 1 << 7;
                let mut res = cpuid!(regs_clone.rax, regs_clone.rcx);
                res.ecx &= !FEATURE_VMX;
                res.ecx |= FEATURE_HYPERVISOR;
                res.eax &= !FEATURE_MCE;
                res
            }
            // See SDM Table 3-8. Information Returned by CPUID Instruction (Contd.)
            LEAF_STRUCTURED_EXTENDED_FEATURE_FLAGS_ENUMERATION => {
                let mut res = cpuid!(regs_clone.rax, regs_clone.rcx);
                if regs_clone.rcx == 0 {
                    // Bit 05: WAITPKG.
                    res.ecx.set_bit(5, false); // clear waitpkg
                    // Bit 16: LA57. Supports 57-bit linear addresses and five-level paging if 1.
                    res.ecx.set_bit(16, false); // clear LA57
                }

                res
            }
            LEAF_PROCESSOR_EXTENDED_STATE_ENUMERATION => {
                self.load_guest_xstate();
                let res = cpuid!(regs_clone.rax, regs_clone.rcx);
                self.load_host_xstate();
                res
            }
            LEAF_HYPERVISOR_INFO => CpuIdResult {
                eax: LEAF_HYPERVISOR_FEATURE,
                ebx: vendor_regs[0],
                ecx: vendor_regs[1],
                edx: vendor_regs[2],
            },
            LEAF_HYPERVISOR_FEATURE => CpuIdResult {
                eax: 0,
                ebx: 0,
                ecx: 0,
                edx: 0,
            },
            EAX_FREQUENCY_INFO => {
                /// Timer interrupt frequencyin Hz.
                /// Todo: this should be the same as `axconfig::TIMER_FREQUENCY` defined in ArceOS's config file.
                const TIMER_FREQUENCY_MHZ: u32 = 3_000;
                let mut res = cpuid!(regs_clone.rax, regs_clone.rcx);
                if res.eax == 0 {
                    warn!(
                        "handle_cpuid: Failed to get TSC frequency by CPUID, default to {} MHz",
                        TIMER_FREQUENCY_MHZ
                    );
                    res.eax = TIMER_FREQUENCY_MHZ;
                }
                res
            }
            _ => cpuid!(regs_clone.rax, regs_clone.rcx),
        };

        trace!(
            "VM exit: CPUID({:#x}, {:#x}): {:?}",
            regs_clone.rax, regs_clone.rcx, res
        );

        let regs = self.regs_mut();
        regs.rax = res.eax as _;
        regs.rbx = res.ebx as _;
        regs.rcx = res.ecx as _;
        regs.rdx = res.edx as _;
        self.advance_rip(VM_EXIT_INSTR_LEN_CPUID)?;

        Ok(())
    }

    fn handle_xsetbv(&mut self) -> AxResult {
        const XCR_XCR0: u64 = 0;
        const VM_EXIT_INSTR_LEN_XSETBV: u8 = 3;

        let index = self.guest_regs.rcx.get_bits(0..32);
        let value = self.guest_regs.rdx.get_bits(0..32) << 32 | self.guest_regs.rax.get_bits(0..32);

        // TODO: get host-supported xcr0 mask by cpuid and reject any guest-xsetbv violating that
        if index == XCR_XCR0 {
            Xcr0::from_bits(value)
                .and_then(|x| {
                    if !x.contains(Xcr0::XCR0_FPU_MMX_STATE) {
                        return None;
                    }

                    if x.contains(Xcr0::XCR0_AVX_STATE) && !x.contains(Xcr0::XCR0_SSE_STATE) {
                        return None;
                    }

                    if x.contains(Xcr0::XCR0_BNDCSR_STATE) ^ x.contains(Xcr0::XCR0_BNDREG_STATE) {
                        return None;
                    }

                    if x.contains(Xcr0::XCR0_OPMASK_STATE)
                        || x.contains(Xcr0::XCR0_ZMM_HI256_STATE)
                        || x.contains(Xcr0::XCR0_HI16_ZMM_STATE)
                        || !x.contains(Xcr0::XCR0_AVX_STATE)
                        || !x.contains(Xcr0::XCR0_OPMASK_STATE)
                        || !x.contains(Xcr0::XCR0_ZMM_HI256_STATE)
                        || !x.contains(Xcr0::XCR0_HI16_ZMM_STATE)
                    {
                        return None;
                    }

                    Some(x)
                })
                .ok_or(ax_err_type!(InvalidInput))
                .and_then(|x| {
                    self.xstate.guest_xcr0 = x.bits();
                    self.advance_rip(VM_EXIT_INSTR_LEN_XSETBV)
                })
        } else {
            // xcr0 only
            ax_err!(Unsupported, "only xcr0 is supported")
        }
    }

    /// Save the current host state to the vcpu,
    /// restore the guest state from the vcpu into registers.
    ///
    /// This function is generally called before VM-entry.
    fn load_guest_xstate(&mut self) {
        // FIXME: Linux will throw a UD exception if we save/restore xstate.
        // self.xstate.switch_to_guest();
    }

    /// Save the current guest state to the vcpu,
    /// restore the host state from the vcpu into registers.
    ///
    /// This function is generally called after VM-exit.
    fn load_host_xstate(&mut self) {
        // self.xstate.switch_to_host();
    }
}

impl<H: AxVCpuHal> Drop for VmxVcpu<H> {
    fn drop(&mut self) {
        unsafe { vmx::vmclear(self.vmcs.phys_addr().as_usize() as u64).unwrap() };
        info!("[HV] dropped VmxVcpu(vmcs: {:#x})", self.vmcs.phys_addr());
    }
}

fn get_tr_base(tr: SegmentSelector, gdt: &DescriptorTablePointer<u64>) -> u64 {
    let index = tr.index() as usize;
    let table_len = (gdt.limit as usize + 1) / core::mem::size_of::<u64>();
    let table = unsafe { core::slice::from_raw_parts(gdt.base, table_len) };
    let entry = table[index];
    if entry & (1 << 47) != 0 {
        // present
        let base_low = entry.get_bits(16..40) | entry.get_bits(56..64) << 24;
        let base_high = table[index + 1] & 0xffff_ffff;
        base_low | base_high << 32
    } else {
        // no present
        0
    }
}

impl<H: AxVCpuHal> Debug for VmxVcpu<H> {
    fn fmt(&self, f: &mut Formatter) -> Result {
        (|| -> AxResult<Result> {
            let cs_selector = SegmentSelector::from_raw(VmcsGuest16::CS_SELECTOR.read()?);
            let cs_access_rights_raw = VmcsGuest32::CS_ACCESS_RIGHTS.read()?;
            let cs_access_rights = SegmentAccessRights::from_bits_truncate(cs_access_rights_raw);
            let ss_selector = SegmentSelector::from_raw(VmcsGuest16::SS_SELECTOR.read()?);
            let ss_access_rights_raw = VmcsGuest32::SS_ACCESS_RIGHTS.read()?;
            let ss_access_rights = SegmentAccessRights::from_bits_truncate(ss_access_rights_raw);
            let ds_selector = SegmentSelector::from_raw(VmcsGuest16::DS_SELECTOR.read()?);
            let ds_access_rights =
                SegmentAccessRights::from_bits_truncate(VmcsGuest32::DS_ACCESS_RIGHTS.read()?);
            let fs_selector = SegmentSelector::from_raw(VmcsGuest16::FS_SELECTOR.read()?);
            let fs_access_rights =
                SegmentAccessRights::from_bits_truncate(VmcsGuest32::FS_ACCESS_RIGHTS.read()?);
            let gs_selector = SegmentSelector::from_raw(VmcsGuest16::GS_SELECTOR.read()?);
            let gs_access_rights =
                SegmentAccessRights::from_bits_truncate(VmcsGuest32::GS_ACCESS_RIGHTS.read()?);
            let tr_selector = SegmentSelector::from_raw(VmcsGuest16::TR_SELECTOR.read()?);
            let tr_access_rights =
                SegmentAccessRights::from_bits_truncate(VmcsGuest32::TR_ACCESS_RIGHTS.read()?);
            let gdt_base = VirtAddr::new(VmcsGuestNW::GDTR_BASE.read()? as _);
            let gdt_limit = VmcsGuest32::GDTR_LIMIT.read()?;
            let idt_base = VirtAddr::new(VmcsGuestNW::IDTR_BASE.read()? as _);
            let idt_limit = VmcsGuest32::IDTR_LIMIT.read()?;

            let ia32_sysenter_cs = VmcsGuest32::IA32_SYSENTER_CS.read()?;
            let ia32_sysenter_esp = VmcsGuestNW::IA32_SYSENTER_ESP.read()?;
            let ia32_sysenter_eip = VmcsGuestNW::IA32_SYSENTER_EIP.read()?;

            Ok(f.debug_struct("VmxVcpu")
                .field("guest_regs", &self.guest_regs)
                .field("rip", &VmcsGuestNW::RIP.read()?)
                .field("rsp", &VmcsGuestNW::RSP.read()?)
                .field("rflags", &VmcsGuestNW::RFLAGS.read()?)
                .field("cr0", &VmcsGuestNW::CR0.read()?)
                .field("cr3", &VmcsGuestNW::CR3.read()?)
                .field("cr4", &VmcsGuestNW::CR4.read()?)
                .field("cs_base", &VmcsGuestNW::CS_BASE.read()?)
                .field("cs_selector", &cs_selector)
                .field("cs_access_rights", &cs_access_rights)
                .field("cs_access_rights_raw", &cs_access_rights_raw)
                .field("ss_base", &VmcsGuestNW::SS_BASE.read()?)
                .field("ss_selector", &ss_selector)
                .field("ss_access_rights_raw", &ss_access_rights_raw)
                .field("ss_access_rights", &ss_access_rights)
                .field("ds_base", &VmcsGuestNW::DS_BASE.read()?)
                .field("ds_selector", &ds_selector)
                .field("ds_access_rights", &ds_access_rights)
                .field("fs_base", &VmcsGuestNW::FS_BASE.read()?)
                .field("fs_selector", &fs_selector)
                .field("fs_access_rights", &fs_access_rights)
                .field("gs_base", &VmcsGuestNW::GS_BASE.read()?)
                .field("gs_selector", &gs_selector)
                .field("gs_access_rights", &gs_access_rights)
                .field("tr_selector", &tr_selector)
                .field("tr_access_rights", &tr_access_rights)
                .field("gdt_base", &gdt_base)
                .field("gdt_limit", &gdt_limit)
                .field("idt_base", &idt_base)
                .field("idt_limit", &idt_limit)
                .field("ia32_sysenter_cs", &ia32_sysenter_cs)
                .field("ia32_sysenter_esp", &ia32_sysenter_esp)
                .field("ia32_sysenter_eip", &ia32_sysenter_eip)
                .finish())
        })()
        .unwrap()
    }
}

impl<H: AxVCpuHal> AxArchVCpu for VmxVcpu<H> {
    type CreateConfig = usize;

    type SetupConfig = ();

    type HostContext = crate::context::LinuxContext;

    fn new(id: Self::CreateConfig) -> AxResult<Self> {
        Self::new(id)
    }

    fn load_context(&self, config: &mut Self::HostContext) -> AxResult {
        // info!("Loading context {:#x?}", self);

        self.load_vmcs_guest(config)?;
        Ok(())
    }

    fn set_entry(&mut self, entry: GuestPhysAddr) -> AxResult {
        self.entry = Some(entry);
        Ok(())
    }

    fn set_ept_root(&mut self, ept_root: HostPhysAddr) -> AxResult {
        self.ept_root = Some(ept_root);
        Ok(())
    }

    fn setup(&mut self, _config: Self::SetupConfig) -> AxResult {
        self.setup_vmcs(self.ept_root.unwrap(), self.entry, None)
    }

    fn setup_from_context(&mut self, ctx: Self::HostContext) -> AxResult {
        self.guest_regs.load_from_context(&ctx);
        self.setup_vmcs(self.ept_root.unwrap(), None, Some(ctx))
    }

    fn run(&mut self) -> AxResult<AxVCpuExitReason> {
        match self.inner_run() {
            Some(exit_info) => Ok(if exit_info.entry_failure {
                match exit_info.exit_reason {
                    VmxExitReason::INVALID_GUEST_STATE
                    | VmxExitReason::MCE_DURING_VMENTRY
                    | VmxExitReason::MSR_LOAD_FAIL => {}
                    _ => {
                        error!("Invalid exit reasion when entry failure: {:#x?}", exit_info);
                    }
                };

                let exit_qualification = exit_qualification()?;

                warn!("VMX entry failure: {:#x?}", exit_info);
                warn!("Exit qualification: {:#x?}", exit_qualification);
                warn!("VCpu {:#x?}", self);

                AxVCpuExitReason::FailEntry {
                    // Todo: get `hardware_entry_failure_reason` somehow.
                    hardware_entry_failure_reason: 0,
                }
            } else {
                match exit_info.exit_reason {
                    VmxExitReason::VMCALL => {
                        self.advance_rip(exit_info.exit_instruction_length as _)?;
                        AxVCpuExitReason::Hypercall {
                            nr: self.regs().rax,
                            args: [
                                self.regs().rdi,
                                self.regs().rsi,
                                self.regs().rdx,
                                self.regs().rcx,
                                self.regs().r8,
                                self.regs().r9,
                            ],
                        }
                    }
                    VmxExitReason::IO_INSTRUCTION => {
                        let io_info = self.io_exit_info().unwrap();
                        self.advance_rip(exit_info.exit_instruction_length as _)?;

                        let port = io_info.port;

                        if io_info.is_repeat || io_info.is_string {
                            warn!(
                                "VMX unsupported IO-Exit: {:#x?} of {:#x?}",
                                io_info, exit_info
                            );
                            warn!("VCpu {:#x?}", self);
                            AxVCpuExitReason::Halt
                        } else {
                            let width = match AccessWidth::try_from(io_info.access_size as usize) {
                                Ok(width) => width,
                                Err(_) => {
                                    warn!(
                                        "VMX invalid IO-Exit: {:#x?} of {:#x?}",
                                        io_info, exit_info
                                    );
                                    warn!("VCpu {:#x?}", self);
                                    return Ok(AxVCpuExitReason::Halt);
                                }
                            };

                            if io_info.is_in {
                                AxVCpuExitReason::IoRead { port, width }
                            } else if port == QEMU_EXIT_PORT
                                && width == AccessWidth::Word
                                && self.regs().rax == QEMU_EXIT_MAGIC
                            {
                                AxVCpuExitReason::SystemDown
                            } else {
                                AxVCpuExitReason::IoWrite {
                                    port,
                                    width,
                                    data: self.regs().rax.get_bits(width.bits_range()),
                                }
                            }
                        }
                    }
                    VmxExitReason::EPT_VIOLATION => {
                        let ept_info = self.nested_page_fault_info()?;
                        self.decode_instruction(
                            GuestVirtAddr::from_usize(exit_info.guest_rip),
                            exit_info.exit_instruction_length as _,
                        )?;

                        AxVCpuExitReason::NestedPageFault {
                            addr: ept_info.fault_guest_paddr,
                            access_flags: ept_info.access_flags,
                        }
                    }
                    VmxExitReason::TRIPLE_FAULT => {
                        error!("VMX triple fault: {:#x?}", exit_info);
                        error!("VCpu {:#x?}", self);
                        AxVCpuExitReason::Halt
                    }
                    _ => {
                        warn!("VMX unsupported VM-Exit: {:#x?}", exit_info);
                        warn!("VCpu {:#x?}", self);
                        AxVCpuExitReason::Halt
                    }
                }
            }),
            None => Ok(AxVCpuExitReason::Nothing),
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
}

impl<H: AxVCpuHal> AxVcpuAccessGuestState for VmxVcpu<H> {
    type GeneralRegisters = GeneralRegisters;

    fn regs(&self) -> &Self::GeneralRegisters {
        self.regs()
    }

    fn regs_mut(&mut self) -> &mut Self::GeneralRegisters {
        self.regs_mut()
    }

    fn read_gpr(&self, reg: usize) -> usize {
        self.regs().get_reg_of_index(reg as u8) as usize
    }

    fn write_gpr(&mut self, reg: usize, val: usize) {
        self.regs_mut().set_reg_of_index(reg as u8, val as u64);
    }

    fn instr_pointer(&self) -> usize {
        VmcsGuestNW::RIP.read().expect("Failed to read RIP") as usize
    }

    fn set_instr_pointer(&mut self, val: usize) {
        VmcsGuestNW::RIP.write(val as _).expect("Failed to set RIP");
    }

    fn stack_pointer(&self) -> usize {
        self.stack_pointer()
    }

    fn set_stack_pointer(&mut self, val: usize) {
        self.set_stack_pointer(val);
    }

    fn frame_pointer(&self) -> usize {
        self.regs().rbp as usize
    }

    fn set_frame_pointer(&mut self, val: usize) {
        self.regs_mut().rbp = val as u64;
    }

    fn return_value(&self) -> usize {
        self.regs().rax as usize
    }

    fn set_return_value(&mut self, val: usize) {
        self.regs_mut().rax = val as u64;
    }

    fn guest_is_privileged(&self) -> bool {
        use crate::segmentation::SegmentAccessRights;
        SegmentAccessRights::from_bits_truncate(
            VmcsGuest32::CS_ACCESS_RIGHTS
                .read()
                .expect("Failed to read CS_ACCESS_RIGHTS"),
        )
        .dpl()
            == 0
    }

    fn guest_page_table_query(
        &self,
        gva: GuestVirtAddr,
    ) -> Option<(GuestPhysAddr, MappingFlags, PageSize)> {
        self.guest_page_table_query(gva).ok()
    }

    fn current_ept_root(&self) -> HostPhysAddr {
        vmcs::get_ept_pointer()
    }

    fn eptp_list_region(&self) -> HostPhysAddr {
        self.eptp_list.phys_addr()
    }
}
