//! x86-64 Instruction Length Calculator for EPT Violation Handling
//!
//! This module provides a lightweight x86-64 instruction length calculator
//! specifically designed to handle EPT violations where modern processors
//! don't provide reliable exit_instruction_length values.

use axerrno::{AxResult, AxError, ax_err, ax_err_type};
use axaddrspace::GuestVirtAddr;
use crate::vmx::VmCpuMode;

/// Maximum instruction length in x86-64 (15 bytes)
const MAX_INSTRUCTION_LENGTH: usize = 15;

/// Legacy instruction prefixes
#[derive(Debug, Clone, Copy)]
pub enum LegacyPrefix {
    /// Lock prefix (0xF0)
    Lock = 0xF0,
    /// REPNE/REPNZ prefix (0xF2)
    RepNe = 0xF2,
    /// REP/REPE/REPZ prefix (0xF3)
    Rep = 0xF3,
    /// CS segment override (0x2E)
    CsOverride = 0x2E,
    /// SS segment override (0x36)
    SsOverride = 0x36,
    /// DS segment override (0x3E)
    DsOverride = 0x3E,
    /// ES segment override (0x26)
    EsOverride = 0x26,
    /// FS segment override (0x64)
    FsOverride = 0x64,
    /// GS segment override (0x65)
    GsOverride = 0x65,
    /// Operand size override (0x66)
    OperandSizeOverride = 0x66,
    /// Address size override (0x67)
    AddressSizeOverride = 0x67,
}

/// REX prefix structure for 64-bit mode
#[derive(Debug, Clone, Copy, Default)]
pub struct RexPrefix {
    /// Extension of the ModRM reg field
    pub r: bool,
    /// Extension of the SIB index field
    pub x: bool,
    /// Extension of the ModRM r/m field, SIB base field, or Opcode reg field
    pub b: bool,
    /// 64-bit operand size (0 = default operand size, 1 = 64-bit operand size)
    pub w: bool,
}

impl RexPrefix {
    /// Parse REX prefix from byte
    pub fn from_byte(byte: u8) -> Option<Self> {
        if byte & 0xF0 == 0x40 {
            Some(Self {
                w: (byte & 0x08) != 0,
                r: (byte & 0x04) != 0,
                x: (byte & 0x02) != 0,
                b: (byte & 0x01) != 0,
            })
        } else {
            None
        }
    }
}

/// VEX prefix structure
#[derive(Debug, Clone, Copy)]
pub struct VexPrefix {
    /// Length of VEX prefix (2 or 3 bytes)
    pub length: usize,
    /// Vector length (0 = 128-bit, 1 = 256-bit)
    pub l: bool,
    /// Source register specifier
    pub vvvv: u8,
    /// Operand type
    pub w: bool,
}

/// Instruction prefix information
#[derive(Debug, Clone, Default)]
pub struct PrefixInfo {
    /// Legacy prefixes found
    pub legacy_prefixes: [Option<LegacyPrefix>; 4],
    /// REX prefix (64-bit mode only)
    pub rex: Option<RexPrefix>,
    /// VEX prefix (AVX instructions)
    pub vex: Option<VexPrefix>,
    /// Total prefix length in bytes
    pub total_length: usize,
}

/// ModR/M byte structure
#[derive(Debug, Clone, Copy)]
pub struct ModRm {
    /// Mode field (2 bits)
    pub mode: u8,
    /// Register/opcode field (3 bits) 
    pub reg: u8,
    /// R/M field (3 bits)
    pub rm: u8,
}

impl ModRm {
    /// Parse ModR/M from byte
    pub fn from_byte(byte: u8) -> Self {
        Self {
            mode: (byte >> 6) & 0x03,
            reg: (byte >> 3) & 0x07,
            rm: byte & 0x07,
        }
    }

    /// Check if SIB byte is required
    pub fn needs_sib(&self) -> bool {
        self.mode != 3 && self.rm == 4
    }

    /// Calculate displacement length in bytes
    pub fn displacement_length(&self, address_size: u8) -> usize {
        match self.mode {
            0 => {
                if self.rm == 5 {
                    // Direct addressing (RIP-relative in 64-bit, absolute in 32-bit)
                    address_size as usize
                } else {
                    0
                }
            }
            1 => 1, // 8-bit displacement
            2 => address_size as usize, // 32-bit displacement
            3 => 0, // Register addressing
            _ => unreachable!(),
        }
    }
}

