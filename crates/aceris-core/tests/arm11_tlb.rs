//! Guest-visible ARM11 unified TLB maintenance regression tests.

use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

const L1_BASE: u32 = 0x4000;
const MEMORY_SIZE: usize = 0x0040_0000;
const SECTION_RW: u32 = 0b10 | (0b11 << 10);
const SECTION_NOT_GLOBAL: u32 = 1 << 17;

fn mcr(rd: u32, crn: u32, crm: u32, opc2: u32) -> u32 {
    0xEE00_0F10 | (crn << 16) | (rd << 12) | (opc2 << 5) | crm
}

fn section_descriptor(physical_base: u32, non_global: bool) -> u32 {
    (physical_base & 0xFFF0_0000) | SECTION_RW | if non_global { SECTION_NOT_GLOBAL } else { 0 }
}

fn mmu_machine() -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, MEMORY_SIZE);
    memory
        .write32(L1_BASE, section_descriptor(0, false))
        .unwrap();

    // Configure TTBR0, DACR (domain 0 = client), then enable SCTLR.M and XP.
    memory.write32(0x00, mcr(0, 2, 0, 0)).unwrap();
    memory.write32(0x04, mcr(1, 3, 0, 0)).unwrap();
    memory.write32(0x08, mcr(2, 1, 0, 0)).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    cpu.regs[0] = L1_BASE;
    cpu.regs[1] = 0b01;
    cpu.regs[2] = 1 | (1 << 23);
    cpu.set_pc(0);

    for _ in 0..3 {
        cpu.step(&mut memory).unwrap();
    }

    assert!(cpu.cp15().mmu_enabled());
    (cpu, memory)
}

#[test]
fn guest_tlbi_asid_invalidates_non_global_entries_but_preserves_global_entries() {
    const NON_GLOBAL_VA: u32 = 0x1230_0100;
    const GLOBAL_VA: u32 = 0x1240_0200;
    const OLD_PHYSICAL: u32 = 0x0010_0000;
    const NEW_PHYSICAL: u32 = 0x0020_0000;

    let (mut cpu, mut memory) = mmu_machine();

    memory
        .write32(L1_BASE + 0x123 * 4, section_descriptor(OLD_PHYSICAL, true))
        .unwrap();
    memory
        .write32(L1_BASE + 0x124 * 4, section_descriptor(OLD_PHYSICAL, false))
        .unwrap();
    memory.write32(OLD_PHYSICAL + 0x100, 0x1111_1111).unwrap();
    memory.write32(NEW_PHYSICAL + 0x100, 0x2222_2222).unwrap();
    memory.write32(OLD_PHYSICAL + 0x200, 0x3333_3333).unwrap();
    memory.write32(NEW_PHYSICAL + 0x200, 0x4444_4444).unwrap();

    // Set ASID 7, cache a non-global and a global translation, then invalidate
    // only the entries carrying ASID 7.
    memory.write32(0x100, mcr(4, 13, 0, 1)).unwrap();
    memory.write32(0x104, 0xE590_5000).unwrap(); // LDR r5, [r0]
    memory.write32(0x108, 0xE592_6000).unwrap(); // LDR r6, [r2]
    memory.write32(0x10C, mcr(1, 8, 7, 2)).unwrap(); // TLBIASID r1
    memory.write32(0x110, 0xE590_7000).unwrap(); // LDR r7, [r0]
    memory.write32(0x114, 0xE592_8000).unwrap(); // LDR r8, [r2]

    cpu.regs[0] = NON_GLOBAL_VA;
    cpu.regs[1] = 7;
    cpu.regs[2] = GLOBAL_VA;
    cpu.regs[4] = 7;
    cpu.set_pc(0x100);

    for _ in 0..3 {
        cpu.step(&mut memory).unwrap();
    }
    assert_eq!(cpu.regs[5], 0x1111_1111);
    assert_eq!(cpu.regs[6], 0x3333_3333);

    // Remap both sections. TLBIASID must discard the ASID-specific entry,
    // but the global entry must continue using the translation already cached.
    memory
        .write32(L1_BASE + 0x123 * 4, section_descriptor(NEW_PHYSICAL, true))
        .unwrap();
    memory
        .write32(L1_BASE + 0x124 * 4, section_descriptor(NEW_PHYSICAL, false))
        .unwrap();

    cpu.step(&mut memory).unwrap(); // MCR TLBIASID queues the operation.
    cpu.step(&mut memory).unwrap(); // Applies TLBIASID, then loads non-global.
    cpu.step(&mut memory).unwrap(); // The global entry remains cached.

    assert_eq!(cpu.regs[7], 0x2222_2222);
    assert_eq!(cpu.regs[8], 0x3333_3333);
}

