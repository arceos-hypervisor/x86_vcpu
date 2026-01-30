//! Tests for GeneralRegisters structure.

use crate::regs::GeneralRegisters;

#[test]
fn test_general_registers_default() {
    let regs = GeneralRegisters::default();
    assert_eq!(regs.rax, 0);
    assert_eq!(regs.rcx, 0);
    assert_eq!(regs.rdx, 0);
    assert_eq!(regs.rbx, 0);
    assert_eq!(regs.rbp, 0);
    assert_eq!(regs.rsi, 0);
    assert_eq!(regs.rdi, 0);
    assert_eq!(regs.r8, 0);
    assert_eq!(regs.r9, 0);
    assert_eq!(regs.r10, 0);
    assert_eq!(regs.r11, 0);
    assert_eq!(regs.r12, 0);
    assert_eq!(regs.r13, 0);
    assert_eq!(regs.r14, 0);
    assert_eq!(regs.r15, 0);
}

#[test]
fn test_general_registers_clone() {
    let mut regs1 = GeneralRegisters::default();
    regs1.rax = 0x1234;
    regs1.rbx = 0x5678;

    let regs2 = regs1.clone();
    assert_eq!(regs1.rax, regs2.rax);
    assert_eq!(regs1.rbx, regs2.rbx);
}

#[test]
fn test_general_registers_copy() {
    let mut regs1 = GeneralRegisters::default();
    regs1.rax = 0xabcd;

    let regs2 = regs1; // Copy
    assert_eq!(regs1.rax, regs2.rax);
}

#[test]
fn test_general_registers_eq() {
    let regs1 = GeneralRegisters::default();
    let regs2 = GeneralRegisters::default();
    assert_eq!(regs1, regs2);

    let mut regs3 = GeneralRegisters::default();
    regs3.rax = 1;
    assert_ne!(regs1, regs3);
}

#[test]
fn test_register_names() {
    assert_eq!(GeneralRegisters::REGISTER_NAMES[0], "rax");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[1], "rcx");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[2], "rdx");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[3], "rbx");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[4], "rsp");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[5], "rbp");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[6], "rsi");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[7], "rdi");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[8], "r8");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[9], "r9");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[10], "r10");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[11], "r11");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[12], "r12");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[13], "r13");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[14], "r14");
    assert_eq!(GeneralRegisters::REGISTER_NAMES[15], "r15");
}

#[test]
fn test_register_name_function() {
    assert_eq!(GeneralRegisters::register_name(0), "rax");
    assert_eq!(GeneralRegisters::register_name(8), "r8");
    assert_eq!(GeneralRegisters::register_name(15), "r15");
}

#[test]
fn test_get_reg_of_index() {
    let mut regs = GeneralRegisters::default();
    regs.rax = 0x100;
    regs.rcx = 0x101;
    regs.rdx = 0x102;
    regs.rbx = 0x103;
    regs.rbp = 0x105;
    regs.rsi = 0x106;
    regs.rdi = 0x107;
    regs.r8 = 0x108;
    regs.r9 = 0x109;
    regs.r10 = 0x10a;
    regs.r11 = 0x10b;
    regs.r12 = 0x10c;
    regs.r13 = 0x10d;
    regs.r14 = 0x10e;
    regs.r15 = 0x10f;

    assert_eq!(regs.get_reg_of_index(0), 0x100);
    assert_eq!(regs.get_reg_of_index(1), 0x101);
    assert_eq!(regs.get_reg_of_index(2), 0x102);
    assert_eq!(regs.get_reg_of_index(3), 0x103);
    assert_eq!(regs.get_reg_of_index(5), 0x105);
    assert_eq!(regs.get_reg_of_index(6), 0x106);
    assert_eq!(regs.get_reg_of_index(7), 0x107);
    assert_eq!(regs.get_reg_of_index(8), 0x108);
    assert_eq!(regs.get_reg_of_index(9), 0x109);
    assert_eq!(regs.get_reg_of_index(10), 0x10a);
    assert_eq!(regs.get_reg_of_index(11), 0x10b);
    assert_eq!(regs.get_reg_of_index(12), 0x10c);
    assert_eq!(regs.get_reg_of_index(13), 0x10d);
    assert_eq!(regs.get_reg_of_index(14), 0x10e);
    assert_eq!(regs.get_reg_of_index(15), 0x10f);
}

#[test]
fn test_set_reg_of_index() {
    let mut regs = GeneralRegisters::default();

    regs.set_reg_of_index(0, 0x1000);
    assert_eq!(regs.rax, 0x1000);

    regs.set_reg_of_index(8, 0x8000);
    assert_eq!(regs.r8, 0x8000);

    regs.set_reg_of_index(15, 0xf000);
    assert_eq!(regs.r15, 0xf000);
}

