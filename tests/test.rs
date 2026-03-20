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

extern crate alloc;

#[macro_use]
extern crate log;

use alloc::format;

#[path = "../src/ept.rs"]
mod ept;
#[path = "../src/msr.rs"]
mod msr;
#[macro_use]
#[path = "../src/regs/mod.rs"]
mod regs;
#[path = "../src/test_utils.rs"]
mod test_utils;
#[path = "../src/vmx/mod.rs"]
mod vmx;

use axaddrspace::device::AccessWidth;
use axvcpu::AxArchPerCpu;
use axvisor_api::memory::MemoryIf;
use bit_field::BitField;
use ept::GuestPageWalkInfo;
use memory_addr::PhysAddr;
use msr::{Msr, MsrReadWrite};
use regs::GeneralRegisters;
use test_utils::mock::MockMmHal;
use vmx::{
    CR0_PE, EPTPointer, FeatureControlFlags, IOBitmap, MSR_IA32_EFER_LMA_BIT, MsrBitmap,
    QEMU_EXIT_MAGIC, QEMU_EXIT_PORT, VMX_PREEMPTION_TIMER_SET_VALUE, VmCpuMode, VmxBasic,
    VmxExitReason, VmxPerCpuState, VmxRegion,
};

#[test]
fn test_mock_allocator() {
    MockMmHal::reset();

    let addr1 = MockMmHal::alloc_frame().unwrap();
    let addr2 = MockMmHal::alloc_frame().unwrap();
    let addr3 = MockMmHal::alloc_frame().unwrap();

    assert_ne!(addr1.as_usize(), addr2.as_usize());
    assert_ne!(addr2.as_usize(), addr3.as_usize());
    assert_ne!(addr1.as_usize(), addr3.as_usize());

    assert_eq!(addr1.as_usize() % 0x1000, 0);
    assert_eq!(addr2.as_usize() % 0x1000, 0);
    assert_eq!(addr3.as_usize() % 0x1000, 0);
}

#[test]
fn test_msr_enum_values() {
    assert_eq!(Msr::IA32_FEATURE_CONTROL as u32, 0x3a);
    assert_eq!(Msr::IA32_PAT as u32, 0x277);
    assert_eq!(Msr::IA32_VMX_BASIC as u32, 0x480);
    assert_eq!(Msr::IA32_EFER as u32, 0xc000_0080);
    assert_eq!(Msr::IA32_LSTAR as u32, 0xc000_0082);
}

#[test]
fn test_msr_debug() {
    let msr = Msr::IA32_VMX_BASIC;
    let debug_str = format!("{:?}", msr);
    assert!(debug_str.contains("IA32_VMX_BASIC"));
}

#[test]
fn test_msr_copy_clone() {
    let msr1 = Msr::IA32_EFER;
    let msr2 = msr1;
    let msr3 = msr1;

    assert_eq!(msr1 as u32, msr2 as u32);
    assert_eq!(msr1 as u32, msr3 as u32);
}

#[test]
fn test_vmx_msr_ranges() {
    assert!(Msr::IA32_VMX_BASIC as u32 >= 0x480);
    assert!(Msr::IA32_VMX_TRUE_ENTRY_CTLS as u32 <= 0x490);
    assert_eq!(
        Msr::IA32_VMX_BASIC as u32 + 1,
        Msr::IA32_VMX_PINBASED_CTLS as u32
    );
    assert_eq!(
        Msr::IA32_VMX_PINBASED_CTLS as u32 + 1,
        Msr::IA32_VMX_PROCBASED_CTLS as u32
    );
}

#[test]
fn test_fs_gs_base_msr_values() {
    assert_eq!(Msr::IA32_FS_BASE as u32, 0xc000_0100);
    assert_eq!(Msr::IA32_GS_BASE as u32, 0xc000_0101);
    assert_eq!(Msr::IA32_KERNEL_GSBASE as u32, 0xc000_0102);
    assert_eq!(Msr::IA32_FS_BASE as u32 + 1, Msr::IA32_GS_BASE as u32);
    assert_eq!(Msr::IA32_GS_BASE as u32 + 1, Msr::IA32_KERNEL_GSBASE as u32);
}

