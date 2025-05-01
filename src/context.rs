use x86::segmentation::SegmentSelector;
use x86::{Ring, segmentation, task};
use x86_64::VirtAddr;
use x86_64::instructions::tables::{lgdt, lidt, sidt};
use x86_64::registers::control::{Cr0, Cr0Flags, Cr3, Cr3Flags, Cr4, Cr4Flags, Efer, EferFlags};
use x86_64::structures::DescriptorTablePointer;
use x86_64::{addr::PhysAddr, structures::paging::PhysFrame};

use crate::msr::Msr;
use crate::regs::GeneralRegisters;
use crate::segmentation::{Segment, SegmentAccessRights};

const SAVED_LINUX_REGS: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct LinuxContext {
    pub rsp: u64,
    pub rip: u64,

    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbx: u64,
    pub rbp: u64,

    pub es: Segment,
    pub cs: Segment,
    pub ss: Segment,
    pub ds: Segment,
    pub fs: Segment,
    pub gs: Segment,
    pub tss: Segment,
    pub gdt: DescriptorTablePointer,
    pub idt: DescriptorTablePointer,

    pub cr0: Cr0Flags,
    pub cr3: u64,
    pub cr4: Cr4Flags,

    pub efer: EferFlags,
    pub star: u64,
    pub lstar: u64,
    pub cstar: u64,
    pub fmask: u64,

    pub ia32_sysenter_cs: u64,
    pub ia32_sysenter_esp: u64,
    pub ia32_sysenter_eip: u64,

    pub kernel_gsbase: u64,
    pub pat: u64,
    pub mtrr_def_type: u64,
    // TODO: xstate
}

unsafe impl Send for LinuxContext {}
unsafe impl Sync for LinuxContext {}

impl Default for LinuxContext {
    fn default() -> Self {
        Self {
            rsp: 0,
            rip: 0,
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            rbx: 0,
            rbp: 0,
            es: Segment::invalid(),
            cs: Segment::invalid(),
            ss: Segment::invalid(),
            ds: Segment::invalid(),
            fs: Segment::invalid(),
            gs: Segment::invalid(),
            tss: Segment::invalid(),
            gdt: DescriptorTablePointer {
                limit: 0,
                base: VirtAddr::zero(),
            },
            idt: DescriptorTablePointer {
                limit: 0,
                base: VirtAddr::zero(),
            },
            cr0: Cr0Flags::empty(),
            cr3: 0,
            cr4: Cr4Flags::empty(),
            efer: EferFlags::empty(),
            star: 0,
            lstar: 0,
            cstar: 0,
            fmask: 0,
            ia32_sysenter_cs: 0,
            ia32_sysenter_esp: 0,
            ia32_sysenter_eip: 0,
            kernel_gsbase: 0,
            pat: 0,
            mtrr_def_type: 0,
        }
    }
}

fn sgdt() -> DescriptorTablePointer {
    let mut gdt = DescriptorTablePointer {
        limit: 0,
        base: VirtAddr::zero(),
    };
    unsafe {
        core::arch::asm!("sgdt [{0}]", in(reg) &mut gdt, options(nostack, preserves_flags));
    }
    gdt
}

