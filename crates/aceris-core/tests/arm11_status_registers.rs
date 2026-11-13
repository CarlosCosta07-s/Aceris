//! Independent integration tests for ARM CPSR/SPSR instruction behaviour.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

fn execute(cpu: &mut Arm11Core, word: u32) -> aceris_core::Result<()> {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, word)?;
    cpu.set_pc(0);
    cpu.step(&mut memory)?;
    Ok(())
}

#[test]
fn mrs_reads_the_active_cpsr_and_exception_mode_spsr() {
    let mut cpu = Arm11Core::new();
    cpu.cpsr = Psr(Psr::USER_MODE | Psr::N | Psr::C | Psr::E);
    execute(&mut cpu, 0xE10F_2000).unwrap(); // MRS r2, CPSR
    assert_eq!(
        cpu.regs[2],
        Psr(Psr::USER_MODE | Psr::N | Psr::C | Psr::E).0
    );

    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    let saved = Psr(Psr::SYSTEM_MODE | Psr::N | Psr::V | Psr::T | Psr::I);
    cpu.set_saved_program_status(saved).unwrap();
    execute(&mut cpu, 0xE14F_3000).unwrap(); // MRS r3, SPSR
    assert_eq!(cpu.regs[3], saved.0);
}

#[test]
fn user_msr_changes_only_unprivileged_status_fields() {
    let mut cpu = Arm11Core::new();
    cpu.regs[1] = u32::MAX;
    execute(&mut cpu, 0xE12F_F001).unwrap(); // MSR CPSR_fsxc, r1

    let expected =
        Psr::USER_MODE | Psr::N | Psr::Z | Psr::C | Psr::V | Psr::Q | Psr::GE_MASK | Psr::E;
    assert_eq!(cpu.cpsr.0, expected);
}

#[test]
fn privileged_msr_switches_banks_and_partial_spsr_writes_preserve_unselected_fields() {
    let mut cpu = Arm11Core::new();
    cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
    cpu.regs[13] = 0x1000;
    cpu.regs[14] = 0x1004;
    cpu.regs[6] = Psr::IRQ_MODE;

    execute(&mut cpu, 0xE121_F006).unwrap(); // MSR CPSR_c, r6
    assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    assert_eq!((cpu.regs[13], cpu.regs[14]), (0, 0));

    cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
    assert_eq!((cpu.regs[13], cpu.regs[14]), (0x1000, 0x1004));

    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    cpu.set_saved_program_status(Psr(0x1234_0010)).unwrap();
    cpu.regs[4] = 0xA500_0000;
    execute(&mut cpu, 0xE168_F004).unwrap(); // MSR SPSR_f, r4
    execute(&mut cpu, 0xE14F_1000).unwrap(); // MRS r1, SPSR
    assert_eq!(cpu.regs[1], 0xA534_0010);
}

#[test]
fn user_mode_cannot_read_spsr() {
    let mut cpu = Arm11Core::new();
    assert!(matches!(
        execute(&mut cpu, 0xE14F_0000),
        Err(Error::UndefinedInstruction { .. })
    ));
    assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
    assert_eq!(cpu.pc(), 0);
}

#[test]
fn mrs_msr_reject_pc_operands_and_reserved_misc_encodings_atomically() {
    for instruction in [
        0xE10F_F000, // MRS pc, CPSR
        0xE12F_F00F, // MSR CPSR_fsxc, pc
        0xE16F_F00F, // MSR SPSR_fsxc, pc
        0xE10F_00F0, // MRS with reserved low bits
        0xE12F_F010, // MSR register form with reserved bit[4]
    ] {
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.regs[0] = 0x1357_9BDF;
        cpu.regs[15] = 0;
        let registers_before = cpu.regs;
        let status_before = cpu.cpsr;
        let saved_before = cpu.saved_program_status();

        assert!(
            matches!(
                execute(&mut cpu, instruction),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
            ),
            "{instruction:#010x}"
        );

        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
        assert_eq!(
            cpu.saved_program_status(),
            saved_before,
            "{instruction:#010x}"
        );
        assert_eq!(cpu.pc(), 0, "{instruction:#010x}");
    }
}

