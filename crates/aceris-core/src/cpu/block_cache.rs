//! Bounded cache of straight-line A32 blocks from immutable executable text.
//!
//! A block is cached only when the bus explicitly identifies the fetched range
//! as immutable and executable. Writable RAM, dynamically generated code and
//! buses without this guarantee stay on the existing per-instruction path.

use super::Arm11Core;
use super::decode_cache::{DecodeCache, FastOp};
use super::ir::IrInstruction;
#[cfg(test)]
use super::ir::IrOperation;
use crate::memory::{Bus, ImmutableCodeLocation};
use aceris_common::Result;

const CACHE_ENTRIES: usize = 256;
const BLOCK_MAX_INSTRUCTIONS: usize = 16;

#[derive(Clone, Copy, Debug)]
pub(super) struct CachedInstruction {
    pub(super) ir: IrInstruction,
    pub(super) ends_block: bool,
}

const EMPTY_INSTRUCTION: CachedInstruction = CachedInstruction {
    ir: IrInstruction::EMPTY,
    ends_block: true,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct BasicBlock {
    pub(super) start_pc: u32,
    backing_id: u64,
    backing_offset: u64,
    mode: u32,
    len: u8,
    instructions: [CachedInstruction; BLOCK_MAX_INSTRUCTIONS],
}

impl BasicBlock {
    pub(super) fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub(super) fn instruction(&self, index: usize) -> Option<CachedInstruction> {
        (index < self.len()).then_some(self.instructions[index])
    }

    fn matches(&self, pc: u32, mode: u32, location: ImmutableCodeLocation) -> bool {
        self.start_pc == pc
            && self.mode == mode
            && self.backing_id == location.backing_id
            && self.backing_offset == location.offset
    }
}

/// Counters for validating the real value of the immutable-text block cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockCacheStats {
    pub lookups: u64,
    pub hits: u64,
    pub misses: u64,
    pub compiled_blocks: u64,
    pub compiled_instructions: u64,
    pub non_cacheable_fallbacks: u64,
}

impl BlockCacheStats {
    fn record_fallback(&mut self) {
        self.non_cacheable_fallbacks = self.non_cacheable_fallbacks.saturating_add(1);
    }
}

/// Direct-mapped block cache. A slot stores at most sixteen A32 instructions;
/// blocks are tied to an immutable backing identity rather than only a guest PC.
#[derive(Clone)]
pub(super) struct BlockCache {
    entries: Box<[Option<BasicBlock>]>,
    stats: BlockCacheStats,
}

impl Default for BlockCache {
    fn default() -> Self {
        Self {
            entries: vec![None; CACHE_ENTRIES].into_boxed_slice(),
            stats: BlockCacheStats::default(),
        }
    }
}

impl std::fmt::Debug for BlockCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockCache").finish_non_exhaustive()
    }
}

impl BlockCache {
    #[inline]
    fn slot(pc: u32, mode: u32) -> usize {
        ((pc >> 2) as usize ^ mode as usize) & (CACHE_ENTRIES - 1)
    }

    /// Returns a compiled block or constructs one from a proven immutable range.
    ///
    /// Fetching is still permission checked by the bus. The range is queried for
    /// each instruction, so blocks stop at non-contiguous translations, segment
    /// boundaries, unsupported instructions and precise-dispatch instructions.
    pub(super) fn block_for<B: Bus + ?Sized>(
        &mut self,
        pc: u32,
        mode: u32,
        bus: &mut B,
        decode_cache: &mut DecodeCache,
    ) -> Option<BasicBlock> {
        self.stats.lookups = self.stats.lookups.saturating_add(1);
        if pc & 3 != 0 {
            self.stats.record_fallback();
            return None;
        }
        let Some(first_location) = bus.immutable_instruction_location(pc, 4) else {
            self.stats.record_fallback();
            return None;
        };
        let slot = Self::slot(pc, mode);
        if let Some(block) = self.entries[slot] {
            if block.matches(pc, mode, first_location) {
                let cached_size = u32::from(block.len) * 4;
                let valid = bus
                    .immutable_instruction_location(pc, cached_size)
                    .is_some_and(|location| {
                        location.backing_id == block.backing_id
                            && location.offset == block.backing_offset
                    });
                if valid {
                    self.stats.hits = self.stats.hits.saturating_add(1);
                    return Some(block);
                }
            }
        }

        self.stats.misses = self.stats.misses.saturating_add(1);
        let mut instructions = [EMPTY_INSTRUCTION; BLOCK_MAX_INSTRUCTIONS];
        let mut len = 0usize;
        for index in 0..BLOCK_MAX_INSTRUCTIONS {
            let byte_offset = u32::try_from(index.checked_mul(4)?).ok()?;
            let address = pc.checked_add(byte_offset)?;
            let Some(location) = bus.immutable_instruction_location(address, 4) else {
                break;
            };
            if location.backing_id != first_location.backing_id
                || location.offset != first_location.offset.checked_add(u64::from(byte_offset))?
            {
                break;
            }

            let Some(word) = bus.fetch32_speculative(address) else {
                break;
            };
            if Arm11Core::needs_precise_step(word, false) {
                break;
            }

            let entry = decode_cache.lookup(word);
            if entry.op == FastOp::Generic {
                break;
            }
            let ends_block = entry.ends_block();
            instructions[len] = CachedInstruction {
                ir: IrInstruction::lower(entry),
                ends_block,
            };
            len += 1;
            if ends_block {
                break;
            }
        }

        if len == 0 {
            self.stats.record_fallback();
            return None;
        }
        let block = BasicBlock {
            start_pc: pc,
            backing_id: first_location.backing_id,
            backing_offset: first_location.offset,
            mode,
            len: len as u8,
            instructions,
        };
        self.entries[slot] = Some(block);
        self.stats.compiled_blocks = self.stats.compiled_blocks.saturating_add(1);
        self.stats.compiled_instructions = self
            .stats
            .compiled_instructions
            .saturating_add(block.len() as u64);
        Some(block)
    }

