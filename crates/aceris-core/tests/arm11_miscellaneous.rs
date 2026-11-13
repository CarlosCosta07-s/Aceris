//! Independent integration coverage for reserved A32 miscellaneous encodings.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

#[test]
fn reserved_compare_space_encodings_are_undefined_without_side_effects() {
    // TST/TEQ/CMP/CMN with S=0 enter the A32 miscellaneous decode space.
    // These four encodings do not select a defined ARMv6/ARMv6K instruction.
    for instruction in [0xE100_0000, 0xE120_0000, 0xE140_0000, 0xE160_0000] {
        let mut memory = FlatMemory::new(0, 0x40);
        memory.write32(0, instruction).unwrap();
        memory.write32(0x20, 0xA5A5_5A5A).unwrap();

        let mut cpu = Arm11Core::new();
        cpu.regs[0] = 0x0123_4567;
        cpu.regs[1] = 0x89AB_CDEF;
        cpu.regs[13] = 0x1000;
        cpu.regs[14] = 0x2000;
        cpu.cpsr.set_flags(true, false, true, true);
        cpu.cpsr.set_q();
        cpu.cpsr.set_ge(0b1010);

        let registers_before = cpu.regs;
        let status_before = cpu.cpsr;
        let data_before = memory.read32(0x20).unwrap();

        assert!(
            matches!(
                cpu.step(&mut memory),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
            ),
            "{instruction:#010x}"
        );
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
        assert_eq!(cpu.pc(), 0, "{instruction:#010x}");
        assert_eq!(
            memory.read32(0x20).unwrap(),
            data_before,
            "{instruction:#010x}"
        );
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
    }
}
