use raw_cpuid::CpuId;
use x86::controlregs::{Xcr0, xcr0 as xcr0_read, xcr0_write};
use x86_64::registers::control::{Cr4, Cr4Flags};

use crate::msr::Msr;

/// Indicates the availability of extended processor state management features.
#[derive(Debug, Clone, Copy)]
pub struct XAvailable {
    /// Indicates if XSAVE (as well as xcr0) is available.
    pub xsave: bool,
    /// Indicates if XSAVES and XRSTORS (as well as IA32_XSS) are available.
    pub xsaves: bool,
}

impl XAvailable {
    /// Create a new [`XAvailable`] instance by querying the CPU features.
    pub fn new() -> Self {
        let xsave_avail = xsave_available();
        let xsaves_avail = xsave_avail && xsaves_available();

        Self {
            xsave: xsave_avail,
            xsaves: xsaves_avail,
        }
    }
}

/// Control and state registers for extended processor state management.
#[derive(Debug, Clone, Copy)]
pub struct XRegs {
    /// The xcr0 extended control register.
    pub xcr0: u64,
    /// The IA32_XSS model-specific register.
    pub xss: u64,
}

impl XRegs {
    /// Create a new [`XRegs`] instance by querying the current CPU state.
    pub fn new(avail: XAvailable) -> Self {
        let xcr0 = if avail.xsave {
            unsafe { xcr0_read().bits() }
        } else {
            0
        };

        let xss = if avail.xsaves {
            Msr::IA32_XSS.read()
        } else {
            0
        };

        Self { xcr0, xss }
    }

    /// Load the extended processor state registers from this instance.
    pub fn load(&self, avail: XAvailable) {
        unsafe {
            if avail.xsave {
                // info!("Loading XCR0: {:#x}", self.xcr0);
                xcr0_write(Xcr0::from_bits_unchecked(self.xcr0));

                if avail.xsaves {
                    Msr::IA32_XSS.write(self.xss);
                }
            }
        }
    }

    /// Save the current extended processor state registers into this instance.
    pub fn save(&mut self, avail: XAvailable) {
        unsafe {
            if avail.xsave {
                self.xcr0 = xcr0_read().bits();

                if avail.xsaves {
                    self.xss = Msr::IA32_XSS.read();
                }
            }
        }
    }
}

/// Extended processor state storage for vcpus.
#[derive(Debug)]
pub struct XState {
    pub host: XRegs,
    pub guest: XRegs,
    pub avail: XAvailable,
}

impl XState {
    /// Create a new [`XState`] instance with current host state.
    pub fn new() -> Self {
        let avail = XAvailable::new();
        let host = XRegs::new(avail);
        let guest = host;

        Self { host, guest, avail }
    }

    /// Save the current host XCR0 and IA32_XSS values and load the guest values.
    pub fn switch_to_guest(&mut self) {
        // info!("Switching to guest xstate");
        self.host.save(self.avail);
        // info!("Host xstate saved: {:?}", self.host);
        // info!("Guest xstate loading: {:?}", self.guest);
        self.guest.load(self.avail);
    }

    /// Save the current guest XCR0 and IA32_XSS values and load the host values.
    pub fn switch_to_host(&mut self) {
        // info!("Switching to host xstate");
        self.guest.save(self.avail);
        // info!("Guest xstate saved: {:?}", self.guest);
        // info!("Host xstate loading: {:?}", self.host);
        self.host.load(self.avail);
    }
}

/// Check if XSAVE is available on the current CPU.
pub fn xsave_available() -> bool {
    let cpuid = CpuId::new();
    cpuid
        .get_feature_info()
        .map(|f| f.has_xsave())
        .unwrap_or(false)
}

/// Check if XSAVES and XRSTORS (as well as IA32_XSS) are available on the current CPU.
pub fn xsaves_available() -> bool {
    let cpuid = CpuId::new();
    cpuid
        .get_extended_state_info()
        .map(|f| f.has_xsaves_xrstors())
        .unwrap_or(false)
}

/// Enable extended processor state management instructions, including XGETBV and XSAVE.
pub fn enable_xsave() {
    if xsave_available() {
        unsafe { Cr4::write(Cr4::read() | Cr4Flags::OSXSAVE) };
    }
}