#[test]
fn test_system_call_msr_values() {
    assert_eq!(Msr::IA32_STAR as u32, 0xc000_0081);
    assert_eq!(Msr::IA32_LSTAR as u32, 0xc000_0082);
    assert_eq!(Msr::IA32_CSTAR as u32, 0xc000_0083);
    assert_eq!(Msr::IA32_FMASK as u32, 0xc000_0084);
    assert_eq!(Msr::IA32_STAR as u32 + 1, Msr::IA32_LSTAR as u32);
    assert_eq!(Msr::IA32_LSTAR as u32 + 1, Msr::IA32_CSTAR as u32);
    assert_eq!(Msr::IA32_CSTAR as u32 + 1, Msr::IA32_FMASK as u32);
}

struct TestMsr;

impl MsrReadWrite for TestMsr {
    const MSR: Msr = Msr::IA32_PAT;
}

#[test]
fn test_msr_read_write_trait() {
    assert_eq!(TestMsr::MSR as u32, 0x277);
}

#[test]
fn test_msr_as_u32_conversion() {
    let msrs = [
        Msr::IA32_FEATURE_CONTROL,
        Msr::IA32_VMX_BASIC,
        Msr::IA32_EFER,
        Msr::IA32_LSTAR,
    ];

    for msr in msrs {
        let value = msr as u32;
        assert!(value > 0);
        assert!(value < 0xffff_ffff);
    }
}

macro_rules! test_rw_on_reg {
    ([$(($pos:literal, $reg:ident, $reg32:ident, $reg16:ident, $reg8:ident $(, $reg8h:ident)? $(,)?)),+ $(,)?]) => {
        paste::paste! {
            $(
                #[test]
                fn [< test_read_write_on_reg_ $reg >]() {
                    let mut regs = GeneralRegisters::default();
                    regs.$reg = 0xfedcba9876543210;
                    assert_eq!(regs.get_reg_of_index($pos), 0xfedcba9876543210);

                    regs.set_reg_of_index($pos, 0x123456789abcdef0);
                    assert_eq!(regs.$reg, 0x123456789abcdef0);

                    $(
                        regs.[< set_ $reg8h >](0x12);
                        assert_eq!(regs.$reg, 0x123456789abc12f0);
                        regs.[< set_ $reg8h >](0xde);
                    )?

                    regs.[< set_ $reg8 >](0x34);
                    assert_eq!(regs.$reg, 0x123456789abcde34);

                    regs.[< set_ $reg16 >](0x5678);
                    assert_eq!(regs.$reg, 0x123456789abc5678);

                    regs.[< set_ $reg32 >](0x9abcdef0);
                    assert_eq!(regs.$reg, 0x9abcdef0);
                }
            )+
        }
    };
}

test_rw_on_reg!([
    (0, rax, eax, ax, al, ah),
    (1, rcx, ecx, cx, cl, ch),
    (2, rdx, edx, dx, dl, dh),
    (3, rbx, ebx, bx, bl, bh),
    (5, rbp, ebp, bp, bpl),
    (6, rsi, esi, si, sil),
    (7, rdi, edi, di, dil),
    (8, r8, r8d, r8w, r8b),
    (9, r9, r9d, r9w, r9b),
    (10, r10, r10d, r10w, r10b),
    (11, r11, r11d, r11w, r11b),
    (12, r12, r12d, r12w, r12b),
    (13, r13, r13d, r13w, r13b),
    (14, r14, r14d, r14w, r14b),
    (15, r15, r15d, r15w, r15b),
]);