#[test]
fn guest_tlbi_all_preserves_locked_entries_until_mva_asid_invalidates_them() {
    const DATA_VA: u32 = 0x1250_0100;
    const OLD_PHYSICAL: u32 = 0x0010_0000;
    const NEW_PHYSICAL: u32 = 0x0020_0000;

    let (mut cpu, mut memory) = mmu_machine();
    memory
        .write32(L1_BASE + 0x125 * 4, section_descriptor(OLD_PHYSICAL, true))
        .unwrap();
    memory.write32(OLD_PHYSICAL + 0x100, 0xAAAA_0001).unwrap();
    memory.write32(NEW_PHYSICAL + 0x100, 0xBBBB_0002).unwrap();

    // First lock the code mapping in slot 2. TLBIALL removes its initial
    // unlocked copy, so the next instruction fetch refills that slot.
    memory.write32(0x100, mcr(4, 10, 0, 0)).unwrap(); // Set P, victim = 2.
    memory.write32(0x104, mcr(4, 8, 7, 0)).unwrap(); // TLBIALL.
    memory.write32(0x108, mcr(5, 10, 0, 0)).unwrap(); // Set P, victim = 3.
    memory.write32(0x10C, 0xE590_2000).unwrap(); // LDR r2, [r0], locks data in slot 3.
    memory.write32(0x110, mcr(4, 8, 7, 0)).unwrap(); // TLBIALL.
    memory.write32(0x114, 0xE590_3000).unwrap(); // LDR r3, [r0].
    memory.write32(0x118, mcr(1, 8, 7, 1)).unwrap(); // TLBIMVA r1.
    memory.write32(0x11C, 0xE590_6000).unwrap(); // LDR r6, [r0].

    cpu.regs[0] = DATA_VA;
    cpu.regs[1] = DATA_VA; // MVA plus current ASID 0.
    cpu.regs[4] = 0x8800_0000; // Preserve, victim slot 2.
    cpu.regs[5] = 0x8C00_0000; // Preserve, victim slot 3.
    cpu.set_pc(0x100);

    for _ in 0..4 {
        cpu.step(&mut memory).unwrap();
    }
    assert_eq!(cpu.regs[2], 0xAAAA_0001);

    memory
        .write32(L1_BASE + 0x125 * 4, section_descriptor(NEW_PHYSICAL, true))
        .unwrap();

    cpu.step(&mut memory).unwrap(); // TLBIALL.
    cpu.step(&mut memory).unwrap(); // Locked data mapping survives TLBIALL.
    assert_eq!(cpu.regs[3], 0xAAAA_0001);

    cpu.step(&mut memory).unwrap(); // TLBIMVA queues single-entry invalidation.
    cpu.step(&mut memory).unwrap(); // Locked data mapping is invalidated and refilled.
    assert_eq!(cpu.regs[6], 0xBBBB_0002);
}

