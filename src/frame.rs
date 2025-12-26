use core::marker::PhantomData;

use axaddrspace::HostPhysAddr;
use axerrno::{AxResult, ax_err_type};

use axvcpu::AxVCpuHal;

pub(crate) use memory_addr::PAGE_SIZE_4K as PAGE_SIZE;

/// A 4K-sized contiguous physical memory page, it will deallocate the page
/// automatically on drop.
#[derive(Debug)]
pub struct PhysFrame<H: AxVCpuHal> {
    start_paddr: Option<HostPhysAddr>,
    _marker: PhantomData<H>,
}

impl<H: AxVCpuHal> PhysFrame<H> {
    pub fn alloc() -> AxResult<Self> {
        let start_paddr = H::alloc_frame()
            .ok_or_else(|| ax_err_type!(NoMemory, "allocate physical frame failed"))?;
        assert_ne!(start_paddr.as_usize(), 0);
        Ok(Self {
            start_paddr: Some(start_paddr),
            _marker: PhantomData,
        })
    }

    pub fn alloc_zero() -> AxResult<Self> {
        let mut f = Self::alloc()?;
        f.fill(0);
        Ok(f)
    }

    pub const unsafe fn uninit() -> Self {
        Self {
            start_paddr: None,
            _marker: PhantomData,
        }
    }

    pub fn start_paddr(&self) -> HostPhysAddr {
        self.start_paddr.expect("uninitialized PhysFrame")
    }

    pub fn as_mut_ptr(&self) -> *mut u8 {
        H::phys_to_virt(self.start_paddr()).as_mut_ptr()
    }

    pub fn fill(&mut self, byte: u8) {
        unsafe { core::ptr::write_bytes(self.as_mut_ptr(), byte, PAGE_SIZE) }
    }
}

impl<H: AxVCpuHal> Drop for PhysFrame<H> {
    fn drop(&mut self) {
        if let Some(start_paddr) = self.start_paddr {
            H::dealloc_frame(start_paddr);
            debug!("[AxVM] deallocated PhysFrame({:#x})", start_paddr);
        }
    }
}

/// A contiguous block of physical memory frames that will be automatically
/// deallocated when dropped. Used for hardware structures requiring contiguous
/// physical memory (e.g., IOPM, MSRPM).
#[derive(Debug)]
pub struct ContiguousPhysFrames<H: AxVCpuHal> {
    start_paddr: Option<HostPhysAddr>,
    frame_count: usize,
    _marker: PhantomData<H>,
}

impl<H: AxVCpuHal> ContiguousPhysFrames<H> {
    pub fn alloc(frame_count: usize) -> AxResult<Self> {
        let start_paddr = H::alloc_contiguous_frames(frame_count)
            .ok_or_else(|| ax_err_type!(NoMemory, "allocate contiguous frames failed"))?;

        assert_ne!(start_paddr.as_usize(), 0);
        Ok(Self {
            start_paddr: Some(start_paddr),
            frame_count,
            _marker: PhantomData,
        })
    }

    pub fn alloc_zero(frame_count: usize) -> AxResult<Self> {
        let mut frames = Self::alloc(frame_count)?;
        frames.fill(0);
        Ok(frames)
    }

    pub const unsafe fn uninit() -> Self {
        Self {
            start_paddr: None,
            frame_count: 0,
            _marker: PhantomData,
        }
    }


    pub fn start_paddr(&self) -> HostPhysAddr {
        self.start_paddr.expect("uninitialized ContiguousPhysFrames")
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    pub fn size(&self) -> usize {
        PAGE_SIZE * self.frame_count
    }


    pub fn as_mut_ptr(&self) -> *mut u8 {
        H::phys_to_virt(self.start_paddr()).as_mut_ptr()
    }


    pub fn fill(&mut self, byte: u8) {
        unsafe {
            core::ptr::write_bytes(self.as_mut_ptr(), byte, self.size());
        }
    }
}

impl<H: AxVCpuHal> Drop for ContiguousPhysFrames<H> {
    fn drop(&mut self) {
        if let Some(start_paddr) = self.start_paddr {
            H::dealloc_contiguous_frames(start_paddr, self.frame_count);
            debug!(
                "[AxVM] deallocated ContiguousPhysFrames({:#x}, {} frames)",
                start_paddr, self.frame_count
            );
        }
    }
}