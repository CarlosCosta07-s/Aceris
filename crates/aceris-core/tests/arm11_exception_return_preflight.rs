//! Independent integration tests for A32 LDM exception-return preflight.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

const DATA: u32 = 0x200;
const LDMIA_R4_WB_R0_PC_S: u32 = 0xE8F4_8001;

fn machine(saved_status: Option<Psr>) -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, 0x1000);
    memory.write32(0, LDMIA_R4_WB_R0_PC_S).unwrap();
    memory.write32(DATA, 0xA1B2_C3D4).unwrap();
    memory.write32(DATA + 4, 0x300).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    cpu.regs[0] = 0x1111_2222;
    cpu.regs[4] = DATA;
    cpu.regs[14] = 0x5555_6666;
    if let Some(status) = saved_status {
        cpu.set_saved_program_status(status).unwrap();
    }
    cpu.set_pc(0);
    (cpu, memory)
}

#[test]
fn ldm_exception_return_requires_initialized_spsr_before_register_writeback() {
    let (mut cpu, mut memory) = machine(None);
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::Unimplemented(_))
    ));

    assert_eq!(cpu.regs, registers_before);
    assert_eq!(cpu.cpsr, status_before);
    assert_eq!(cpu.saved_program_status(), None);
    assert_eq!(cpu.pc(), 0);
}

#[test]
fn ldm_exception_return_rejects_invalid_saved_mode_before_commit() {
    let (mut cpu, mut memory) = machine(Some(Psr(0x0000_0016)));
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;
    let saved_before = cpu.saved_program_status();

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction {
            pc: 0,
            word: LDMIA_R4_WB_R0_PC_S
        })
    ));

    assert_eq!(cpu.regs, registers_before);
    assert_eq!(cpu.cpsr, status_before);
    assert_eq!(cpu.saved_program_status(), saved_before);
    assert_eq!(cpu.pc(), 0);
}

#[test]
fn ldm_exception_return_with_valid_spsr_commits_registers_and_restores_user_mode() {
    let saved = Psr(Psr::USER_MODE | Psr::N);
    let (mut cpu, mut memory) = machine(Some(saved));

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr, saved);
    assert_eq!(cpu.regs[0], 0xA1B2_C3D4);
    assert_eq!(cpu.regs[4], DATA + 8);
    assert_eq!(cpu.pc(), 0x300);
    assert_eq!(cpu.saved_program_status(), None);
}

#[test]
fn ldm_exception_return_loads_multiple_registers_restores_thumb_and_branches() {
    const instruction: u32 = 0xE8F4_8007; // LDMIA r4!, {r0,r1,r2,pc}^
    let saved = Psr(Psr::USER_MODE | Psr::T | Psr::N | Psr::C);
    let mut memory = FlatMemory::new(0, 0x1000);
    memory.write32(0, instruction).unwrap();
    memory.write32(DATA, 0xA0A0_0000).unwrap();
    memory.write32(DATA + 4, 0xB1B1_0001).unwrap();
    memory.write32(DATA + 8, 0xC2C2_0002).unwrap();
    memory.write32(DATA + 12, 0x301).unwrap(); // Thumb target, aligned to 0x300.
    memory.write16(0x300, 0x2307).unwrap(); // MOVS r3, #7

    let mut cpu = Arm11Core::new();
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    cpu.regs[0] = 0x1111_1111;
    cpu.regs[1] = 0x2222_2222;
    cpu.regs[2] = 0x3333_3333;
    cpu.regs[4] = DATA;
    cpu.set_saved_program_status(saved).unwrap();
    cpu.set_pc(0);

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr, saved);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x300);
    assert_eq!(cpu.regs[0], 0xA0A0_0000);
    assert_eq!(cpu.regs[1], 0xB1B1_0001);
    assert_eq!(cpu.regs[2], 0xC2C2_0002);
    assert_eq!(cpu.regs[4], DATA + 16);
    assert_eq!(cpu.saved_program_status(), None);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[3], 7);
    assert_eq!(cpu.pc(), 0x302);
}
