//! ARMv6 short-descriptor MMU: virtual-to-physical translation for the core.
//!
//! When SCTLR.M is set, every instruction fetch and data access made by the
//! core goes through [`MmuBus`], which walks the two-level page tables
//! (TTBR0/TTBR1 split by TTBCR.N), applies domain and access-permission checks
//! and forwards the physical access to the underlying bus.
//!
//! Supported: ARMv6 XP=1 sections/supersections and coarse-table large/small
//! pages with AP/APX/XN; legacy XP=0 sections and coarse-table large/small
//! pages with AP subpages and SCTLR.S/R rules; and XP=0 extended-small pages
//! in coarse tables. FCSE PID remapping happens before TLB lookup and walking.
//! The XP=1 L2 type 0b11 descriptor is reserved and faults. Fine page tables
//! and tiny pages are obsolete in VMSAv6 and are not implemented.
//!
//! The unified software TLB supports ASID/global matching, eight ARM11
//! lockdown slots, and CP15 c8 invalidation operations. Set-associative
//! replacement remains a bounded software approximation; memory attributes
//! TEX, data-alignment faults, and cache behavior are not yet modeled. A
//! translation fault is reported as `Error::InvalidAccess` with the virtual
//! address, and DFSR/DFAR (or IFSR/IFAR) are latched in CP15.

use super::arm::{bit, bits};
use super::cp15::TlbMaintenance;
use super::{ARM_INSTRUCTION_SIZE, Arm11Core, PC, StepEvent, THUMB_INSTRUCTION_SIZE};
use crate::memory::Bus;
use aceris_common::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Access {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Fault {
    pub address: u32,
    /// FSR status bits including the domain field (without the WnR bit).
    pub status: u32,
    pub access: Access,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Params {
    pub ttbr0: u32,
    pub ttbr1: u32,
    pub n: u32,
    pub dacr: u32,
    pub asid: u8,
    pub fcse_pid: u32,
    pub xp: bool,
    pub sctlr_s: bool,
    pub sctlr_r: bool,
    pub tre: bool,
    pub primary_remap: u32,
    pub normal_remap: u32,
    pub tlb_preserve: bool,
    pub tlb_victim: usize,
    pub privileged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Walk {
    Mapped(u32),
    Fault(Fault),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaTranslation {
    Mapped {
        physical_address: u32,
        attributes: MemoryAttributes,
    },
    Fault(Fault),
}

/// ARM11 memory type reported by the VA-to-PA Physical Address Register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum MemoryType {
    StronglyOrdered = 0,
    Device = 1,
    Normal = 2,
    Reserved = 3,
}

/// Resolved attributes retained by a TLB entry and exposed through PAR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MemoryAttributes {
    pub memory_type: MemoryType,
    pub shareable: bool,
    pub inner: u8,
    pub outer: u8,
}

impl MemoryAttributes {
    fn strongly_ordered() -> Self {
        Self {
            memory_type: MemoryType::StronglyOrdered,
            shareable: true,
            inner: 0,
            outer: 0,
        }
    }

    /// Encodes the successful ARM11 MPCore Physical Address Register.
    fn to_pa_register(self, physical_address: u32) -> u32 {
        (physical_address & 0xFFFF_F000)
            | (u32::from(self.shareable) << 8)
            | ((self.memory_type as u32) << 6)
            | (u32::from(self.inner & 3) << 4)
            | (u32::from(self.outer & 3) << 2)
    }
}

/// Raw TEX/C/B/S fields captured from one page-table descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DescriptorMemoryAttributes {
    tex: u8,
    cacheable: bool,
    bufferable: bool,
    shareable: bool,
}

impl DescriptorMemoryAttributes {
    fn resolve(self, params: &Params) -> MemoryAttributes {
        // With TEX remapping enabled, TEX[0], C and B select one of eight
        // PRRR/NMRR entries; TEX[2:1] are OS-defined page-table bits.
        let index = (u32::from(self.tex & 1) << 2)
            | (u32::from(self.cacheable) << 1)
            | u32::from(self.bufferable);

        if params.tre {
            let primary_type = (params.primary_remap >> (index * 2)) & 3;
            let memory_type = match primary_type {
                0 => MemoryType::StronglyOrdered,
                1 => MemoryType::Device,
                2 => MemoryType::Normal,
                _ => MemoryType::Reserved,
            };
            let shareable = match memory_type {
                MemoryType::StronglyOrdered => true,
                MemoryType::Device => {
                    let bit = if self.shareable { 17 } else { 16 };
                    params.primary_remap & (1 << bit) != 0
                }
                MemoryType::Normal => {
                    let bit = if self.shareable { 19 } else { 18 };
                    params.primary_remap & (1 << bit) != 0
                }
                MemoryType::Reserved => false,
            };
            let (inner, outer) = if memory_type == MemoryType::Normal {
                (
                    ((params.normal_remap >> (index * 2)) & 3) as u8,
                    ((params.normal_remap >> (16 + index * 2)) & 3) as u8,
                )
            } else {
                (0, 0)
            };
            return MemoryAttributes {
                memory_type,
                shareable,
                inner,
                outer,
            };
        }

        let cache_policy = (u8::from(self.cacheable) << 1) | u8::from(self.bufferable);
        let (memory_type, shareable, inner, outer) = match (self.tex, cache_policy) {
            (0, 0) => (MemoryType::StronglyOrdered, true, 0, 0),
            (0, 1) => (MemoryType::Device, true, 0, 0),
            (0, 2) => (MemoryType::Normal, self.shareable, 2, 2),
            (0, 3) => (MemoryType::Normal, self.shareable, 3, 3),
            (1, 0) => (MemoryType::Normal, self.shareable, 0, 0),
            (1, 3) => (MemoryType::Normal, self.shareable, 1, 1),
            (2, 0) => (MemoryType::Device, false, 0, 0),
            (4..=7, policy) => (MemoryType::Normal, self.shareable, policy, self.tex & 3),
            _ => (MemoryType::Reserved, false, 0, 0),
        };
        MemoryAttributes {
            memory_type,
            shareable,
            inner,
            outer,
        }
    }
}

#[derive(Clone, Copy)]
enum FaultKind {
    Translation,
    Domain,
    Permission,
}

fn fault(kind: FaultKind, section: bool, domain: u32, address: u32, access: Access) -> Walk {
    let code = match (kind, section) {
        (FaultKind::Translation, true) => 0x5,
        (FaultKind::Translation, false) => 0x7,
        (FaultKind::Domain, true) => 0x9,
        (FaultKind::Domain, false) => 0xB,
        (FaultKind::Permission, true) => 0xD,
        (FaultKind::Permission, false) => 0xF,
    };
    Walk::Fault(Fault {
        address,
        status: code | (domain << 4),
        access,
    })
}

/// Whether (APX, AP) grants the requested access to the requesting privilege level.
fn permission_allows(apx: u32, ap: u32, privileged: bool, write: bool) -> bool {
    // (privileged access, user access) as 0 = none, 1 = read-only, 2 = read/write
    let (privileged_level, user_level) = match (apx, ap) {
        (0, 1) => (2, 0),
        (0, 2) => (2, 1),
        (0, 3) => (2, 2),
        (1, 1) => (1, 0),
        (1, 2) | (1, 3) => (1, 1),
        _ => (0, 0),
    };
    let level = if privileged {
        privileged_level
    } else {
        user_level
    };
    if write { level == 2 } else { level >= 1 }
}

/// Legacy AP[1:0] permission rules used when SCTLR.XP is clear.
fn legacy_permission_allows(
    ap: u8,
    sctlr_s: bool,
    sctlr_r: bool,
    privileged: bool,
    write: bool,
) -> bool {
    let (privileged_level, user_level) = match ap {
        0 => match (sctlr_s, sctlr_r) {
            (false, false) => (0, 0),
            (false, true) => (1, 1),
            (true, false) => (1, 0),
            // S=1,R=1 is reserved; deny access rather than invent behavior.
            (true, true) => (0, 0),
        },
        1 => (2, 0),
        2 => (2, 1),
        3 => (2, 2),
        _ => (0, 0),
    };
    let level = if privileged {
        privileged_level
    } else {
        user_level
    };
    if write { level == 2 } else { level >= 1 }
}

/// One cached short-descriptor translation. Entries retain their original
/// mapping size so MVA invalidation removes a complete section/large page,
/// rather than only an arbitrary 4 KiB subpage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TlbEntry {
    virtual_base: u32,
    physical_base: u32,
    address_mask: u32,
    asid: u8,
    global: bool,
    domain: u8,
    ap: [u8; 4],
    subpage_shift: u8,
    legacy: bool,
    sctlr_s: bool,
    sctlr_r: bool,
    apx: bool,
    xn: bool,
    section: bool,
    memory_attributes: MemoryAttributes,
}

impl TlbEntry {
    fn matches_address(&self, address: u32) -> bool {
        address & self.address_mask == self.virtual_base
    }

    fn matches_context(&self, params: &Params, address: u32) -> bool {
        self.matches_address(address) && (self.global || self.asid == params.asid)
    }

