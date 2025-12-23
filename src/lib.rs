#![no_std]
#![feature(doc_cfg)]
#![cfg(target_arch = "x86_64")]
#![doc = include_str!("../README.md")]

#[macro_use]
extern crate log;

extern crate alloc;
use alloc::string::String;

use thiserror::Error;

#[cfg(test)]
mod test_utils;

pub(crate) mod msr;
#[macro_use]
pub(crate) mod regs;
mod ept;
pub(crate) mod mem;

cfg_if::cfg_if! {
    if #[cfg(feature = "vmx")] {
        mod vmx;
        use vmx as vender;
        pub use vmx::{VmxExitInfo, VmxRawExitReason, VmxInterruptInfo, VmxIoExitInfo};

        pub use vender::VmxArchVCpu;
        pub use vender::VmxArchPerCpuState;
    }
}

pub use ept::GuestPageWalkInfo;
pub use regs::GeneralRegisters;
pub use vender::has_hardware_support;

pub type HostVirtAddr = usize;
pub type HostPhysAddr = usize;

/// x86 VCPU 错误类型
#[derive(Error, Debug)]
pub enum VmxError {
    /// VMX 指令错误
    #[error("VMX instruction error: {0}")]
    VmxInstructionError(String),

    /// VMCS 指针无效
    #[error("VMCS pointer is not valid")]
    InvalidVmcsPtr,

    /// VMX 未被启用
    #[error("VMX is not enabled")]
    VmxNotEnabled,

    /// VMX 已被启用
    #[error("VMX is already enabled")]
    VmxAlreadyEnabled,

    /// 内存分配失败
    #[error("memory allocation failed")]
    MemoryAllocationFailed,

    /// 无效的物理地址
    #[error("invalid physical address: {0:#x}")]
    InvalidPhysAddr(usize),

    /// 无效的虚拟地址
    #[error("invalid virtual address: {0:#x}")]
    InvalidVirtAddr(usize),

    /// 无效的 VMCS 配置
    #[error("invalid VMCS configuration: {0}")]
    InvalidVmcsConfig(String),

    /// EPT 违规
    #[error("EPT violation at GPA {0:#x}, error code {1:#x}")]
    EptViolation(usize, u64),

    /// IO 指令错误
    #[error("IO instruction error: port={0:#x}, width={1}")]
    IoError(u16, u8),

    /// MSR 访问错误
    #[error("MSR access error: {0:#x}")]
    MsrError(u32),

    /// VCpu 未绑定
    #[error("VCpu is not bound to current CPU")]
    VCPUNotBound,

    /// 不支持的 VMX 功能
    #[error("unsupported VMX feature: {0}")]
    UnsupportedFeature(String),

    /// 其他 VMX 错误
    #[error("VMX error: {0}")]
    Other(String),
}

/// x86 VCPU Result 类型
pub type Result<T> = core::result::Result<T, VmxError>;

/// Hardware abstraction layer for memory management.
pub trait Hal {
    /// Allocates a frame and returns its host physical address. The
    ///
    /// # Returns
    ///
    /// * `Option<HostPhysAddr>` - Some containing the physical address of the allocated frame, or None if allocation fails.
    fn alloc_frame() -> Option<HostPhysAddr>;

    /// Deallocates a frame given its physical address.
    ///
    /// # Parameters
    ///
    /// * `paddr` - The physical address of the frame to deallocate.
    fn dealloc_frame(paddr: HostPhysAddr);

    /// Converts a host physical address to a host virtual address.
    ///
    /// # Parameters
    ///
    /// * `paddr` - The physical address to convert.
    ///
    /// # Returns
    ///
    /// * `HostVirtAddr` - The corresponding virtual address.
    fn phys_to_virt(paddr: HostPhysAddr) -> HostVirtAddr;

    /// Converts a host virtual address to a host physical address.
    ///
    /// # Parameters
    ///
    /// * `vaddr` - The virtual address to convert.
    ///
    /// # Returns
    ///
    /// * `HostPhysAddr` - The corresponding physical address.
    fn virt_to_phys(vaddr: HostVirtAddr) -> HostPhysAddr;
}

// ==================== x86 特定的 VCPU 退出原因定义 ====================
// 这些类型替代 axvcpu 中的定义，使 x86_vcpu 完全独立

use axaddrspace::{
    GuestPhysAddr,
    device::{AccessWidth, Port, SysRegAddr},
};

/// x86 VCPU 退出原因
#[derive(Debug)]
pub enum VmxExitReason {
    /// 超级调用
    Hypercall { nr: usize, args: [usize; 8] },
    /// IO 读
    IoRead { port: Port, width: AccessWidth },
    /// IO 写
    IoWrite { port: Port, width: AccessWidth, data: u32 },
    /// 系统寄存器读
    SysRegRead { addr: SysRegAddr, reg: usize },
    /// 系统寄存器写
    SysRegWrite { addr: SysRegAddr, value: u64 },
    /// 外部中断
    ExternalInterrupt { vector: usize },
    /// CPU 启动
    CpuUp { target_cpu: usize, entry_point: GuestPhysAddr, arg: u64 },
    /// CPU 关闭
    CpuDown { state: usize },
    /// 系统关闭
    SystemDown,
    /// 无操作
    Nothing,
}

