// vmcb.rs — AMD‑SVM Virtual‑Machine Control Block helpers
//
// This is the SVM counterpart of `vmcs.rs`.  A VMCB is a single 4‑KiB page
// split into a 1024‑byte Control Area (offset 0x0) and a 3‑KiB
// State‑Save Area (offset 0x400).  Unlike Intel VMCS, each field is
// located at a fixed offset inside the page.  That means the hypervisor can
// touch the fields with normal memory operations – no special I/O encoding or
// `VMWRITE/VMREAD` instructions are required.
//
// We use tock‑registers to generate strongly‑typed register proxies so you
// get:
//   • type‑safe read/write helpers (no accidental mix‑ups)
//   • handy bitfield accessors for intercept vectors, event injection, …
//   • zero‑cost abstractions that compile to plain loads/stores.
//
// Reference: AMD 64 APM v2,Appendix B VMCB Layout

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

use tock_registers::registers::ReadWrite;
use tock_registers::{register_bitfields, register_structs};

use axaddrspace::HostPhysAddr;
use axerrno::AxResult;

use super::definitions::{SvmExitCode, SvmIntercept};
use super::structs::VmcbFrame; // the user‑supplied wrapper that owns the backing page
use axvcpu::AxVCpuHal;
use memory_addr::MemoryAddr;
use tock_registers::interfaces::{ReadWriteable, Readable, Writeable};
// ─────────────────────────────────────────────────────────────────────────────
//  Control‑area bitfields
// ─────────────────────────────────────────────────────────────────────────────

register_bitfields![u32,
    // vector 0
    pub InterceptCrRw [
        READ_CR0   0,  READ_CR3   3,  READ_CR4   4,  READ_CR8   8,
        WRITE_CR0 16,  WRITE_CR3 19,  WRITE_CR4 20,  WRITE_CR8 24,
    ],

    // vector 1
    pub InterceptDrRw [
        READ_DR0   0,  READ_DR7   7,
        WRITE_DR0 16,  WRITE_DR7 23,
    ],

    // vector 2
    pub InterceptExceptions [
        DE 0, DB 1, BP 3, OF 4, DF 8, GP 13, PF 14, MC 18,
    ],

    /// Vector 3  (offset 0x000C)
    pub InterceptVec3 [
        INTR            0,  NMI             1,  SMI              2,  INIT            3,
        VINTR           4,  CR0_SEL_WRITE   5,  IDTR_READ        6,  GDTR_READ       7,
        LDTR_READ       8,  TR_READ         9,  IDTR_WRITE      10,  GDTR_WRITE     11,
        LDTR_WRITE     12,  TR_WRITE       13,  RDTSC           14,  RDPMC          15,
        PUSHF          16,  POPF           17,  CPUID           18,  RSM            19,
        IRET           20,  SWINT          21,  INVD            22,  PAUSE          23,
        HLT            24,  INVLPG         25,  INVLPGA         26,  IOIO_PROT      27,
        MSR_PROT       28,  TASK_SWITCH    29,  FERR_FREEZE     30,  SHUTDOWN       31,
    ],

    /// Vector 4  (offset 0x0010)
    pub InterceptVec4 [
        VMRUN           0,  VMMCALL         1,  VMLOAD          2,  VMSAVE          3,
        STGI            4,  CLGI            5,  SKINIT          6,  RDTSCP          7,
        ICEBP           8,  WBINVD          9,  MONITOR        10,  MWAIT          11,
        MWAIT_CONDITIONAL 12, XSETBV       13,  RDPRU          14,  EFER_WRITE_TRAP 15,
    ],

    /// Vector 5  (offset 0x0014)
    pub InterceptVec5 [
        INVLPGB         0,  INVLPGB_ILLEGAL 1,  INVPCID         2,
        MCOMMIT         3,  TLBSYNC         4,
    ],
    // VMCB Clean-Bits 15.15.3
    pub VmcbCleanBits [
        INTERCEPTS  0,
        IOPM        1,
        ASID        2,
        TPR         3,
        NP          4,
        CRx         5,
        DRx         6,
        DT          7,
        SEG         8,
        CR2         9,
        LBR         10,
        AVIC        11,
        CET         12,
    ],
];

