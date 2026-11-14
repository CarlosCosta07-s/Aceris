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