#[test]
fn test_get_edx_eax() {
    let mut regs = GeneralRegisters::default();
    regs.rax = 0x12345678;
    regs.rdx = 0xabcdef00;

    let combined = regs.get_edx_eax();
    // edx:eax = (edx << 32) | eax
    assert_eq!(combined, 0xabcdef0012345678);
}

#[test]
fn test_32bit_register_accessors() {
    let mut regs = GeneralRegisters::default();

    // Set 32-bit value - should clear upper 32 bits
    regs.rax = 0xffffffff_ffffffff;
    regs.set_eax(0x12345678);
    assert_eq!(regs.rax, 0x12345678);
    assert_eq!(regs.eax(), 0x12345678);
}

#[test]
fn test_16bit_register_accessors() {
    let mut regs = GeneralRegisters::default();

    // Set 16-bit value - should NOT clear other bits
    regs.rax = 0xfedcba9876543210;
    regs.set_ax(0xabcd);
    assert_eq!(regs.rax, 0xfedcba987654abcd);
    assert_eq!(regs.ax(), 0xabcd);
}

#[test]
fn test_8bit_register_accessors() {
    let mut regs = GeneralRegisters::default();

    // Set 8-bit low value - should NOT clear other bits
    regs.rax = 0xfedcba9876543210;
    regs.set_al(0xef);
    assert_eq!(regs.rax, 0xfedcba98765432ef);
    assert_eq!(regs.al(), 0xef);
}

#[test]
fn test_8bit_high_register_accessors() {
    let mut regs = GeneralRegisters::default();

    // Set 8-bit high value (ah, bh, ch, dh)
    regs.rax = 0xfedcba9876543210;
    regs.set_ah(0xab);
    assert_eq!(regs.rax, 0xfedcba987654ab10);
    assert_eq!(regs.ah(), 0xab);
}

#[test]
fn test_debug_format() {
    let mut regs = GeneralRegisters::default();
    regs.rax = 0x1234;
    let debug_str = alloc::format!("{:?}", regs);
    // Just verify the debug string is not empty and contains the struct name
    assert!(!debug_str.is_empty());
    assert!(debug_str.contains("GeneralRegisters"));
}

#[test]
fn test_all_r8_to_r15_registers() {
    let mut regs = GeneralRegisters::default();

    // Test 64-bit access
    regs.r8 = 0x0808080808080808;
    regs.r9 = 0x0909090909090909;
    regs.r10 = 0x1010101010101010;
    regs.r11 = 0x1111111111111111;
    regs.r12 = 0x1212121212121212;
    regs.r13 = 0x1313131313131313;
    regs.r14 = 0x1414141414141414;
    regs.r15 = 0x1515151515151515;

    // Test 32-bit access
    assert_eq!(regs.r8d(), 0x08080808);
    assert_eq!(regs.r9d(), 0x09090909);
    assert_eq!(regs.r10d(), 0x10101010);
    assert_eq!(regs.r11d(), 0x11111111);
    assert_eq!(regs.r12d(), 0x12121212);
    assert_eq!(regs.r13d(), 0x13131313);
    assert_eq!(regs.r14d(), 0x14141414);
    assert_eq!(regs.r15d(), 0x15151515);

    // Test 16-bit access
    assert_eq!(regs.r8w(), 0x0808);
    assert_eq!(regs.r9w(), 0x0909);
    assert_eq!(regs.r10w(), 0x1010);
    assert_eq!(regs.r11w(), 0x1111);
    assert_eq!(regs.r12w(), 0x1212);
    assert_eq!(regs.r13w(), 0x1313);
    assert_eq!(regs.r14w(), 0x1414);
    assert_eq!(regs.r15w(), 0x1515);

    // Test 8-bit access
    assert_eq!(regs.r8b(), 0x08);
    assert_eq!(regs.r9b(), 0x09);
    assert_eq!(regs.r10b(), 0x10);
    assert_eq!(regs.r11b(), 0x11);
    assert_eq!(regs.r12b(), 0x12);
    assert_eq!(regs.r13b(), 0x13);
    assert_eq!(regs.r14b(), 0x14);
    assert_eq!(regs.r15b(), 0x15);
}

#[test]
#[should_panic(expected = "Illegal index")]
fn test_get_reg_invalid_index_high() {
    let regs = GeneralRegisters::default();
    let _ = regs.get_reg_of_index(16);
}

#[test]
#[should_panic(expected = "Illegal index")]
fn test_get_reg_invalid_index_rsp() {
    let regs = GeneralRegisters::default();
    // Index 4 is RSP which is unused
    let _ = regs.get_reg_of_index(4);
}

#[test]
#[should_panic(expected = "Illegal index")]
fn test_set_reg_invalid_index_high() {
    let mut regs = GeneralRegisters::default();
    regs.set_reg_of_index(16, 0);
}

#[test]
#[should_panic(expected = "Illegal index")]
fn test_set_reg_invalid_index_rsp() {
    let mut regs = GeneralRegisters::default();
    // Index 4 is RSP which is unused
    regs.set_reg_of_index(4, 0);
}
