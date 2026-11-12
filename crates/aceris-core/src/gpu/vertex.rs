//! PICA200 vertex-array layout decoding and raw attribute fetching.
//!
//! Component payloads remain raw little-endian bits. Numeric conversion and
//! vertex-shader execution are intentionally separate stages.
//!
//! The physical base passed to read_vertex must already be a decoded byte
//! address. This module does not guess GPU register address units or implement
//! DMA/address translation.

use super::{GpuPhysicalMemory, GpuPhysicalMemoryError, Pica200};

pub const REG_ATTRIBBUFFERS_LOC: u16 = 0x200;
pub const REG_ATTRIBBUFFERS_FORMAT_LOW: u16 = 0x201;
pub const REG_ATTRIBBUFFERS_FORMAT_HIGH: u16 = 0x202;
pub const REG_ATTRIBBUFFER0_OFFSET: u16 = 0x203;
pub const MAX_VERTEX_ATTRIBUTES: usize = 12;
pub const MAX_VERTEX_ARRAYS: usize = 12;
pub const MAX_COMPONENTS_PER_ARRAY: usize = 12;
pub const MAX_COMPONENTS_PER_ATTRIBUTE: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeDataType {
    SignedByte,
    UnsignedByte,
    SignedShort,
    Float,
}

impl AttributeDataType {
    fn decode(bits: u32) -> Self {
        match bits & 0x3 {
            0 => Self::SignedByte,
            1 => Self::UnsignedByte,
            2 => Self::SignedShort,
            _ => Self::Float,
        }
    }

    /// Storage width of one scalar component in the PICA200 vertex stream.
    pub const fn byte_width(self) -> usize {
        match self {
            Self::SignedByte | Self::UnsignedByte => 1,
            Self::SignedShort => 2,
            Self::Float => 4,
        }
    }

    /// Converts one zero-extended raw component to the numeric shader input.
    ///
    /// Integer formats are converted to their numeric values; they are not
    /// normalized to [0, 1] or [-1, 1] here.
    pub fn decode_component(self, raw_bits: u32) -> f32 {
        match self {
            Self::SignedByte => (raw_bits as u8 as i8) as f32,
            Self::UnsignedByte => (raw_bits as u8) as f32,
            Self::SignedShort => (raw_bits as u16 as i16) as f32,
            Self::Float => f32::from_bits(raw_bits),
        }
    }
}

/// Format of one PICA200 vertex attribute.
///
/// The component-count field is encoded as count minus one in the two high
/// bits of each four-bit format descriptor. Scalar storage width is selected
/// independently by `data_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexAttributeFormat {
    pub data_type: AttributeDataType,
    pub component_count: u8,
}

