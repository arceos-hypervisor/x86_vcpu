//! Used to query and manipulate the page tables of a guest.
use core::marker::PhantomData;

use memory_addr::MemoryAddr;
use page_table_entry::{GenericPTE, MappingFlags};
use page_table_multiarch::{PageSize, PagingError, PagingHandler, PagingResult};

use axaddrspace::{EPTTranslator, GuestPhysAddr, GuestVirtAddr};

const fn p5_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 36)) & (ENTRY_COUNT - 1)
}

const fn p4_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 27)) & (ENTRY_COUNT - 1)
}

const fn p3_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 18)) & (ENTRY_COUNT - 1)
}

const fn p2_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 9)) & (ENTRY_COUNT - 1)
}

const fn p1_index(vaddr: usize) -> usize {
    (vaddr >> 12) & (ENTRY_COUNT - 1)
}

#[derive(Debug)]
/// The information of guest page walk.
pub struct GuestPageWalkInfo {
    /// Guest VM cr3 value.
    pub cr3: usize,
    /// Guest page table level.
    pub level: usize,
    /// Guest page table width
    pub width: u32,
    /// Guest page table user mode
    pub is_user_mode_access: bool,
    /// Guest page table write access
    pub is_write_access: bool,
    /// Guest page table instruction fetch
    pub is_inst_fetch: bool,
    /// CR4.PSE for 32bit paging, true for PAE/4-level paging
    pub pse: bool,
    /// CR0.WP
    pub wp: bool, // CR0.WP
    /// MSR_IA32_EFER_NXE_BIT
    pub nxe: bool,

    /// Guest page table Supervisor mode access prevention
    pub is_smap_on: bool,
    /// Guest page table Supervisor mode execution protection
    pub is_smep_on: bool,
}

// /// Metadata of guest page tables.
// pub struct GuestPageTableMetadata;

// impl PagingMetaData for GuestPageTableMetadata<EPT> {
//     const LEVELS: usize = 4;
//     const PA_MAX_BITS: usize = 52;
//     const VA_MAX_BITS: usize = 48;

//     type VirtAddr = GuestVirtAddr;
//     type PhysAddr = GuestPhysAddr;

//     fn to_actual_paddr(paddr: Self::PhysAddr) -> HostPhysAddr {
//         EPT::guest_phys_to_host_phys(paddr).unwrap()
//     }

//     fn flush_tlb(_vaddr: Option<GuestVirtAddr>) {
//         warn!("flush_tlb is not implemented for guest page tables");
//     }
// }

const ENTRY_COUNT: usize = 512;

// pub type GuestPageTable<EPT: EPTTranslator,H> = PageTable64<GuestPageTableMetadata<EPT>, X64PTE, H>;

/// A generic page table struct for 64-bit platform.
///
/// It also tracks all intermediate level tables. They will be deallocated
/// When the [`GuestPageTable64`] itself is dropped.
pub struct GuestPageTable64<PTE: GenericPTE, H: PagingHandler, EPT: EPTTranslator> {
    root_paddr: GuestPhysAddr,
    levels: usize,
    _phantom: PhantomData<(PTE, H, EPT)>,
}

impl<PTE: GenericPTE, H: PagingHandler, EPT: EPTTranslator> GuestPageTable64<PTE, H, EPT> {
    /// Create a new page table.
    pub fn construct(guest_ptw_info: &GuestPageWalkInfo) -> Self {
        const PHYS_ADDR_MASK: usize = 0x000f_ffff_ffff_f000; // bits 12..52

        Self {
            root_paddr: GuestPhysAddr::from(guest_ptw_info.cr3 & &PHYS_ADDR_MASK),
            levels: guest_ptw_info.level,
            _phantom: PhantomData,
        }
    }

    /// Get the root page table physical address.
    pub fn root_paddr(&self) -> GuestPhysAddr {
        self.root_paddr
    }