register_bitfields![u64,
    pub NestedCtl [
        NP_ENABLE        0,
        SEV_ENABLE       1,
        SEV_ES_ENABLE    2,
        GMET_ENABLE      3,   // Guest-Mode-Exec-Trap
        SSCheckEn        4,
        VTE_ENABLE       5,   // Virtual Transparent Encryption
        RO_GPT_EN        6,   // Read-Only Guest Page Tables
        INVLPGB_TLBSYNC  7,
    ],
];

register_bitfields![u8,
    pub VmcbTlbControl [
        CONTROL OFFSET(0) NUMBITS(3) [
            DoNothing                = 0,
            FlushAllOnVmrun          = 1,
            FlushGuestTlb            = 3,
            FlushGuestNonGlobalTlb   = 7,
        ]
    ]
];

// register_bitfields![u16,
//     pub VmcbSegmentAttr [
//         // ACCESSED        OFFSET(0) NUMBITS(1),  // not used in VMCB
//         READABLE        OFFSET(1) NUMBITS(1),

//         /// Code/Data bit, available for User segments (`S` = 1) only
//         CODE            OFFSET(3) NUMBITS(1),
//         /// User/System bit `S`
//         USER            OFFSET(4) NUMBITS(1),
//         /// DPL
//         DPL             OFFSET(5) NUMBITS(2),
//     ]
// ];

register_structs![
    pub VmcbControlArea {
        (0x0000 => pub intercept_cr:         ReadWrite<u32, InterceptCrRw::Register>),
        (0x0004 => pub intercept_dr:         ReadWrite<u32, InterceptDrRw::Register>),

        (0x0008 => pub intercept_exceptions: ReadWrite<u32, InterceptExceptions::Register>),
        (0x000C => pub intercept_vector3:    ReadWrite<u32, InterceptVec3::Register>),
        (0x0010 => pub intercept_vector4:    ReadWrite<u32, InterceptVec4::Register>),
        (0x0014 => pub intercept_vector5:    ReadWrite<u32, InterceptVec5::Register>),
        (0x0018 => _reserved_0018),
        (0x003C => pub pause_filter_thresh:   ReadWrite<u16>),
        (0x003E => pub pause_filter_count:    ReadWrite<u16>),

        (0x0040 => pub iopm_base_pa:          ReadWrite<u64>),
        (0x0048 => pub msrpm_base_pa:         ReadWrite<u64>),
        (0x0050 => pub tsc_offset:            ReadWrite<u64>),

        (0x0058 => pub guest_asid:            ReadWrite<u32>),
        (0x005C => pub tlb_control:           ReadWrite<u8, VmcbTlbControl::Register>),
        (0x005D => _reserved_005D),

        (0x0060 => pub int_control:           ReadWrite<u32>),
        (0x0064 => pub int_vector:            ReadWrite<u32>),
        (0x0068 => pub int_state:             ReadWrite<u32>),
        (0x006C => _reserved_006C),

        // ───── VMEXIT  ---------------------------------------------------
        (0x0070 => pub exit_code:             ReadWrite<u64>),
        (0x0078 => pub exit_info_1:           ReadWrite<u64>),
        (0x0080 => pub exit_info_2:           ReadWrite<u64>),
        // 15.7.2
        (0x0088 => pub exit_int_info:         ReadWrite<u32>),
        (0x008C => pub exit_int_info_err:     ReadWrite<u32>),

        // ───── Nested Paging / AVIC -----------------------------------------
        (0x0090 => pub nested_ctl:         ReadWrite<u64, NestedCtl::Register>),

        // 0x0098 — AVIC_VAPIC_BAR（APIC-access BAR，only 40 bit are vaild）
        (0x0098 => pub avic_vapic_bar:     ReadWrite<u64>),

        // 0x00A0 — GHCB guest-physical address
        (0x00A0 => pub ghcb_gpa:           ReadWrite<u64>),

        // ── Event-injection / Nested CR3 / LBR --------------------------------
        (0x00A8 => pub event_inj:          ReadWrite<u32>),
        (0x00AC => pub event_inj_err:      ReadWrite<u32>),
        (0x00B0 => pub nested_cr3:         ReadWrite<u64>),
        (0x00B8 => pub virt_ext:           ReadWrite<u64>),   // LBR-control & V-VMLOAD/VMSAVE

        // ── Clean-bits & Next-RIP --------------------------------------------
        (0x00C0 => pub clean_bits:         ReadWrite<u32, VmcbCleanBits::Register>),
        (0x00C4 => pub _rsvd5:             ReadWrite<u32>),
        (0x00C8 => pub next_rip:           ReadWrite<u64>),

        // ── Decoded-instruction cache ----------------------------------------
        (0x00D0 => pub insn_len:           ReadWrite<u8>),
        (0x00D1 => pub insn_bytes:         [ReadWrite<u8>; 15]),

        // ── AVIC extra --------------------------------------------------------
        (0x00E0 => pub avic_backing_page:  ReadWrite<u64>),
        (0x00E8 => _reserved_00E8),

        (0x00F0 => pub avic_logical_id:    ReadWrite<u64>),
        (0x00F8 => pub avic_physical_id:   ReadWrite<u64>),
        (0x0100 => _reserved_0100),

        (0x0108 => pub vmsa_pa:            ReadWrite<u64>),    // SEV-ES guest only
        (0x0110 => _reserved_0110),

        (0x0120 => pub bus_lock_counter:   ReadWrite<u16>),
        (0x0122 => _reserved_0122),

        (0x0138 => pub allowed_sev_features: ReadWrite<u64>),
        (0x0140 => pub guest_sev_features:   ReadWrite<u64>),
        (0x0148 => _reserved_0148),

        (0x0400 => @END),
    }
];

