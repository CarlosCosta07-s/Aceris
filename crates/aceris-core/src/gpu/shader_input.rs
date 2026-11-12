//! PICA200 vertex-shader input-buffer and attribute-permutation decoding.
//!
//! This module describes how fetched vertex attributes are assigned to shader
//! input registers. It does not execute the vertex shader or resolve fixed
//! attribute register payloads.

use super::{NumericVertexAttribute, NumericVertexInput, Pica200};

pub const REG_VSH_INPUTBUFFER_CONFIG: u16 = 0x2B9;
pub const REG_VSH_ATTRIBUTES_PERMUTATION_LOW: u16 = 0x2BB;
pub const REG_VSH_ATTRIBUTES_PERMUTATION_HIGH: u16 = 0x2BC;
pub const MAX_VERTEX_SHADER_INPUTS: usize = 12;
pub const MAX_VERTEX_SHADER_REGISTERS: usize = 16;

/// Captured shader-input state from the PICA200 register bank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PicaVertexShaderInputState {
    pub raw_input_buffer_config: u32,
    pub layout: PicaVertexShaderInputLayout,
}

/// Mapping from input-buffer attribute order to shader registers v0-v15.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PicaVertexShaderInputLayout {
    attribute_count: u8,
    register_indices: [u8; MAX_VERTEX_SHADER_REGISTERS],
}

/// One supplied attribute and the shader input register it targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VertexShaderAttributeBinding {
    pub attribute_index: u8,
    pub shader_register: u8,
    pub value: NumericVertexAttribute,
}

/// Vertex shader bindings keep the source order and any duplicate destination
/// registers explicit; no undocumented collision precedence is assumed.
#[derive(Clone, Debug, PartialEq)]
pub struct VertexShaderInputBindings {
    pub attributes: Vec<VertexShaderAttributeBinding>,
    /// Attributes marked fixed by the vertex-loader mask still require values
    /// from the fixed-attribute registers, which are not decoded here.
    pub unresolved_fixed_attribute_mask: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VertexShaderInputError {
    MissingRegister { register: u16 },
    TooManyAttributes { count: usize },
}

impl std::fmt::Display for VertexShaderInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for VertexShaderInputError {}

impl Pica200 {
    /// Captures the vertex shader input count and attribute permutation.
    ///
    /// The count comes from GPUREG_VSH_INPUTBUFFER_CONFIG bits 0-3. Attribute
    /// destinations are decoded from the two 4-bit-per-entry permutation
    /// registers. Raw register values are preserved only where useful for
    /// diagnostics; no shader execution is performed.
    pub fn vertex_shader_input_state(
        &self,
    ) -> Result<PicaVertexShaderInputState, VertexShaderInputError> {
        let read_register = |register: u16| {
            self.register(register)
                .ok_or(VertexShaderInputError::MissingRegister { register })
        };

        let raw_input_buffer_config = read_register(REG_VSH_INPUTBUFFER_CONFIG)?;
        let permutation_low = read_register(REG_VSH_ATTRIBUTES_PERMUTATION_LOW)?;
        let permutation_high = read_register(REG_VSH_ATTRIBUTES_PERMUTATION_HIGH)?;

        let layout = PicaVertexShaderInputLayout::from_registers(
            raw_input_buffer_config,
            permutation_low,
            permutation_high,
        )?;

        Ok(PicaVertexShaderInputState {
            raw_input_buffer_config,
            layout,
        })
    }
}

impl PicaVertexShaderInputLayout {
    /// Decodes input count (bits 0-3, encoded as count minus one) and sixteen
    /// 4-bit shader-register destinations.
    pub fn from_registers(
        input_buffer_config: u32,
        permutation_low: u32,
        permutation_high: u32,
    ) -> Result<Self, VertexShaderInputError> {
        let attribute_count = ((input_buffer_config & 0xF) + 1) as usize;
        if attribute_count > MAX_VERTEX_SHADER_INPUTS {
            return Err(VertexShaderInputError::TooManyAttributes {
                count: attribute_count,
            });
        }

        let mut register_indices = [0u8; MAX_VERTEX_SHADER_REGISTERS];
        for (index, destination) in register_indices.iter_mut().enumerate() {
            let (register, shift) = if index < 8 {
                (permutation_low, index * 4)
            } else {
                (permutation_high, (index - 8) * 4)
            };
            *destination = ((register >> shift) & 0xF) as u8;
        }

        Ok(Self {
            attribute_count: attribute_count as u8,
            register_indices,
        })
    }

    pub const fn attribute_count(&self) -> u8 {
        self.attribute_count
    }

    /// Returns the shader input register for a zero-based loader attribute.
    pub fn shader_register_for_attribute(&self, attribute: usize) -> Option<u8> {
        if attribute < usize::from(self.attribute_count) {
            Some(self.register_indices[attribute])
        } else {
            None
        }
    }