#[test]
fn guest_tlb_keeps_same_virtual_address_per_asid_and_mva_invalidation_is_asid_scoped() {
    const DATA_VA: u32 = 0x1230_0100;
    const SECOND_L1_BASE: u32 = 0x8000;
    const ASID_ONE_PHYSICAL: u32 = 0x0010_0000;
    const ASID_TWO_PHYSICAL: u32 = 0x0020_0000;
    const ASID_ONE_NEW_PHYSICAL: u32 = 0x0030_0000;
    const ASID_TWO_NEW_PHYSICAL: u32 = 0x0000_0000;

    let (mut cpu, mut memory) = mmu_machine();

    // Both address spaces execute the same code section, but map the same
    // data VA to different physical sections with non-global descriptors.
    memory
        .write32(L1_BASE + 0x300 * 4, section_descriptor(0, false))
        .unwrap();
    memory
        .write32(SECOND_L1_BASE, section_descriptor(0, false))
        .unwrap();
    memory
        .write32(SECOND_L1_BASE + 0x300 * 4, section_descriptor(0, false))
        .unwrap();
    memory
        .write32(
            L1_BASE + 0x123 * 4,
            section_descriptor(ASID_ONE_PHYSICAL, true),
        )
        .unwrap();
    memory
        .write32(
            SECOND_L1_BASE + 0x123 * 4,
            section_descriptor(ASID_TWO_PHYSICAL, true),
        )
        .unwrap();

    memory
        .write32(ASID_ONE_PHYSICAL + 0x100, 0x1111_0001)
        .unwrap();
    memory
        .write32(ASID_TWO_PHYSICAL + 0x100, 0x2222_0002)
        .unwrap();
    memory
        .write32(ASID_ONE_NEW_PHYSICAL + 0x100, 0x3333_0003)
        .unwrap();
    memory
        .write32(ASID_TWO_NEW_PHYSICAL + 0x100, 0x4444_0004)
        .unwrap();

    // ASID 1 -> TTBR0 A; ASID 2 -> TTBR0 B; return to ASID 1.
    memory.write32(0x100, 0xE3A0_4001).unwrap(); // MOV r4, #1
    memory.write32(0x104, mcr(4, 13, 0, 1)).unwrap(); // CONTEXTIDR = 1
    memory.write32(0x108, 0xE590_5000).unwrap(); // LDR r5, [r0]
    memory.write32(0x10C, mcr(3, 2, 0, 0)).unwrap(); // TTBR0 = B
    memory.write32(0x110, 0xE3A0_4002).unwrap(); // MOV r4, #2
    memory.write32(0x114, mcr(4, 13, 0, 1)).unwrap(); // CONTEXTIDR = 2
    memory.write32(0x118, 0xE590_6000).unwrap(); // LDR r6, [r0]
    memory.write32(0x11C, mcr(2, 2, 0, 0)).unwrap(); // TTBR0 = A
    memory.write32(0x120, 0xE3A0_4001).unwrap(); // MOV r4, #1
    memory.write32(0x124, mcr(4, 13, 0, 1)).unwrap(); // CONTEXTIDR = 1
    memory.write32(0x128, 0xE590_7000).unwrap(); // LDR r7, [r0]
    memory.write32(0x12C, mcr(1, 8, 7, 1)).unwrap(); // TLBIMVA r1 (MVA + ASID).
    memory.write32(0x130, 0xE590_8000).unwrap(); // LDR r8, [r0].
    memory.write32(0x134, mcr(3, 2, 0, 0)).unwrap(); // TTBR0 = B.
    memory.write32(0x138, 0xE3A0_4002).unwrap(); // MOV r4, #2.
    memory.write32(0x13C, mcr(4, 13, 0, 1)).unwrap(); // CONTEXTIDR = 2.
    memory.write32(0x140, 0xE590_9000).unwrap(); // LDR r9, [r0].

    // The high code VA is deliberately outside the FCSE-remapped low window.
    cpu.regs[0] = DATA_VA;
    cpu.regs[1] = (DATA_VA & 0xFFFF_F000) | 1;
    cpu.regs[2] = L1_BASE;
    cpu.regs[3] = SECOND_L1_BASE;
    cpu.set_pc(0x3000_0100);

    for _ in 0..11 {
        cpu.step(&mut memory).unwrap();
    }
    assert_eq!(cpu.regs[5], 0x1111_0001);
    assert_eq!(cpu.regs[6], 0x2222_0002);
    assert_eq!(cpu.regs[7], 0x1111_0001);

    // Change both page-table entries. MVA+ASID invalidation must replace only
    // the ASID 1 translation; ASID 2 must keep its already-cached mapping.
    memory
        .write32(
            L1_BASE + 0x123 * 4,
            section_descriptor(ASID_ONE_NEW_PHYSICAL, true),
        )
        .unwrap();
    memory
        .write32(
            SECOND_L1_BASE + 0x123 * 4,
            section_descriptor(ASID_TWO_NEW_PHYSICAL, true),
        )
        .unwrap();

    cpu.step(&mut memory).unwrap(); // Queues MVA+ASID invalidation for ASID 1.
    cpu.step(&mut memory).unwrap(); // ASID 1 reloads from its updated L1 table.
    assert_eq!(cpu.regs[8], 0x3333_0003);

    cpu.step(&mut memory).unwrap(); // Switch TTBR0 back to B.
    cpu.step(&mut memory).unwrap(); // MOV r4, #2.
    cpu.step(&mut memory).unwrap(); // Switch CONTEXTIDR to ASID 2.
    cpu.step(&mut memory).unwrap(); // ASID 2 still uses its cached old mapping.
    assert_eq!(cpu.regs[6], 0x2222_0002);
    assert_eq!(cpu.regs[9], 0x2222_0002);
}

