//! Bounded cache of straight-line A32 blocks from immutable executable text.
//!
//! A block is cached only when the bus explicitly identifies the fetched range
//! as immutable and executable. Writable RAM, dynamically generated code and
//! buses without this guarantee stay on the existing per-instruction path.

use super::decode_cache::{DecodeCache, Entry, FastOp};
use super::Arm11Core;
use crate::memory::{Bus, ImmutableCodeLocation};
use aceris_common::Result;

const CACHE_ENTRIES: usize = 256;
const BLOCK_MAX_INSTRUCTIONS: usize = 16;

const EMPTY_ENTRY: Entry = Entry {
    word: u32::MAX,
    condition: 0xF,
    op: FastOp::Generic,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct CachedInstruction {
    pub(super) word: u32,
    pub(super) entry: Entry,
    pub(super) ends_block: bool,
}

const EMPTY_INSTRUCTION: CachedInstruction = CachedInstruction {
    word: u32::MAX,
    entry: EMPTY_ENTRY,
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

    fn matches(
        &self,
        pc: u32,
        mode: u32,
        location: ImmutableCodeLocation,
    ) -> bool {
        self.start_pc == pc
            && self.mode == mode
            && self.backing_id == location.backing_id
            && self.backing_offset == location.offset
    }
}

/// Direct-mapped block cache. A slot stores at most sixteen A32 instructions;
/// blocks are tied to an immutable backing identity rather than only a guest PC.
#[derive(Clone)]
pub(super) struct BlockCache {
    entries: Box<[Option<BasicBlock>]>,
}

impl Default for BlockCache {
    fn default() -> Self {
        Self {
            entries: vec![None; CACHE_ENTRIES].into_boxed_slice(),
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
        if pc & 3 != 0 {
            return None;
        }
        let first_location = bus.immutable_instruction_location(pc, 4)?;
        let slot = Self::slot(pc, mode);
        if let Some(block) = self.entries[slot]
            && block.matches(pc, mode, first_location)
        {
            let mut valid = true;
            for index in 0..block.len() {
                let byte_offset = u32::try_from(index.checked_mul(4)?).ok()?;
                let address = pc.checked_add(byte_offset)?;
                let Some(location) = bus.immutable_instruction_location(address, 4) else {
                    valid = false;
                    break;
                };
                if location.backing_id != block.backing_id
                    || location.offset != block.backing_offset.checked_add(u64::from(byte_offset))?
                {
                    valid = false;
                    break;
                }
            }
            if valid {
                return Some(block);
            }
        }

        let mut instructions = [EMPTY_INSTRUCTION; BLOCK_MAX_INSTRUCTIONS];
        let mut len = 0usize;
        for index in 0..BLOCK_MAX_INSTRUCTIONS {
            let byte_offset = u32::try_from(index.checked_mul(4)?).ok()?;
            let address = pc.checked_add(byte_offset)?;
            let location = bus.immutable_instruction_location(address, 4)?;
            if location.backing_id != first_location.backing_id
                || location.offset != first_location.offset.checked_add(u64::from(byte_offset))?
            {
                break;
            }

            let word = match bus.fetch32(address) {
                Ok(word) => word,
                Err(_) => break,
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
                word,
                entry,
                ends_block,
            };
            len += 1;
            if ends_block {
                break;
            }
        }

        if len == 0 {
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
        Some(block)
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

        fn write8(
            &mut self,
            address: u32,
            value: u8,
        ) -> Result<()> {
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
            if offset.checked_add(size)? > self.size {
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
        assert_eq!(block.instruction(0).unwrap().word, 0xE3A0_0001);
        assert!(block.instruction(3).unwrap().ends_block);

        let reused = cache
            .block_for(0x100, 0x13, &mut bus, &mut decode)
            .expect("same identity should hit");
        assert_eq!(reused.start_pc, block.start_pc);
        assert_eq!(reused.len(), block.len());
    }

    #[test]
    fn refuses_writable_or_unidentified_instruction_memory() {
        let mut bus = bus();
        let mut cache = BlockCache::default();
        let mut decode = DecodeCache::default();

        bus.expose_immutable_text = false;
        assert!(cache.block_for(0x100, 0x13, &mut bus, &mut decode).is_none());
    }

    #[test]
    fn validates_every_cached_instruction_after_a_later_mapping_changes() {
        let mut bus = bus();
        let mut cache = BlockCache::default();
        let mut decode = DecodeCache::default();

        let original = cache
            .block_for(0x100, 0x13, &mut bus, &mut decode)
            .unwrap();
        assert_eq!(original.len(), 4);

        // The first two instructions still map to the original backing, but
        // the following instruction now belongs to another code allocation.
        bus.split_address = Some(0x108);
        let remapped = cache
            .block_for(0x100, 0x13, &mut bus, &mut decode)
            .unwrap();
        assert_eq!(remapped.len(), 2);
        assert_ne!(remapped.backing_id, 0);
    }

    #[test]
    fn block_cache_keys_include_backing_identity_and_mode() {
        let mut bus = bus();
        let mut cache = BlockCache::default();
        let mut decode = DecodeCache::default();

        let first = cache
            .block_for(0x100, 0x13, &mut bus, &mut decode)
            .unwrap();
        bus.backing_id = 8;
        let second = cache
            .block_for(0x100, 0x13, &mut bus, &mut decode)
            .unwrap();
        assert_eq!(first.backing_id, 7);
        assert_eq!(second.backing_id, 8);
        let third = cache
            .block_for(0x100, 0x1F, &mut bus, &mut decode)
            .unwrap();
        assert_eq!(third.mode, 0x1F);
    }
}