/// SIB (Scale-Index-Base) byte structure
#[derive(Debug, Clone, Copy)]
pub struct Sib {
    /// Scale field (2 bits)
    pub scale: u8,
    /// Index field (3 bits)
    pub index: u8,
    /// Base field (3 bits)
    pub base: u8,
}

impl Sib {
    /// Parse SIB from byte
    pub fn from_byte(byte: u8) -> Self {
        Self {
            scale: (byte >> 6) & 0x03,
            index: (byte >> 3) & 0x07,
            base: byte & 0x07,
        }
    }

    /// Check if additional displacement is needed
    pub fn needs_displacement(&self, modrm: &ModRm) -> bool {
        self.base == 5 && (modrm.mode == 0 || modrm.mode == 2)
    }
}

/// Instruction operand size and address size information
#[derive(Debug, Clone, Copy)]
pub struct SizeInfo {
    /// Operand size in bytes (1, 2, 4, or 8)
    pub operand_size: u8,
    /// Address size in bytes (2, 4, or 8) 
    pub address_size: u8,
}

impl SizeInfo {
    /// Calculate sizes based on CPU mode and prefixes
    pub fn calculate(cpu_mode: VmCpuMode, prefix_info: &PrefixInfo) -> Self {
        let (default_operand, default_address) = match cpu_mode {
            VmCpuMode::Real => (2, 2),
            VmCpuMode::Protected => (4, 4),
            VmCpuMode::Mode64 => (4, 8), // Default 32-bit operands, 64-bit addresses
            VmCpuMode::Compatibility => (4, 4),
        };

        let mut operand_size = default_operand;
        let mut address_size = default_address;

        // Check for operand size override
        if prefix_info.legacy_prefixes.iter().any(|p| {
            matches!(p, Some(LegacyPrefix::OperandSizeOverride))
        }) {
            operand_size = match cpu_mode {
                VmCpuMode::Real | VmCpuMode::Protected | VmCpuMode::Compatibility => {
                    if default_operand == 2 { 4 } else { 2 }
                }
                VmCpuMode::Mode64 => 2, // 66h prefix gives 16-bit operands in 64-bit mode
            };
        }

        // Check for address size override
        if prefix_info.legacy_prefixes.iter().any(|p| {
            matches!(p, Some(LegacyPrefix::AddressSizeOverride))
        }) {
            address_size = match cpu_mode {
                VmCpuMode::Real => 4, // 67h in real mode gives 32-bit addressing
                VmCpuMode::Protected | VmCpuMode::Compatibility => {
                    if default_address == 2 { 4 } else { 2 }
                }
                VmCpuMode::Mode64 => 4, // 67h in 64-bit mode gives 32-bit addressing
            };
        }

        // REX.W overrides operand size to 64-bit in long mode
        if let Some(rex) = prefix_info.rex {
            if rex.w && cpu_mode == VmCpuMode::Mode64 {
                operand_size = 8;
            }
        }

        Self {
            operand_size,
            address_size,
        }
    }
}

/// Parse instruction prefixes
fn parse_prefixes(bytes: &[u8]) -> AxResult<PrefixInfo> {
    let mut info = PrefixInfo::default();
    let mut pos = 0;

    // Parse legacy prefixes (up to 4)
    let mut legacy_count = 0;
    while pos < bytes.len() && legacy_count < 4 {
        let byte = bytes[pos];
        
        let prefix = match byte {
            0xF0 => Some(LegacyPrefix::Lock),
            0xF2 => Some(LegacyPrefix::RepNe),
            0xF3 => Some(LegacyPrefix::Rep),
            0x2E => Some(LegacyPrefix::CsOverride),
            0x36 => Some(LegacyPrefix::SsOverride),
            0x3E => Some(LegacyPrefix::DsOverride),
            0x26 => Some(LegacyPrefix::EsOverride),
            0x64 => Some(LegacyPrefix::FsOverride),
            0x65 => Some(LegacyPrefix::GsOverride),
            0x66 => Some(LegacyPrefix::OperandSizeOverride),
            0x67 => Some(LegacyPrefix::AddressSizeOverride),
            _ => None,
        };

        if let Some(prefix) = prefix {
            info.legacy_prefixes[legacy_count] = Some(prefix);
            legacy_count += 1;
            pos += 1;
        } else {
            break;
        }
    }

    // Check for REX prefix (only in 64-bit mode)
    if pos < bytes.len() {
        if let Some(rex) = RexPrefix::from_byte(bytes[pos]) {
            info.rex = Some(rex);
            pos += 1;
        }
    }

    // TODO: Add VEX prefix parsing if needed

    info.total_length = pos;
    Ok(info)
}

