//! PICA200 fixed vertex attributes and packed float24 decoding.
//!
//! A fixed attribute is a four-component float1.7.16 tuple written through
//! GPUREG_FIXEDATTRIB_INDEX and GPUREG_FIXEDATTRIB_DATA0..2. The raw 24-bit
//! components are retained alongside their numeric representation.

use super::{
    AttributeBitWidth, AttributeDataType, NumericVertexAttribute, Pica200, VertexAttributeFormat,
};

pub const REG_FIXEDATTRIB_INDEX: u16 = 0x232;
pub const REG_FIXEDATTRIB_DATA0: u16 = 0x233;
pub const REG_FIXEDATTRIB_DATA1: u16 = 0x234;
pub const REG_FIXEDATTRIB_DATA2: u16 = 0x235;
pub const FIXED_ATTRIBUTE_COUNT: usize = 12;
pub const IMMEDIATE_ATTRIBUTE_INDEX: u8 = 0xF;

/// Raw PICA200 float1.7.16 scalar, stored in the low 24 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Float24(u32);

impl Float24 {
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & 0x00FF_FFFF)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Converts the PICA 1-sign / 7-exponent / 16-mantissa representation to
    /// f32. Exponent zero is flushed to positive zero, matching the documented
    /// PICA shader input behavior for subnormal values. Exponent 0x7F maps to
    /// IEEE infinity/NaN; normal exponents use the PICA bias of 63.
    pub fn to_f32(self) -> f32 {
        let sign = (self.0 >> 23) & 1;
        let exponent = (self.0 >> 16) & 0x7F;
        let mantissa = self.0 & 0xFFFF;

        if exponent == 0 {
            return 0.0;
        }

        let ieee_exponent = if exponent == 0x7F {
            0xFF
        } else {
            exponent + 64
        };
        let bits = (sign << 31) | (ieee_exponent << 23) | (mantissa << 7);
        f32::from_bits(bits)
    }
}

/// One stored fixed vertex attribute, kept in packed PICA float24 form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedVertexAttribute {
    components: [Float24; 4],
}

impl FixedVertexAttribute {
    /// Unpacks DATA0, DATA1 and DATA2 into X/Y/Z/W 24-bit scalars.
    pub const fn from_register_words(data0: u32, data1: u32, data2: u32) -> Self {
        let x = data2 & 0x00FF_FFFF;
        let y = ((data2 >> 24) & 0xFF) | ((data1 & 0xFFFF) << 8);
        let z = ((data1 >> 16) & 0xFFFF) | ((data0 & 0xFF) << 16);
        let w = (data0 >> 8) & 0x00FF_FFFF;

        Self {
            components: [
                Float24::from_bits(x),
                Float24::from_bits(y),
                Float24::from_bits(z),
                Float24::from_bits(w),
            ],
        }
    }

    pub const fn raw_components(self) -> [Float24; 4] {
        self.components
    }

    pub fn to_numeric(self) -> NumericVertexAttribute {
        NumericVertexAttribute {
            format: VertexAttributeFormat {
                data_type: AttributeDataType::Float,
                bit_width: AttributeBitWidth::Bits24,
            },
            components: self.components.map(Float24::to_f32),
            component_count: 4,
        }
    }
}

impl Pica200 {
    /// Returns the last fixed-attribute value written for an index 0..11.
    pub fn fixed_vertex_attribute(&self, index: usize) -> Option<FixedVertexAttribute> {
        let [data0, data1, data2] = self.fixed_attribute_words(index)?;
        Some(FixedVertexAttribute::from_register_words(
            data0, data1, data2,
        ))
    }