#[test]
fn test_vmx_region_uninit() {
    let region = unsafe { VmxRegion::uninit() };
    let debug_str = format!("{:?}", region);
    assert!(!debug_str.is_empty());
}

#[test]
fn test_vmx_region_new() {
    MockMmHal::reset();
    let region = VmxRegion::new(0x12345, false).unwrap();
    let addr = region.phys_addr();
    assert_ne!(addr.as_usize(), 0);
    assert_eq!(addr.as_usize() % 0x1000, 0);
}

#[test]
fn test_vmx_region_new_with_shadow() {
    MockMmHal::reset();
    let region1 = VmxRegion::new(0x12345, false).unwrap();
    let region2 = VmxRegion::new(0x12345, true).unwrap();

    let addr1 = region1.phys_addr();
    let addr2 = region2.phys_addr();
    assert_ne!(addr1.as_usize(), 0);
    assert_ne!(addr2.as_usize(), 0);
    assert_ne!(addr1.as_usize(), addr2.as_usize());
    assert_eq!(addr1.as_usize() % 0x1000, 0);
    assert_eq!(addr2.as_usize() % 0x1000, 0);
}

#[test]
fn test_io_bitmap_creation() {
    MockMmHal::reset();
    let passthrough_bitmap = IOBitmap::passthrough_all().unwrap();
    assert!(IOBitmap::intercept_all().is_ok());
    let (addr_a, addr_b) = passthrough_bitmap.phys_addr();
    assert_ne!(addr_a.as_usize(), 0);
    assert_ne!(addr_b.as_usize(), 0);
    assert_ne!(addr_a.as_usize(), addr_b.as_usize());
}

#[test]
fn test_msr_bitmap_creation() {
    MockMmHal::reset();
    let passthrough_bitmap = MsrBitmap::passthrough_all().unwrap();
    assert!(MsrBitmap::intercept_all().is_ok());
    let addr = passthrough_bitmap.phys_addr();
    assert_ne!(addr.as_usize(), 0);
    assert_eq!(addr.as_usize() % 0x1000, 0);
}

#[test]
fn test_ept_pointer_creation() {
    let ept_ptr1 = EPTPointer::from_table_phys(PhysAddr::from(0x1000));
    let ept_ptr2 = EPTPointer::from_table_phys(PhysAddr::from(0x2000));
    assert_ne!(ept_ptr1.bits(), ept_ptr2.bits());
}

#[test]
fn test_ept_pointer_getters() {
    let ept_ptr = EPTPointer::from_table_phys(PhysAddr::from(0x3000));
    let bits = ept_ptr.bits();
    assert_ne!(bits, 0);
    let expected =
        EPTPointer::MEM_TYPE_WB | EPTPointer::WALK_LENGTH_4 | EPTPointer::ENABLE_ACCESSED_DIRTY;
    assert_eq!(bits & expected.bits(), expected.bits());
}

#[test]
fn test_vmx_basic_constants() {
    assert_eq!(VmxBasic::VMX_MEMORY_TYPE_WRITE_BACK, 6);
}

#[test]
fn test_feature_control_flags() {
    let flags = FeatureControlFlags::LOCKED | FeatureControlFlags::VMXON_ENABLED_OUTSIDE_SMX;
    assert!(flags.contains(FeatureControlFlags::LOCKED));
    assert!(flags.contains(FeatureControlFlags::VMXON_ENABLED_OUTSIDE_SMX));
    assert!(!flags.contains(FeatureControlFlags::VMXON_ENABLED_INSIDE_SMX));
}

#[test]
fn test_ept_pointer_flags() {
    use EPTPointer as EPT;
    assert_eq!(EPT::MEM_TYPE_UC.bits(), 0);
    assert_eq!(EPT::MEM_TYPE_WB.bits(), 6);
    assert_eq!(EPT::WALK_LENGTH_4.bits(), 3 << 3);
    let combined = EPT::MEM_TYPE_WB | EPT::WALK_LENGTH_4 | EPT::ENABLE_ACCESSED_DIRTY;
    assert!(combined.contains(EPT::MEM_TYPE_WB));
    assert!(combined.contains(EPT::WALK_LENGTH_4));
    assert!(combined.contains(EPT::ENABLE_ACCESSED_DIRTY));
}

