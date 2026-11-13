//! Independent integration tests for VFP11 FPEXC/FPINST context registers.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

fn execute(cpu: &mut Arm11Core, instruction: u32) -> aceris_core::Result<()> {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, instruction)?;
    cpu.set_pc(0);
    cpu.step(&mut memory)?;
    Ok(())
}

#[test]
fn privileged_mode_can_read_write_fpexc_fpinst_and_fpinst2_while_vfp_is_disabled() {
    let mut cpu = Arm11Core::new();
    cpu.cp15_mut().grant_vfp_access();
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();

    // FPINST has a defined VFP11 reset value; FPINST2 is architecturally
    // UNPREDICTABLE on reset, so the emulator chooses deterministic zero.
    execute(&mut cpu, 0xEEF9_0A10).unwrap(); // VMRS r0, FPINST
    assert_eq!(cpu.regs[0], 0xEE00_0A00);
    execute(&mut cpu, 0xEEFA_1A10).unwrap(); // VMRS r1, FPINST2
    assert_eq!(cpu.regs[1], 0);

    // All implemented FPEXC fields can be restored; SBZ bits remain zero.
    cpu.regs[2] = u32::MAX;
    execute(&mut cpu, 0xEEE8_2A10).unwrap(); // VMSR FPEXC, r2
    execute(&mut cpu, 0xEEF8_3A10).unwrap(); // VMRS r3, FPEXC
    assert_eq!(cpu.regs[3], 0xD000_078D);

    cpu.regs[4] = 0x1234_5678;
    execute(&mut cpu, 0xEEE9_4A10).unwrap(); // VMSR FPINST, r4
    execute(&mut cpu, 0xEEF9_5A10).unwrap(); // VMRS r5, FPINST
    assert_eq!(cpu.regs[5], 0x1234_5678);

    cpu.regs[6] = 0x89AB_CDEF;
    execute(&mut cpu, 0xEEEA_6A10).unwrap(); // VMSR FPINST2, r6
    execute(&mut cpu, 0xEEFA_7A10).unwrap(); // VMRS r7, FPINST2
    assert_eq!(cpu.regs[7], 0x89AB_CDEF);
    assert_eq!(cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
}

#[test]
fn fpexc_fpinst_and_fpinst2_require_privilege_even_when_vfp_is_enabled() {
    let mut cpu = Arm11Core::new();
    cpu.cp15_mut().grant_vfp_access();
    cpu.vfp_mut().set_enabled(true);
    cpu.regs[1] = 0xCAFE_BABE;
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;

    // User mode cannot access the three privileged context registers.
    for instruction in [
        0xEEF8_0A10, // VMRS r0, FPEXC
        0xEEF9_0A10, // VMRS r0, FPINST
        0xEEFA_0A10, // VMRS r0, FPINST2
        0xEEE8_1A10, // VMSR FPEXC, r1
        0xEEE9_1A10, // VMSR FPINST, r1
        0xEEEA_1A10, // VMSR FPINST2, r1
    ] {
        assert!(
            matches!(
                execute(&mut cpu, instruction),
                Err(Error::UndefinedInstruction { .. })
            ),
            "User-mode access must be undefined: {instruction:#010x}"
        );
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
        assert_eq!(cpu.pc(), 0, "{instruction:#010x}");
    }
}

#[test]
fn privileged_context_register_access_does_not_depend_on_fpexc_en() {
    let mut cpu = Arm11Core::new();
    cpu.cp15_mut().grant_vfp_access();
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    assert!(!cpu.vfp().enabled());

    cpu.regs[1] = 0x0102_0304;
    execute(&mut cpu, 0xEEE9_1A10).unwrap(); // VMSR FPINST, r1
    execute(&mut cpu, 0xEEF9_2A10).unwrap(); // VMRS r2, FPINST
    assert_eq!(cpu.regs[2], 0x0102_0304);

    cpu.regs[3] = 0x5566_7788;
    execute(&mut cpu, 0xEEEA_3A10).unwrap(); // VMSR FPINST2, r3
    execute(&mut cpu, 0xEEFA_4A10).unwrap(); // VMRS r4, FPINST2
    assert_eq!(cpu.regs[4], 0x5566_7788);

    cpu.regs[5] = 1 << 31;
    execute(&mut cpu, 0xEEE8_5A10).unwrap(); // VMSR FPEXC, r5
    execute(&mut cpu, 0xEEF8_6A10).unwrap(); // VMRS r6, FPEXC
    assert_eq!(cpu.regs[6], 1 << 31);
    assert!(!cpu.vfp().enabled());
}