impl VertexAttributeFormat {
    fn decode(field: u32) -> Self {
        Self {
            data_type: AttributeDataType::decode(field),
            component_count: (((field >> 2) & 0x3) + 1) as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawVertexAttribute {
    pub format: VertexAttributeFormat,
    pub components: [u32; MAX_COMPONENTS_PER_ATTRIBUTE],
    pub component_count: u8,
}

impl RawVertexAttribute {
    fn new(format: VertexAttributeFormat) -> Self {
        Self {
            format,
            components: [0; MAX_COMPONENTS_PER_ATTRIBUTE],
            component_count: 0,
        }
    }

    fn push(&mut self, attribute: usize, value: u32) -> Result<(), VertexInputError> {
        let index = usize::from(self.component_count);
        if index >= MAX_COMPONENTS_PER_ATTRIBUTE {
            return Err(VertexInputError::TooManyComponentsForAttribute {
                attribute,
                count: index + 1,
            });
        }
        self.components[index] = value;
        self.component_count += 1;
        Ok(())
    }

    /// Converts the fetched integer/float bit patterns into shader-ready f32s.
    pub fn to_numeric(&self) -> NumericVertexAttribute {
        let mut components = [0.0; MAX_COMPONENTS_PER_ATTRIBUTE];
        for (index, component) in self.components[..usize::from(self.component_count)]
            .iter()
            .copied()
            .enumerate()
        {
            components[index] = self.format.data_type.decode_component(component);
        }
        NumericVertexAttribute {
            format: self.format,
            components,
            component_count: self.component_count,
        }
    }
}

/// One attribute after scalar conversion, ready to be supplied to a vertex shader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumericVertexAttribute {
    pub format: VertexAttributeFormat,
    pub components: [f32; MAX_COMPONENTS_PER_ATTRIBUTE],
    pub component_count: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawVertexInput {
    pub attributes: [Option<RawVertexAttribute>; MAX_VERTEX_ATTRIBUTES],
    /// Fixed attributes are preserved as a mask; their separate float24
    /// registers are not decoded by this raw-array reader.
    pub fixed_attribute_mask: u16,
}

impl RawVertexInput {
    fn new(mask: u16) -> Self {
        Self {
            attributes: [None; MAX_VERTEX_ATTRIBUTES],
            fixed_attribute_mask: mask,
        }
    }
}

impl RawVertexInput {
    /// Converts all fetched array attributes while preserving the fixed mask.
    pub fn to_numeric(&self) -> NumericVertexInput {
        NumericVertexInput {
            attributes: self.attributes.map(|attribute| attribute.map(|value| value.to_numeric())),
            fixed_attribute_mask: self.fixed_attribute_mask,
        }
    }
}

/// All vertex-array attributes after numeric scalar conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct NumericVertexInput {
    pub attributes: [Option<NumericVertexAttribute>; MAX_VERTEX_ATTRIBUTES],
    pub fixed_attribute_mask: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ArrayComponent {
    attribute: Option<u8>,
    byte_offset: u8,
    byte_width: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexArrayLayout {
    base_offset: u32,
    stride_bytes: u8,
    minimum_record_size: u8,
    components: Vec<ArrayComponent>,
}

impl VertexArrayLayout {
    pub const fn base_offset(&self) -> u32 {
        self.base_offset
    }

    pub const fn stride_bytes(&self) -> u8 {
        self.stride_bytes
    }

    pub const fn minimum_record_size(&self) -> u8 {
        self.minimum_record_size
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PicaVertexInputLayout {
    formats: [VertexAttributeFormat; MAX_VERTEX_ATTRIBUTES],
    attribute_count: u8,
    fixed_attribute_mask: u16,
    arrays: Vec<VertexArrayLayout>,
}

/// Snapshot of the vertex-input registers decoded from a PICA200 register file.
///
/// The base address remains the raw value from GPUREG_ATTRIBBUFFERS_LOC. It
/// must not be treated as a host pointer or physical byte address until the
/// GPU address translation/DMA path has been implemented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PicaVertexInputState {
    pub raw_base_address_register: u32,
    pub layout: PicaVertexInputLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VertexInputError {
    TooManyArrays {
        count: usize,
    },
    InvalidAttributeCount {
        count: usize,
    },
    MissingRegister {
        register: u16,
    },
    InvalidAttributeIndex {
        array: usize,
        attribute: usize,
        active_count: usize,
    },
    InvalidComponentCount {
        array: usize,
        count: usize,
    },
    TooManyComponentsForAttribute {
        attribute: usize,
        count: usize,
    },
    InvalidStride {
        array: usize,
        calculated: usize,
        configured: usize,
    },
    AddressOverflow {
        base: u32,
        offset: u64,
    },
    PhysicalMemory(GpuPhysicalMemoryError),
}

impl std::fmt::Display for VertexInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for VertexInputError {}

impl From<GpuPhysicalMemoryError> for VertexInputError {
    fn from(error: GpuPhysicalMemoryError) -> Self {
        Self::PhysicalMemory(error)
    }
}

impl Pica200 {
    /// Reads the current vertex-input register block and decodes its array layouts.
    ///
    /// Twelve array descriptors are read from the documented three-register
    /// sequence beginning at GPUREG_ATTRIBBUFFER0_OFFSET. The raw base-address
    /// register is returned unchanged so the caller must resolve it through the
    /// physical GPU address mapping before calling read_vertex.
    pub fn vertex_input_state(&self) -> Result<PicaVertexInputState, VertexInputError> {
        let read_register = |register: u16| {
            self.register(register)
                .ok_or(VertexInputError::MissingRegister { register })
        };

        let raw_base_address_register = read_register(REG_ATTRIBBUFFERS_LOC)?;
        let format_low = read_register(REG_ATTRIBBUFFERS_FORMAT_LOW)?;
        let format_high = read_register(REG_ATTRIBBUFFERS_FORMAT_HIGH)?;
        let mut arrays = Vec::with_capacity(MAX_VERTEX_ARRAYS);

        for array_index in 0..MAX_VERTEX_ARRAYS {
            let offset_register = REG_ATTRIBBUFFER0_OFFSET + (array_index as u16 * 3);
            arrays.push((
                read_register(offset_register)?,
                read_register(offset_register + 1)?,
                read_register(offset_register + 2)?,
            ));
        }

        let layout = PicaVertexInputLayout::from_registers(format_low, format_high, &arrays)?;
        Ok(PicaVertexInputState {
            raw_base_address_register,
            layout,
        })
    }
}

impl PicaVertexInputLayout {
    /// Decodes global format registers plus up to twelve array triplets.
    ///
    /// Each array tuple is (offset, config1, config2), ordered from array 0.
    /// The global vertex-array base is supplied later to read_vertex.
    pub fn from_registers(
        format_low: u32,
        format_high: u32,
        arrays: &[(u32, u32, u32)],
    ) -> Result<Self, VertexInputError> {
        if arrays.len() > MAX_VERTEX_ARRAYS {
            return Err(VertexInputError::TooManyArrays {
                count: arrays.len(),
            });
        }

        let attribute_count = (((format_high >> 28) & 0xF) + 1) as usize;
        if attribute_count > MAX_VERTEX_ATTRIBUTES {
            return Err(VertexInputError::InvalidAttributeCount {
                count: attribute_count,
            });
        }
        let fixed_attribute_mask = ((format_high >> 16) & 0x0FFF) as u16;
        let default_format = VertexAttributeFormat {
            data_type: AttributeDataType::SignedByte,
            component_count: 1,
        };
        let mut formats = [default_format; MAX_VERTEX_ATTRIBUTES];

        for (index, format) in formats.iter_mut().enumerate() {
            let field = if index < 8 {
                (format_low >> (index * 4)) & 0xF
            } else {
                (format_high >> ((index - 8) * 4)) & 0xF
            };
            *format = VertexAttributeFormat::decode(field);
        }

        let mut decoded_arrays = Vec::with_capacity(arrays.len());
        for (array_index, (offset, config1, config2)) in arrays.iter().copied().enumerate() {
            let component_count = ((config2 >> 28) & 0xF) as usize;
            let stride_bytes = ((config2 >> 16) & 0xFF) as usize;

            if component_count > MAX_COMPONENTS_PER_ARRAY {
                return Err(VertexInputError::InvalidComponentCount {
                    array: array_index,
                    count: component_count,
                });
            }
            if component_count == 0 {
                continue;
            }

            let mut components = Vec::with_capacity(component_count);
            let mut cursor = 0usize;
            let mut component_counts = [0usize; MAX_VERTEX_ATTRIBUTES];

            for component_index in 0..component_count {
                let code = if component_index < 8 {
                    ((config1 >> (component_index * 4)) & 0xF) as u8
                } else {
                    ((config2 >> ((component_index - 8) * 4)) & 0xF) as u8
                };

                if usize::from(code) < MAX_VERTEX_ATTRIBUTES {
                    let attribute = usize::from(code);
                    if attribute >= attribute_count {
                        return Err(VertexInputError::InvalidAttributeIndex {
                            array: array_index,
                            attribute,
                            active_count: attribute_count,
                        });
                    }

                    component_counts[attribute] += 1;
                    if component_counts[attribute] > MAX_COMPONENTS_PER_ATTRIBUTE {
                        return Err(VertexInputError::TooManyComponentsForAttribute {
                            attribute,
                            count: component_counts[attribute],
                        });
                    }

                    let width = formats[attribute].data_type.byte_width();
                    cursor = align_up(cursor, width);
                    components.push(ArrayComponent {
                        attribute: Some(code),
                        byte_offset: u8::try_from(cursor).map_err(|_| {
                            VertexInputError::InvalidComponentCount {
                                array: array_index,
                                count: component_count,
                            }
                        })?,
                        byte_width: width as u8,
                    });
                    cursor = cursor.checked_add(width).ok_or(
                        VertexInputError::InvalidComponentCount {
                            array: array_index,
                            count: component_count,
                        },
                    )?;
                } else {
                    let padding_bytes = match code {
                        12 => 4,
                        13 => 8,
                        14 => 12,
                        _ => 16,
                    };
                    cursor = align_up(cursor, 4);
                    components.push(ArrayComponent {
                        attribute: None,
                        byte_offset: u8::try_from(cursor).map_err(|_| {
                            VertexInputError::InvalidComponentCount {
                                array: array_index,
                                count: component_count,
                            }
                        })?,
                        byte_width: padding_bytes,
                    });
                    cursor = cursor.checked_add(padding_bytes as usize).ok_or(
                        VertexInputError::InvalidComponentCount {
                            array: array_index,
                            count: component_count,
                        },
                    )?;
                }
            }

            if cursor != stride_bytes {
                return Err(VertexInputError::InvalidStride {
                    array: array_index,
                    calculated: cursor,
                    configured: stride_bytes,
                });
            }

            decoded_arrays.push(VertexArrayLayout {
                base_offset: offset & 0x0FFF_FFFF,
                stride_bytes: stride_bytes as u8,
                minimum_record_size: cursor as u8,
                components,
            });
        }

        Ok(Self {
            formats,
            attribute_count: attribute_count as u8,
            fixed_attribute_mask,
            arrays: decoded_arrays,
        })
    }

    pub const fn attribute_count(&self) -> u8 {
        self.attribute_count
    }

    pub const fn fixed_attribute_mask(&self) -> u16 {
        self.fixed_attribute_mask
    }

    pub fn format(&self, attribute: usize) -> Option<VertexAttributeFormat> {
        if attribute < usize::from(self.attribute_count) {
            Some(self.formats[attribute])
        } else {
            None
        }
    }

    pub fn arrays(&self) -> &[VertexArrayLayout] {
        &self.arrays
    }

    /// Reads raw components for one vertex from each configured array.
    ///
    /// physical_base must already be a decoded byte address. This reader does
    /// not infer GPU register address units or implement DMA translation.
    pub fn read_vertex(
        &self,
        memory: &GpuPhysicalMemory,
        physical_base: u32,
        vertex_index: u32,
    ) -> Result<RawVertexInput, VertexInputError> {
        let mut output = RawVertexInput::new(self.fixed_attribute_mask);

        for array in &self.arrays {
            let stride_offset = u64::from(vertex_index)
                .checked_mul(u64::from(array.stride_bytes))
                .ok_or(VertexInputError::AddressOverflow {
                    base: physical_base,
                    offset: u64::MAX,
                })?;
            let array_offset = u64::from(array.base_offset)
                .checked_add(stride_offset)
                .ok_or(VertexInputError::AddressOverflow {
                    base: physical_base,
                    offset: u64::MAX,
                })?;

            for component in &array.components {
                let Some(attribute_code) = component.attribute else {
                    continue;
                };
                let relative_offset = array_offset
                    .checked_add(u64::from(component.byte_offset))
                    .ok_or(VertexInputError::AddressOverflow {
                        base: physical_base,
                        offset: u64::MAX,
                    })?;
                let absolute_address = u64::from(physical_base)
                    .checked_add(relative_offset)
                    .ok_or(VertexInputError::AddressOverflow {
                        base: physical_base,
                        offset: relative_offset,
                    })?;
                let address = u32::try_from(absolute_address).map_err(|_| {
                    VertexInputError::AddressOverflow {
                        base: physical_base,
                        offset: relative_offset,
                    }
                })?;

                let mut bytes = [0u8; 4];
                memory.read(address, &mut bytes[..usize::from(component.byte_width)])?;
                let raw_bits = u32::from_le_bytes(bytes);
                let attribute_index = usize::from(attribute_code);
                let attribute = output.attributes[attribute_index]
                    .get_or_insert_with(|| RawVertexAttribute::new(self.formats[attribute_index]));
                attribute.push(attribute_index, raw_bits)?;
            }
        }

        Ok(output)
    }
}

fn align_up(value: usize, alignment: usize) -> usize {
    let remainder = value % alignment;
    if remainder == 0 {
        value
    } else {
        value + alignment - remainder
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{FCRAM_BASE, VRAM_BASE};

    #[test]
    fn reads_global_and_twelve_array_registers_from_pica200() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x0123_4000)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_LOW, 1).unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, (1 << 28) | (1 << 16))
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET, 0x20).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 1, 0).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 2, (1 << 16) | (1 << 28))
            .unwrap();

        let state = gpu.vertex_input_state().unwrap();
        assert_eq!(state.raw_base_address_register, 0x0123_4000);
        assert_eq!(state.layout.attribute_count(), 2);
        assert_eq!(state.layout.fixed_attribute_mask(), 1);
        assert_eq!(state.layout.arrays().len(), 1);
        assert_eq!(state.layout.arrays()[0].base_offset(), 0x20);
        assert_eq!(state.layout.arrays()[0].stride_bytes(), 1);
    }