#[test]
fn partial_spsr_msr_requires_an_initialized_spsr_but_full_write_initializes_it() {
    let mut cpu = Arm11Core::new();
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    cpu.regs[0] = 0xA000_0013;
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;

    assert!(matches!(
        execute(&mut cpu, 0xE168_F000), // MSR SPSR_f, r0
        Err(Error::Unimplemented(_))
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(cpu.cpsr, status_before);
    assert_eq!(cpu.saved_program_status(), None);
    assert_eq!(cpu.pc(), 0);

    execute(&mut cpu, 0xE16F_F000).unwrap(); // MSR SPSR_fsxc, r0
    assert_eq!(cpu.saved_program_status(), Some(Psr(0xA000_0013)));
    execute(&mut cpu, 0xE14F_1000).unwrap(); // MRS r1, SPSR
    assert_eq!(cpu.regs[1], 0xA000_0013);
}
#[test]
fn exception_modes_preserve_their_banked_registers_and_independent_spsrs() {
    let user_r8_r12 = [
        0x0808_0008,
        0x0809_0009,
        0x080A_000A,
        0x080B_000B,
        0x080C_000C,
    ];
    let user_sp_lr = [0x1000_0013, 0x1000_0014];
    let fiq_r8_r14 = [
        0xF008_0008,
        0xF009_0009,
        0xF00A_000A,
        0xF00B_000B,
        0xF00C_000C,
        0xF00D_000D,
        0xF00E_000E,
    ];
    let modes = [
        Psr::FIQ_MODE,
        Psr::IRQ_MODE,
        Psr::SUPERVISOR_MODE,
        Psr::ABORT_MODE,
        Psr::UNDEFINED_MODE,
    ];

    let mut cpu = Arm11Core::new();
    cpu.regs[8..13].copy_from_slice(&user_r8_r12);
    cpu.regs[13] = user_sp_lr[0];
    cpu.regs[14] = user_sp_lr[1];

    let mut expected_sp_lr = [[0; 2]; 5];
    let mut expected_spsr = [Psr(0); 5];

    for (index, mode) in modes.iter().copied().enumerate() {
        cpu.switch_mode(mode).unwrap();
        expected_spsr[index] = Psr(Psr::USER_MODE | (1 << (28 - index)));
        cpu.regs[0] = expected_spsr[index].0;
        execute(&mut cpu, 0xE16F_F000).unwrap(); // MSR SPSR_fsxc, r0

        if mode == Psr::FIQ_MODE {
            cpu.regs[8..15].copy_from_slice(&fiq_r8_r14);
        } else {
            assert_eq!(&cpu.regs[8..13], &user_r8_r12);
            expected_sp_lr[index] = [
                0x2000_0013 + (index as u32 * 0x100),
                0x2000_0014 + (index as u32 * 0x100),
            ];
            cpu.regs[13] = expected_sp_lr[index][0];
            cpu.regs[14] = expected_sp_lr[index][1];
        }
    }

    cpu.switch_mode(Psr::USER_MODE).unwrap();
    assert_eq!(&cpu.regs[8..13], &user_r8_r12);
    assert_eq!([cpu.regs[13], cpu.regs[14]], user_sp_lr);
    assert_eq!(cpu.saved_program_status(), None);

    cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
    assert_eq!(&cpu.regs[8..13], &user_r8_r12);
    assert_eq!([cpu.regs[13], cpu.regs[14]], user_sp_lr);
    assert_eq!(cpu.saved_program_status(), None);

    for (index, mode) in modes.iter().copied().enumerate() {
        cpu.switch_mode(mode).unwrap();
        if mode == Psr::FIQ_MODE {
            assert_eq!(&cpu.regs[8..15], &fiq_r8_r14);
        } else {
            assert_eq!(&cpu.regs[8..13], &user_r8_r12);
            assert_eq!(
                [cpu.regs[13], cpu.regs[14]],
                expected_sp_lr[index],
                "mode {mode:#x}"
            );
        }

        execute(&mut cpu, 0xE14F_1000).unwrap(); // MRS r1, SPSR
        assert_eq!(
            cpu.regs[1],
            expected_spsr[index].0,
            "SPSR bank for mode {mode:#x}"
        );
    }
}