impl LinuxContext {
    /// Load linux callee-saved registers from the stack, and other system registers.
    pub fn load_from(linux_sp: usize) -> Self {
        let regs = unsafe { core::slice::from_raw_parts(linux_sp as *const u64, SAVED_LINUX_REGS) };
        let gdt = sgdt();

        let mut fs = Segment::from_selector(x86::segmentation::fs(), &gdt);
        let mut gs = Segment::from_selector(x86::segmentation::gs(), &gdt);
        fs.base = Msr::IA32_FS_BASE.read();
        gs.base = regs[0];

        Self {
            rsp: regs.as_ptr_range().end as _,
            r15: regs[1],
            r14: regs[2],
            r13: regs[3],
            r12: regs[4],
            rbx: regs[5],
            rbp: regs[6],
            rip: regs[7],
            es: Segment::from_selector(segmentation::es(), &gdt),
            cs: Segment::from_selector(segmentation::cs(), &gdt),
            ss: Segment::from_selector(segmentation::ss(), &gdt),
            ds: Segment::from_selector(segmentation::ds(), &gdt),
            fs,
            gs,
            tss: Segment::from_selector(unsafe { task::tr() }, &gdt),
            gdt,
            idt: sidt(),
            cr0: Cr0::read(),
            cr3: Cr3::read().0.start_address().as_u64(),
            cr4: Cr4::read(),
            efer: Efer::read(),
            star: Msr::IA32_STAR.read(),
            lstar: Msr::IA32_LSTAR.read(),
            cstar: Msr::IA32_CSTAR.read(),
            fmask: Msr::IA32_FMASK.read(),
            ia32_sysenter_cs: Msr::IA32_SYSENTER_CS.read(),
            ia32_sysenter_esp: Msr::IA32_SYSENTER_ESP.read(),
            ia32_sysenter_eip: Msr::IA32_SYSENTER_EIP.read(),
            kernel_gsbase: Msr::IA32_KERNEL_GSBASE.read(),
            pat: Msr::IA32_PAT.read(),
            mtrr_def_type: Msr::IA32_MTRR_DEF_TYPE.read(),
        }
    }

    pub fn construct_guest64(rip: u64, cr3: u64) -> Self {
        Self {
            rsp: 0,
            rip,
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            rbx: 0,
            rbp: 0,
            es: Segment::invalid(),
            cs: Segment {
                selector: SegmentSelector::new(1, Ring::Ring0),
                base: 0,
                limit: 0xffff,
                access_rights: SegmentAccessRights::ACCESSED
                    | SegmentAccessRights::WRITABLE
                    | SegmentAccessRights::EXECUTABLE
                    | SegmentAccessRights::CODE_DATA
                    | SegmentAccessRights::PRESENT
                    | SegmentAccessRights::LONG_MODE
                    | SegmentAccessRights::GRANULARITY,
            },
            ss: Segment {
                selector: SegmentSelector::new(2, Ring::Ring0),
                base: 0,
                limit: 0xffff,
                access_rights: SegmentAccessRights::ACCESSED
                    | SegmentAccessRights::WRITABLE
                    | SegmentAccessRights::CODE_DATA
                    | SegmentAccessRights::PRESENT
                    | SegmentAccessRights::DB
                    | SegmentAccessRights::GRANULARITY,
            },
            ds: Segment::invalid(),
            fs: Segment::invalid(),
            gs: Segment::invalid(),
            tss: Segment {
                selector: SegmentSelector::new(2, Ring::Ring0),
                base: 0,
                limit: 0,
                access_rights: SegmentAccessRights::ACCESSED
                    | SegmentAccessRights::WRITABLE
                    | SegmentAccessRights::EXECUTABLE
                    | SegmentAccessRights::PRESENT,
            },
            gdt: DescriptorTablePointer {
                limit: 0,
                base: VirtAddr::zero(),
            },
            idt: DescriptorTablePointer {
                limit: 0,
                base: VirtAddr::zero(),
            },
            cr0: Cr0Flags::PROTECTED_MODE_ENABLE
                | Cr0Flags::MONITOR_COPROCESSOR
                | Cr0Flags::EXTENSION_TYPE
                | Cr0Flags::NUMERIC_ERROR
                | Cr0Flags::WRITE_PROTECT
                | Cr0Flags::ALIGNMENT_MASK
                | Cr0Flags::PAGING,
            cr3,
            cr4: Cr4Flags::PHYSICAL_ADDRESS_EXTENSION | Cr4Flags::PAGE_GLOBAL,
            efer: EferFlags::LONG_MODE_ENABLE
                | EferFlags::LONG_MODE_ACTIVE
                | EferFlags::NO_EXECUTE_ENABLE
                | EferFlags::SYSTEM_CALL_EXTENSIONS,
            star: 0,
            lstar: 0,
            cstar: 0,
            fmask: 0,
            ia32_sysenter_cs: 0,
            ia32_sysenter_esp: 0,
            ia32_sysenter_eip: 0,
            kernel_gsbase: 0,
            pat: 0,
            mtrr_def_type: 0,
        }
    }

