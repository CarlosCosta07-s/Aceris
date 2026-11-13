//! Differential tests for QADD/QSUB/QDADD/QDSUB and sticky Q behaviour.
//! The reference model uses i64 clamping, independent of the interpreter.

use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

const GRID: [u32; 9] = [
    0,
    1,
    0xFFFF_FFFF,
    0x3FFF_FFFF,
    0x4000_0000,
    0x7FFF_FFFF,
    0x8000_0000,
    0xC000_0000,
    0xBFFF_FFFF,
];

type Model = fn(i64, i64) -> (u32, bool);

fn clamp(value: i64) -> (u32, bool) {
    let clamped = value.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    (clamped as i32 as u32, clamped != value)
}

fn run(word: u32, rm: u32, rn: u32, q: bool) -> Arm11Core {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, word).unwrap();
    let mut cpu = Arm11Core::new();
    cpu.regs[0] = rm;
    cpu.regs[1] = rn;
    if q {
        cpu.cpsr.set_q();
    }
    cpu.cpsr.set_flags(true, false, true, true);
    cpu.step(&mut memory).unwrap();
    cpu
}

#[test]
fn saturating_add_sub_match_reference_and_keep_q_sticky() {
    // QADD/QSUB/QDADD/QDSUB r2, r0, r1 (Rm = r0, Rn = r1).
    let forms: [(u32, Model); 4] = [
        (0xE101_2050, |m, n| clamp(m + n)),
        (0xE121_2050, |m, n| clamp(m - n)),
        (0xE141_2050, |m, n| {
            let (d, s1) = clamp(2 * n);
            let (r, s2) = clamp(m + i64::from(d as i32));
            (r, s1 || s2)
        }),
        (0xE161_2050, |m, n| {
            let (d, s1) = clamp(2 * n);
            let (r, s2) = clamp(m - i64::from(d as i32));
            (r, s1 || s2)
        }),
    ];
    for (word, model) in forms {
        for &rm in &GRID {
            for &rn in &GRID {
                let (expected, saturated) = model(i64::from(rm as i32), i64::from(rn as i32));
                for q_before in [false, true] {
                    let cpu = run(word, rm, rn, q_before);
                    let ctx = format!("{word:#010x} rm={rm:#x} rn={rn:#x} q={q_before}");
                    assert_eq!(cpu.regs[2], expected, "{ctx}");
                    assert_eq!(cpu.cpsr.q(), q_before || saturated, "{ctx}");
                    // Saturating ops never touch N/Z/C/V.
                    assert!(cpu.cpsr.n() && !cpu.cpsr.z() && cpu.cpsr.c() && cpu.cpsr.v());
                }
            }
        }
    }
}