    fn resolve_with_privilege(
        &self,
        params: &Params,
        address: u32,
        access: Access,
        privileged: bool,
    ) -> Walk {
        let physical = self.physical_base | (address & !self.address_mask);
        let subpage = if self.subpage_shift == 0 {
            0
        } else {
            ((address >> self.subpage_shift) & 3) as usize
        };
        let allowed = if self.legacy {
            legacy_permission_allows(
                self.ap[subpage],
                self.sctlr_s,
                self.sctlr_r,
                privileged,
                access == Access::Write,
            )
        } else {
            permission_allows(
                u32::from(self.apx),
                u32::from(self.ap[subpage]),
                privileged,
                access == Access::Write,
            )
        };
        match (params.dacr >> (2 * u32::from(self.domain))) & 3 {
            0b11 => Walk::Mapped(physical),
            0b01 => {
                let allowed = allowed && !(access == Access::Execute && self.xn);
                if allowed {
                    Walk::Mapped(physical)
                } else {
                    fault(
                        FaultKind::Permission,
                        self.section,
                        u32::from(self.domain),
                        address,
                        access,
                    )
                }
            }
            _ => fault(
                FaultKind::Domain,
                self.section,
                u32::from(self.domain),
                address,
                access,
            ),
        }
    }
}

const TLB_CAPACITY: usize = 64;

/// Bounded unified software TLB. Capacity and replacement are implementation
/// details; architectural matching and CP15 invalidations are modeled explicitly.
#[derive(Clone, Debug, Default)]
pub(super) struct TranslationLookasideBuffer {
    entries: Vec<TlbEntry>,
    lockdown_entries: [Option<TlbEntry>; 8],
    next_victim: usize,
}

impl TranslationLookasideBuffer {
    fn lookup(&self, params: &Params, address: u32) -> Option<TlbEntry> {
        self.lockdown_entries
            .iter()
            .rev()
            .flatten()
            .copied()
            .find(|entry| entry.matches_context(params, address))
            .or_else(|| {
                self.entries
                    .iter()
                    .rev()
                    .copied()
                    .find(|entry| entry.matches_context(params, address))
            })
    }

    fn insert(&mut self, entry: TlbEntry, preserve: bool, victim: usize) {
        let same_mapping = |existing: &TlbEntry| {
            existing.virtual_base == entry.virtual_base
                && existing.address_mask == entry.address_mask
                && (entry.global || existing.global || existing.asid == entry.asid)
        };

        if preserve {
            self.entries.retain(|existing| !same_mapping(existing));
            self.lockdown_entries[victim % self.lockdown_entries.len()] = Some(entry);
            return;
        }

        self.entries.retain(|existing| !same_mapping(existing));
        if self.entries.len() < TLB_CAPACITY {
            self.entries.push(entry);
            return;
        }

        self.next_victim %= TLB_CAPACITY;
        self.entries[self.next_victim] = entry;
        self.next_victim = (self.next_victim + 1) % TLB_CAPACITY;
    }