#[test]
fn guest_fcse_pid_separates_low_virtual_address_tlb_entries_and_tlbi_uses_mva() {
    const DATA_VA: u32 = 0x0010_0120;
    const PID_ONE: u32 = 0x0200_0000;
    const PID_TWO: u32 = 0x0400_0000;
    const MVA_ONE: u32 = PID_ONE | DATA_VA;
    const PID_ONE_PHYSICAL: u32 = 0x0010_0000;
    const PID_TWO_PHYSICAL: u32 = 0x0020_0000;
    const PID_ONE_NEW_PHYSICAL: u32 = 0x0030_0000;

    let (mut cpu, mut memory) = mmu_machine();

    // Keep instruction fetches outside the FCSE window; data accesses use
    // the same low VA under two different process identifiers.
    memory
        .write32(L1_BASE + 0x300 * 4, section_descriptor(0, false))
        .unwrap();
    memory
        .write32(
            L1_BASE + 0x21 * 4,
            section_descriptor(PID_ONE_PHYSICAL, false),
        )
        .unwrap();
    memory
        .write32(
            L1_BASE + 0x41 * 4,
            section_descriptor(PID_TWO_PHYSICAL, false),
        )
        .unwrap();

    memory
        .write32(PID_ONE_PHYSICAL + 0x120, 0xAAAA_0001)
        .unwrap();
    memory
        .write32(PID_TWO_PHYSICAL + 0x120, 0xBBBB_0002)
        .unwrap();
    memory
        .write32(PID_ONE_NEW_PHYSICAL + 0x120, 0xCCCC_0003)
        .unwrap();

    memory.write32(0x100, mcr(1, 13, 0, 0)).unwrap(); // FCSE PID = r1.
    memory.write32(0x104, 0xE590_5000).unwrap(); // LDR r5, [r0].
    memory.write32(0x108, mcr(2, 13, 0, 0)).unwrap(); // FCSE PID = r2.
    memory.write32(0x10C, 0xE590_6000).unwrap(); // LDR r6, [r0].
    memory.write32(0x110, mcr(1, 13, 0, 0)).unwrap(); // FCSE PID = PID_ONE.
    memory.write32(0x114, 0xE590_7000).unwrap(); // LDR r7, [r0].
    memory.write32(0x118, mcr(3, 8, 7, 3)).unwrap(); // TLBIMVAA r3.
    memory.write32(0x11C, 0xE590_8000).unwrap(); // LDR r8, [r0].
    memory.write32(0x120, mcr(2, 13, 0, 0)).unwrap(); // FCSE PID = PID_TWO.
    memory.write32(0x124, 0xE590_9000).unwrap(); // LDR r9, [r0].

    cpu.regs[0] = DATA_VA;
    cpu.regs[1] = PID_ONE;
    cpu.regs[2] = PID_TWO;
    cpu.regs[3] = MVA_ONE; // TLBI uses the modified VA, not the low true VA.
    cpu.set_pc(0x3000_0100);

    for _ in 0..6 {
        cpu.step(&mut memory).unwrap();
    }
    assert_eq!(cpu.regs[5], 0xAAAA_0001);
    assert_eq!(cpu.regs[6], 0xBBBB_0002);
    assert_eq!(cpu.regs[7], 0xAAAA_0001);

    // Only PID_ONE's modified VA is remapped. The guest invalidates by MVA;
    // the entry for PID_TWO must remain cached at its prior physical address.
    memory
        .write32(
            L1_BASE + 0x21 * 4,
            section_descriptor(PID_ONE_NEW_PHYSICAL, false),
        )
        .unwrap();

    cpu.step(&mut memory).unwrap(); // TLBIMVAA for MVA_ONE.
    cpu.step(&mut memory).unwrap(); // PID_ONE sees the updated translation.
    assert_eq!(cpu.regs[8], 0xCCCC_0003);

    cpu.step(&mut memory).unwrap(); // Switch to PID_TWO.
    cpu.step(&mut memory).unwrap(); // PID_TWO retains its cached mapping.
    assert_eq!(cpu.regs[9], 0xBBBB_0002);
}