register_structs![
    pub VmcbSegment {
        (0x0 => pub selector: ReadWrite<u16>),
        (0x2 => pub attr:     ReadWrite<u16>),
        (0x4 => pub limit:    ReadWrite<u32>),
        (0x8 => pub base:     ReadWrite<u64>),
        (0x10 => @END),
    }
];

register_structs![
    pub VmcbStateSaveArea {
        (0x0000 => pub es:   VmcbSegment),
        (0x0010 => pub cs:   VmcbSegment),
        (0x0020 => pub ss:   VmcbSegment),
        (0x0030 => pub ds:   VmcbSegment),
        (0x0040 => pub fs:   VmcbSegment),
        (0x0050 => pub gs:   VmcbSegment),
        (0x0060 => pub gdtr: VmcbSegment),
        (0x0070 => pub ldtr: VmcbSegment),
        (0x0080 => pub idtr: VmcbSegment),
        (0x0090 => pub tr:   VmcbSegment),
        (0x00A0 => _reserved_00A0),

        (0x00CB => pub cpl:  ReadWrite<u8>),
        (0x00CC => _reserved_00CC),

        (0x00D0 => pub efer: ReadWrite<u64>),
        (0x00D8 => _reserved_00D8),

        (0x0148 => pub cr4:   ReadWrite<u64>),
        (0x0150 => pub cr3:   ReadWrite<u64>),
        (0x0158 => pub cr0:   ReadWrite<u64>),
        (0x0160 => pub dr7:   ReadWrite<u64>),
        (0x0168 => pub dr6:   ReadWrite<u64>),
        (0x0170 => pub rflags:ReadWrite<u64>),
        (0x0178 => pub rip:   ReadWrite<u64>),
        (0x0180 => _reserved_0180),

        (0x01D8 => pub rsp:          ReadWrite<u64>),
        (0x01E0 => pub s_cet:        ReadWrite<u64>),
        (0x01E8 => pub ssp:          ReadWrite<u64>),
        (0x01F0 => pub isst_addr:    ReadWrite<u64>),
        (0x01F8 => pub rax:          ReadWrite<u64>),

        (0x0200 => pub star:          ReadWrite<u64>),
        (0x0208 => pub lstar:         ReadWrite<u64>),
        (0x0210 => pub cstar:         ReadWrite<u64>),
        (0x0218 => pub sfmask:        ReadWrite<u64>),
        (0x0220 => pub kernel_gs_base:ReadWrite<u64>),
        (0x0228 => pub sysenter_cs:   ReadWrite<u64>),
        (0x0230 => pub sysenter_esp:  ReadWrite<u64>),
        (0x0238 => pub sysenter_eip:  ReadWrite<u64>),
        (0x0240 => pub cr2:           ReadWrite<u64>),
        (0x0248 => _reserved_0248),

        (0x0268 => pub g_pat:         ReadWrite<u64>),
        (0x0270 => pub dbgctl:        ReadWrite<u64>),
        (0x0278 => pub br_from:       ReadWrite<u64>),
        (0x0280 => pub br_to:         ReadWrite<u64>),
        (0x0288 => pub last_excp_from:ReadWrite<u64>),
        (0x0290 => pub last_excp_to:  ReadWrite<u64>),
        (0x0298 => _reserved_0298),

        (0x0C00 => @END),
    }
];