#[test]
fn test_ept_pointer_from_table_phys() {
    let ept_ptr = EPTPointer::from_table_phys(PhysAddr::from(0x12345000_usize));
    assert!(ept_ptr.contains(EPTPointer::MEM_TYPE_WB));
    assert!(ept_ptr.contains(EPTPointer::WALK_LENGTH_4));
    assert!(ept_ptr.contains(EPTPointer::ENABLE_ACCESSED_DIRTY));
    let addr_part = ept_ptr.bits() & !0xfff;
    assert_eq!(addr_part, 0x12345000);
}

#[test]
fn test_ept_pointer_from_unaligned_addr() {
    let ept_ptr = EPTPointer::from_table_phys(PhysAddr::from(0x12345678_usize));
    let addr_part = ept_ptr.bits() & !0xfff;
    assert_eq!(addr_part, 0x12345000);
}

#[test]
fn test_structs_debug_implementations() {
    let vmx_region = unsafe { VmxRegion::uninit() };
    let _ = format!("{:?}", vmx_region);

    let io_bitmap = IOBitmap::passthrough_all().unwrap();
    let _ = format!("{:?}", io_bitmap);

    let msr_bitmap = MsrBitmap::passthrough_all().unwrap();
    let _ = format!("{:?}", msr_bitmap);

    let flags = FeatureControlFlags::LOCKED;
    let _ = format!("{:?}", flags);

    let ept_flags = EPTPointer::MEM_TYPE_WB;
    let _ = format!("{:?}", ept_flags);
}

#[test]
fn test_vmx_per_cpu_state_new() {
    MockMmHal::reset();
    let result = VmxPerCpuState::new(0);
    assert!(result.is_ok());

    let state = result.unwrap();
    assert_eq!(state.vmcs_revision_id, 0);
}

#[test]
fn test_vmx_per_cpu_state_default_values() {
    MockMmHal::reset();
    let state = VmxPerCpuState::new(0).unwrap();
    assert_eq!(state.vmcs_revision_id, 0);
}

#[test]
fn test_multiple_cpu_states_independence() {
    MockMmHal::reset();
    let mut states = alloc::vec::Vec::new();
    for cpu_id in 0..4 {
        states.push(VmxPerCpuState::new(cpu_id).unwrap());
    }

    states[0].vmcs_revision_id = 0x12345678;
    states[1].vmcs_revision_id = 0x87654321;

    assert_eq!(states[0].vmcs_revision_id, 0x12345678);
    assert_eq!(states[1].vmcs_revision_id, 0x87654321);
    assert_eq!(states[2].vmcs_revision_id, 0);
    assert_eq!(states[3].vmcs_revision_id, 0);
}

#[test]
fn test_vmx_per_cpu_state_debug() {
    MockMmHal::reset();
    let state = VmxPerCpuState::new(0).unwrap();
    let debug_str = format!("{:?}", state);
    assert!(!debug_str.is_empty());
}

#[test]
fn test_vmx_per_cpu_state_size() {
    let size = core::mem::size_of::<VmxPerCpuState>();
    assert!(size > 4);
    assert!(size < 1024);
}

#[test]
fn test_vm_cpu_mode_enum() {
    assert_ne!(VmCpuMode::Real, VmCpuMode::Protected);
    assert_ne!(VmCpuMode::Protected, VmCpuMode::Compatibility);
    assert_ne!(VmCpuMode::Compatibility, VmCpuMode::Mode64);
    let debug_str = format!("{:?}", VmCpuMode::Mode64);
    assert!(debug_str.contains("Mode64"));
}