#[test]
fn guest_tlbimvaa_invalidates_only_the_target_small_page_inside_a_coarse_table() {
    const L2_BASE: u32 = 0x8000;
    const DATA_VA: u32 = 0x1234_5100;
    const SIBLING_VA: u32 = DATA_VA + 0x1000;
    const OLD_PHYSICAL: u32 = 0x0012_3000;
    const SIBLING_OLD_PHYSICAL: u32 = 0x0013_4000;
    const NEW_PHYSICAL: u32 = 0x0023_4000;
    const SIBLING_NEW_PHYSICAL: u32 = 0x0024_5000;
    const SMALL_PAGE: u32 = 0b10 | (0b11 << 4) | (1 << 11);

    let (mut cpu, mut memory) = mmu_machine();

    memory
        .write32(L1_BASE + 0x300 * 4, section_descriptor(0, false))
        .unwrap();
    memory.write32(L1_BASE + 0x123 * 4, L2_BASE | 0b01).unwrap();

    let data_l2_index = ((DATA_VA >> 12) & 0xFF) as usize;
    let sibling_l2_index = ((SIBLING_VA >> 12) & 0xFF) as usize;
    memory
        .write32(
            L2_BASE + data_l2_index as u32 * 4,
            (OLD_PHYSICAL & 0xFFFF_F000) | SMALL_PAGE,
        )
        .unwrap();
    memory
        .write32(
            L2_BASE + sibling_l2_index as u32 * 4,
            (SIBLING_OLD_PHYSICAL & 0xFFFF_F000) | SMALL_PAGE,
        )
        .unwrap();

    memory.write32(OLD_PHYSICAL + 0x100, 0x1111_1001).unwrap();
    memory
        .write32(SIBLING_OLD_PHYSICAL + 0x100, 0x2222_2002)
        .unwrap();
    memory.write32(NEW_PHYSICAL + 0x100, 0x3333_3003).unwrap();
    memory
        .write32(SIBLING_NEW_PHYSICAL + 0x100, 0x4444_4004)
        .unwrap();

    memory.write32(0x100, 0xE590_5000).unwrap(); // LDR r5, [r0].
    memory.write32(0x104, 0xE592_6000).unwrap(); // LDR r6, [r2].
    memory.write32(0x108, mcr(3, 8, 7, 3)).unwrap(); // TLBIMVAA r3.
    memory.write32(0x10C, 0xE590_7000).unwrap(); // LDR r7, [r0].
    memory.write32(0x110, 0xE592_8000).unwrap(); // LDR r8, [r2].

    cpu.regs[0] = DATA_VA;
    cpu.regs[2] = SIBLING_VA;
    cpu.regs[3] = DATA_VA & 0xFFFF_F000;
    cpu.set_pc(0x3000_0100);

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[5], 0x1111_1001);
    assert_eq!(cpu.regs[6], 0x2222_2002);

    // Remap both adjacent 4 KiB pages, then invalidate only the first MVA.
    memory
        .write32(
            L2_BASE + data_l2_index as u32 * 4,
            (NEW_PHYSICAL & 0xFFFF_F000) | SMALL_PAGE,
        )
        .unwrap();
    memory
        .write32(
            L2_BASE + sibling_l2_index as u32 * 4,
            (SIBLING_NEW_PHYSICAL & 0xFFFF_F000) | SMALL_PAGE,
        )
        .unwrap();

    cpu.step(&mut memory).unwrap(); // Queue TLBIMVAA.
    cpu.step(&mut memory).unwrap(); // Invalidate target page, then read new PA.
    cpu.step(&mut memory).unwrap(); // Sibling translation remains cached.

    assert_eq!(cpu.regs[7], 0x3333_3003);
    assert_eq!(cpu.regs[8], 0x2222_2002);
}