    pub(super) fn maintain(&mut self, operation: TlbMaintenance) {
        self.entries.retain(|entry| match operation {
            TlbMaintenance::All => false,
            TlbMaintenance::MvaAsid { mva, asid } => {
                !(entry.matches_address(mva) && (entry.global || entry.asid == asid))
            }
            TlbMaintenance::Asid { asid } => entry.global || entry.asid != asid,
            TlbMaintenance::MvaAllAsids { mva } => !entry.matches_address(mva),
        });

        for entry in &mut self.lockdown_entries {
            let Some(cached) = entry else {
                continue;
            };
            let invalidate = match operation {
                TlbMaintenance::All => false,
                TlbMaintenance::MvaAsid { mva, asid } => {
                    cached.matches_address(mva) && (cached.global || cached.asid == asid)
                }
                TlbMaintenance::Asid { asid } => !cached.global && cached.asid == asid,
                TlbMaintenance::MvaAllAsids { mva } => cached.matches_address(mva),
            };
            if invalidate {
                *entry = None;
            }
        }

        if self.entries.len() < TLB_CAPACITY {
            self.next_victim = 0;
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len() + self.lockdown_entries.iter().flatten().count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageWalk {
    Mapped(TlbEntry),
    Fault(Fault),
}

/// Walks the ARMv6 short-descriptor tables and returns attributes retained by
/// the TLB. Translation faults are not cached.
fn walk_page_tables<B: Bus + ?Sized>(
    bus: &mut B,
    params: &Params,
    va: u32,
    access: Access,
) -> Result<PageWalk> {
    let n = params.n;
    let l1_index = (va >> 20) << 2;
    let l1_address = if n == 0 || va >> (32 - n) == 0 {
        (params.ttbr0 & !((1u32 << (14 - n)) - 1)) | l1_index
    } else {
        (params.ttbr1 & !0x3FFF) | l1_index
    };
    let l1 = bus.read32(l1_address)?;

    let (
        domain,
        ap,
        subpage_shift,
        apx,
        xn,
        physical,
        global,
        address_mask,
        section,
        legacy,
        descriptor_attributes,
    ) = match l1 & 3 {
        0b10 if params.xp => {
            let supersection = bit(l1, 18);
            let address_mask = if supersection {
                0xFF00_0000
            } else {
                0xFFF0_0000
            };
            let physical = if supersection {
                (l1 & 0xFF00_0000) | (va & 0x00FF_FFFF)
            } else {
                (l1 & 0xFFF0_0000) | (va & 0x000F_FFFF)
            };
            (
                if supersection { 0 } else { bits(l1, 8, 5) },
                [bits(l1, 11, 10) as u8; 4],
                0,
                bits(l1, 15, 15) != 0,
                bit(l1, 4),
                physical,
                !bit(l1, 17),
                address_mask,
                true,
                false,
                DescriptorMemoryAttributes {
                    tex: bits(l1, 14, 12) as u8,
                    cacheable: bit(l1, 3),
                    bufferable: bit(l1, 2),
                    shareable: bit(l1, 16),
                },
            )
        }
        0b10 => {
            // Legacy section descriptors have one AP field for the full 1 MiB.
            let ap = bits(l1, 11, 10) as u8;
            let address_mask = 0xFFF0_0000;
            (
                bits(l1, 8, 5),
                [ap; 4],
                0,
                false,
                false,
                (l1 & address_mask) | (va & !address_mask),
                true,
                address_mask,
                true,
                true,
                DescriptorMemoryAttributes {
                    tex: bits(l1, 14, 12) as u8,
                    cacheable: bit(l1, 3),
                    bufferable: bit(l1, 2),
                    shareable: false,
                },
            )
        }
        0b01 => {
            let domain = bits(l1, 8, 5);
            let l2_address = (l1 & 0xFFFF_FC00) | (((va >> 12) & 0xFF) << 2);
            let l2 = bus.read32(l2_address)?;
            let descriptor_type = l2 & 3;

            // In the ARMv6 XP=1 format, L2 type 0b11 is reserved.
            // It is an extended-small-page descriptor only in the
            // backwards-compatible XP=0 format handled below.
            if descriptor_type == 0 || (params.xp && descriptor_type == 0b11) {
                let page_fault = match fault(FaultKind::Translation, false, domain, va, access) {
                    Walk::Fault(value) => value,
                    Walk::Mapped(_) => unreachable!(),
                };
                return Ok(PageWalk::Fault(page_fault));
            }

            if params.xp {
                if descriptor_type == 0b01 {
                    (
                        domain,
                        [bits(l2, 5, 4) as u8; 4],
                        0,
                        bits(l2, 9, 9) != 0,
                        bit(l2, 15),
                        (l2 & 0xFFFF_0000) | (va & 0xFFFF),
                        !bit(l2, 11),
                        0xFFFF_0000,
                        false,
                        false,
                        DescriptorMemoryAttributes {
                            tex: bits(l2, 14, 12) as u8,
                            cacheable: bit(l2, 3),
                            bufferable: bit(l2, 2),
                            shareable: bit(l2, 10),
                        },
                    )
                } else {
                    (
                        domain,
                        [bits(l2, 5, 4) as u8; 4],
                        0,
                        bits(l2, 9, 9) != 0,
                        descriptor_type == 0b11,
                        (l2 & 0xFFFF_F000) | (va & 0xFFF),
                        !bit(l2, 11),
                        0xFFFF_F000,
                        false,
                        false,
                        DescriptorMemoryAttributes {
                            tex: bits(l2, 8, 6) as u8,
                            cacheable: bit(l2, 3),
                            bufferable: bit(l2, 2),
                            shareable: bit(l2, 10),
                        },
                    )
                }
            } else if descriptor_type == 0b11 {
                // Backwards-compatible extended-small-page descriptor.
                // In a coarse table this maps one 4 KiB page and uses a
                // single AP field; the 1 KiB extended subpage form requires
                // a fine table, which is not implemented by this MMU yet.
                let ap = bits(l2, 5, 4) as u8;
                (
                    domain,
                    [ap; 4],
                    0,
                    false,
                    false,
                    (l2 & 0xFFFF_F000) | (va & 0xFFF),
                    true,
                    0xFFFF_F000,
                    false,
                    true,
                    DescriptorMemoryAttributes {
                        tex: bits(l2, 8, 6) as u8,
                        cacheable: bit(l2, 3),
                        bufferable: bit(l2, 2),
                        shareable: false,
                    },
                )
            } else {
                // Legacy large/small pages each have four independent AP fields.
                let ap = [
                    bits(l2, 5, 4) as u8,
                    bits(l2, 7, 6) as u8,
                    bits(l2, 9, 8) as u8,
                    bits(l2, 11, 10) as u8,
                ];
                if descriptor_type == 0b01 {
                    (
                        domain,
                        ap,
                        14,
                        false,
                        false,
                        (l2 & 0xFFFF_0000) | (va & 0xFFFF),
                        true,
                        0xFFFF_0000,
                        false,
                        true,
                        DescriptorMemoryAttributes {
                            tex: bits(l2, 14, 12) as u8,
                            cacheable: bit(l2, 3),
                            bufferable: bit(l2, 2),
                            shareable: false,
                        },
                    )
                } else {
                    (
                        domain,
                        ap,
                        10,
                        false,
                        false,
                        (l2 & 0xFFFF_F000) | (va & 0xFFF),
                        true,
                        0xFFFF_F000,
                        false,
                        true,
                        DescriptorMemoryAttributes {
                            tex: 0,
                            cacheable: bit(l2, 3),
                            bufferable: bit(l2, 2),
                            shareable: false,
                        },
                    )
                }
            }
        }
        _ => {
            let page_fault = match fault(FaultKind::Translation, true, 0, va, access) {
                Walk::Fault(value) => value,
                Walk::Mapped(_) => unreachable!(),
            };
            return Ok(PageWalk::Fault(page_fault));
        }
    };

    Ok(PageWalk::Mapped(TlbEntry {
        virtual_base: va & address_mask,
        physical_base: physical & address_mask,
        address_mask,
        asid: params.asid,
        global,
        domain: domain as u8,
        ap,
        subpage_shift,
        legacy,
        sctlr_s: params.sctlr_s,
        sctlr_r: params.sctlr_r,
        apx,
        xn,
        section,
        memory_attributes: descriptor_attributes.resolve(params),
    }))
}

/// Performs one page-table walk without retaining the result in the TLB.
#[cfg(test)]
fn translate<B: Bus + ?Sized>(
    bus: &mut B,
    params: &Params,
    va: u32,
    access: Access,
) -> Result<Walk> {
    match walk_page_tables(bus, params, va, access)? {
        PageWalk::Mapped(entry) => {
            Ok(entry.resolve_with_privilege(params, va, access, params.privileged))
        }
        PageWalk::Fault(fault) => Ok(Walk::Fault(fault)),
    }
}

/// A bus adapter that translates every access before forwarding it.
pub(super) struct MmuBus<'a, B: Bus + ?Sized> {
    inner: &'a mut B,
    tlb: &'a mut TranslationLookasideBuffer,
    params: Params,
    fetching: bool,
    fault: Option<Fault>,
}

impl<'a, B: Bus + ?Sized> MmuBus<'a, B> {
    pub(super) fn new(
        inner: &'a mut B,
        params: Params,
        tlb: &'a mut TranslationLookasideBuffer,
    ) -> Self {
        Self {
            inner,
            tlb,
            params,
            fetching: false,
            fault: None,
        }
    }

    fn map(&mut self, va: u32, access: Access) -> Result<u32> {
        self.map_with_privilege(va, access, self.params.privileged)
    }

    fn map_with_privilege(&mut self, va: u32, access: Access, privileged: bool) -> Result<u32> {
        let access = if self.fetching && access == Access::Read {
            Access::Execute
        } else {
            access
        };

        // FCSE remaps the low 32 MiB into a process-specific 32 MiB
        // window before the address reaches the MMU and the unified TLB.
        let mva = if va < 0x0200_0000 {
            self.params.fcse_pid | va
        } else {
            va
        };

        let walk = if let Some(entry) = self.tlb.lookup(&self.params, mva) {
            entry.resolve_with_privilege(&self.params, mva, access, privileged)
        } else {
            match walk_page_tables(self.inner, &self.params, mva, access)? {
                PageWalk::Mapped(entry) => {
                    let result =
                        entry.resolve_with_privilege(&self.params, mva, access, privileged);
                    self.tlb
                        .insert(entry, self.params.tlb_preserve, self.params.tlb_victim);
                    result
                }
                PageWalk::Fault(fault) => Walk::Fault(fault),
            }
        };

        match walk {
            Walk::Mapped(physical) => Ok(physical),
            Walk::Fault(fault) => {
                self.fault.get_or_insert(fault);
                Err(Error::InvalidAccess { addr: va })
            }
        }
    }

    /// Translates a true virtual address for a CP15 c7,c8 VA-to-PA request.
    ///
    /// Unlike a normal access, a permission fault is returned to the caller so
    /// it can be encoded in PAR; it does not become the instruction's Data Abort.
    pub(super) fn translate_for_pa_register(
        &mut self,
        va: u32,
        access: Access,
        privileged: bool,
    ) -> Result<PaTranslation> {
        // The architectural register accepts a true VA. FCSE is applied here,
        // immediately before the lookup/walk, just as it is for ordinary MMU
        // accesses.
        let mva = if va < 0x0200_0000 {
            self.params.fcse_pid | va
        } else {
            va
        };

        let page_walk = if let Some(entry) = self.tlb.lookup(&self.params, mva) {
            PageWalk::Mapped(entry)
        } else {
            walk_page_tables(self.inner, &self.params, mva, access)?
        };

        match page_walk {
            PageWalk::Mapped(entry) => {
                let result = entry.resolve_with_privilege(&self.params, mva, access, privileged);
                self.tlb
                    .insert(entry, self.params.tlb_preserve, self.params.tlb_victim);
                match result {
                    Walk::Mapped(physical_address) => Ok(PaTranslation::Mapped {
                        physical_address,
                        attributes: entry.memory_attributes,
                    }),
                    Walk::Fault(fault) => Ok(PaTranslation::Fault(fault)),
                }
            }
            PageWalk::Fault(fault) => Ok(PaTranslation::Fault(fault)),
        }
    }
}

impl<B: Bus + ?Sized> Bus for MmuBus<'_, B> {
    fn translate_va_to_pa_for_par(
        &mut self,
        virtual_address: u32,
        write: bool,
        privileged: bool,
    ) -> Option<Result<u32>> {
        let access = if write { Access::Write } else { Access::Read };
        Some(
            self.translate_for_pa_register(virtual_address, access, privileged)
                .map(|translation| match translation {
                PaTranslation::Mapped {
                    physical_address,
                    attributes,
                } => attributes.to_pa_register(physical_address),
                PaTranslation::Fault(fault) => ((fault.status & 0xF) << 1) | 1,
                }),
        )
    }

    fn read8(&mut self, addr: u32) -> Result<u8> {
        let physical = self.map(addr, Access::Read)?;
        self.inner.read8(physical)
    }

    fn write8(&mut self, addr: u32, value: u8) -> Result<()> {
        let physical = self.map(addr, Access::Write)?;
        self.inner.write8(physical, value)
    }

    fn read8_unprivileged(&mut self, addr: u32) -> Result<u8> {
        let physical = self.map_with_privilege(addr, Access::Read, false)?;
        self.inner.read8(physical)
    }

    fn write8_unprivileged(&mut self, addr: u32, value: u8) -> Result<()> {
        let physical = self.map_with_privilege(addr, Access::Write, false)?;
        self.inner.write8(physical, value)
    }

    fn read32_unprivileged(&mut self, addr: u32) -> Result<u32> {
        if addr & 0xFFF <= 0xFFC {
            let physical = self.map_with_privilege(addr, Access::Read, false)?;
            return self.inner.read32(physical);
        }
        let mut bytes = [0u8; 4];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self.read8_unprivileged(addr.wrapping_add(index as u32))?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    fn write32_unprivileged(&mut self, addr: u32, value: u32) -> Result<()> {
        if addr & 0xFFF <= 0xFFC {
            let physical = self.map_with_privilege(addr, Access::Write, false)?;
            return self.inner.write32(physical, value);
        }
        for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8_unprivileged(addr.wrapping_add(index as u32), byte)?;
        }
        Ok(())
    }

    fn read16(&mut self, addr: u32) -> Result<u16> {
        if addr & 0xFFF <= 0xFFE {
            let physical = self.map(addr, Access::Read)?;
            return self.inner.read16(physical);
        }
        let lo = self.read8(addr)?;
        let hi = self.read8(addr.wrapping_add(1))?;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    fn read32(&mut self, addr: u32) -> Result<u32> {
        if addr & 0xFFF <= 0xFFC {
            let physical = self.map(addr, Access::Read)?;
            return self.inner.read32(physical);
        }
        let mut bytes = [0u8; 4];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = self.read8(addr.wrapping_add(i as u32))?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    fn write16(&mut self, addr: u32, value: u16) -> Result<()> {
        if addr & 0xFFF <= 0xFFE {
            let physical = self.map(addr, Access::Write)?;
            return self.inner.write16(physical, value);
        }
        for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8(addr.wrapping_add(i as u32), byte)?;
        }
        Ok(())
    }

    fn write32(&mut self, addr: u32, value: u32) -> Result<()> {
        if addr & 0xFFF <= 0xFFC {
            let physical = self.map(addr, Access::Write)?;
            return self.inner.write32(physical, value);
        }
        for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8(addr.wrapping_add(i as u32), byte)?;
        }
        Ok(())
    }

    fn exclusive_read(&mut self, addr: u32, size: usize) -> Result<(u64, u64)> {
        let physical = self.map(addr, Access::Read)?;
        self.inner.exclusive_read(physical, size)
    }

    fn exclusive_write(
        &mut self,
        addr: u32,
        size: usize,
        value: u64,
        reservation: Option<u64>,
    ) -> Result<bool> {
        let physical = self.map(addr, Access::Write)?;
        self.inner
            .exclusive_write(physical, size, value, reservation)
    }
}

impl Arm11Core {
    /// Executes the privileged CP15 VA-to-PA operation and updates PAR.
    pub(super) fn translate_va_to_pa_register<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        virtual_address: u32,
        access: Access,
        privileged_access: bool,
    ) -> Result<()> {
        let mut tlb = std::mem::take(&mut self.tlb);
        let params = Params {
            ttbr0: self.cp15.ttbr0(),
            ttbr1: self.cp15.ttbr1(),
            n: self.cp15.ttbcr_n(),
            dacr: self.cp15.dacr(),
            asid: self.cp15.asid(),
            fcse_pid: self.cp15.fcse_pid(),
            xp: self.cp15.extended_page_tables(),
            sctlr_s: self.cp15.legacy_sctlr_s(),
            sctlr_r: self.cp15.legacy_sctlr_r(),
            tre: self.cp15.memory_region_remap_enabled(),
            primary_remap: self.cp15.primary_memory_remap(),
            normal_remap: self.cp15.normal_memory_remap(),
            tlb_preserve: self.cp15.tlb_preserve(),
            tlb_victim: self.cp15.tlb_victim(),
            privileged: self.cpsr.privileged(),
        };

        let translation_result = if self.cp15.mmu_enabled() {
            let mut mmu = MmuBus::new(bus, params, &mut tlb);
            mmu.translate_for_pa_register(virtual_address, access, privileged_access)
        } else {
            // With translation disabled, data accesses use a flat address map.
            // The default data-side type is Strongly Ordered before TEX remap.
            let attributes = if params.tre {
                DescriptorMemoryAttributes {
                    tex: 0,
                    cacheable: false,
                    bufferable: false,
                    shareable: true,
                }
                .resolve(&params)
            } else {
                MemoryAttributes::strongly_ordered()
            };
            Ok(PaTranslation::Mapped {
                physical_address: virtual_address,
                attributes,
            })
        };
        // Restore the CPU's TLB even when the page-table bus itself fails.
        self.tlb = tlb;
        let translation = translation_result?;

        let pa_register = match translation {
            PaTranslation::Mapped {
                physical_address,
                attributes,
            } => attributes.to_pa_register(physical_address),
            PaTranslation::Fault(fault) => ((fault.status & 0xF) << 1) | 1,
        };
        self.cp15.set_pa_register(pa_register);
        Ok(())
    }