#[test]
fn test_general_registers_operations() {
    let mut regs = GeneralRegisters::default();
    assert_eq!(regs.rax, 0);
    assert_eq!(regs.rbx, 0);

    regs.rax = 0x1234567890abcdef;
    regs.rbx = 0xfedcba0987654321;

    assert_eq!(regs.rax, 0x1234567890abcdef);
    assert_eq!(regs.rbx, 0xfedcba0987654321);

    regs.set_reg_of_index(0, 0x1111111111111111);
    assert_eq!(regs.get_reg_of_index(0), 0x1111111111111111);

    regs.set_reg_of_index(1, 0x2222222222222222);
    assert_eq!(regs.get_reg_of_index(1), 0x2222222222222222);
}

#[test]
fn test_constants() {
    assert_eq!(VMX_PREEMPTION_TIMER_SET_VALUE, 1_000_000);
    assert_eq!(QEMU_EXIT_PORT, 0x604);
    assert_eq!(QEMU_EXIT_MAGIC, 0x2000);
    assert_eq!(MSR_IA32_EFER_LMA_BIT, 1 << 10);
    assert_eq!(CR0_PE, 1 << 0);
}

#[test]
fn test_bit_operations() {
    let mut value = 0u64;
    value.set_bits(0..32, 0x12345678);
    value.set_bits(32..64, 0xabcdef00);

    assert_eq!(value.get_bits(0..32), 0x12345678);
    assert_eq!(value.get_bits(32..64), 0xabcdef00);
}

fn create_test_vcpu_regs() -> GeneralRegisters {
    let mut regs = GeneralRegisters::default();
    regs.rax = 0x1000;
    regs.rbx = 0x2000;
    regs.rcx = 0x3000;
    regs.rdx = 0x4000;
    regs
}

#[test]
fn test_general_registers_clone() {
    let regs = create_test_vcpu_regs();
    let cloned_regs = regs.clone();
    assert_eq!(regs.rax, cloned_regs.rax);
    assert_eq!(regs.rbx, cloned_regs.rbx);
    assert_eq!(regs.rcx, cloned_regs.rcx);
    assert_eq!(regs.rdx, cloned_regs.rdx);
}

#[test]
fn test_edx_eax_operations() {
    let rax = 0x12345678u64;
    let rdx = 0xabcdef00u64;
    let combined = ((rdx & 0xffff_ffff) << 32) | (rax & 0xffff_ffff);
    assert_eq!(combined, 0xabcdef0012345678);

    let val = 0xfedcba0987654321u64;
    let new_rax = val & 0xffff_ffff;
    let new_rdx = val >> 32;
    assert_eq!(new_rax, 0x87654321);
    assert_eq!(new_rdx, 0xfedcba09);
}

#[test]
fn test_register_bit_operations() {
    let mut regs = GeneralRegisters::default();
    regs.rcx = 0;
    regs.rcx.set_bits(0..32, 0x12345678);
    assert_eq!(regs.rcx.get_bits(0..32), 0x12345678);

    regs.rdx = 0xffffffffffffffff;
    regs.rdx.set_bits(32..64, 0);
    assert_eq!(regs.rdx.get_bits(32..64), 0);
    assert_eq!(regs.rdx.get_bits(0..32), 0xffffffff);
}

#[test]
fn test_gla2gva_logic() {
    let guest_rip = 0x1000usize;
    let seg_base_64bit = 0;
    let seg_base_other = 0x10000;

    assert_eq!(guest_rip + seg_base_64bit, 0x1000);
    assert_eq!(guest_rip + seg_base_other, 0x11000);
}

#[test]
fn test_interrupt_vector_validation() {
    let valid_exception = 6;
    let valid_interrupt = 0x20;
    let invalid_vector = 0;

    assert!(valid_exception < 32);
    assert!(valid_interrupt >= 32);
    assert_eq!(invalid_vector, 0);
}