#[test]
fn guest_tlbimvaa_matches_an_address_inside_a_large_page_and_preserves_neighbor_page() {
    const L2_BASE: u32 = 0x8000;
    const DATA_VA: u32 = 0x1235_2340;
    const SIBLING_VA: u32 = DATA_VA + 0x1_0000;
    const OLD_PHYSICAL: u32 = 0x0010_0000;
    const SIBLING_OLD_PHYSICAL: u32 = 0x0011_0000;
    const NEW_PHYSICAL: u32 = 0x0020_0000;
    const SIBLING_NEW_PHYSICAL: u32 = 0x0021_0000;
    const LARGE_PAGE: u32 = 0b01 | (0b11 << 4) | (1 << 11);
    const LARGE_PAGE_MASK: u32 = 0xFFFF_0000;
    const DATA_PAGE_INDEX: usize = ((DATA_VA >> 12) & 0xFF) as usize;
    const SIBLING_PAGE_INDEX: usize = ((SIBLING_VA >> 12) & 0xFF) as usize;
    const DATA_GROUP_START: usize = DATA_PAGE_INDEX & !0xF;
    const SIBLING_GROUP_START: usize = SIBLING_PAGE_INDEX & !0xF;
    const PAGE_OFFSET: u32 = DATA_VA & 0xFFFF;

    let (mut cpu, mut memory) = mmu_machine();

    memory
        .write32(L1_BASE + 0x300 * 4, section_descriptor(0, false))
        .unwrap();
    memory.write32(L1_BASE + 0x123 * 4, L2_BASE | 0b01).unwrap();

    for index in 0..16 {
        memory
            .write32(
                L2_BASE + (DATA_GROUP_START + index) as u32 * 4,
                (OLD_PHYSICAL & LARGE_PAGE_MASK) | LARGE_PAGE,
            )
            .unwrap();
        memory
            .write32(
                L2_BASE + (SIBLING_GROUP_START + index) as u32 * 4,
                (SIBLING_OLD_PHYSICAL & LARGE_PAGE_MASK) | LARGE_PAGE,
            )
            .unwrap();
    }

    memory
        .write32(OLD_PHYSICAL + PAGE_OFFSET, 0xAAAA_1001)
        .unwrap();
    memory
        .write32(SIBLING_OLD_PHYSICAL + PAGE_OFFSET, 0xBBBB_2002)
        .unwrap();
    memory
        .write32(NEW_PHYSICAL + PAGE_OFFSET, 0xCCCC_3003)
        .unwrap();
    memory
        .write32(SIBLING_NEW_PHYSICAL + PAGE_OFFSET, 0xDDDD_4004)
        .unwrap();

    memory.write32(0x100, 0xE590_5000).unwrap(); // LDR r5, [r0].
    memory.write32(0x104, 0xE592_6000).unwrap(); // LDR r6, [r2].
    memory.write32(0x108, mcr(3, 8, 7, 3)).unwrap(); // TLBIMVAA r3.
    memory.write32(0x10C, 0xE590_7000).unwrap(); // LDR r7, [r0].
    memory.write32(0x110, 0xE592_8000).unwrap(); // LDR r8, [r2].

    cpu.regs[0] = DATA_VA;
    cpu.regs[2] = SIBLING_VA;
    // The encoded MVA points at another 4 KiB offset inside the same 64 KiB
    // large page. The TLB must compare against the full large-page range.
    cpu.regs[3] = (DATA_VA + 0xA000) & 0xFFFF_F000;
    cpu.set_pc(0x3000_0100);

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[5], 0xAAAA_1001);
    assert_eq!(cpu.regs[6], 0xBBBB_2002);

    // Both large-page descriptors are updated. Only the first mapping's
    // TLB entry should be invalidated by an MVA inside its 64 KiB range.
    for index in 0..16 {
        memory
            .write32(
                L2_BASE + (DATA_GROUP_START + index) as u32 * 4,
                (NEW_PHYSICAL & LARGE_PAGE_MASK) | LARGE_PAGE,
            )
            .unwrap();
        memory
            .write32(
                L2_BASE + (SIBLING_GROUP_START + index) as u32 * 4,
                (SIBLING_NEW_PHYSICAL & LARGE_PAGE_MASK) | LARGE_PAGE,
            )
            .unwrap();
    }

    cpu.step(&mut memory).unwrap(); // Queue TLBIMVAA with an interior large-page MVA.
    cpu.step(&mut memory).unwrap(); // Re-walk target and read the new physical page.
    cpu.step(&mut memory).unwrap(); // Neighbor large page remains cached.

    assert_eq!(cpu.regs[7], 0xCCCC_3003);
    assert_eq!(cpu.regs[8], 0xBBBB_2002);
}