/// Calculate instruction length using lightweight x86-64 decoder
pub fn calculate_instruction_length(
    instruction_bytes: &[u8],
    cpu_mode: VmCpuMode,
) -> AxResult<usize> {
    if instruction_bytes.is_empty() {
        return Err(AxError::InvalidInput);
    }

    if instruction_bytes.len() > MAX_INSTRUCTION_LENGTH {
        return Err(AxError::InvalidInput);
    }

    // Parse prefixes
    let prefix_info = parse_prefixes(instruction_bytes)?;
    let mut pos = prefix_info.total_length;

    if pos >= instruction_bytes.len() {
        return Err(AxError::InvalidInput);
    }

    // Calculate operand and address sizes
    let size_info = SizeInfo::calculate(cpu_mode, &prefix_info);

    // Parse opcode
    let opcode = instruction_bytes[pos];
    pos += 1;

    // Handle escape opcodes (0x0F prefix)
    let mut is_two_byte_opcode = false;
    if opcode == 0x0F {
        if pos >= instruction_bytes.len() {
            return Err(AxError::InvalidInput);
        }
        is_two_byte_opcode = true;
        pos += 1; // Skip the second opcode byte for now
    }

    // Parse ModR/M byte if needed
    let needs_modrm = instruction_needs_modrm(opcode, is_two_byte_opcode);
    let mut modrm = None;
    let mut sib = None;
    let mut displacement_length = 0;

    if needs_modrm {
        if pos >= instruction_bytes.len() {
            return Err(AxError::InvalidInput);
        }

        let modrm_byte = ModRm::from_byte(instruction_bytes[pos]);
        pos += 1;

        // Parse SIB byte if needed
        if modrm_byte.needs_sib() {
            if pos >= instruction_bytes.len() {
                return Err(AxError::InvalidInput);
            }
            
            let sib_byte = Sib::from_byte(instruction_bytes[pos]);
            pos += 1;

            // Calculate displacement length
            displacement_length = modrm_byte.displacement_length(size_info.address_size);
            if sib_byte.needs_displacement(&modrm_byte) {
                displacement_length = size_info.address_size as usize;
            }

            sib = Some(sib_byte);
        } else {
            displacement_length = modrm_byte.displacement_length(size_info.address_size);
        }

        modrm = Some(modrm_byte);
    }

    // Skip displacement bytes
    pos += displacement_length;

    // Calculate immediate operand length
    let immediate_length = calculate_immediate_length(opcode, is_two_byte_opcode, &size_info, modrm.as_ref())?;
    pos += immediate_length;

    if pos > MAX_INSTRUCTION_LENGTH {
        return Err(AxError::InvalidInput);
    }

    Ok(pos)
}

/// Check if opcode requires ModR/M byte
fn instruction_needs_modrm(opcode: u8, is_two_byte: bool) -> bool {
    if is_two_byte {
        // Most two-byte opcodes need ModR/M
        // TODO: Add specific exceptions
        true
    } else {
        match opcode {
            // Instructions that don't use ModR/M
            0x06 | 0x07 | 0x0E | 0x16 | 0x17 | 0x1E | 0x1F => false, // PUSH/POP segment registers
            0x27 | 0x2F | 0x37 | 0x3F => false, // Decimal adjust, AAS, AAA, AAS
            0x40..=0x4F => false, // REX prefixes / INC/DEC in 32-bit mode
            0x50..=0x5F => false, // PUSH/POP general registers
            0x60 | 0x61 => false, // PUSHA/POPA
            0x70..=0x7F => false, // Short conditional jumps
            0x90..=0x97 => false, // NOP, XCHG
            0x98..=0x9F => false, // CBW, CWD, etc.
            0xA0..=0xA3 => false, // MOV AL/AX/EAX/RAX, moffs
            0xA4..=0xA7 => false, // MOVS, CMPS
            0xA8..=0xAF => false, // TEST, STOS, LODS, SCAS
            0xB0..=0xBF => false, // MOV immediate to register
            0xC2 | 0xC3 | 0xCA | 0xCB => false, // RET
            0xCC..=0xCE => false, // INT
            0xCF => false, // IRET
            0xD4 | 0xD5 => false, // AAM, AAD
            0xE0..=0xE3 => false, // LOOP, JCXZ
            0xE4..=0xE7 => false, // IN, OUT immediate
            0xE8 | 0xE9 => false, // CALL, JMP relative
            0xEB => false, // JMP short
            0xEC..=0xEF => false, // IN, OUT DX
            0xF1 | 0xF4 | 0xF5 | 0xF8..=0xFD => false, // Single-byte instructions
            _ => true, // Default: needs ModR/M
        }
    }
}