    /// Executes one instruction with address translation enabled.
    pub(super) fn step_translated<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        pc: u32,
    ) -> Result<StepEvent> {
        let mut tlb = std::mem::take(&mut self.tlb);

        let params = Params {
            ttbr0: self.cp15.ttbr0(),
            ttbr1: self.cp15.ttbr1(),
            n: self.cp15.ttbcr_n(),
            dacr: self.cp15.dacr(),
            asid: self.cp15.asid(),
            fcse_pid: self.cp15.fcse_pid(),
            xp: self.cp15.extended_page_tables(),
            sctlr_s: self.cp15.legacy_sctlr_s(),
            sctlr_r: self.cp15.legacy_sctlr_r(),
            tre: self.cp15.memory_region_remap_enabled(),
            primary_remap: self.cp15.primary_memory_remap(),
            normal_remap: self.cp15.normal_memory_remap(),
            tlb_preserve: self.cp15.tlb_preserve(),
            tlb_victim: self.cp15.tlb_victim(),
            privileged: self.cpsr.privileged(),
        };
        let (outcome, fault) = {
            let mut mmu = MmuBus::new(bus, params, &mut tlb);
            let outcome = self.fetch_and_execute(&mut mmu, pc);
            (outcome, mmu.fault)
        };
        self.tlb = tlb;

        if let Some(fault) = fault {
            self.fault_latched = true;
            self.prefetch_fault = fault.access == Access::Execute;
            match fault.access {
                Access::Execute => self.cp15.latch_prefetch_fault(fault.status, fault.address),
                access => {
                    self.cp15
                        .latch_data_fault(fault.status, fault.address, access == Access::Write)
                }
            }
        }
        let (event, size) = outcome?;
        if !self.branch_taken {
            self.regs[PC] = pc.wrapping_add(size);
        }
        Ok(event)
    }

    fn fetch_and_execute<B: Bus + ?Sized>(
        &mut self,
        mmu: &mut MmuBus<'_, B>,
        pc: u32,
    ) -> Result<(StepEvent, u32)> {
        mmu.fetching = true;
        let thumb = self.cpsr.thumb();
        let fetched = if thumb {
            mmu.read16(pc).map(u32::from)
        } else {
            mmu.read32(pc)
        };
        mmu.fetching = false;
        let instruction = fetched?;
        if thumb {
            Ok((
                self.execute_thumb(mmu, instruction)?,
                THUMB_INSTRUCTION_SIZE,
            ))
        } else {
            Ok((self.execute_arm(mmu, instruction)?, ARM_INSTRUCTION_SIZE))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Psr;
    use crate::memory::FlatMemory;

    const L1: u32 = 0x4000;
    const L2: u32 = 0x8000;
    const MEM_SIZE: usize = 0x4_0000;

    const SECTION: u32 = 0b10;
    const SECTION_AP_RW_ALL: u32 = 0b11 << 10;
    const SECTION_AP_PRIV_RW: u32 = 0b01 << 10;
    const SECTION_APX: u32 = 1 << 15;
    const SECTION_XN: u32 = 1 << 4;
    const COARSE: u32 = 0b01;
    const SMALL: u32 = 0b10;
    const SMALL_AP_RW_ALL: u32 = 0b11 << 4;
    const SMALL_APX: u32 = 1 << 9;

    fn domain(d: u32) -> u32 {
        d << 5
    }

    /// Memory with an empty L1 table and a CPU with the MMU enabled (XP=1).
    fn machine() -> (Arm11Core, FlatMemory) {
        let mem = FlatMemory::new(0, MEM_SIZE);
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.cp15.write(true, 0, 2, 0, 0, L1).unwrap();
        // Domain 0 = client, domain 1 = manager, domain 2 = no access.
        cpu.cp15
            .write(true, 0, 3, 0, 0, 0b01 | (0b11 << 2))
            .unwrap();
        cpu.cp15.write(true, 0, 1, 0, 0, 1 | (1 << 23)).unwrap();
        (cpu, mem)
    }

    fn set_l1(mem: &mut FlatMemory, index: u32, descriptor: u32) {
        mem.write32(L1 + index * 4, descriptor).unwrap();
    }

    fn params(cpu: &Arm11Core) -> Params {
        Params {
            ttbr0: cpu.cp15.ttbr0(),
            ttbr1: cpu.cp15.ttbr1(),
            n: cpu.cp15.ttbcr_n(),
            dacr: cpu.cp15.dacr(),
            asid: cpu.cp15.asid(),
            fcse_pid: cpu.cp15.fcse_pid(),
            xp: cpu.cp15.extended_page_tables(),
            sctlr_s: cpu.cp15.legacy_sctlr_s(),
            sctlr_r: cpu.cp15.legacy_sctlr_r(),
            tre: cpu.cp15.memory_region_remap_enabled(),
            primary_remap: cpu.cp15.primary_memory_remap(),
            normal_remap: cpu.cp15.normal_memory_remap(),
            tlb_preserve: cpu.cp15.tlb_preserve(),
            tlb_victim: cpu.cp15.tlb_victim(),
            privileged: cpu.cpsr.privileged(),
        }
    }

    fn walk(cpu: &Arm11Core, mem: &mut FlatMemory, va: u32, access: Access) -> Walk {
        translate(mem, &params(cpu), va, access).unwrap()
    }

    fn entry(cpu: &Arm11Core, mem: &mut FlatMemory, va: u32) -> TlbEntry {
        match walk_page_tables(mem, &params(cpu), va, Access::Read).unwrap() {
            PageWalk::Mapped(entry) => entry,
            PageWalk::Fault(fault) => panic!("expected a mapping, got fault {:#x}", fault.status),
        }
    }

    fn status_of(walk: Walk) -> u32 {
        match walk {
            Walk::Fault(fault) => fault.status,
            Walk::Mapped(pa) => panic!("expected a fault, got {pa:#x}"),
        }
    }

    #[test]
    fn xp_section_retains_tex_cb_and_shareable_attributes() {
        let (cpu, mut mem) = machine();
        let va = 0x1234_5678;
        set_l1(
            &mut mem,
            0x123,
            0x0120_0000 | SECTION | SECTION_AP_RW_ALL | (4 << 12) | (1 << 16) | (1 << 3) | (1 << 2),
        );

        assert_eq!(
            entry(&cpu, &mut mem, va).memory_attributes,
            MemoryAttributes {
                memory_type: MemoryType::Normal,
                shareable: true,
                inner: 3,
                outer: 0,
            }
        );
    }

    #[test]
    fn xp_small_page_uses_its_own_tex_field_and_shareability_bit() {
        let (cpu, mut mem) = machine();
        let va = 0x2000_1000;
        set_l1(&mut mem, 0x200, L2 | COARSE);
        mem.write32(
            L2 + 4,
            0x0002_0000 | SMALL | SMALL_AP_RW_ALL | (5 << 6) | (1 << 10) | (1 << 3) | (1 << 2),
        )
        .unwrap();

        assert_eq!(
            entry(&cpu, &mut mem, va).memory_attributes,
            MemoryAttributes {
                memory_type: MemoryType::Normal,
                shareable: true,
                inner: 3,
                outer: 1,
            }
        );
    }

    #[test]
    fn legacy_small_page_without_tex_uses_device_cb_encoding() {
        let (cpu, mut mem) = legacy_machine();
        let va = 0x2000_0000;
        set_l1(&mut mem, 0x200, L2 | COARSE);
        mem.write32(L2, 0x0002_0000 | SMALL | SMALL_AP_RW_ALL | (1 << 2))
            .unwrap();

        assert_eq!(
            entry(&cpu, &mut mem, va).memory_attributes,
            MemoryAttributes {
                memory_type: MemoryType::Device,
                shareable: true,
                inner: 0,
                outer: 0,
            }
        );
    }

    #[test]
    fn tre_maps_descriptor_type_and_cache_policy_through_prrr_and_nmrr() {
        let (mut cpu, mut mem) = machine();
        let va = 0x2000_0000;
        set_l1(
            &mut mem,
            0x200,
            SECTION | SECTION_AP_RW_ALL | (1 << 16) | (1 << 3) | (1 << 2),
        );

        // Index {TEX[0],C,B}=011 maps to Normal. Remap S=1 as shareable,
        // with Inner=01 and Outer=11.
        cpu.cp15
            .write(true, 0, 10, 2, 0, (1 << 19) | (2 << 6))
            .unwrap();
        cpu.cp15
            .write(true, 0, 10, 2, 1, (1 << 6) | (3 << 22))
            .unwrap();
        cpu.cp15
            .write(true, 0, 1, 0, 0, 1 | (1 << 23) | (1 << 28))
            .unwrap();

        assert_eq!(
            entry(&cpu, &mut mem, va).memory_attributes,
            MemoryAttributes {
                memory_type: MemoryType::Normal,
                shareable: true,
                inner: 1,
                outer: 3,
            }
        );
    }

    #[test]
    fn va_to_pa_populates_par_with_physical_page_and_resolved_attributes() {
        let (mut cpu, mut mem) = machine();
        let va = 0x1234_5678;
        set_l1(
            &mut mem,
            0x123,
            0x0120_0000 | SECTION | SECTION_AP_RW_ALL | (4 << 12) | (1 << 16) | (1 << 3) | (1 << 2),
        );

        cpu.translate_va_to_pa_register(&mut mem, va, Access::Read, true)
            .unwrap();
        assert_eq!(cpu.cp15.read(true, 0, 7, 4, 0), Ok(0x0124_51B0));

        // A translation fault is reported in PAR, not raised as an instruction abort.
        cpu.translate_va_to_pa_register(&mut mem, 0x5000_0000, Access::Read, true)
            .unwrap();
        assert_eq!(cpu.cp15.read(true, 0, 7, 4, 0), Ok(0x0B));
    }

    #[test]
    fn va_to_pa_applies_the_requested_user_permission_check() {
        let (mut cpu, mut mem) = machine();
        let va = 0x2000_0000;
        set_l1(&mut mem, 0x200, SECTION | SECTION_AP_PRIV_RW);

        // The MCR itself runs privileged, but opc2=3 requests a User write check.
        cpu.translate_va_to_pa_register(&mut mem, va, Access::Write, false)
            .unwrap();
        assert_eq!(cpu.cp15.read(true, 0, 7, 4, 0), Ok(0x1B));
    }

    #[test]
    fn guest_mcr_va_to_pa_uses_the_active_translated_mmu_bus() {
        let (mut cpu, mut mem) = machine();
        let target_va = 0x1234_5678;

        // Map instruction VA 0 to PA 0 and the target VA to a different PA section.
        set_l1(&mut mem, 0, SECTION | SECTION_AP_RW_ALL);
        set_l1(
            &mut mem,
            0x123,
            0x0120_0000 | SECTION | SECTION_AP_RW_ALL | (4 << 12) | (1 << 16) | (1 << 3) | (1 << 2),
        );
        mem.write32(0, 0xEE07_0F18).unwrap(); // MCR p15,0,r0,c7,c8,0
        cpu.regs[0] = target_va;

        assert_eq!(cpu.step(&mut mem).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.cp15.read(true, 0, 7, 4, 0), Ok(0x0124_51B0));
    }

    #[test]
    fn va_to_pa_is_flat_mapped_when_the_mmu_is_disabled() {
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        let mut mem = FlatMemory::new(0, 16);
        cpu.translate_va_to_pa_register(&mut mem, 0x1234_5678, Access::Read, true)
            .unwrap();
        assert_eq!(cpu.cp15.read(true, 0, 7, 4, 0), Ok(0x1234_5100));
    }

    #[test]
    fn section_translation_keeps_the_offset_within_one_megabyte() {
        let (cpu, mut mem) = machine();
        set_l1(&mut mem, 0x123, SECTION | SECTION_AP_RW_ALL);
        assert_eq!(
            walk(&cpu, &mut mem, 0x1230_4567, Access::Read),
            Walk::Mapped(0x0000_4567)
        );
        // Section base comes from the descriptor.
        set_l1(&mut mem, 0x124, 0x0010_0000 | SECTION | SECTION_AP_RW_ALL);
        assert_eq!(
            walk(&cpu, &mut mem, 0x1240_0010, Access::Read),
            Walk::Mapped(0x0010_0010)
        );
    }

    #[test]
    fn supersection_translation_uses_sixteen_megabyte_granularity() {
        let (cpu, mut mem) = machine();
        for index in 0x300..0x310 {
            set_l1(
                &mut mem,
                index,
                0x0300_0000 | (1 << 18) | SECTION | SECTION_AP_RW_ALL,
            );
        }
        assert_eq!(
            walk(&cpu, &mut mem, 0x3012_3456, Access::Read),
            Walk::Mapped(0x0312_3456)
        );
    }

    #[test]
    fn coarse_tables_map_small_and_large_pages() {
        let (cpu, mut mem) = machine();
        set_l1(&mut mem, 0x200, L2 | COARSE);
        mem.write32(L2 + 4, 0x0002_0000 | SMALL | SMALL_AP_RW_ALL)
            .unwrap();
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_1ABC, Access::Write),
            Walk::Mapped(0x0002_0ABC)
        );
        // Large page: 64 KiB, replicated across sixteen entries.
        let large = 0x0003_0000 | 0b01 | SMALL_AP_RW_ALL;
        for i in 0..16 {
            mem.write32(L2 + (16 + i) * 4, large).unwrap();
        }
        assert_eq!(
            walk(&cpu, &mut mem, 0x2001_2345, Access::Read),
            Walk::Mapped(0x0003_2345)
        );
    }

    #[test]
    fn unmapped_addresses_raise_translation_faults() {
        let (cpu, mut mem) = machine();
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x5000_0000, Access::Read)),
            0x5
        );
        set_l1(&mut mem, 0x200, L2 | COARSE | domain(1));
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Read)),
            0x7 | (1 << 4)
        );
    }

    #[test]
    fn domain_no_access_and_manager_override_permissions() {
        let (cpu, mut mem) = machine();
        // Domain 2 has no access; domain 1 is manager and ignores AP/XN.
        set_l1(&mut mem, 1, SECTION | SECTION_AP_RW_ALL | domain(2));
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x0010_0000, Access::Read)),
            0x9 | (2 << 4)
        );
        set_l1(&mut mem, 2, SECTION | SECTION_APX | SECTION_XN | domain(1));
        assert_eq!(
            walk(&cpu, &mut mem, 0x0020_0000, Access::Write),
            Walk::Mapped(0)
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x0020_0000, Access::Execute),
            Walk::Mapped(0)
        );
    }

    #[test]
    fn access_permissions_distinguish_privilege_write_and_execute() {
        let (mut cpu, mut mem) = machine();
        set_l1(&mut mem, 1, SECTION | SECTION_AP_PRIV_RW);
        set_l1(&mut mem, 2, SECTION | SECTION_AP_RW_ALL | SECTION_APX);
        set_l1(&mut mem, 3, SECTION | SECTION_AP_RW_ALL | SECTION_XN);

        // Privileged: RW on a privileged-only section, RO when APX is set.
        assert!(matches!(
            walk(&cpu, &mut mem, 0x0010_0000, Access::Write),
            Walk::Mapped(_)
        ));
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x0020_0000, Access::Write)),
            0xD
        );
        assert!(matches!(
            walk(&cpu, &mut mem, 0x0020_0000, Access::Read),
            Walk::Mapped(_)
        ));
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x0030_0000, Access::Execute)),
            0xD
        );

        // User mode cannot touch the privileged-only section.
        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x0010_0000, Access::Read)),
            0xD
        );
        assert!(matches!(
            walk(&cpu, &mut mem, 0x0030_0000, Access::Write),
            Walk::Mapped(_)
        ));
    }

    #[test]
    fn xp_format_rejects_reserved_extended_small_page_descriptor() {
        let (cpu, mut mem) = machine();
        set_l1(&mut mem, 0x200, L2 | COARSE);

        // L2 type 0b11 is reserved with SCTLR.XP=1. It must not be
        // interpreted as the legacy extended-small-page format.
        mem.write32(L2, 0x0004_0000 | (0b11 << 4) | 0b11).unwrap();
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Read)),
            0x7
        );
    }

    #[test]
    fn small_page_apx_makes_the_page_read_only() {
        let (cpu, mut mem) = machine();
        set_l1(&mut mem, 0x200, L2 | COARSE);
        mem.write32(L2, 0x0002_0000 | SMALL | SMALL_AP_RW_ALL | SMALL_APX)
            .unwrap();
        assert!(matches!(
            walk(&cpu, &mut mem, 0x2000_0000, Access::Read),
            Walk::Mapped(_)
        ));
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Write)),
            0xF
        );
    }

    #[test]
    fn ttbcr_n_routes_low_addresses_to_ttbr0_and_the_rest_to_ttbr1() {
        let (mut cpu, mut mem) = machine();
        // N = 1: the lower 2 GiB use TTBR0 (a 8 KiB table at 0xC000), the rest TTBR1.
        cpu.cp15.write(true, 0, 2, 0, 2, 1).unwrap();
        cpu.cp15.write(true, 0, 2, 0, 0, 0xC000).unwrap();
        cpu.cp15.write(true, 0, 2, 0, 1, L1).unwrap();
        mem.write32(0xC000 + 4, SECTION | SECTION_AP_RW_ALL)
            .unwrap();
        set_l1(&mut mem, 0x801, 0x0010_0000 | SECTION | SECTION_AP_RW_ALL);
        assert_eq!(
            walk(&cpu, &mut mem, 0x0010_0008, Access::Read),
            Walk::Mapped(8)
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x8010_0008, Access::Read),
            Walk::Mapped(0x0010_0008)
        );
    }

    #[test]
    fn unprivileged_bus_accesses_recheck_user_permissions_on_cached_tlb_entries() {
        let (cpu, mut mem) = machine();
        let address = 0x0020_0100;
        set_l1(&mut mem, 2, SECTION | SECTION_AP_PRIV_RW);
        mem.write32(0x100, 0xCAFE_BABE).unwrap();
        mem.write8(0x104, 0x5A).unwrap();

        let mut tlb = TranslationLookasideBuffer::default();
        let mut bus = MmuBus::new(&mut mem, params(&cpu), &mut tlb);

        // The privileged access fills the TLB and succeeds.
        assert_eq!(bus.read32(address).unwrap(), 0xCAFE_BABE);

        // The cached descriptor must still be checked as User for translated accesses.
        assert!(matches!(
            bus.read32_unprivileged(address),
            Err(Error::InvalidAccess { addr }) if addr == address
        ));
        assert!(matches!(
            bus.read8_unprivileged(address + 4),
            Err(Error::InvalidAccess { addr }) if addr == address + 4
        ));
        assert!(matches!(
            bus.write32_unprivileged(address, 0x1122_3344),
            Err(Error::InvalidAccess { addr }) if addr == address
        ));
        assert!(matches!(
            bus.write8_unprivileged(address + 4, 0xAA),
            Err(Error::InvalidAccess { addr }) if addr == address + 4
        ));
        assert_eq!(mem.read32(0x100).unwrap(), 0xCAFE_BABE);
        assert_eq!(mem.read8(0x104).unwrap(), 0x5A);
    }

    #[test]
    fn a32_ldrt_ldrbt_strt_and_strbt_use_user_memory_permissions() {
        let (mut cpu, mut mem) = machine();
        let data_va = 0x0020_0100;

        // Code remains executable in privileged mode. Data is privileged RW only.
        set_l1(&mut mem, 0x401, SECTION | SECTION_AP_RW_ALL);
        set_l1(&mut mem, 0x002, SECTION | SECTION_AP_PRIV_RW);
        mem.write32(0x100, 0xCAFE_BABE).unwrap();
        mem.write8(0x104, 0x5A).unwrap();

        // LDR r1,[r0] ; LDRT r2,[r0],#4 ; LDRBT r3,[r0],#1 ;
        // STRT r4,[r0],#4 ; STRBT r5,[r0],#1.
        for (offset, instruction) in [
            (0, 0xE590_1000),
            (4, 0xE4B0_2004),
            (8, 0xE4F0_3001),
            (12, 0xE4A0_4004),
            (16, 0xE4E0_5001),
        ] {
            mem.write32(offset, instruction).unwrap();
        }

        cpu.regs[0] = data_va;
        cpu.regs[4] = 0x1122_3344;
        cpu.regs[5] = 0xAA;
        cpu.set_pc(0x4010_0000);

        // Ordinary privileged LDR succeeds on the privileged-only page.
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs[1], 0xCAFE_BABE);

        // The translation variants keep Supervisor mode but apply User AP checks.
        for pc in [0x4010_0004, 0x4010_0008, 0x4010_000C, 0x4010_0010] {
            cpu.set_pc(pc);
            assert!(matches!(
                cpu.step(&mut mem),
                Err(Error::InvalidAccess { addr }) if addr == data_va
            ));
            assert_eq!(cpu.regs[0], data_va, "failed access must not write back");
        }
        assert_eq!(cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
        assert_eq!(mem.read32(0x100).unwrap(), 0xCAFE_BABE);
        assert_eq!(mem.read8(0x104).unwrap(), 0x5A);
    }

    #[test]
    fn a32_unprivileged_transfers_succeed_with_user_permissions_and_write_back() {
        let (mut cpu, mut mem) = machine();
        let data_va = 0x0020_0100;

        set_l1(&mut mem, 0x401, SECTION | SECTION_AP_RW_ALL);
        set_l1(&mut mem, 0x002, SECTION | SECTION_AP_RW_ALL);
        mem.write32(0x100, 0xCAFE_BABE).unwrap();
        mem.write8(0x104, 0x5A).unwrap();

        // LDRT r1,[r0],#4 ; LDRBT r2,[r0],#1 ;
        // STRT r3,[r0],#4 ; STRBT r4,[r0],#1.
        for (offset, instruction) in [
            (0, 0xE4B0_1004),
            (4, 0xE4F0_2001),
            (8, 0xE4A0_3004),
            (12, 0xE4E0_4001),
        ] {
            mem.write32(offset, instruction).unwrap();
        }

        cpu.regs[0] = data_va;
        cpu.regs[3] = 0x1122_3344;
        cpu.regs[4] = 0xAA;
        cpu.set_pc(0x4010_0000);

        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs[1], 0xCAFE_BABE);
        assert_eq!(cpu.regs[0], data_va + 4);

        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs[2], 0x5A);
        assert_eq!(cpu.regs[0], data_va + 5);

        cpu.regs[0] = data_va;
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs[0], data_va + 4);
        assert_eq!(mem.read32(0x100).unwrap(), 0x1122_3344);

        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs[0], data_va + 5);
        assert_eq!(mem.read8(0x104).unwrap(), 0xAA);
        assert_eq!(cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
    }

    #[test]
    fn cpu_runs_translated_code_and_data_accesses() {
        let (mut cpu, mut mem) = machine();
        // VA 0x4010_0000 -> PA 0 (code); VA 0x2000_0000 page -> PA 0x20000 (data).
        set_l1(&mut mem, 0x401, SECTION | SECTION_AP_RW_ALL);
        set_l1(&mut mem, 0x200, L2 | COARSE);
        mem.write32(L2, 0x0002_0000 | SMALL | SMALL_AP_RW_ALL)
            .unwrap();
        mem.write32(0x2_0010, 0xCAFE_F00D).unwrap();
        // ldr r1,[r0,#0x10] ; str r1,[r0,#0x14]
        mem.write32(0, 0xE590_1010).unwrap();
        mem.write32(4, 0xE580_1014).unwrap();
        cpu.regs[0] = 0x2000_0000;
        cpu.set_pc(0x4010_0000);
        cpu.step(&mut mem).unwrap();
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs[1], 0xCAFE_F00D);
        assert_eq!(mem.read32(0x2_0014).unwrap(), 0xCAFE_F00D);
        assert_eq!(cpu.pc(), 0x4010_0008);
    }

    #[test]
    fn data_abort_latches_dfsr_and_dfar_and_reports_the_virtual_address() {
        let (mut cpu, mut mem) = machine();
        set_l1(&mut mem, 0x401, SECTION | SECTION_AP_RW_ALL);
        mem.write32(0, 0xE580_1000).unwrap(); // str r1,[r0]
        cpu.regs[0] = 0x7000_0010;
        cpu.set_pc(0x4010_0000);
        assert!(matches!(
            cpu.step(&mut mem),
            Err(Error::InvalidAccess { addr: 0x7000_0010 })
        ));
        assert_eq!(cpu.cp15.dfar(), 0x7000_0010);
        assert_eq!(cpu.cp15.dfsr(), 0x5 | (1 << 11));
    }

    #[test]
    fn execute_never_pages_cause_prefetch_aborts() {
        let (mut cpu, mut mem) = machine();
        set_l1(&mut mem, 0x401, SECTION | SECTION_AP_RW_ALL | SECTION_XN);
        cpu.set_pc(0x4010_0000);
        assert!(matches!(
            cpu.step(&mut mem),
            Err(Error::InvalidAccess { addr: 0x4010_0000 })
        ));
        assert_eq!(cpu.cp15.ifar(), 0x4010_0000);
        assert_eq!(cpu.cp15.ifsr(), 0xD);
    }

    #[test]
    fn accesses_straddling_a_page_boundary_translate_each_page() {
        let (cpu, mut mem) = machine();
        set_l1(&mut mem, 0x200, L2 | COARSE);
        mem.write32(L2, 0x0002_0000 | SMALL | SMALL_AP_RW_ALL)
            .unwrap();
        mem.write32(L2 + 4, 0x0003_0000 | SMALL | SMALL_AP_RW_ALL)
            .unwrap();
        mem.write8(0x2_0FFE, 0x11).unwrap();
        mem.write8(0x2_0FFF, 0x22).unwrap();
        mem.write8(0x3_0000, 0x33).unwrap();
        mem.write8(0x3_0001, 0x44).unwrap();
        let mut tlb = TranslationLookasideBuffer::default();
        let mut bus = MmuBus::new(&mut mem, params(&cpu), &mut tlb);
        assert_eq!(bus.read32(0x2000_0FFE).unwrap(), 0x4433_2211);
        bus.write32(0x2000_0FFE, 0xDDCC_BBAA).unwrap();
        assert_eq!(bus.read16(0x2000_0FFF).unwrap(), 0xCCBB);
        assert_eq!(mem.read8(0x3_0001).unwrap(), 0xDD);
        assert_eq!(mem.read8(0x2_0FFE).unwrap(), 0xAA);
    }

    #[test]
    fn exclusive_accesses_follow_the_physical_address_across_aliases() {
        let (mut cpu, mut mem) = machine();
        set_l1(&mut mem, 0x401, SECTION | SECTION_AP_RW_ALL);
        set_l1(&mut mem, 0x200, L2 | COARSE);
        // Two virtual pages alias the same physical page.
        mem.write32(L2, 0x0002_0000 | SMALL | SMALL_AP_RW_ALL)
            .unwrap();
        mem.write32(L2 + 4, 0x0002_0000 | SMALL | SMALL_AP_RW_ALL)
            .unwrap();
        let program = [
            0xE190_1F9F, // ldrex r1,[r0]
            0xE580_3000, // str r3,[r0]   (store through the other alias)
            0xE180_2F91, // strex r2,r1,[r0]
        ];
        for (i, word) in program.iter().enumerate() {
            mem.write32(i as u32 * 4, *word).unwrap();
        }
        cpu.cp15.write(true, 0, 1, 0, 0, 1 | (1 << 23)).unwrap();

        // Undisturbed pair succeeds and the value lands at the physical address.
        cpu.regs[0] = 0x2000_0000;
        cpu.regs[1] = 0;
        cpu.set_pc(0x4010_0000);
        cpu.step(&mut mem).unwrap();
        cpu.regs[1] = 0xAAAA_0001;
        cpu.set_pc(0x4010_0008);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs[2], 0);
        assert_eq!(mem.read32(0x2_0000).unwrap(), 0xAAAA_0001);

        // A store through the other virtual alias in between kills the reservation.
        cpu.regs[0] = 0x2000_0000;
        cpu.set_pc(0x4010_0000);
        cpu.step(&mut mem).unwrap();
        cpu.regs[0] = 0x2000_1004; // second alias, same 8-byte granule
        cpu.regs[3] = 0x55;
        cpu.set_pc(0x4010_0004);
        cpu.step(&mut mem).unwrap();
        cpu.regs[0] = 0x2000_0000;
        cpu.regs[1] = 0xBBBB_0002;
        cpu.set_pc(0x4010_0008);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs[2], 1);
        assert_eq!(mem.read32(0x2_0000).unwrap(), 0xAAAA_0001);
    }

    #[test]
    fn fcse_pid_remaps_low_virtual_addresses_before_tlb_lookup() {
        let (cpu, mut mem) = machine();
        let va = 0x0012_3456;
        let mva = 0x0212_3456;
        set_l1(
            &mut mem,
            mva >> 20,
            0x0050_0000 | SECTION | SECTION_AP_RW_ALL,
        );

        let mut params = params(&cpu);
        params.fcse_pid = 0x0200_0000;
        let mut tlb = TranslationLookasideBuffer::default();
        {
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert_eq!(mmu.map(va, Access::Read).unwrap(), 0x0052_3456);
        }
        assert_eq!(tlb.len(), 1);
        assert_eq!(tlb.entries[0].virtual_base, mva & 0xFFF0_0000);

        // A zero PID uses the unmodified VA and must not alias the process-1 entry.
        params.fcse_pid = 0;
        {
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert!(matches!(
                mmu.map(va, Access::Read),
                Err(Error::InvalidAccess { addr }) if addr == va
            ));
        }
        assert_eq!(tlb.len(), 1);
    }

    #[test]
    fn unified_tlb_keeps_a_section_mapping_until_mva_asid_invalidation() {
        let (cpu, mut mem) = machine();
        let va = 0x1230_4567;
        set_l1(&mut mem, 0x123, SECTION | SECTION_AP_RW_ALL);

        let mut tlb = TranslationLookasideBuffer::default();
        let params = params(&cpu);
        {
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert_eq!(mmu.map(va, Access::Read).unwrap(), 0x0000_4567);
        }
        assert_eq!(tlb.len(), 1);

        set_l1(&mut mem, 0x123, 0x0010_0000 | SECTION | SECTION_AP_RW_ALL);
        {
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert_eq!(mmu.map(va, Access::Read).unwrap(), 0x0000_4567);
        }

        // An MVA from anywhere inside the section invalidates the whole entry.
        tlb.maintain(TlbMaintenance::MvaAsid {
            mva: 0x123F_FFFF,
            asid: params.asid,
        });
        {
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert_eq!(mmu.map(va, Access::Read).unwrap(), 0x0010_4567);
        }
    }

    #[test]
    fn asid_invalidation_preserves_global_entries_and_mva_only_ignores_asid() {
        let mut tlb = TranslationLookasideBuffer::default();
        let make_entry = |virtual_base, physical_base, asid, global| TlbEntry {
            virtual_base,
            physical_base,
            address_mask: 0xFFFF_F000,
            asid,
            global,
            domain: 0,
            ap: [3; 4],
            subpage_shift: 0,
            legacy: false,
            sctlr_s: false,
            sctlr_r: false,
            apx: false,
            xn: false,
            section: false,
        };

        tlb.insert(make_entry(0x1000, 0xA000, 7, true), false, 0);
        tlb.insert(make_entry(0x2000, 0xB000, 7, false), false, 0);
        tlb.insert(make_entry(0x3000, 0xC000, 8, false), false, 0);

        tlb.maintain(TlbMaintenance::Asid { asid: 7 });
        assert_eq!(tlb.len(), 2);
        assert!(
            tlb.entries
                .iter()
                .any(|entry| entry.virtual_base == 0x1000 && entry.global)
        );
        assert!(
            tlb.entries
                .iter()
                .any(|entry| entry.virtual_base == 0x3000 && entry.asid == 8)
        );

        tlb.maintain(TlbMaintenance::MvaAsid {
            mva: 0x1000,
            asid: 0xFF,
        });
        assert_eq!(tlb.len(), 1);
        assert_eq!(tlb.entries[0].virtual_base, 0x3000);

        tlb.maintain(TlbMaintenance::MvaAllAsids { mva: 0x3004 });
        assert!(tlb.entries.is_empty());
    }

    #[test]
    fn invalidate_all_clears_the_unified_tlb() {
        let mut tlb = TranslationLookasideBuffer::default();
        tlb.insert(
            TlbEntry {
                virtual_base: 0,
                physical_base: 0,
                address_mask: 0xFFFF_F000,
                asid: 0,
                global: false,
                domain: 0,
                ap: [3; 4],
                subpage_shift: 0,
                legacy: false,
                sctlr_s: false,
                sctlr_r: false,
                apx: false,
                xn: false,
                section: false,
            },
            false,
            0,
        );
        tlb.maintain(TlbMaintenance::All);
        assert_eq!(tlb.len(), 0);
    }

    #[test]
    fn tlbi_all_preserves_lockdown_entries_but_mva_invalidations_remove_them() {
        let mut tlb = TranslationLookasideBuffer::default();
        let make_entry = |virtual_base, physical_base, asid, global| TlbEntry {
            virtual_base,
            physical_base,
            address_mask: 0xFFFF_F000,
            asid,
            global,
            domain: 0,
            ap: [3; 4],
            subpage_shift: 0,
            legacy: false,
            sctlr_s: false,
            sctlr_r: false,
            apx: false,
            xn: false,
            section: false,
        };
        tlb.insert(make_entry(0x1000, 0xA000, 7, false), true, 2);
        tlb.insert(make_entry(0x2000, 0xB000, 7, false), false, 0);
        tlb.insert(make_entry(0x3000, 0xC000, 7, true), true, 3);

        tlb.maintain(TlbMaintenance::All);
        assert_eq!(tlb.len(), 2);
        assert!(tlb.lockdown_entries[2].is_some());
        assert!(tlb.lockdown_entries[3].is_some());

        tlb.maintain(TlbMaintenance::Asid { asid: 7 });
        assert_eq!(tlb.len(), 1);
        assert!(tlb.lockdown_entries[2].is_none());
        assert!(tlb.lockdown_entries[3].is_some());

        tlb.maintain(TlbMaintenance::MvaAllAsids { mva: 0x3004 });
        assert!(tlb.lockdown_entries[3].is_none());
        assert_eq!(tlb.len(), 0);
    }

    #[test]
    fn tlb_lockdown_register_routes_page_walk_fill_to_selected_slot() {
        let (cpu, mut mem) = machine();
        let va = 0x1230_4567;
        set_l1(&mut mem, 0x123, SECTION | SECTION_AP_RW_ALL);
        let mut tlb = TranslationLookasideBuffer::default();
        let mut params = params(&cpu);
        params.tlb_preserve = true;
        params.tlb_victim = 6;
        {
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert_eq!(mmu.map(va, Access::Read).unwrap(), 0x0000_4567);
        }
        assert_eq!(tlb.len(), 1);
        assert!(tlb.lockdown_entries[6].is_some());
        assert!(tlb.entries.is_empty());
    }

    #[test]
    fn guest_mcr_tlbi_invalidates_cached_translation_before_the_next_fetch() {
        let mut mem = FlatMemory::new(0, 0x20_0000);
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.cp15.write(true, 0, 2, 0, 0, L1).unwrap();
        cpu.cp15.write(true, 0, 3, 0, 0, 0b01).unwrap();
        cpu.cp15.write(true, 0, 1, 0, 0, 1 | (1 << 23)).unwrap();

        let va = 0x1230_0000;
        set_l1(&mut mem, 0x123, SECTION | SECTION_AP_RW_ALL);
        mem.write32(0x0000_0000, 0xE1A0_0000).unwrap(); // nop
        mem.write32(0x0000_0004, 0xEE08_1F37).unwrap(); // mcr p15,0,r1,c8,c7,1
        mem.write32(0x0010_0004, 0xE3A0_0003).unwrap(); // Must not run before MCR.
        mem.write32(0x0010_0008, 0xE3A0_0002).unwrap(); // mov r0,#2
        mem.write32(0x0000_0008, 0xE3A0_0001).unwrap(); // Stale mapping would produce #1.

        cpu.regs[1] = va; // MVA with ASID 0.
        cpu.set_pc(va);
        cpu.step(&mut mem).unwrap(); // Fills the section translation.
        set_l1(&mut mem, 0x123, 0x0010_0000 | SECTION | SECTION_AP_RW_ALL);

        cpu.step(&mut mem).unwrap(); // Fetches old cached MCR and queues TLBI.
        assert_eq!(cpu.regs[0], 0, "the remapped instruction must not run yet");

        cpu.step(&mut mem).unwrap(); // Applies TLBI, then fetches from the new section base.
        assert_eq!(cpu.regs[0], 2);
    }

    fn legacy_machine() -> (Arm11Core, FlatMemory) {
        let (mut cpu, mem) = machine();
        // Enable the MMU with the legacy short-descriptor format (XP=0), S=R=0.
        cpu.cp15.write(true, 0, 1, 0, 0, 1).unwrap();
        (cpu, mem)
    }

    fn legacy_ap_fields(ap: [u32; 4]) -> u32 {
        (ap[0] << 4) | (ap[1] << 6) | (ap[2] << 8) | (ap[3] << 10)
    }

    #[test]
    fn legacy_small_page_uses_four_1kb_access_permission_subpages() {
        let (mut cpu, mut mem) = legacy_machine();
        set_l1(&mut mem, 0x200, L2 | COARSE);
        let descriptor = 0x0002_0000 | SMALL | legacy_ap_fields([1, 2, 3, 0]);
        mem.write32(L2, descriptor).unwrap();

        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0000, Access::Write),
            Walk::Mapped(0x0002_0000)
        );
        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Read)),
            0xF
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0400, Access::Read),
            Walk::Mapped(0x0002_0400)
        );
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0400, Access::Write)),
            0xF
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0800, Access::Write),
            Walk::Mapped(0x0002_0800)
        );
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0C00, Access::Read)),
            0xF
        );
    }

    #[test]
    fn legacy_extended_small_page_uses_one_ap_field_for_the_full_4kb() {
        let (mut cpu, mut mem) = legacy_machine();
        set_l1(&mut mem, 0x200, L2 | COARSE);

        // L2[1:0] = 0b11 is an ARMv6 extended-small-page descriptor
        // when used by a coarse table with SCTLR.XP clear.
        let descriptor = 0x0004_0000 | (0b10 << 4) | 0b11;
        mem.write32(L2, descriptor).unwrap();

        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0000, Access::Read),
            Walk::Mapped(0x0004_0000)
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0FFF, Access::Write),
            Walk::Mapped(0x0004_0FFF)
        );

        // AP=10 is privileged read/write and user read-only for the whole page.
        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0400, Access::Read),
            Walk::Mapped(0x0004_0400)
        );
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0400, Access::Write)),
            0xF
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0FFF, Access::Read),
            Walk::Mapped(0x0004_0FFF)
        );
    }

    #[test]
    fn legacy_large_page_uses_four_16kb_access_permission_subpages() {
        let (mut cpu, mut mem) = legacy_machine();
        set_l1(&mut mem, 0x200, L2 | COARSE);
        let descriptor = 0x0003_0000 | 0b01 | legacy_ap_fields([3, 2, 1, 0]);
        for index in 0..16 {
            mem.write32(L2 + index * 4, descriptor).unwrap();
        }

        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0000, Access::Write),
            Walk::Mapped(0x0003_0000)
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_4000, Access::Read),
            Walk::Mapped(0x0003_4000)
        );
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_4000, Access::Write)),
            0xF
        );
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_8000, Access::Read)),
            0xF
        );
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_C000, Access::Read)),
            0xF
        );
    }

    #[test]
    fn legacy_ap_zero_uses_sctlr_s_and_r_access_rules() {
        let (mut cpu, mut mem) = legacy_machine();
        set_l1(&mut mem, 0x200, SECTION);

        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Read)),
            0xD
        );

        cpu.cp15.write(true, 0, 1, 0, 0, 1 | (1 << 9)).unwrap();
        assert!(matches!(
            walk(&cpu, &mut mem, 0x2000_0000, Access::Read),
            Walk::Mapped(_)
        ));
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Write)),
            0xD
        );
        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert!(matches!(
            walk(&cpu, &mut mem, 0x2000_0000, Access::Read),
            Walk::Mapped(_)
        ));

        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.cp15.write(true, 0, 1, 0, 0, 1 | (1 << 8)).unwrap();
        assert!(matches!(
            walk(&cpu, &mut mem, 0x2000_0000, Access::Read),
            Walk::Mapped(_)
        ));
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Write)),
            0xD
        );
        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Read)),
            0xD
        );

        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.cp15
            .write(true, 0, 1, 0, 0, 1 | (1 << 8) | (1 << 9))
            .unwrap();
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2000_0000, Access::Read)),
            0xD
        );
    }

    #[test]
    fn legacy_sctlr_sr_changes_only_affect_entries_after_tlb_invalidation() {
        let (mut cpu, mut mem) = legacy_machine();
        set_l1(&mut mem, 0x200, SECTION);
        let va = 0x2000_0000;
        let mut tlb = TranslationLookasideBuffer::default();

        // The initial AP=00, S=0, R=0 mapping denies all access, but is cached.
        {
            let params = params(&cpu);
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert!(mmu.map(va, Access::Read).is_err());
        }
        assert_eq!(tlb.len(), 1);

        // S=0, R=1 changes AP=00 to read-only for both privilege levels.
        // ARMv6 specifies that already-cached entries retain their old S/R interpretation.
        cpu.cp15.write(true, 0, 1, 0, 0, 1 | (1 << 9)).unwrap();
        {
            let params = params(&cpu);
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert!(mmu.map(va, Access::Read).is_err());
        }

        // After TLBIALL, the next page walk captures the updated S/R controls.
        tlb.maintain(TlbMaintenance::All);
        {
            let params = params(&cpu);
            let mut mmu = MmuBus::new(&mut mem, params, &mut tlb);
            assert_eq!(mmu.map(va, Access::Read).unwrap(), 0);
            assert!(mmu.map(va, Access::Write).is_err());
        }
    }

    #[test]
    fn legacy_section_uses_a_single_ap_field_for_the_entire_megabyte() {
        let (mut cpu, mut mem) = legacy_machine();
        set_l1(&mut mem, 0x200, SECTION | (0b10 << 10));
        assert_eq!(
            walk(&cpu, &mut mem, 0x2000_0000, Access::Read),
            Walk::Mapped(0)
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x200F_FFFF, Access::Read),
            Walk::Mapped(0x000F_FFFF)
        );
        assert_eq!(
            walk(&cpu, &mut mem, 0x2008_0000, Access::Write),
            Walk::Mapped(0x0008_0000)
        );

        // AP=10 is privileged read/write and user read-only.
        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!(
            walk(&cpu, &mut mem, 0x2008_0000, Access::Read),
            Walk::Mapped(0x0008_0000)
        );
        assert_eq!(
            status_of(walk(&cpu, &mut mem, 0x2008_0000, Access::Write)),
            0xD
        );
    }
}