#[test]
fn guest_tlbimvaa_invalidates_the_same_mva_across_all_asids() {
    const DATA_VA: u32 = 0x1230_0100;
    const SECOND_L1_BASE: u32 = 0x8000;
    const ASID_ONE_OLD_PHYSICAL: u32 = 0x0010_0000;
    const ASID_TWO_OLD_PHYSICAL: u32 = 0x0020_0000;
    const ASID_ONE_NEW_PHYSICAL: u32 = 0x0030_0000;
    const ASID_TWO_NEW_PHYSICAL: u32 = 0x0000_0000;

    let (mut cpu, mut memory) = mmu_machine();

    // Both processes execute from the same high virtual section. Their data
    // page tables map the same data VA to different physical sections.
    memory
        .write32(L1_BASE + 0x300 * 4, section_descriptor(0, false))
        .unwrap();
    memory
        .write32(SECOND_L1_BASE, section_descriptor(0, false))
        .unwrap();
    memory
        .write32(SECOND_L1_BASE + 0x300 * 4, section_descriptor(0, false))
        .unwrap();
    memory
        .write32(
            L1_BASE + 0x123 * 4,
            section_descriptor(ASID_ONE_OLD_PHYSICAL, true),
        )
        .unwrap();
    memory
        .write32(
            SECOND_L1_BASE + 0x123 * 4,
            section_descriptor(ASID_TWO_OLD_PHYSICAL, true),
        )
        .unwrap();

    memory
        .write32(ASID_ONE_OLD_PHYSICAL + 0x100, 0x1111_0001)
        .unwrap();
    memory
        .write32(ASID_TWO_OLD_PHYSICAL + 0x100, 0x2222_0002)
        .unwrap();
    memory
        .write32(ASID_ONE_NEW_PHYSICAL + 0x100, 0x3333_0003)
        .unwrap();
    memory
        .write32(ASID_TWO_NEW_PHYSICAL + 0x100, 0x4444_0004)
        .unwrap();

    // Populate the same MVA under ASID 1 and ASID 2, then return to ASID 1
    // and execute TLBIMVAA (c8,c7,3) for that MVA.
    memory.write32(0x100, 0xE3A0_4001).unwrap(); // MOV r4, #1
    memory.write32(0x104, mcr(4, 13, 0, 1)).unwrap(); // CONTEXTIDR = 1
    memory.write32(0x108, 0xE590_5000).unwrap(); // LDR r5, [r0]
    memory.write32(0x10C, mcr(3, 2, 0, 0)).unwrap(); // TTBR0 = B
    memory.write32(0x110, 0xE3A0_4002).unwrap(); // MOV r4, #2
    memory.write32(0x114, mcr(4, 13, 0, 1)).unwrap(); // CONTEXTIDR = 2
    memory.write32(0x118, 0xE590_6000).unwrap(); // LDR r6, [r0]
    memory.write32(0x11C, mcr(2, 2, 0, 0)).unwrap(); // TTBR0 = A
    memory.write32(0x120, 0xE3A0_4001).unwrap(); // MOV r4, #1
    memory.write32(0x124, mcr(4, 13, 0, 1)).unwrap(); // CONTEXTIDR = 1
    memory.write32(0x128, 0xE590_7000).unwrap(); // LDR r7, [r0]
    memory.write32(0x12C, mcr(1, 8, 7, 3)).unwrap(); // TLBIMVAA r1.
    memory.write32(0x130, 0xE590_8000).unwrap(); // LDR r8, [r0]
    memory.write32(0x134, mcr(3, 2, 0, 0)).unwrap(); // TTBR0 = B
    memory.write32(0x138, 0xE3A0_4002).unwrap(); // MOV r4, #2
    memory.write32(0x13C, mcr(4, 13, 0, 1)).unwrap(); // CONTEXTIDR = 2
    memory.write32(0x140, 0xE590_9000).unwrap(); // LDR r9, [r0]

    cpu.regs[0] = DATA_VA;
    cpu.regs[1] = DATA_VA & 0xFFFF_F000; // TLBIMVAA takes an MVA, not an ASID.
    cpu.regs[2] = L1_BASE;
    cpu.regs[3] = SECOND_L1_BASE;
    cpu.set_pc(0x3000_0100);

    for _ in 0..11 {
        cpu.step(&mut memory).unwrap();
    }
    assert_eq!(cpu.regs[5], 0x1111_0001);
    assert_eq!(cpu.regs[6], 0x2222_0002);
    assert_eq!(cpu.regs[7], 0x1111_0001);

    // Change both page-table mappings after both ASID-specific entries exist.
    memory
        .write32(
            L1_BASE + 0x123 * 4,
            section_descriptor(ASID_ONE_NEW_PHYSICAL, true),
        )
        .unwrap();
    memory
        .write32(
            SECOND_L1_BASE + 0x123 * 4,
            section_descriptor(ASID_TWO_NEW_PHYSICAL, true),
        )
        .unwrap();

    cpu.step(&mut memory).unwrap(); // Queues MVA-only invalidation.
    cpu.step(&mut memory).unwrap(); // ASID 1 must walk its updated mapping.
    assert_eq!(cpu.regs[8], 0x3333_0003);

    cpu.step(&mut memory).unwrap(); // Switch TTBR0 back to B.
    cpu.step(&mut memory).unwrap(); // MOV r4, #2.
    cpu.step(&mut memory).unwrap(); // Switch CONTEXTIDR to ASID 2.
    cpu.step(&mut memory).unwrap(); // ASID 2 must also walk its updated mapping.
    assert_eq!(cpu.regs[9], 0x4444_0004);
}

