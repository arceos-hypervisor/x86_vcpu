use x86::controlregs::{Xcr0, xcr0 as xcr0_read, xcr0_write};
use x86_64::registers::control::{Cr4, Cr4Flags};

use crate::msr::Msr;

#[derive(Debug, Clone, Copy, Default)]
pub struct XState {
    pub host_xcr0: u64,
    pub guest_xcr0: u64,
    pub host_xss: u64,
    pub guest_xss: u64,
}

impl XState {
    /// Create a new [`XState`] instance with current host state
    pub fn new() -> Self {
        let xcr0 = unsafe { xcr0_read().bits() };
        let xss = Msr::IA32_XSS.read();

        Self {
            host_xcr0: xcr0,
            guest_xcr0: xcr0,
            host_xss: xss,
            guest_xss: xss,
        }
    }

    /// Enables extended processor state management instructions, including XGETBV and XSAVE.
    pub fn enable_xsave() {
        unsafe { Cr4::write(Cr4::read() | Cr4Flags::OSXSAVE) };
    }
}
