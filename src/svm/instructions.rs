//! AMD-SVM 指令封装
//!
//! 资料来源：AMD64 APM Vol-2 “Secure Virtual Machine” §15 & §16
//! - VMRUN / VMLOAD / VMSAVE / STGI / CLGI / INVLPGA
//! - 这些指令 **不会像 VMX 指令那样用 RFLAGS.CF / ZF 返回错误**；
//!   如果控制区不合法，会直接 #GP(0) 或 #UD，所以简单包一层即可。

use core::arch::asm;

/// SVM 指令基本都不会返回状态；如果失败直接 #UD/#GP，
/// 我们姑且用最简单的 `Result<(), ()>` 占位，后续如需
/// 细分错误再扩展枚举即可。
pub type Result<T = ()> = core::result::Result<T, ()>;

/// 进入来宾：`vmrun rax`
#[inline(always)]
pub unsafe fn vmrun(vmcb_pa: u64) -> ! {
    asm!(
    "vmrun {0}",
    in(reg) vmcb_pa,
    options(noreturn, nostack),
    )
}

/// 保存 Host-state：`vmsave rax`
#[inline(always)]
pub unsafe fn vmsave(vmcb_pa: u64) -> Result {
    asm!(
    "vmsave {0}",
    in(reg) vmcb_pa,
    options(nostack, preserves_flags),
    );
    Ok(())
}

/// 恢复 Host-state：`vmload rax`
#[inline(always)]
pub unsafe fn vmload(vmcb_pa: u64) -> Result {
    asm!(
    "vmload {0}",
    in(reg) vmcb_pa,
    options(nostack, preserves_flags),
    );
    Ok(())
}

/// 允许全局中断 (`STGI`)
#[inline(always)]
pub unsafe fn stgi() {
    asm!("stgi", options(nostack, preserves_flags));
}

/// 禁止全局中断 (`CLGI`)
#[inline(always)]
pub unsafe fn clgi() {
    asm!("clgi", options(nostack, preserves_flags));
}

/// `INVLpga` —— 按 (guest-virt addr, ASID) 刷 TLB
///
/// * `addr`  : Guest 虚拟地址 (通常传 0 代表“全页表”)
/// * `asid`  : Address-Space ID，0 触发全局 flush
#[inline(always)]
pub unsafe fn invlpga(addr: u64, asid: u32) {
    asm!(
    "invlpga {0}, {1:e}",
    in(reg) addr,
    in(reg) asid,
    options(nostack, preserves_flags),
    );
}
