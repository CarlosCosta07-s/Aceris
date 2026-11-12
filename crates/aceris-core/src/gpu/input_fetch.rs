//! Explicit bridge from PICA200 input-register snapshots to physical memory reads.
//!
//! The caller must resolve GPUREG_ATTRIBBUFFERS_LOC through the system's GPU
//! address/DMA mapping and pass the resulting physical byte address. This
//! module deliberately does not interpret raw GPU register addresses.

use super::{
    GpuDmaAddressSpace, GpuPhysicalMemory, IndexBufferError, NumericVertexInput, Pica200,
    RawVertexInput, VertexInputError,
};

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

    /// Fetches one vertex and converts its scalar components to numeric shader inputs.
    ///
    /// Integer components are sign-extended where applicable and converted to
    /// f32 values. The conversion does not apply implicit normalization.
    pub fn read_numeric_vertex_at_physical_base(
        &self,
        memory: &GpuPhysicalMemory,
        resolved_physical_byte_base: u32,
        vertex_index: u32,
    ) -> Result<NumericVertexInput, VertexInputError> {
        self.read_vertex_at_physical_base(memory, resolved_physical_byte_base, vertex_index)
            .and_then(|raw| raw.to_numeric())
            .map(|mut input| {
                self.apply_fixed_vertex_attributes(&mut input);
                input
            })
    }

    /// Fetches and converts a vertex through an explicitly registered linear DMA mapping.
    ///
    /// `linear_virtual_base` is the CPU-visible pointer retained by the caller.
    /// It is intentionally not inferred from GPUREG_ATTRIBBUFFERS_LOC: that
    /// PICA register contains the device-side encoded base address.
    pub fn read_numeric_vertex_via_dma(
        &self,
        memory: &GpuPhysicalMemory,
        address_space: &GpuDmaAddressSpace,
        linear_virtual_base: u32,
        vertex_index: u32,
    ) -> Result<NumericVertexInput, VertexInputError> {
        let state = self.vertex_input_state()?;
        state
            .layout
            .read_vertex_via_dma(memory, address_space, linear_virtual_base, vertex_index)
            .and_then(|raw| raw.to_numeric())
            .map(|mut input| {
                self.apply_fixed_vertex_attributes(&mut input);
                input
            })
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

    /// Fetches indices through an explicitly registered linear DMA mapping.
    ///
    /// `linear_virtual_base` is the CPU-visible pointer retained by the caller.
    /// The PICA index configuration stores an offset from its device-side base.
    pub fn read_indices_via_dma(
        &self,
        memory: &GpuPhysicalMemory,
        address_space: &GpuDmaAddressSpace,
        linear_virtual_base: u32,
        first_index: u32,
        count: usize,
    ) -> Result<Vec<u32>, IndexBufferError> {
        let state = self.index_input_state()?;
        state.index_buffer.read_indices_via_dma(
            memory,
            address_space,
            linear_virtual_base,
            first_index,
            count,
        )
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
    fn pica200_fetches_numeric_vertex_inputs() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x0BAD_F000)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_LOW, 1).unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, 0)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET, 0x80).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 1, 0).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 2, (1 << 16) | (1 << 28))
            .unwrap();

        let mut memory = GpuPhysicalMemory::new();
        let resolved_base = FCRAM_BASE + 0x5000;
        memory.write(resolved_base + 0x80, &[0xFF]).unwrap();

        let vertex = gpu
            .read_numeric_vertex_at_physical_base(&memory, resolved_base, 0)
            .unwrap();
        let attribute = vertex.attributes[0].unwrap();
        assert_eq!(attribute.components[0], 255.0);
        assert_eq!(attribute.component_count, 1);
    }

    #[test]
    fn numeric_vertex_fetch_merges_fixed_attributes_from_pica_registers() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x0BAD_F000)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, (1 << 16) | (1 << 28))
            .unwrap();

        gpu.write_register(crate::gpu::REG_FIXEDATTRIB_INDEX, 0)
            .unwrap();
        gpu.write_register(crate::gpu::REG_FIXEDATTRIB_DATA0, 0x4100_0040)
            .unwrap();
        gpu.write_register(crate::gpu::REG_FIXEDATTRIB_DATA1, 0x0000_4000)
            .unwrap();
        gpu.write_register(crate::gpu::REG_FIXEDATTRIB_DATA2, 0x003F_0000)
            .unwrap();

        let memory = GpuPhysicalMemory::new();
        let vertex = gpu
            .read_numeric_vertex_at_physical_base(&memory, FCRAM_BASE, 0)
            .unwrap();

        assert_eq!(
            vertex.attributes[0].unwrap().components,
            [1.0, 2.0, 2.0, 4.0]
        );
        assert_eq!(vertex.resolved_fixed_attribute_mask, 1);
        assert_eq!(vertex.fixed_attribute_mask, 1);
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
    fn pica200_fetches_numeric_vertex_through_registered_linear_dma_mapping() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x1400_5000)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_LOW, 1).unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, 0)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET, 0x80).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 1, 0).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 2, (1 << 16) | (1 << 28))
            .unwrap();

        let mut memory = GpuPhysicalMemory::new();
        memory.write(FCRAM_BASE + 0x5080, &[0xA5]).unwrap();
        let mut address_space = GpuDmaAddressSpace::new();
        address_space
            .map_linear_region(0x1400_5000, 0x1000)
            .unwrap();

        let vertex = gpu
            .read_numeric_vertex_via_dma(&memory, &address_space, 0x1400_5000, 0)
            .unwrap();
        assert_eq!(vertex.attributes[0].unwrap().components[0], 165.0);
    }

    #[test]
    fn pica200_fetches_indices_through_registered_linear_dma_mapping() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x1400_6000)
            .unwrap();
        gpu.write_register(REG_INDEXBUFFER_CONFIG, 0x8000_0020)
            .unwrap();
        gpu.write_register(REG_NUMVERTICES, 2).unwrap();

        let mut memory = GpuPhysicalMemory::new();
        memory
            .write(FCRAM_BASE + 0x6020, &[0x34, 0x12, 0x78, 0x56])
            .unwrap();
        let mut address_space = GpuDmaAddressSpace::new();
        address_space
            .map_linear_region(0x1400_6000, 0x1000)
            .unwrap();

        assert_eq!(
            gpu.read_indices_via_dma(&memory, &address_space, 0x1400_6000, 0, 2)
                .unwrap(),
            vec![0x1234, 0x5678]
        );
    }

    #[test]
    fn dma_vertex_fetch_rejects_components_outside_registered_mapping() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x1400_7000)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_LOW, 1).unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, 0)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET, 0xFFC).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 1, 0).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 2, (1 << 16) | (1 << 28))
            .unwrap();

        let memory = GpuPhysicalMemory::new();
        let mut address_space = GpuDmaAddressSpace::new();
        address_space
            .map_linear_region(0x1400_7000, 0x1000)
            .unwrap();

        assert!(matches!(
            gpu.read_numeric_vertex_via_dma(&memory, &address_space, 0x1400_7000, 1),
            Err(VertexInputError::GpuAddressSpace(
                super::super::GpuAddressSpaceError::UnmappedVirtualRange { .. }
            ))
        ));
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