    pub(super) fn stats(&self) -> BlockCacheStats {
        self.stats
    }

    pub(super) fn take_stats(&mut self) -> BlockCacheStats {
        std::mem::take(&mut self.stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;

    struct ImmutableTextBus {
        memory: FlatMemory,
        base: u32,
        size: u32,
        backing_id: u64,
        expose_immutable_text: bool,
        split_address: Option<u32>,
    }

    impl Bus for ImmutableTextBus {
        fn read8(&mut self, address: u32) -> Result<u8> {
            self.memory.read8(address)
        }

        fn write8(&mut self, address: u32, value: u8) -> Result<()> {
            self.memory.write8(address, value)
        }

        fn immutable_instruction_location(
            &mut self,
            address: u32,
            size: u32,
        ) -> Option<ImmutableCodeLocation> {
            if !self.expose_immutable_text || size == 0 {
                return None;
            }
            let offset = address.checked_sub(self.base)?;
            let end = offset.checked_add(size)?;
            if end > self.size {
                return None;
            }
            if self.split_address.is_some_and(|split| {
                address < split && address.checked_add(size).is_some_and(|end| end > split)
            }) {
                return None;
            }
            let backing_id = if self.split_address.is_some_and(|split| address >= split) {
                self.backing_id + 1
            } else {
                self.backing_id
            };
            Some(ImmutableCodeLocation {
                backing_id,
                offset: u64::from(offset),
            })
        }
    }

    fn bus() -> ImmutableTextBus {
        let mut memory = FlatMemory::new(0, 0x1000);
        // A short straight-line block followed by a conditional branch.
        for (index, word) in [0xE3A0_0001, 0xE280_1002, 0xE281_2003, 0xEA00_0000]
            .into_iter()
            .enumerate()
        {
            memory.write32(0x100 + index as u32 * 4, word).unwrap();
        }
        ImmutableTextBus {
            memory,
            base: 0x100,
            size: 0x100,
            backing_id: 7,
            expose_immutable_text: true,
            split_address: None,
        }
    }

    #[test]
    fn compiles_and_reuses_a_bounded_straight_line_a32_block() {
        let mut bus = bus();
        let mut cache = BlockCache::default();
        let mut decode = DecodeCache::default();

        let block = cache
            .block_for(0x100, 0x13, &mut bus, &mut decode)
            .expect("immutable text should compile");
        assert_eq!(block.len(), 4);
        assert!(matches!(
            block.instruction(0).unwrap().ir.operation,
            IrOperation::AluImmediate {
                opcode: 0xD,
                immediate: 1,
                ..
            }
        ));
        assert!(matches!(
            block.instruction(3).unwrap().ir.operation,
            IrOperation::Branch {
                displacement: 0,
                link: false
            }
        ));
        assert!(block.instruction(3).unwrap().ends_block);

        let reused = cache
            .block_for(0x100, 0x13, &mut bus, &mut decode)
            .expect("same identity should hit");
        assert_eq!(reused.start_pc, block.start_pc);
        assert_eq!(reused.len(), block.len());
    }

    #[test]
    fn fast_loop_executes_a_cached_block_from_immutable_text() {
        let mut bus = bus();
        let mut cpu = Arm11Core::new();
        cpu.regs[super::super::PC] = 0x100;

        let (executed, exit) = cpu.run_fast(&mut bus, 4).unwrap();
        assert_eq!(executed, 4);
        assert_eq!(exit, super::super::FastRunExit::Budget);
        assert_eq!((cpu.regs[0], cpu.regs[1], cpu.regs[2]), (1, 3, 6));
        assert!(cpu.block_cache.entries.iter().any(Option::is_some));

        let first_interval = cpu.block_cache_stats();
        assert_eq!(first_interval.lookups, 1);
        assert_eq!(first_interval.hits, 0);
        assert_eq!(first_interval.misses, 1);
        assert_eq!(first_interval.compiled_blocks, 1);
        assert_eq!(first_interval.compiled_instructions, 4);

        cpu.regs[super::super::PC] = 0x100;
        let (executed, exit) = cpu.run_fast(&mut bus, 4).unwrap();
        assert_eq!(executed, 4);
        assert_eq!(exit, super::super::FastRunExit::Budget);
        let second_interval = cpu.take_block_cache_stats();
        assert_eq!(second_interval.lookups, 1);
        assert_eq!(second_interval.hits, 1);
        assert_eq!(second_interval.misses, 0);
        assert_eq!(second_interval.compiled_blocks, 0);
        assert_eq!(cpu.block_cache_stats(), super::BlockCacheStats::default());
    }

    #[test]
    fn ir_block_execution_matches_precise_steps_for_alu_shift_and_conditions() {
        let program = [
            0xE3A0_0003, // MOV r0, #3
            0xE1A0_1080, // MOV r1, r0, LSL #1
            0x03A0_2007, // MOVEQ r2, #7 (skipped while Z is clear)
            0xEA00_0000, // B to the instruction after the branch
        ];
        let mut fast_bus = bus();
        let mut precise_bus = bus();
        for (index, word) in program.into_iter().enumerate() {
            let address = 0x100 + index as u32 * 4;
            fast_bus.memory.write32(address, word).unwrap();
            precise_bus.memory.write32(address, word).unwrap();
        }

        let mut fast_cpu = Arm11Core::new();
        fast_cpu.regs[super::super::PC] = 0x100;
        let mut precise_cpu = Arm11Core::new();
        precise_cpu.regs[super::super::PC] = 0x100;

        for _ in 0..program.len() {
            assert_eq!(
                precise_cpu.step(&mut precise_bus).unwrap(),
                super::super::StepEvent::Executed
            );
        }
        let (executed, exit) = fast_cpu.run_fast(&mut fast_bus, program.len() as u64).unwrap();

        assert_eq!(executed, program.len() as u64);
        assert_eq!(exit, super::super::FastRunExit::Budget);
        assert_eq!(fast_cpu.regs, precise_cpu.regs);
        assert_eq!(fast_cpu.cpsr, precise_cpu.cpsr);
        assert_eq!(fast_cpu.pc(), precise_cpu.pc());
        assert_eq!((fast_cpu.regs[0], fast_cpu.regs[1], fast_cpu.regs[2]), (3, 6, 0));
    }

    #[test]
    fn refuses_writable_or_unidentified_instruction_memory() {
        let mut bus = bus();
        let mut cache = BlockCache::default();
        let mut decode = DecodeCache::default();

        bus.expose_immutable_text = false;
        assert!(
            cache
                .block_for(0x100, 0x13, &mut bus, &mut decode)
                .is_none()
        );
    }

    #[test]
    fn validates_every_cached_instruction_after_a_later_mapping_changes() {
        let mut bus = bus();
        let mut cache = BlockCache::default();
        let mut decode = DecodeCache::default();

        let original = cache.block_for(0x100, 0x13, &mut bus, &mut decode).unwrap();
        assert_eq!(original.len(), 4);

        // The first two instructions still map to the original backing, but
        // the following instruction now belongs to another code allocation.
        bus.split_address = Some(0x108);
        let remapped = cache.block_for(0x100, 0x13, &mut bus, &mut decode).unwrap();
        assert_eq!(remapped.len(), 2);
        assert_ne!(remapped.backing_id, 0);
    }

    #[test]
    fn block_cache_keys_include_backing_identity_and_mode() {
        let mut bus = bus();
        let mut cache = BlockCache::default();
        let mut decode = DecodeCache::default();

        let first = cache.block_for(0x100, 0x13, &mut bus, &mut decode).unwrap();
        bus.backing_id = 8;
        let second = cache.block_for(0x100, 0x13, &mut bus, &mut decode).unwrap();
        assert_eq!(first.backing_id, 7);
        assert_eq!(second.backing_id, 8);
        let third = cache.block_for(0x100, 0x1F, &mut bus, &mut decode).unwrap();
        assert_eq!(third.mode, 0x1F);
    }
}
