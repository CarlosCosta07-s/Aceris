//! Independent integration tests for A32 LDM/STM base-register constraints.

use aceris_core::Error;
use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

const DATA: u32 = 0x400;

fn cpu_and_memory(instruction: u32) -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, 0x1000);
    memory.write32(0, instruction).unwrap();
    // Sentinel words cover the PC-relative base (instruction PC + 8) and the
    // ordinary data area used by the writeback cases.
    memory.write32(8, 0xA1B2_C3D4).unwrap();
    memory.write32(DATA, 0x1122_3344).unwrap();
    memory.write32(DATA + 4, 0x5566_7788).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[0] = 0x0102_0304;
    cpu.regs[1] = DATA;
    cpu.regs[2] = 0xAABB_CCDD;
    (cpu, memory)
}

#[test]
fn a32_ldm_stm_reject_pc_as_the_base_register_before_any_side_effect() {
    // STMIA pc,{r0} ; LDMIA pc,{r0}. R15 is not a legal multiple-transfer base.
    for instruction in [0xE88F_0001, 0xE89F_0001] {
        let (mut cpu, mut memory) = cpu_and_memory(instruction);
        let registers_before = cpu.regs;
        let pc_base_word = memory.read32(8).unwrap();
        let data_before = (
            memory.read32(DATA).unwrap(),
            memory.read32(DATA + 4).unwrap(),
        );

        assert!(matches!(
            cpu.step(&mut memory),
            Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ));
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(
            memory.read32(8).unwrap(),
            pc_base_word,
            "{instruction:#010x}"
        );
        assert_eq!(
            (
                memory.read32(DATA).unwrap(),
                memory.read32(DATA + 4).unwrap()
            ),
            data_before,
            "{instruction:#010x}"
        );
    }
}

#[test]
fn a32_writeback_load_rejects_base_register_in_the_destination_list() {
    // LDMIA r1!,{r1} ; LDMIA r1!,{r0,r1}.
    for instruction in [0xE8B1_0002, 0xE8B1_0003] {
        let (mut cpu, mut memory) = cpu_and_memory(instruction);
        let registers_before = cpu.regs;
        let data_before = (
            memory.read32(DATA).unwrap(),
            memory.read32(DATA + 4).unwrap(),
        );

        assert!(matches!(
            cpu.step(&mut memory),
            Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ));
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(
            (
                memory.read32(DATA).unwrap(),
                memory.read32(DATA + 4).unwrap()
            ),
            data_before,
            "{instruction:#010x}"
        );
    }
}

#[test]
fn a32_writeback_store_rejects_base_register_after_a_lower_list_register() {
    // STMIA r1!,{r0,r1}: the stored value of the base is architecturally unknown.
    let instruction = 0xE8A1_0003;
    let (mut cpu, mut memory) = cpu_and_memory(instruction);
    let registers_before = cpu.regs;
    let data_before = (
        memory.read32(DATA).unwrap(),
        memory.read32(DATA + 4).unwrap(),
    );

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(
        (
            memory.read32(DATA).unwrap(),
            memory.read32(DATA + 4).unwrap()
        ),
        data_before
    );
}

#[test]
fn a32_writeback_store_allows_base_as_lowest_register_and_stores_its_original_value() {
    // STMIA r1!,{r1,r2}: the original base value is stored before writeback.
    let (mut cpu, mut memory) = cpu_and_memory(0xE8A1_0006);
    cpu.step(&mut memory).unwrap();

    assert_eq!(memory.read32(DATA).unwrap(), DATA);
    assert_eq!(memory.read32(DATA + 4).unwrap(), 0xAABB_CCDD);
    assert_eq!(cpu.regs[1], DATA + 8);
}

#[test]
fn a32_ldm_stm_writeback_covers_all_four_addressing_modes() {
    for (mode, pre_index, up) in [
        ("IA", false, true),
        ("IB", true, true),
        ("DA", false, false),
        ("DB", true, false),
    ] {
        let size = 12;
        let start = if up {
            DATA + if pre_index { 4 } else { 0 }
        } else {
            DATA - size + if pre_index { 0 } else { 4 }
        };
        let expected_base = if up { DATA + size } else { DATA - size };
        let values = [0x1122_3344, 0x5566_7788, 0x99AA_BBCC];

        for load in [false, true] {
            let instruction = 0xE800_0000
                | if pre_index { 1 << 24 } else { 0 }
                | if up { 1 << 23 } else { 0 }
                | (1 << 21)
                | if load { 1 << 20 } else { 0 }
                | (1 << 16)
                | 0x001C; // {r2,r3,r4}, with r1 as the writeback base.
            let (mut cpu, mut memory) = cpu_and_memory(instruction);

            for (index, value) in values.iter().copied().enumerate() {
                memory
                    .write32(
                        start + index as u32 * 4,
                        if load { value } else { 0xDEAD_BEEF },
                    )
                    .unwrap();
            }

            if !load {
                cpu.regs[2..5].copy_from_slice(&values);
            }

            cpu.step(&mut memory).unwrap();

            assert_eq!(cpu.regs[1], expected_base, "{mode}, load={load}");
            if load {
                assert_eq!(&cpu.regs[2..5], &values, "{mode}");
            } else {
                for (index, value) in values.iter().copied().enumerate() {
                    assert_eq!(
                        memory.read32(start + index as u32 * 4).unwrap(),
                        value,
                        "{mode}, beat={index}"
                    );
                }
            }
        }
    }
}