    /// Resolves fixed inputs selected by the vertex-loader mask.
    ///
    /// Array-supplied values win over fixed values, as specified by the PICA
    /// programming guide. The resolved mask records which fixed values were
    /// actually inserted into this vertex.
    pub fn apply_fixed_vertex_attributes(&self, input: &mut super::NumericVertexInput) {
        for index in 0..FIXED_ATTRIBUTE_COUNT {
            let bit = 1u16 << index;
            if input.fixed_attribute_mask & bit == 0 || input.attributes[index].is_some() {
                continue;
            }

            if let Some(fixed) = self.fixed_vertex_attribute(index) {
                input.attributes[index] = Some(fixed.to_numeric());
                input.resolved_fixed_attribute_mask |= bit;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_normal_float24_values_and_sign() {
        assert_eq!(Float24::from_bits(0x003F_0000).to_f32(), 1.0);
        assert_eq!(Float24::from_bits(0x0040_0000).to_f32(), 2.0);
        assert_eq!(Float24::from_bits(0x00C0_8000).to_f32(), -3.0);
        assert_eq!(Float24::from_bits(0x003E_0000).to_f32(), 0.5);
    }

    #[test]
    fn flushes_float24_subnormals_to_positive_zero() {
        assert_eq!(Float24::from_bits(0x0000_0001).to_f32().to_bits(), 0);
        assert_eq!(Float24::from_bits(0x0080_0000).to_f32().to_bits(), 0);
    }

    #[test]
    fn preserves_float24_infinity_and_nan_encodings() {
        assert_eq!(Float24::from_bits(0x007F_0000).to_f32(), f32::INFINITY);
        assert_eq!(Float24::from_bits(0x00FF_0000).to_f32(), f32::NEG_INFINITY);
        assert!(Float24::from_bits(0x007F_0001).to_f32().is_nan());
    }

    #[test]
    fn pica200_stores_fixed_attributes_selected_by_index_register() {
        let mut gpu = Pica200::new();
        let data0 = (0x0041_0000u32 << 8) | 0x40;
        let data1 = 0x4000;
        let data2 = 0x003F_0000;

        gpu.write_register(REG_FIXEDATTRIB_INDEX, 2).unwrap();
        gpu.write_register(REG_FIXEDATTRIB_DATA0, data0).unwrap();
        gpu.write_register(REG_FIXEDATTRIB_DATA1, data1).unwrap();
        gpu.write_register(REG_FIXEDATTRIB_DATA2, data2).unwrap();

        assert_eq!(gpu.fixed_attribute_words(2), Some([data0, data1, data2]));
        assert_eq!(gpu.fixed_attribute_words(1), Some([0, 0, 0]));
        assert_eq!(
            gpu.fixed_vertex_attribute(2)
                .unwrap()
                .to_numeric()
                .components,
            [1.0, 2.0, 2.0, 4.0]
        );
    }

    #[test]
    fn command_list_writes_update_the_selected_fixed_attribute() {
        let mut gpu = Pica200::new();
        let words = [
            1,
            (0xF << 16) | u32::from(REG_FIXEDATTRIB_INDEX),
            0x4100_0040,
            (0xF << 16) | u32::from(REG_FIXEDATTRIB_DATA0),
            0x0000_4000,
            (0xF << 16) | u32::from(REG_FIXEDATTRIB_DATA1),
            0x003F_0000,
            (0xF << 16) | u32::from(REG_FIXEDATTRIB_DATA2),
        ];

        gpu.execute_command_list(&words).unwrap();

        assert_eq!(
            gpu.fixed_attribute_words(1),
            Some([0x4100_0040, 0x0000_4000, 0x003F_0000])
        );
    }

    #[test]
    fn fixed_attributes_fill_masked_inputs_but_never_override_array_values() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_FIXEDATTRIB_INDEX, 0).unwrap();
        gpu.write_register(REG_FIXEDATTRIB_DATA0, (0x0041_0000 << 8) | 0x40)
            .unwrap();
        gpu.write_register(REG_FIXEDATTRIB_DATA1, 0x4000).unwrap();
        gpu.write_register(REG_FIXEDATTRIB_DATA2, 0x003F_0000)
            .unwrap();

        let mut input = super::super::NumericVertexInput {
            attributes: [None; 12],
            fixed_attribute_mask: 1,
            resolved_fixed_attribute_mask: 0,
        };
        gpu.apply_fixed_vertex_attributes(&mut input);

        assert_eq!(
            input.attributes[0].unwrap().components,
            [1.0, 2.0, 2.0, 4.0]
        );
        assert_eq!(input.resolved_fixed_attribute_mask, 1);

        let array_value = NumericVertexAttribute {
            format: VertexAttributeFormat {
                data_type: AttributeDataType::Float,
                bit_width: AttributeBitWidth::Bits32,
            },
            components: [9.0, 8.0, 7.0, 6.0],
            component_count: 4,
        };
        let mut array_input = super::super::NumericVertexInput {
            attributes: [None; 12],
            fixed_attribute_mask: 1,
            resolved_fixed_attribute_mask: 0,
        };
        array_input.attributes[0] = Some(array_value);
        gpu.apply_fixed_vertex_attributes(&mut array_input);

        assert_eq!(array_input.attributes[0], Some(array_value));
        assert_eq!(array_input.resolved_fixed_attribute_mask, 0);
    }

    #[test]
    fn unpacks_fixed_attribute_register_words_into_xyzw() {
        let words = [
            // W = 4.0, high byte of Z = 0x40.
            (0x0041_0000u32 << 8) | 0x40,
            // low Z = 0x0000, high Y = 0x4000.
            0x4000u32 << 16,
            // X = 1.0, low Y = 0x0000.
            0x003F_0000,
        ];
        let value = FixedVertexAttribute::from_register_words(words[0], words[1], words[2]);
        assert_eq!(
            value.raw_components().map(Float24::bits),
            [0x003F_0000, 0x0040_0000, 0x0040_0000, 0x0041_0000,]
        );
        assert_eq!(value.to_numeric().components, [1.0, 2.0, 2.0, 4.0]);
    }
}