    /// Restore system registers.
    pub fn restore(&self) {
        unsafe {
            Msr::IA32_SYSENTER_CS.write(self.ia32_sysenter_cs);
            Msr::IA32_SYSENTER_ESP.write(self.ia32_sysenter_esp);
            Msr::IA32_SYSENTER_EIP.write(self.ia32_sysenter_eip);

            Efer::write(self.efer);
            Msr::IA32_STAR.write(self.star);
            Msr::IA32_LSTAR.write(self.lstar);
            Msr::IA32_CSTAR.write(self.cstar);
            Msr::IA32_FMASK.write(self.fmask);
            Msr::IA32_KERNEL_GSBASE.write(self.kernel_gsbase);
            Msr::IA32_PAT.write(self.pat);

            Cr0::write(self.cr0);
            Cr4::write(self.cr4);
            // cr3 must be last in case cr4 enables PCID
            Cr3::write(
                PhysFrame::containing_address(PhysAddr::new(self.cr3)),
                Cr3Flags::empty(), // clear PCID
            );
        }

        // Copy Linux TSS descriptor into our GDT, clearing the busy flag,
        // then reload TR from it. We can't use Linux' GDT as it is r/o.

        let hv_gdt = sgdt();
        let entry_count = (hv_gdt.limit as usize + 1) / size_of::<u64>();

        let hv_gdt_table: &mut [u64] =
            unsafe { core::slice::from_raw_parts_mut(hv_gdt.base.as_mut_ptr(), entry_count) };

        let linux_gdt = &self.gdt;
        let entry_count = (linux_gdt.limit as usize + 1) / size_of::<u64>();
        let linux_gdt_table =
            unsafe { core::slice::from_raw_parts(linux_gdt.base.as_mut_ptr(), entry_count) };

        let tss_idx = self.tss.selector.index() as usize;
        hv_gdt_table[tss_idx] = linux_gdt_table[tss_idx];
        hv_gdt_table[tss_idx + 1] = linux_gdt_table[tss_idx + 1];

        SegmentAccessRights::set_descriptor_type(
            &mut hv_gdt_table[self.tss.selector.index() as usize],
            SegmentAccessRights::TSS_AVAIL,
        );
        unsafe {
            task::load_tr(self.tss.selector);
            lgdt(&self.gdt);
            lidt(&self.idt);

            segmentation::load_es(self.es.selector);
            segmentation::load_cs(self.cs.selector);
            segmentation::load_ss(self.ss.selector);
            segmentation::load_ds(self.ds.selector);
            segmentation::load_fs(self.fs.selector);
            segmentation::load_gs(self.gs.selector);

            Msr::IA32_FS_BASE.write(self.fs.base);
        }
    }

    pub fn load_guest_regs(&mut self, regs: &GeneralRegisters) {
        self.r15 = regs.r15;
        self.r14 = regs.r14;
        self.r13 = regs.r13;
        self.r12 = regs.r12;
        self.rbx = regs.rbx;
        self.rbp = regs.rbp;
    }

    /// Restore linux general-purpose registers and stack, then return back to linux.
    pub fn return_to_linux(&self, guest_regs: &GeneralRegisters) -> ! {
        unsafe {
            Msr::IA32_GS_BASE.write(self.gs.base);
            core::arch::asm!(
                "mov rsp, {linux_rsp}",
                "push {linux_rip}",
                "mov rcx, rsp",
                "mov rsp, {guest_regs}",
                "mov [rsp + {guest_regs_size}], rcx",
                restore_regs_from_stack!(),
                "pop rsp",
                "ret",
                linux_rsp = in(reg) self.rsp,
                linux_rip = in(reg) self.rip,
                guest_regs = in(reg) guest_regs,
                guest_regs_size = const core::mem::size_of::<GeneralRegisters>(),
                options(noreturn),
            );
        }
    }
}