/// Calculate immediate operand length
fn calculate_immediate_length(
    opcode: u8,
    is_two_byte: bool,
    size_info: &SizeInfo,
    _modrm: Option<&ModRm>,
) -> AxResult<usize> {
    if is_two_byte {
        // TODO: Implement two-byte opcode immediate calculation
        return Ok(0);
    }

    match opcode {
        // No immediate
        0x00..=0x3F if ![0x04, 0x05, 0x0C, 0x0D, 0x14, 0x15, 0x1C, 0x1D, 0x24, 0x25, 0x2C, 0x2D, 0x34, 0x35, 0x3C, 0x3D].contains(&opcode) => Ok(0),
        
        // 8-bit immediate
        0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => Ok(1), // ALU ops with AL
        0x6A => Ok(1), // PUSH imm8
        0x70..=0x7F => Ok(1), // Short conditional jumps
        0x80 | 0x82 | 0x83 => Ok(1), // ALU ops with imm8
        0xA8 => Ok(1), // TEST AL, imm8
        0xB0..=0xB7 => Ok(1), // MOV reg8, imm8
        0xC0 | 0xC1 => Ok(1), // Shift/rotate with imm8
        0xC6 => Ok(1), // MOV r/m8, imm8
        0xCD => Ok(1), // INT imm8
        0xD0..=0xD3 => Ok(0), // Shift/rotate by 1 or CL
        0xEB => Ok(1), // JMP short

        // 16/32/64-bit immediate (depends on operand size)
        0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => Ok(size_info.operand_size as usize), // ALU ops with EAX
        0x68 => Ok(size_info.operand_size as usize), // PUSH imm
        0x69 => Ok(size_info.operand_size as usize), // IMUL with imm
        0x81 => Ok(size_info.operand_size as usize), // ALU ops with imm
        0xA9 => Ok(size_info.operand_size as usize), // TEST EAX, imm
        0xB8..=0xBF => Ok(size_info.operand_size as usize), // MOV reg, imm
        0xC7 => Ok(size_info.operand_size as usize), // MOV r/m, imm
        0xE8 | 0xE9 => Ok(size_info.operand_size as usize), // CALL/JMP relative

        // Special cases
        0xC2 | 0xCA => Ok(2), // RET with 16-bit immediate
        0xC8 => Ok(3), // ENTER (16-bit level + 8-bit level)

        _ => Ok(0), // Default: no immediate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_mov_instruction() {
        // MOV EAX, EBX (89 D8)
        let bytes = [0x89, 0xD8];
        let length = calculate_instruction_length(&bytes, VmCpuMode::Mode64).unwrap();
        assert_eq!(length, 2);
    }

    #[test]
    fn test_mov_with_immediate() {
        // MOV EAX, 0x12345678 (B8 78 56 34 12)
        let bytes = [0xB8, 0x78, 0x56, 0x34, 0x12];
        let length = calculate_instruction_length(&bytes, VmCpuMode::Mode64).unwrap();
        assert_eq!(length, 5);
    }

    #[test]
    fn test_rex_prefix() {
        // REX.W + MOV RAX, RBX (48 89 D8)
        let bytes = [0x48, 0x89, 0xD8];
        let length = calculate_instruction_length(&bytes, VmCpuMode::Mode64).unwrap();
        assert_eq!(length, 3);
    }

    #[test]
    fn test_memory_operand() {
        // MOV EAX, [RBX] (8B 03)
        let bytes = [0x8B, 0x03];
        let length = calculate_instruction_length(&bytes, VmCpuMode::Mode64).unwrap();
        assert_eq!(length, 2);
    }

    #[test]
    fn test_memory_with_displacement() {
        // MOV EAX, [RBX+0x12345678] (8B 83 78 56 34 12)
        let bytes = [0x8B, 0x83, 0x78, 0x56, 0x34, 0x12];
        let length = calculate_instruction_length(&bytes, VmCpuMode::Mode64).unwrap();
        assert_eq!(length, 6);
    }

    #[test]
    fn test_prefix_combinations() {
        // LOCK REP MOV EAX, EBX (F0 F3 89 D8)
        let bytes = [0xF0, 0xF3, 0x89, 0xD8];
        let length = calculate_instruction_length(&bytes, VmCpuMode::Mode64).unwrap();
        assert_eq!(length, 4);
    }
}
