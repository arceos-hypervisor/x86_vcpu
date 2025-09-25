# API参考

<cite>
**本文档中引用的文件**  
- [src/lib.rs](file://src/lib.rs)
- [src/vmx/vcpu.rs](file://src/vmx/vcpu.rs)
- [src/vmx/vmcs.rs](file://src/vmx/vmcs.rs)
- [src/vmx/percpu.rs](file://src/vmx/percpu.rs)
- [src/msr.rs](file://src/msr.rs)
- [src/regs/accessors.rs](file://src/regs/accessors.rs)
</cite>

## 目录
1. [简介](#简介)
2. [导出的类型与trait](#导出的类型与trait)
3. [VmxVcpu结构体方法](#vmxvcpu结构体方法)
4. [Vmcs结构体字段读写方法](#vmcs结构体字段读写方法)
5. [VmxPerCpuState方法](#vmxpercpustate方法)
6. [MSR寄存器操作API](#msr寄存器操作api)
7. [通用寄存器访问API](#通用寄存器访问api)

## 简介
`x86_vcpu`库为基于Intel VMX（虚拟机扩展）技术的虚拟CPU提供了底层支持。该库封装了VMX相关的硬件接口，包括虚拟机控制结构（VMCS）、模型特定寄存器（MSR）操作、以及虚拟CPU的状态管理。其主要功能是允许创建和管理运行在VMX非根模式下的虚拟CPU实例，并处理由虚拟化事件触发的VM退出（VM-exit）。本参考文档详细描述了所有公共API，重点关注`VmxVcpu`和`Vmcs`等核心组件。

## 导出的类型与trait
从`src/lib.rs`作为入口点，该库通过条件编译（`#[cfg(feature = "vmx")]`）导出了以下公共类型和trait：

- **类型 (Types)**:
  - `VmxExitInfo`: 包含VM退出的基本信息，如退出原因、指令长度和RIP。
  - `VmxExitReason`: 枚举了所有可能的VM退出原因（例如，外部中断、I/O指令、CR访问等）。
  - `VmxInterruptInfo`: 描述了用于注入或报告中断/异常的信息。
  - `VmxIoExitInfo`: 提供了因执行I/O指令而发生VM退出时的详细信息。
  - `GuestPageWalkInfo`: 在EPT违例时，提供页表遍历所需的信息。
  - `GeneralRegisters`: 表示x86_64架构下的一组通用寄存器。
  - `VmxArchVCpu`: `VmxVcpu<H>`的类型别名，代表一个VMX架构的虚拟CPU。
  - `VmxArchPerCpuState`: `VmxPerCpuState<H>`的类型别名，代表每个逻辑处理器的VMX状态。

- **Trait**:
  - `has_hardware_support()`: 一个函数，用于检测当前平台是否支持VMX硬件扩展。

这些导出项构成了库的公共接口，使调用者能够创建虚拟CPU、配置其行为并响应虚拟化事件。

**Section sources**
- [src/lib.rs](file://src/lib.rs#L0-L31)

## VmxVcpu结构体方法
`VmxVcpu`是表示单个虚拟CPU的核心结构体。它封装了VMCS、I/O位图、MSR位图以及虚拟CPU的运行时状态。

### new()
创建一个新的`VmxVcpu`实例。

- **函数签名**: `pub fn new(vm_id: VMId, vcpu_id: VCpuId) -> AxResult<Self>`
- **参数说明**:
  - `vm_id`: 虚拟机的唯一标识符。
  - `vcpu_id`: 此虚拟CPU在其所属虚拟机内的唯一标识符。
- **返回值定义**: 成功时返回`AxResult<Self>`，其中包含新创建的`VmxVcpu`实例；失败时返回`AxError`。
- **错误码**: 可能因内存分配失败等原因返回错误。
- **调用上下文限制**: 无特定CPU限制，但后续的`bind_to_current_processor`必须在目标CPU上执行。
- **使用示例**:
  ```rust
  let vm_id = VMId::new(1);
  let vcpu_id = VCpuId::new(0);
  let mut vcpu = VmxVcpu::new(vm_id, vcpu_id).unwrap();
  ```

### setup()
为虚拟CPU设置初始上下文，包括EPT根地址和入口点。

- **函数签名**: `pub fn setup(&mut self, ept_root: HostPhysAddr, entry: GuestPhysAddr) -> AxResult`
- **参数说明**:
  - `ept_root`: 指向嵌套分页（EPT）PML4表的主机物理地址。
  - `entry`: 虚拟机操作系统的入口点物理地址。
- **返回值定义**: `AxResult<()>`，表示操作成功或失败。
- **错误码**: 可能因VMCS配置失败而返回错误。
- **调用上下文限制**: 必须在调用`bind_to_current_processor`之后调用。
- **使用示例**:
  ```rust
  let ept_root_paddr = HostPhysAddr::from(0x100000);
  let guest_entry = GuestPhysAddr::from(0x1000);
  vcpu.setup(ept_root_paddr, guest_entry).unwrap();
  ```

### run()
启动或恢复虚拟CPU的执行。此方法会阻塞直到发生VM退出。

- **函数签名**: `pub fn inner_run(&mut self) -> Option<VmxExitInfo>`
- **参数说明**: 无。
- **返回值定义**: 返回`Option<VmxExitInfo>`。如果VM退出是由虚拟CPU自身处理的（如内部事件），则返回`None`；否则返回`Some(VmxExitInfo)`，其中包含了退出原因、指令长度等信息。
- **错误码**: 内部处理失败会导致panic。
- **调用上下文限制**: 必须在调用`bind_to_current_processor`之后调用。
- **使用示例**:
  ```rust
  loop {
      match vcpu.inner_run() {
          Some(exit_info) => {
              // 处理未被内部处理的VM退出
              handle_vm_exit(exit_info);
          }
          None => continue,
      }
  }
  ```

### register_accessors()
`VmxVcpu`提供了多种方法来访问和修改虚拟CPU的寄存器状态。

- **`regs()` 和 `regs_mut()`**: 分别返回对`guest_regs`（`GeneralRegisters`）的不可变和可变引用，用于直接访问所有通用寄存器。
- **`stack_pointer()` 和 `set_stack_pointer()`**: 获取和设置虚拟CPU的栈指针（RSP）。
- **`rip()`**: 获取虚拟CPU的指令指针（RIP）。
- **`advance_rip()`**: 将RIP向前推进指定的字节数，通常在模拟指令后调用。
- **`get_cpu_mode()`**: 推断当前虚拟CPU的运行模式（实模式、保护模式、兼容模式或64位模式）。

**Section sources**
- [src/vmx/vcpu.rs](file://src/vmx/vcpu.rs#L150-L900)

## Vmcs结构体字段读写方法
`Vmcs`模块通过一系列枚举（如`VmcsGuestNW`, `VmcsControl32`等）安全地封装了对VMCS字段的读写操作，避免了直接使用不安全的`vmread`/`vmwrite`指令。

### read_field / write_field 安全封装
这些方法通过为每个VMCS字段定义一个枚举变体来实现类型安全。

- **函数签名**:
  - `impl VmcsFieldEnum { pub fn read(self) -> AxResult<T> }`
  - `impl VmcsFieldEnum { pub fn write(self, value: T) -> AxResult }`
- **参数说明**: `self`是`VmcsControl32`、`VmcsGuest64`等枚举的一个实例，代表一个具体的VMCS字段。
- **返回值定义**: `AxResult<T>`，其中`T`是该字段的数据类型（如`u32`, `u64`, `usize`）。读取成功返回字段值，写入成功返回`Ok(())`。
- **错误码**: 如果`vmread`或`vmwrite`指令失败（例如，由于无效的VMCS指针或尝试写入只读字段），将返回`AxError`。具体错误信息可通过`vmcs::instruction_error()`获取。
- **调用上下文限制**: 必须在`VMPTRLD`指令加载了有效的VMCS之后才能调用。
- **使用示例**:
  ```rust
  // 读取VMCS中的GUEST_RIP
  let guest_rip = VmcsGuestNW::RIP.read().unwrap();
  // 写入新的GUEST_RIP
  VmcsGuestNW::RIP.write(new_rip_value).unwrap();
  ```

**Section sources**
- [src/vmx/vmcs.rs](file://src/vmx/vmcs.rs#L100-L500)

## VmxPerCpuState方法
`VmxPerCpuState`管理每个逻辑处理器上的全局VMX状态，负责启用和禁用VMX操作。

### create()
创建一个新的`VmxPerCpuState`实例。

- **函数签名**: `fn new(_cpu_id: usize) -> AxResult<Self>`
- **参数说明**: `_cpu_id`: 逻辑处理器的ID。
- **返回值定义**: `AxResult<Self>`，成功时返回新实例。
- **错误码**: 通常不会在此阶段失败。
- **调用上下文限制**: 无。

### enable_vmxe()
协同完成VMX的全局与局部启用。此方法首先检查硬件支持，然后配置必要的控制寄存器和MSR，最后执行`VMXON`指令。

- **函数签名**: `fn hardware_enable(&mut self) -> AxResult`
- **参数说明**: 无。
- **返回值定义**: `AxResult<()>`。
- **错误码**:
  - `Unsupported`: CPU不支持VMX，或BIOS禁用了VMX。
  - `ResourceBusy`: VMX已在当前处理器上启用。
  - `BadState`: 主机的CR0或CR4寄存器状态不符合VMX要求。
- **调用上下文限制**: 必须在单个逻辑处理器上执行，且不能在中断上下文中调用。
- **使用示例**:
  ```rust
  let mut per_cpu_state = VmxPerCpuState::new(0).unwrap();
  per_cpu_state.hardware_enable().unwrap(); // 启用VMX
  // ... 创建和运行VmxVcpu ...
  per_cpu_state.hardware_disable().unwrap(); // 禁用VMX
  ```

**Section sources**
- [src/vmx/percpu.rs](file://src/vmx/percpu.rs#L50-L200)

## MSR寄存器操作API
`msr.rs`模块提供了对模型特定寄存器（MSR）的安全访问。

### Msr 枚举
定义了一个`Msr`枚举，列出了所有受支持的MSR，例如：
- `IA32_VMX_BASIC`: 报告基本的VMX能力。
- `IA32_VMX_PINBASED_CTLS`: 针脚为基础的VM执行控制。
- `IA32_EFER`: 扩展功能启用寄存器。

### read() 和 write()
为`Msr`枚举实现了`read`和`write`方法。

- **函数签名**:
  - `pub fn read(self) -> u64`
  - `pub unsafe fn write(self, value: u64)`
- **参数说明**: `value`: 要写入MSR的64位值。
- **返回值定义**: `read`返回64位的MSR值。
- **错误码**: 无显式错误码，但`write`是`unsafe`的，调用者需确保操作的安全性。
- **调用上下文限制**: `read`可在任何特权级调用；`write`通常需要在ring 0执行。
- **使用示例**:
  ```rust
  let vmx_basic = Msr::IA32_VMX_BASIC.read();
  let revision_id = vmx_basic as u32;
  ```

**Section sources**
- [src/msr.rs](file://src/msr.rs#L50-L150)

## 通用寄存器访问API
`regs`模块提供了对通用寄存器的高级访问接口。

### GeneralRegisters 结构体
`GeneralRegisters`是一个包含所有x86_64通用寄存器（rax, rbx, ..., r15）的结构体。

### 访问器方法
通过宏生成了一系列getter和setter方法，以支持不同大小的寄存器访问。

- **函数签名示例**:
  - `pub fn eax(&self) -> u32`
  - `pub fn set_eax(&mut self, value: u32)`
  - `pub fn al(&self) -> u8`
  - `pub fn set_al(&mut self, value: u8)`
- **参数说明**: `value`: 要设置的寄存器值。
- **返回值定义**: 返回指定大小的寄存器值。
- **错误码**: 无。
- **调用上下文限制**: 无。
- **使用示例**:
  ```rust
  let mut regs = GeneralRegisters::default();
  regs.set_rax(0x123456789ABCDEF0);
  assert_eq!(regs.eax(), 0x9ABCDEF0); // 获取低32位
  assert_eq!(regs.al(), 0xF0);        // 获取低8位
  ```

**Section sources**
- [src/regs/accessors.rs](file://src/regs/accessors.rs#L50-L200)