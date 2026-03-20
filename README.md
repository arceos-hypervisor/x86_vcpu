<h1 align="center">x86_vcpu</h1>

<p align="center">x86 Virtual CPU Implementation for ArceOS Hypervisor</p>

<div align="center">

[![Crates.io](https://img.shields.io/crates/v/x86_vcpu.svg)](https://crates.io/crates/x86_vcpu)
[![Docs.rs](https://docs.rs/x86_vcpu/badge.svg)](https://docs.rs/x86_vcpu)
[![Rust](https://img.shields.io/badge/edition-2024-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/arceos-hypervisor/x86_vcpu/blob/main/LICENSE)

</div>

English | [中文](README_CN.md)

# Introduction

`x86_vcpu` provides the x86_64 virtual CPU implementation for the ArceOS hypervisor stack. It focuses on VMX-based virtualization support, register state management, guest page-walk helpers, and VM-exit related data structures for hypervisor components built on top of `axvcpu`.

This library exports the following core public types and functions:

- **`VmxArchVCpu`** - VMX-based architecture-specific VCpu implementation
- **`VmxArchPerCpuState`** - Per-CPU virtualization state for VMX
- **`GeneralRegisters`** - x86_64 general-purpose register set abstraction
- **`GuestPageWalkInfo`** - Guest page-walk information used by EPT-related logic
- **`has_hardware_support()`** - Detects whether x86 virtualization support is available

When the `vmx` feature is enabled, the crate also exports VMX exit and interrupt helper types such as `VmxExitInfo`, `VmxExitReason`, `VmxInterruptInfo`, and `VmxIoExitInfo`.

## Quick Start

### Requirements

- Rust nightly toolchain
- Rust components: rust-src, clippy, rustfmt

```bash
# Install rustup (if not installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install nightly toolchain and components
rustup install nightly
rustup component add rust-src clippy rustfmt --toolchain nightly
```

### Run Check and Test

```bash
# 1. Enter the repository
cd x86_vcpu

# 2. Code check
./scripts/check.sh

# 3. Run tests
./scripts/test.sh
```

## Integration

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
x86_vcpu = "0.3.0"
```

### Example

```rust,ignore
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

Generate and view API documentation:

```bash
cargo doc --no-deps --open
```

Online documentation: [docs.rs/x86_vcpu](https://docs.rs/x86_vcpu)

# Contributing

1. Fork the repository and create a branch
2. Run local check: `./scripts/check.sh`
3. Run local tests: `./scripts/test.sh`
4. Submit PR and pass CI checks

# License

Licensed under the Apache License, Version 2.0. See [LICENSE](https://github.com/arceos-hypervisor/x86_vcpu/blob/main/LICENSE) for details.