    /// Queries the result of the mapping starts with `vaddr`.
    ///
    /// Returns the physical address of the target frame, mapping flags, and
    /// the page size.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// mapping is not present.
    pub fn query(
        &self,
        vaddr: GuestVirtAddr,
    ) -> PagingResult<(GuestPhysAddr, MappingFlags, PageSize)> {
        let (entry, size) = self.get_entry(vaddr)?;
        if entry.is_unused() {
            error!("GuestPT64 query {:?} Entry is unused", vaddr);
            return Err(PagingError::NotMapped);
        }
        let off = size.align_offset(vaddr.into());
        Ok((entry.paddr().add(off).into(), entry.flags(), size))
    }

    /// Queries the result of the mapping starts with `vaddr`.
    ///
    /// Returns the physical address of the target frame, mapping flags,
    /// the page size, and the raw page table entry bits.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// mapping is not present.
    pub fn query_raw(
        &self,
        vaddr: GuestVirtAddr,
    ) -> PagingResult<(GuestPhysAddr, MappingFlags, PageSize, usize)> {
        let (entry, size) = self.get_entry(vaddr)?;
        if entry.is_unused() {
            error!("GuestPT64 query {:?} Entry is unused", vaddr);
            return Err(PagingError::NotMapped);
        }
        let off = size.align_offset(vaddr.into());
        Ok((
            entry.paddr().add(off).into(),
            entry.flags(),
            size,
            entry.bits(),
        ))
    }
}

// private implements
impl<PTE: GenericPTE, H: PagingHandler, EPT: EPTTranslator> GuestPageTable64<PTE, H, EPT> {
    fn table_of<'a>(&self, gpa: GuestPhysAddr) -> PagingResult<&'a [PTE]> {
        let hpa = EPT::guest_phys_to_host_phys(gpa)
            .map(|(hpa, _flags, _pgsize)| hpa)
            .ok_or_else(|| {
                warn!("Failed to translate GPA {:?}", gpa);
                PagingError::NotMapped
            })?;
        let ptr = H::phys_to_virt(hpa).as_ptr() as _;

        Ok(unsafe { core::slice::from_raw_parts(ptr, ENTRY_COUNT) })
    }

    fn next_table<'a>(&self, entry: &PTE) -> PagingResult<&'a [PTE]> {
        if !entry.is_present() {
            error!("GuestPT64 next_table {:?} Entry is not present", entry);
            Err(PagingError::NotMapped)
        } else if entry.is_huge() {
            error!("GuestPT64 next_table {:?} Entry is huge", entry);
            Err(PagingError::MappedToHugePage)
        } else {
            self.table_of(entry.paddr().into())
        }
    }

    fn get_entry(&self, gva: GuestVirtAddr) -> PagingResult<(&PTE, PageSize)> {
        let vaddr: usize = gva.into();

        let p3 = if self.levels == 3 {
            self.table_of(self.root_paddr())?
        } else if self.levels == 4 {
            let p4 = self.table_of(self.root_paddr())?;
            let p4e = &p4[p4_index(vaddr)];
            self.next_table(p4e)?
        } else {
            // 5-level paging
            let p5 = self.table_of(self.root_paddr())?;
            let p5e = &p5[p5_index(vaddr)];
            if p5e.is_huge() {
                return Err(PagingError::MappedToHugePage);
            }
            let p4 = self.next_table(p5e)?;
            let p4e = &p4[p4_index(vaddr)];

            if p4e.is_huge() {
                return Err(PagingError::MappedToHugePage);
            }

            self.next_table(p4e)?
        };

        let p3e = &p3[p3_index(vaddr)];
        if p3e.is_huge() {
            return Ok((p3e, PageSize::Size1G));
        }

        let p2 = self.next_table(p3e)?;
        let p2e = &p2[p2_index(vaddr)];
        if p2e.is_huge() {
            return Ok((p2e, PageSize::Size2M));
        }

        let p1 = self.next_table(p2e)?;
        let p1e = &p1[p1_index(vaddr)];
        Ok((p1e, PageSize::Size4K))
    }
}