    #[test]
    fn decodes_twelve_global_formats_and_fixed_attribute_mask() {
        let format_low = 0x7654_3210;
        let format_high = 0x0000_FBA6 | (0x005 << 16) | (11 << 28);
        let layout = PicaVertexInputLayout::from_registers(format_low, format_high, &[]).unwrap();

        assert_eq!(layout.attribute_count(), 12);
        assert_eq!(layout.fixed_attribute_mask(), 0x005);
        assert_eq!(layout.formats[0].data_type, AttributeDataType::SignedByte);
        assert_eq!(layout.formats[0].component_count, 1);
        assert_eq!(layout.formats[8].data_type, AttributeDataType::SignedShort);
        assert_eq!(layout.formats[8].component_count, 2);
        assert_eq!(layout.formats[11].data_type, AttributeDataType::Float);
        assert_eq!(layout.formats[11].component_count, 4);
    }

    #[test]
    fn rejects_attribute_count_above_hardware_limit() {
        let result = PicaVertexInputLayout::from_registers(0, 15 << 28, &[]);
        assert_eq!(
            result,
            Err(VertexInputError::InvalidAttributeCount { count: 16 })
        );
    }

    #[test]
    fn decodes_component_alignment_padding_and_stride() {
        let format_low = 1 | (2 << 4);
        let config1 = (1 << 4) | (12 << 8);
        let config2 = (8 << 16) | (3 << 28);
        let layout =
            PicaVertexInputLayout::from_registers(format_low, 1 << 28, &[(0x20, config1, config2)])
                .unwrap();

        let array = &layout.arrays()[0];
        assert_eq!(array.base_offset(), 0x20);
        assert_eq!(array.stride_bytes(), 8);
        assert_eq!(array.minimum_record_size(), 8);
    }