    /// Associates fetched numeric attributes with their configured shader
    /// registers while retaining unresolved fixed attributes as a mask.
    ///
    /// Duplicate destination registers are kept as separate bindings in
    /// source order; later shader execution can define their hardware effects.
    pub fn bind_vertex(&self, vertex: &NumericVertexInput) -> VertexShaderInputBindings {
        let mut attributes = Vec::with_capacity(usize::from(self.attribute_count));
        for attribute_index in 0..usize::from(self.attribute_count) {
            let Some(value) = vertex.attributes[attribute_index] else {
                continue;
            };
            attributes.push(VertexShaderAttributeBinding {
                attribute_index: attribute_index as u8,
                shader_register: self.register_indices[attribute_index],
                value,
            });
        }

        let mut unresolved_fixed_attribute_mask =
            vertex.fixed_attribute_mask & !vertex.resolved_fixed_attribute_mask;
        for (index, attribute) in vertex.attributes.iter().enumerate() {
            if attribute.is_some() {
                unresolved_fixed_attribute_mask &= !(1u16 << index);
            }
        }

        VertexShaderInputBindings {
            attributes,
            unresolved_fixed_attribute_mask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{
        AttributeDataType, REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, REG_VSH_ATTRIBUTES_PERMUTATION_LOW,
        REG_VSH_INPUTBUFFER_CONFIG, VertexAttributeFormat,
    };

    fn numeric_attribute(value: f32) -> NumericVertexAttribute {
        NumericVertexAttribute {
            format: VertexAttributeFormat {
                data_type: AttributeDataType::Float,
                bit_width: crate::gpu::AttributeBitWidth::Bits32,
            },
            components: [value, 0.0, 0.0, 0.0],
            component_count: 1,
        }
    }

    #[test]
    fn snapshots_shader_input_count_and_permutation_registers() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_INPUTBUFFER_CONFIG, 2).unwrap();
        gpu.write_register(REG_VSH_ATTRIBUTES_PERMUTATION_LOW, 5 | (1 << 4) | (9 << 8))
            .unwrap();
        gpu.write_register(REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, 0x7654_3210)
            .unwrap();

        let state = gpu.vertex_shader_input_state().unwrap();
        assert_eq!(state.raw_input_buffer_config, 2);
        assert_eq!(state.layout.attribute_count(), 3);
        assert_eq!(state.layout.shader_register_for_attribute(0), Some(5));
        assert_eq!(state.layout.shader_register_for_attribute(1), Some(1));
        assert_eq!(state.layout.shader_register_for_attribute(2), Some(9));
        assert_eq!(state.layout.shader_register_for_attribute(3), None);
        assert_eq!(state.layout.shader_register_for_attribute(8), None);
    }

    #[test]
    fn maps_numeric_attributes_to_shader_registers_and_preserves_fixed_mask() {
        let layout =
            PicaVertexShaderInputLayout::from_registers(2, 7 | (3 << 4) | (12 << 8), 0).unwrap();
        let mut vertex = NumericVertexInput {
            attributes: [None; 12],
            fixed_attribute_mask: 1 << 2,
            resolved_fixed_attribute_mask: 0,
        };
        vertex.attributes[0] = Some(numeric_attribute(1.0));
        vertex.attributes[1] = Some(numeric_attribute(2.0));

        let bindings = layout.bind_vertex(&vertex);
        assert_eq!(bindings.attributes.len(), 2);
        assert_eq!(bindings.attributes[0].attribute_index, 0);
        assert_eq!(bindings.attributes[0].shader_register, 7);
        assert_eq!(bindings.attributes[0].value.components[0], 1.0);
        assert_eq!(bindings.attributes[1].attribute_index, 1);
        assert_eq!(bindings.attributes[1].shader_register, 3);
        assert_eq!(bindings.attributes[1].value.components[0], 2.0);
        assert_eq!(bindings.unresolved_fixed_attribute_mask, 1 << 2);
    }

    #[test]
    fn preserves_duplicate_destination_registers_without_guessing_precedence() {
        let layout = PicaVertexShaderInputLayout::from_registers(1, 4 | (4 << 4), 0).unwrap();
        let mut vertex = NumericVertexInput {
            attributes: [None; 12],
            fixed_attribute_mask: 0,
            resolved_fixed_attribute_mask: 0,
        };
        vertex.attributes[0] = Some(numeric_attribute(3.0));
        vertex.attributes[1] = Some(numeric_attribute(6.0));

        let bindings = layout.bind_vertex(&vertex);
        assert_eq!(bindings.attributes.len(), 2);
        assert_eq!(bindings.attributes[0].shader_register, 4);
        assert_eq!(bindings.attributes[1].shader_register, 4);
    }

    #[test]
    fn rejects_input_attribute_count_above_vertex_loader_limit() {
        assert_eq!(
            PicaVertexShaderInputLayout::from_registers(12, 0, 0),
            Err(VertexShaderInputError::TooManyAttributes { count: 13 })
        );
    }
}