register_structs![
    pub VmcbStruct {
        (0x0000 => pub control: VmcbControlArea),
        (0x0400 => pub state:   VmcbStateSaveArea),
        (0x1000 => @END),
    }
];

/// Unified façade returning typed accessors to both halves of the VMCB.
pub struct Vmcb<'a> {
    pub control: &'a mut VmcbControlArea,
    pub state: &'a mut VmcbStateSaveArea,
}

impl<H: AxVCpuHal> VmcbFrame<H> {
    /// # Safety
    /// caller must guarantee the page is mapped
    pub unsafe fn as_vmcb(&self) -> &mut VmcbStruct {
        unsafe { self.as_mut_ptr_vmcb().as_mut().unwrap() }
    }
}

impl VmcbStruct {
    /// Zero‑initialise the control area
    pub fn clear_control(&mut self) {
        unsafe { core::ptr::write_bytes(&mut self.control as *mut _ as *mut u8, 0, 0x400) };
    }
    pub fn clean_bits(&mut self) -> &mut ReadWrite<u32, VmcbCleanBits::Register> {
        &mut self.control.clean_bits
    }
}

pub fn set_vmcb_segment(seg: &mut VmcbSegment, selector: u16, attr: u16) {
    seg.selector.set(selector); // 一般初始化阶段都传 0
    seg.base.set(0); // 实模式／平坦段：基址 0
    seg.limit.set(0xFFFF); // 64 KiB 段界限
    seg.attr.set(attr); // AR 字节（0x93, 0x9B, 0x8B, 0x82 …）
}

