//! AMD-SVM helper structs
//! https://www.amd.com/content/dam/amd/en/documents/processor-tech-docs/programmer-references/24593.pdf

#![allow(dead_code)]

use axaddrspace::HostPhysAddr;
use axerrno::{AxResult};
use axvcpu::AxVCpuHal;
use memory_addr::PAGE_SIZE_4K as PAGE_SIZE;
use axaddrspace::PhysFrame;
use super::frame::{ContiguousPhysFrames};


/// Virtual-Machine Control Block (VMCB)
/// One 4 KiB page per vCPU: [control-area | save-area].
#[derive(Debug)]
pub struct VmcbFrame<H: AxVCpuHal> {
    page: PhysFrame<H::MmHal>,
}

impl<H: AxVCpuHal> VmcbFrame<H> {
    pub const unsafe fn uninit() -> Self {
        Self { page: unsafe { PhysFrame::uninit() } }
    }

    pub fn new() -> AxResult<Self> {
        Ok(Self { page: PhysFrame::alloc_zero()? })
    }

    pub fn phys_addr(&self) -> HostPhysAddr {
        self.page.start_paddr()
    }

    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.page.as_mut_ptr()
    }
}


// (AMD64 APM Vol.2, Section 15.10)
// The I/O Permissions Map (IOPM) occupies 12 Kbytes of contiguous physical memory.
// The map is structured as a linear array of 64K+3 bits (two 4-Kbyte pages, and the first three bits of a third 4-Kbyte page) and must be aligned on a 4-Kbyte boundary;
#[derive(Debug)]
pub struct IOPm<H: AxVCpuHal> {
    frames: ContiguousPhysFrames<H::MmHal>,  // 3 contiguous frames (12KB)
}

impl<H: AxVCpuHal> IOPm<H> {
    pub fn passthrough_all() -> AxResult<Self> {
        let mut frames = ContiguousPhysFrames::<H::MmHal>::alloc_zero(3)?;

        // Set first 3 bits of third frame to intercept (ports > 0xFFFF)
        let third_frame_start = frames.as_mut_ptr() as usize + 2 * PAGE_SIZE;
        unsafe {
            let third_byte = third_frame_start as *mut u8;
            *third_byte |= 0x07; // Set bits 0-2 (0b00000111)
        }

        Ok(Self { frames })
    }

    #[allow(unused)]
    pub fn intercept_all() -> AxResult<Self> {
        let mut frames = ContiguousPhysFrames::<H::MmHal>::alloc(3)?;
        frames.fill(0xFF); // Set all bits to 1 (intercept)
        Ok(Self { frames })
    }

    pub fn phys_addr(&self) -> HostPhysAddr {
        self.frames.start_paddr()
    }
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.frames.as_mut_ptr()
    }

    pub fn set_intercept(&mut self, port: u32, intercept: bool) {
        let byte_index = port as usize / 8;
        let bit_offset = (port % 8) as u8;
        let iopm_ptr = self.frames.as_mut_ptr();

        unsafe {
            let byte_ptr = iopm_ptr.add(byte_index);
            if intercept {
                *byte_ptr |= 1 << bit_offset;
            } else {
                *byte_ptr &= !(1 << bit_offset);
            }
        }
    }

    pub fn set_intercept_of_range(&mut self, port_base: u32, count: u32, intercept: bool) {
        for port in port_base..port_base + count {
            self.set_intercept(port, intercept)
        }
    }

}
// (AMD64 APM Vol.2, Section 15.10)
// The VMM can intercept RDMSR and WRMSR instructions by means of the SVM MSR permissions map (MSRPM) on a per-MSR basis
// The four separate bit vectors must be packed together and located in two contiguous physical pages of memory.
#[derive(Debug)]
pub struct MSRPm<H: AxVCpuHal> {
    frames: ContiguousPhysFrames<H::MmHal>,
}

impl<H: AxVCpuHal> MSRPm<H> {
    pub fn passthrough_all() -> AxResult<Self> {
        Ok(Self {
            frames: ContiguousPhysFrames::alloc_zero(2)?,
        })
    }

    #[allow(unused)]
    pub fn intercept_all() -> AxResult<Self> {
        let mut frames = ContiguousPhysFrames::alloc(2)?;
        frames.fill(0xFF);
        Ok(Self { frames })
    }

    pub fn phys_addr(&self) -> HostPhysAddr {
        self.frames.start_paddr()
    }
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.frames.as_mut_ptr()
    }

    pub fn set_intercept(&mut self, msr: u32, is_write: bool, intercept: bool) {
        let (segment, msr_low) = if msr <= 0x1fff {
            (0u32, msr)
        } else if (0xc000_0000..=0xc000_1fff).contains(&msr) {
            (1u32, msr & 0x1fff)
        } else if (0xc001_0000..=0xc001_1fff).contains(&msr) {
            (2u32, msr & 0x1fff)
        } else {
            unreachable!("MSR {:#x} Not supported by MSRPM", msr);
        };

        let base_offset      = (segment * 2048) as usize;

        let byte_in_segment  = (msr_low as usize) / 4;
        let bit_pair_offset  = ((msr_low & 0b11) * 2) as u8;      // 0,2,4,6
        let bit_offset       = bit_pair_offset + is_write as u8;  // +0=读, +1=写

        unsafe {
            let byte_ptr = self
                .frames
                .as_mut_ptr()
                .add(base_offset + byte_in_segment);

            let old = core::ptr::read_volatile(byte_ptr);
            let new = if intercept {
                old | (1u8 << bit_offset)
            } else {
                old & !(1u8 << bit_offset)
            };
            core::ptr::write_volatile(byte_ptr, new);
        }
    }

    pub fn set_read_intercept(&mut self, msr: u32, intercept: bool) {
        self.set_intercept(msr, false, intercept);
    }

    pub fn set_write_intercept(&mut self, msr: u32, intercept: bool) {
        self.set_intercept(msr, true, intercept);
    }

}