#[test]
fn test_page_walk_info_struct() {
    let ptw_info = GuestPageWalkInfo {
        top_entry: 0x1000,
        level: 4,
        width: 9,
        is_user_mode_access: false,
        is_write_access: false,
        is_inst_fetch: false,
        pse: true,
        wp: true,
        nxe: true,
        is_smap_on: false,
        is_smep_on: false,
    };

    assert_eq!(ptw_info.level, 4);
    assert_eq!(ptw_info.width, 9);
    assert_eq!(ptw_info.top_entry, 0x1000);
}

#[test]
fn test_cpuid_constants() {
    const LEAF_FEATURE_INFO: u32 = 0x1;
    const LEAF_HYPERVISOR_INFO: u32 = 0x4000_0000;
    const FEATURE_VMX: u32 = 1 << 5;
    const FEATURE_HYPERVISOR: u32 = 1 << 31;

    assert_eq!(LEAF_FEATURE_INFO, 1);
    assert_eq!(LEAF_HYPERVISOR_INFO, 0x40000000);
    assert_eq!(FEATURE_VMX, 32);
    assert_eq!(FEATURE_HYPERVISOR, 0x80000000);
}

#[test]
fn test_cr_flags_operations() {
    use x86_64::registers::control::{Cr0Flags, Cr4Flags};

    let cr0_flags = Cr0Flags::PAGING | Cr0Flags::PROTECTED_MODE_ENABLE;
    assert!(cr0_flags.contains(Cr0Flags::PAGING));
    assert!(cr0_flags.contains(Cr0Flags::PROTECTED_MODE_ENABLE));
    assert!(!cr0_flags.contains(Cr0Flags::CACHE_DISABLE));

    let cr4_flags = Cr4Flags::VIRTUAL_MACHINE_EXTENSIONS | Cr4Flags::PAGE_SIZE_EXTENSION;
    assert!(cr4_flags.contains(Cr4Flags::VIRTUAL_MACHINE_EXTENSIONS));
    assert!(cr4_flags.contains(Cr4Flags::PAGE_SIZE_EXTENSION));
}

#[test]
fn test_access_width_operations() {
    assert_eq!(AccessWidth::Byte as usize, 0);
    assert_eq!(AccessWidth::Word as usize, 1);
    assert_eq!(AccessWidth::Dword as usize, 2);
    assert_eq!(AccessWidth::Qword as usize, 3);

    assert_eq!(AccessWidth::try_from(1), Ok(AccessWidth::Byte));
    assert_eq!(AccessWidth::try_from(2), Ok(AccessWidth::Word));
    assert_eq!(AccessWidth::try_from(4), Ok(AccessWidth::Dword));
    assert_eq!(AccessWidth::try_from(8), Ok(AccessWidth::Qword));
}

#[test]
fn test_get_tr_base_logic() {
    let mut test_entry = 0u64;
    test_entry |= 1u64 << 47;
    test_entry |= (0x1000u64 & 0xFFFFFF) << 16;

    let present = test_entry & (1 << 47) != 0;
    assert!(present);

    let base_low = (test_entry >> 16) & 0xFFFFFF;
    let base_high = (test_entry >> 56) & 0xFF;
    let base_addr = base_low | (base_high << 24);
    assert_eq!(base_addr, 0x1000);
}

#[test]
fn test_vmx_exit_reason_enum() {
    let test_reason = VmxExitReason::VMCALL;
    match test_reason {
        VmxExitReason::VMCALL => assert!(true),
        _ => assert!(false),
    }
}

#[test]
fn test_vcpu_debug_implementations() {
    let cpu_mode = VmCpuMode::Mode64;
    let debug_str = format!("{:?}", cpu_mode);
    assert!(!debug_str.is_empty());

    let regs = GeneralRegisters::default();
    let debug_str = format!("{:?}", regs);
    assert!(!debug_str.is_empty());
}
