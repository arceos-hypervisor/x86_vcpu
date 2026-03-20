<h1 align="center">x86_vcpu</h1>

<p align="center">面向 ArceOS Hypervisor 的 x86 虚拟 CPU 实现</p>

<div align="center">

[![Crates.io](https://img.shields.io/crates/v/x86_vcpu.svg)](https://crates.io/crates/x86_vcpu)
[![Docs.rs](https://docs.rs/x86_vcpu/badge.svg)](https://docs.rs/x86_vcpu)
[![Rust](https://img.shields.io/badge/edition-2024-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/arceos-hypervisor/x86_vcpu/blob/main/LICENSE)

</div>

[English](README.md) | 中文

# Introduction

`x86_vcpu` 为 ArceOS hypervisor 栈提供 x86_64 虚拟 CPU 实现。它主要聚焦于基于 VMX 的虚拟化支持、寄存器状态管理、guest page walk 辅助信息，以及构建在 `axvcpu` 之上的 hypervisor 组件所需的 VM-exit 相关数据结构。

该库导出以下核心公开类型和函数：

- **`VmxArchVCpu`** - 基于 VMX 的架构相关 VCpu 实现
- **`VmxArchPerCpuState`** - VMX 的每 CPU 虚拟化状态
- **`GeneralRegisters`** - x86_64 通用寄存器集合抽象
- **`GuestPageWalkInfo`** - EPT 相关逻辑使用的 guest page walk 信息
- **`has_hardware_support()`** - 检测当前硬件是否支持 x86 虚拟化

当启用 `vmx` feature 时，该 crate 还会导出 `VmxExitInfo`、`VmxExitReason`、`VmxInterruptInfo`、`VmxIoExitInfo` 等 VMX 退出与中断辅助类型。

## Quick Start

### Requirements

- Rust nightly 工具链
- Rust 组件：rust-src、clippy、rustfmt

```bash
# 安装 rustup（如果尚未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 安装 nightly 工具链与所需组件
rustup install nightly
rustup component add rust-src clippy rustfmt --toolchain nightly
```

### Run Check and Test

```bash
# 1. 进入仓库目录
cd x86_vcpu

# 2. 代码检查
./scripts/check.sh

# 3. 运行测试
./scripts/test.sh
```

## Integration

### Installation

将以下依赖加入 `Cargo.toml`：

```toml
[dependencies]
x86_vcpu = "0.3.0"
```

### Example

```rust
use x86_vcpu::{has_hardware_support, GeneralRegisters};

fn main() {
    let vmx_available = has_hardware_support();
    println!("VMX available: {}", vmx_available);

    let mut regs = GeneralRegisters::default();
    regs.rax = 0x1234;
    regs.rbx = 0x5678;
    regs.rcx = 0x1000;
    regs.rdx = 0x2;

    assert_eq!(regs.rax, 0x1234);
    assert_eq!(regs.rbx, 0x5678);
    assert_eq!(GeneralRegisters::register_name(0), "rax");
}
```

### Documentation

生成并查看 API 文档：

```bash
cargo doc --no-deps --open
```

在线文档： [docs.rs/x86_vcpu](https://docs.rs/x86_vcpu)

# Contributing

1. Fork 仓库并创建分支
2. 本地运行检查：`./scripts/check.sh`
3. 本地运行测试：`./scripts/test.sh`
4. 提交 PR 并通过 CI 检查

# License

本项目基于 Apache License 2.0 许可证发布。详见 [LICENSE](https://github.com/arceos-hypervisor/x86_vcpu/blob/main/LICENSE)。