    #[test]
    fn reads_raw_unsigned_byte_and_signed_short_components_from_fcram() {
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = FCRAM_BASE + 0x1000;
        let format_low = 1 | (2 << 4);
        let config1 = (1 << 4) | (12 << 8);
        let config2 = (8 << 16) | (3 << 28);
        let layout =
            PicaVertexInputLayout::from_registers(format_low, 1 << 28, &[(0x20, config1, config2)])
                .unwrap();

        memory
            .write(physical_base + 0x20, &[0x7F, 0x00, 0xFE, 0xFF, 0, 0, 0, 0])
            .unwrap();
        let vertex = layout.read_vertex(&memory, physical_base, 0).unwrap();

        let first = vertex.attributes[0].unwrap();
        assert_eq!(first.format.data_type, AttributeDataType::UnsignedByte);
        assert_eq!(first.component_count, 1);
        assert_eq!(first.components[0], 0x7F);

        let second = vertex.attributes[1].unwrap();
        assert_eq!(second.format.data_type, AttributeDataType::SignedShort);
        assert_eq!(second.component_count, 1);
        assert_eq!(second.components[0], 0xFFFE);
    }

    #[test]
    fn preserves_multiple_float_component_payloads_as_raw_bits() {
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = VRAM_BASE + 0x1000;
        let format_low = 0xB;
        let config1 = 0;
        let config2 = (12 << 16) | (3 << 28);
        let layout =
            PicaVertexInputLayout::from_registers(format_low, 1 << 28, &[(0, config1, config2)])
                .unwrap();

        memory
            .write(
                physical_base,
                &[
                    0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x40,
                ],
            )
            .unwrap();

        let vertex = layout.read_vertex(&memory, physical_base, 0).unwrap();
        let position = vertex.attributes[0].unwrap();
        assert_eq!(position.component_count, 3);
        assert_eq!(
            position.components[..3],
            [1.0f32.to_bits(), 2.0f32.to_bits(), 3.0f32.to_bits()]
        );
    }

