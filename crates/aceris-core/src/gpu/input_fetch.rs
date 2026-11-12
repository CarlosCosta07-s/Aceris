//! Explicit bridge from PICA200 input-register snapshots to physical memory reads.
//!
//! The caller must resolve GPUREG_ATTRIBBUFFERS_LOC through the system's GPU
//! address/DMA mapping and pass the resulting physical byte address. This
//! module deliberately does not interpret raw GPU register addresses.

use super::{GpuPhysicalMemory, IndexBufferError, Pica200, RawVertexInput, VertexInputError};

impl Pica200 {
    /// Fetches one vertex using the current PICA200 attribute-array registers.
    ///
    /// `resolved_physical_byte_base` must be the already-translated physical
    /// address corresponding to GPUREG_ATTRIBBUFFERS_LOC. Array offsets and
    /// per-vertex strides are applied by the decoded vertex layout. This
    /// method does not perform DMA translation or numeric format conversion.
    pub fn read_vertex_at_physical_base(
        &self,
        memory: &GpuPhysicalMemory,
        resolved_physical_byte_base: u32,
        vertex_index: u32,
    ) -> Result<RawVertexInput, VertexInputError> {
        let state = self.vertex_input_state()?;
        state
            .layout
            .read_vertex(memory, resolved_physical_byte_base, vertex_index)
    }

    /// Fetches raw indices using the current PICA200 index-buffer register.
    ///
    /// `resolved_physical_byte_base` must be the already-translated physical
    /// address corresponding to GPUREG_ATTRIBBUFFERS_LOC. The returned index
    /// values are not adjusted by GPUREG_VERTEX_OFFSET and do not trigger a
    /// draw operation.
    pub fn read_indices_at_physical_base(
        &self,
        memory: &GpuPhysicalMemory,
        resolved_physical_byte_base: u32,
        first_index: u32,
        count: usize,
    ) -> Result<Vec<u32>, IndexBufferError> {
        let state = self.index_input_state()?;
        state
            .index_buffer
            .read_indices(memory, resolved_physical_byte_base, first_index, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{
        AttributeDataType, FCRAM_BASE, IndexElementWidth, REG_ATTRIBBUFFER0_OFFSET,
        REG_ATTRIBBUFFERS_FORMAT_HIGH, REG_ATTRIBBUFFERS_FORMAT_LOW, REG_ATTRIBBUFFERS_LOC,
        REG_INDEXBUFFER_CONFIG, REG_NUMVERTICES, REG_VERTEX_OFFSET,
    };

    #[test]
    fn pica200_fetches_vertex_from_caller_resolved_physical_base() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x0BAD_F000)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_LOW, 1).unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, 0)
            .unwrap();

        // One unsigned 8-bit component in attribute 0; one-byte stride.
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET, 0x40).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 1, 0).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 2, (1 << 16) | (1 << 28))
            .unwrap();

        let mut memory = GpuPhysicalMemory::new();
        let resolved_base = FCRAM_BASE + 0x3000;
        memory.write(resolved_base + 0x40, &[0x7F]).unwrap();

        let vertex = gpu
            .read_vertex_at_physical_base(&memory, resolved_base, 0)
            .unwrap();
        let attribute = vertex.attributes[0].unwrap();

        assert_eq!(attribute.format.data_type, AttributeDataType::UnsignedByte);
        assert_eq!(attribute.component_count, 1);
        assert_eq!(attribute.components[0], 0x7F);
    }

    #[test]
    fn pica200_fetches_indices_using_current_index_registers() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x0BAD_F000)
            .unwrap();
        gpu.write_register(REG_INDEXBUFFER_CONFIG, 0x8000_0020)
            .unwrap();
        gpu.write_register(REG_NUMVERTICES, 2).unwrap();
        gpu.write_register(REG_VERTEX_OFFSET, 0xFFFF_FFFE).unwrap();

        let state = gpu.index_input_state().unwrap();
        assert_eq!(
            state.index_buffer.element_width(),
            IndexElementWidth::UnsignedShort
        );
        assert_eq!(state.vertex_count, 2);
        assert_eq!(state.raw_vertex_offset_register, 0xFFFF_FFFE);

        let mut memory = GpuPhysicalMemory::new();
        let resolved_base = FCRAM_BASE + 0x4000;
        memory
            .write(resolved_base + 0x20, &[0x34, 0x12, 0x78, 0x56])
            .unwrap();

        // The fetch API returns raw indices; the signed vertex offset is
        // intentionally left for the later draw-command stage.
        assert_eq!(
            gpu.read_indices_at_physical_base(&memory, resolved_base, 0, 2)
                .unwrap(),
            vec![0x1234, 0x5678]
        );
    }

    #[test]
    fn pica200_fetch_reports_unmapped_resolved_addresses() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_INDEXBUFFER_CONFIG, 0).unwrap();

        assert!(matches!(
            gpu.read_indices_at_physical_base(&GpuPhysicalMemory::new(), 0x1000, 0, 1),
            Err(IndexBufferError::PhysicalMemory(
                super::super::GpuPhysicalMemoryError::UnmappedAddress { .. }
            ))
        ));
    }
}
