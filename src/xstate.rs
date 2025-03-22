use x86::controlregs::{Xcr0, xcr0 as xcr0_read, xcr0_write};
use x86_64::registers::control::{Cr4, Cr4Flags};

use crate::msr::Msr;

#[derive(Debug, Clone, Copy)]
pub struct XState {
    pub xcr0: Xcr0,
    pub xss: u64,
}

impl Default for XState {
    fn default() -> Self {
        Self {
            xcr0: Xcr0::empty(),
            xss: 0,
        }
    }
}

impl XState {
    /// Create a new [`XState`] instance with current host state
    pub fn new() -> Self {
        Self {
            xcr0: unsafe { xcr0_read() },
            xss: Msr::IA32_XSS.read(),
        }
    }

    pub fn save(&mut self) {
        self.xcr0 = unsafe { xcr0_read() };
        self.xss = Msr::IA32_XSS.read();
        warn!("XState::save: xcr0: {:?}, xss: {:#x}", self.xcr0, self.xss);
    }

    pub fn restore(&self) {
        warn!(
            "XState::restore: xcr0: {:?}, xss: {:#x}",
            self.xcr0, self.xss
        );
        unsafe {
            xcr0_write(self.xcr0);
            Msr::IA32_XSS.write(self.xss);
        }
    }

    /// Enables extended processor state management instructions, including XGETBV and XSAVE.
    pub fn enable_xsave() {
        unsafe { Cr4::write(Cr4::read() | Cr4Flags::OSXSAVE) };
    }
}