    #[test]
    fn rejects_stride_that_does_not_match_decoded_record_size() {
        let layout =
            PicaVertexInputLayout::from_registers(1, 1 << 28, &[(0, 0, (4 << 16) | (1 << 28))]);
        assert!(matches!(
            layout,
            Err(VertexInputError::InvalidStride {
                array: 0,
                calculated: 1,
                configured: 4
            })
        ));
    }

    #[test]
    fn rejects_inactive_attributes_and_more_than_four_components() {
        let inactive =
            PicaVertexInputLayout::from_registers(0, 0, &[(0, 1, (1 << 16) | (1 << 28))]);
        assert!(matches!(
            inactive,
            Err(VertexInputError::InvalidAttributeIndex { .. })
        ));

        let too_many =
            PicaVertexInputLayout::from_registers(0, 0, &[(0, 0, (5 << 16) | (5 << 28))]);
        assert!(matches!(
            too_many,
            Err(VertexInputError::TooManyComponentsForAttribute { .. })
        ));
    }

    #[test]
    fn rejects_reads_from_unmapped_physical_memory() {
        let memory = GpuPhysicalMemory::new();
        let layout =
            PicaVertexInputLayout::from_registers(1, 1 << 28, &[(0, 0, (1 << 16) | (1 << 28))])
                .unwrap();

        assert!(matches!(
            layout.read_vertex(&memory, 0x1000, 0),
            Err(VertexInputError::PhysicalMemory(
                GpuPhysicalMemoryError::UnmappedAddress { .. }
            ))
        ));
    }
}