impl VmcbControlArea {
    pub fn set_intercept(&mut self, intc: SvmIntercept) {
        use super::definitions::SvmIntercept::*;
        match intc {
            // ── Vector 3 ───────────────────────────────
            INTR => self.intercept_vector3.modify(InterceptVec3::INTR::SET),
            NMI => self.intercept_vector3.modify(InterceptVec3::NMI::SET),
            SMI => self.intercept_vector3.modify(InterceptVec3::SMI::SET),
            INIT => self.intercept_vector3.modify(InterceptVec3::INIT::SET),
            VINTR => self.intercept_vector3.modify(InterceptVec3::VINTR::SET),
            CR0_SEL_WRITE => self
                .intercept_vector3
                .modify(InterceptVec3::CR0_SEL_WRITE::SET),
            IDTR_READ => self.intercept_vector3.modify(InterceptVec3::IDTR_READ::SET),
            GDTR_READ => self.intercept_vector3.modify(InterceptVec3::GDTR_READ::SET),
            LDTR_READ => self.intercept_vector3.modify(InterceptVec3::LDTR_READ::SET),
            TR_READ => self.intercept_vector3.modify(InterceptVec3::TR_READ::SET),
            IDTR_WRITE => self
                .intercept_vector3
                .modify(InterceptVec3::IDTR_WRITE::SET),
            GDTR_WRITE => self
                .intercept_vector3
                .modify(InterceptVec3::GDTR_WRITE::SET),
            LDTR_WRITE => self
                .intercept_vector3
                .modify(InterceptVec3::LDTR_WRITE::SET),
            TR_WRITE => self.intercept_vector3.modify(InterceptVec3::TR_WRITE::SET),
            RDTSC => self.intercept_vector3.modify(InterceptVec3::RDTSC::SET),
            RDPMC => self.intercept_vector3.modify(InterceptVec3::RDPMC::SET),
            PUSHF => self.intercept_vector3.modify(InterceptVec3::PUSHF::SET),
            POPF => self.intercept_vector3.modify(InterceptVec3::POPF::SET),
            CPUID => self.intercept_vector3.modify(InterceptVec3::CPUID::SET),
            RSM => self.intercept_vector3.modify(InterceptVec3::RSM::SET),
            IRET => self.intercept_vector3.modify(InterceptVec3::IRET::SET),
            SWINT => self.intercept_vector3.modify(InterceptVec3::SWINT::SET),
            INVD => self.intercept_vector3.modify(InterceptVec3::INVD::SET),
            PAUSE => self.intercept_vector3.modify(InterceptVec3::PAUSE::SET),
            HLT => self.intercept_vector3.modify(InterceptVec3::HLT::SET),
            INVLPG => self.intercept_vector3.modify(InterceptVec3::INVLPG::SET),
            INVLPGA => self.intercept_vector3.modify(InterceptVec3::INVLPGA::SET),
            IOIO_PROT => self.intercept_vector3.modify(InterceptVec3::IOIO_PROT::SET),
            MSR_PROT => self.intercept_vector3.modify(InterceptVec3::MSR_PROT::SET),
            TASK_SWITCH => self
                .intercept_vector3
                .modify(InterceptVec3::TASK_SWITCH::SET),
            FERR_FREEZE => self
                .intercept_vector3
                .modify(InterceptVec3::FERR_FREEZE::SET),
            SHUTDOWN => self.intercept_vector3.modify(InterceptVec3::SHUTDOWN::SET),

            // ── Vector 4 ───────────────────────────────
            VMRUN => self.intercept_vector4.modify(InterceptVec4::VMRUN::SET),
            VMMCALL => self.intercept_vector4.modify(InterceptVec4::VMMCALL::SET),
            VMLOAD => self.intercept_vector4.modify(InterceptVec4::VMLOAD::SET),
            VMSAVE => self.intercept_vector4.modify(InterceptVec4::VMSAVE::SET),
            STGI => self.intercept_vector4.modify(InterceptVec4::STGI::SET),
            CLGI => self.intercept_vector4.modify(InterceptVec4::CLGI::SET),
            SKINIT => self.intercept_vector4.modify(InterceptVec4::SKINIT::SET),
            RDTSCP => self.intercept_vector4.modify(InterceptVec4::RDTSCP::SET),
            ICEBP => self.intercept_vector4.modify(InterceptVec4::ICEBP::SET),
            WBINVD => self.intercept_vector4.modify(InterceptVec4::WBINVD::SET),
            MONITOR => self.intercept_vector4.modify(InterceptVec4::MONITOR::SET),
            MWAIT => self.intercept_vector4.modify(InterceptVec4::MWAIT::SET),
            MWAIT_CONDITIONAL => self
                .intercept_vector4
                .modify(InterceptVec4::MWAIT_CONDITIONAL::SET),
            XSETBV => self.intercept_vector4.modify(InterceptVec4::XSETBV::SET),
            RDPRU => self.intercept_vector4.modify(InterceptVec4::RDPRU::SET),
            EFER_WRITE_TRAP => self
                .intercept_vector4
                .modify(InterceptVec4::EFER_WRITE_TRAP::SET),

            // ── Vector 5 ───────────────────────────────
            INVLPGB => self.intercept_vector5.modify(InterceptVec5::INVLPGB::SET),
            INVLPGB_ILLEGAL => self
                .intercept_vector5
                .modify(InterceptVec5::INVLPGB_ILLEGAL::SET),
            INVPCID => self.intercept_vector5.modify(InterceptVec5::INVPCID::SET),
            MCOMMIT => self.intercept_vector5.modify(InterceptVec5::MCOMMIT::SET),
            TLBSYNC => self.intercept_vector5.modify(InterceptVec5::TLBSYNC::SET),
        }
    }
}

#[derive(Debug)]
pub struct SvmExitInfo {
    pub exit_code: core::result::Result<SvmExitCode, u64>,
    pub exit_info_1: u64,
    pub exit_info_2: u64,
    pub guest_rip: u64,
    pub guest_next_rip: u64,
}

impl VmcbStruct {
    pub fn exit_info(&self) -> AxResult<SvmExitInfo> {
        Ok(SvmExitInfo {
            exit_code: self.control.exit_code.get().try_into(),
            exit_info_1: self.control.exit_info_1.get(),
            exit_info_2: self.control.exit_info_2.get(),
            guest_rip: self.state.rip.get(),
            guest_next_rip: self.control.next_rip.get(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn vmcb_size_check() {
        use core::mem::size_of;

        assert_eq!(size_of::<VmcbControlArea>(), 0x400);
        assert_eq!(size_of::<VmcbStateSaveArea>(), 0xC00);
        assert_eq!(size_of::<VmcbStruct>(), 0x1000);
    }

    #[test]
    fn vmcb_offset_check() {
        use memoffset::offset_of;

        assert_eq!(offset_of!(VmcbStruct, control), 0x0000);
        assert_eq!(offset_of!(VmcbStruct, state), 0x0400);

        macro_rules! assert_vmcb_ctrl_offset {
            ($field:ident, $offset:expr) => {
                assert_eq!(offset_of!(VmcbControlArea, $field), $offset);
            };
        }

        macro_rules! assert_vmcb_save_offset {
            ($field:ident, $offset:expr) => {
                assert_eq!(offset_of!(VmcbStateSaveArea, $field), $offset);
            };
        }

        assert_vmcb_ctrl_offset!(intercept_cr, 0x00);
        assert_vmcb_ctrl_offset!(intercept_dr, 0x04);
        assert_vmcb_ctrl_offset!(intercept_exceptions, 0x08);
        assert_vmcb_ctrl_offset!(intercept_vector3, 0x0C);
        assert_vmcb_ctrl_offset!(intercept_vector4, 0x10);
        assert_vmcb_ctrl_offset!(intercept_vector5, 0x14);
        assert_vmcb_ctrl_offset!(pause_filter_thresh, 0x3C);
        assert_vmcb_ctrl_offset!(pause_filter_count, 0x3E);
        assert_vmcb_ctrl_offset!(iopm_base_pa, 0x40);
        assert_vmcb_ctrl_offset!(msrpm_base_pa, 0x48);
        assert_vmcb_ctrl_offset!(tsc_offset, 0x50);
        assert_vmcb_ctrl_offset!(guest_asid, 0x58);
        assert_vmcb_ctrl_offset!(tlb_control, 0x5C);
        assert_vmcb_ctrl_offset!(int_control, 0x60);
        assert_vmcb_ctrl_offset!(int_vector, 0x64);
        assert_vmcb_ctrl_offset!(int_state, 0x68);
        assert_vmcb_ctrl_offset!(exit_code, 0x70);
        assert_vmcb_ctrl_offset!(exit_info_1, 0x78);
        assert_vmcb_ctrl_offset!(exit_info_2, 0x80);
        assert_vmcb_ctrl_offset!(exit_int_info, 0x88);
        assert_vmcb_ctrl_offset!(exit_int_info_err, 0x8C);
        assert_vmcb_ctrl_offset!(nested_ctl, 0x90);
        assert_vmcb_ctrl_offset!(avic_vapic_bar, 0x98);
        assert_vmcb_ctrl_offset!(event_inj, 0xA8);
        assert_vmcb_ctrl_offset!(event_inj_err, 0xAC);
        assert_vmcb_ctrl_offset!(nested_cr3, 0xB0);
        assert_vmcb_ctrl_offset!(virt_ext, 0xB8);
        assert_vmcb_ctrl_offset!(clean_bits, 0xC0);
        assert_vmcb_ctrl_offset!(next_rip, 0xC8);
        assert_vmcb_ctrl_offset!(insn_len, 0xD0);
        assert_vmcb_ctrl_offset!(insn_bytes, 0xD1);
        assert_vmcb_ctrl_offset!(avic_backing_page, 0xE0);
        assert_vmcb_ctrl_offset!(avic_logical_id, 0xF0);
        assert_vmcb_ctrl_offset!(avic_physical_id, 0xF8);

        assert_vmcb_save_offset!(es, 0x00);
        assert_vmcb_save_offset!(cs, 0x10);
        assert_vmcb_save_offset!(ss, 0x20);
        assert_vmcb_save_offset!(ds, 0x30);
        assert_vmcb_save_offset!(fs, 0x40);
        assert_vmcb_save_offset!(gs, 0x50);
        assert_vmcb_save_offset!(gdtr, 0x60);
        assert_vmcb_save_offset!(ldtr, 0x70);
        assert_vmcb_save_offset!(idtr, 0x80);
        assert_vmcb_save_offset!(tr, 0x90);
        assert_vmcb_save_offset!(cpl, 0xCB);
        assert_vmcb_save_offset!(efer, 0xD0);
        assert_vmcb_save_offset!(cr4, 0x148);
        assert_vmcb_save_offset!(cr3, 0x150);
        assert_vmcb_save_offset!(cr0, 0x158);
        assert_vmcb_save_offset!(dr7, 0x160);
        assert_vmcb_save_offset!(dr6, 0x168);
        assert_vmcb_save_offset!(rflags, 0x170);
        assert_vmcb_save_offset!(rip, 0x178);
        assert_vmcb_save_offset!(rsp, 0x1D8);
        assert_vmcb_save_offset!(s_cet, 0x1E0);
        assert_vmcb_save_offset!(ssp, 0x1E8);
        assert_vmcb_save_offset!(isst_addr, 0x1F0);
        assert_vmcb_save_offset!(rax, 0x1F8);
        assert_vmcb_save_offset!(star, 0x200);
        assert_vmcb_save_offset!(lstar, 0x208);
        assert_vmcb_save_offset!(cstar, 0x210);
        assert_vmcb_save_offset!(sfmask, 0x218);
        assert_vmcb_save_offset!(kernel_gs_base, 0x220);
        assert_vmcb_save_offset!(sysenter_cs, 0x228);
        assert_vmcb_save_offset!(sysenter_esp, 0x230);
        assert_vmcb_save_offset!(sysenter_eip, 0x238);
        assert_vmcb_save_offset!(cr2, 0x240);
        assert_vmcb_save_offset!(g_pat, 0x268);
        assert_vmcb_save_offset!(dbgctl, 0x270);
        assert_vmcb_save_offset!(br_from, 0x278);
        assert_vmcb_save_offset!(br_to, 0x280);
        assert_vmcb_save_offset!(last_excp_from, 0x288);
        assert_vmcb_save_offset!(last_excp_to, 0x290);
    }
}
