//! PICA200 vertex shader entrypoint and output map.
//!
//! The output map routes compacted vertex-shader outputs (o0-o15) to PICA
//! semantics. GPUREG_VSH_OUTMAP_MASK selects enabled shader output registers;
//! GPUREG_SH_OUTMAP_TOTAL limits how many semantic-map rows are consumed from
//! GPUREG_SH_OUTMAP_O0..O6. Rows map to enabled outputs in ascending register
//! order, so a sparse mask does not shift data into the wrong shader register.
//!
//! Only position, color and texture-coordinate groups are resolved. Other
//! documented semantics (quaternion, view vector, texcoord0 w) are accepted
//! and ignored. The semantic numbering follows public hardware documentation.
//!
//! Incomplete semantic groups and duplicate semantics are explicit errors.

use super::geometry::GeometryVertex;
use super::pica200::Pica200;
use super::{PicaShadedVertex, SHADER_OUTPUT_REGISTERS};

/// GPUREG_VSH_ENTRYPOINT; bits 0..15 hold the first instruction index.
pub const REG_VSH_ENTRYPOINT: u16 = 0x2BA;
/// GPUREG_SH_OUTMAP_TOTAL: number of semantic-map rows consumed (0..=7).
pub const REG_SH_OUTMAP_TOTAL: u16 = 0x04F;
/// First of the seven GPUREG_SH_OUTMAP_O registers (o0).
pub const REG_SH_OUTMAP_O0: u16 = 0x050;
/// GPUREG_VSH_OUTMAP_MASK: enabled vertex-shader output registers o0-o15.
pub const REG_VSH_OUTMAP_MASK: u16 = 0x2BD;
/// Number of output-map rows available (o0-o6).
pub const OUTMAP_REGISTER_COUNT: usize = 7;

const UNUSED_SEMANTIC: u8 = 0x1F;
const SEMANTIC_SLOTS: usize = 32;

const POSITION: [u8; 4] = [0x00, 0x01, 0x02, 0x03];
const COLOR: [u8; 4] = [0x08, 0x09, 0x0A, 0x0B];
const TEXCOORD0: [u8; 2] = [0x0C, 0x0D];
const TEXCOORD1: [u8; 2] = [0x0E, 0x0F];
const TEXCOORD2: [u8; 2] = [0x16, 0x17];

/// A group of output semantics that must be mapped completely or not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaOutputGroup {
    Position,
    Color,
    TexCoord0,
    TexCoord1,
    TexCoord2,
}

/// Failure while decoding or applying an output map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaOutputMapError {
    /// Two lanes carry the same semantic; which one wins is undocumented.
    DuplicateSemantic { semantic: u8 },
    /// Some, but not all, lanes of a group are mapped.
    IncompleteGroup { group: PicaOutputGroup },
    /// No position is mapped, so the vertex cannot be placed.
    MissingPosition,
    /// The guest requested more semantic-map rows than the hardware exposes.
    OutputCountExceedsMap { count: usize, maximum: usize },
    /// The vertex-shader output mask enables fewer registers than the map uses.
    OutputMaskTooSmall { required: usize, enabled: usize },
}

impl std::fmt::Display for PicaOutputMapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateSemantic { semantic } => {
                write!(formatter, "output semantic {semantic:#04x} is mapped twice")
            }
            Self::IncompleteGroup { group } => {
                write!(formatter, "output group {group:?} is only partly mapped")
            }
            Self::MissingPosition => write!(formatter, "the output map has no position"),
            Self::OutputCountExceedsMap { count, maximum } => write!(
                formatter,
                "PICA output-map count {count} exceeds the {maximum} available rows"
            ),
            Self::OutputMaskTooSmall { required, enabled } => write!(
                formatter,
                "PICA output-map needs {required} enabled shader outputs, but the mask enables {enabled}"
            ),
        }
    }
}

impl std::error::Error for PicaOutputMapError {}

/// Decoded semantic-to-output-lane mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PicaVertexOutputMap {
    /// For each semantic byte below 32: the (output register, lane) carrying it.
    slots: [Option<(u8, u8)>; SEMANTIC_SLOTS],
}

/// A vertex whose outputs were routed through the output map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaResolvedVertex {
    pub position: [f32; 4],
    pub color: Option<[f32; 4]>,
    pub texcoords: [Option<[f32; 2]>; 3],
}

impl PicaVertexOutputMap {
    /// Decodes all seven rows as a dense o0-o6 map.
    ///
    /// This convenience constructor is useful for isolated tests and callers
    /// that already have dense output registers. Guest state should use the
    /// masked constructor so disabled shader outputs are skipped.
    pub fn from_registers(
        registers: [u32; OUTMAP_REGISTER_COUNT],
    ) -> Result<Self, PicaOutputMapError> {
        let dense_mask = (1u16 << OUTMAP_REGISTER_COUNT) - 1;
        Self::from_registers_masked(registers, dense_mask, OUTMAP_REGISTER_COUNT)
    }

    /// Decodes semantic-map rows using the guest's vertex-shader output mask.
    ///
    /// The first output_count rows are associated with enabled shader outputs
    /// in ascending bit order. Rows beyond output_count are intentionally
    /// ignored, even if their register contents are stale.
    pub fn from_registers_masked(
        registers: [u32; OUTMAP_REGISTER_COUNT],
        output_mask: u16,
        output_count: usize,
    ) -> Result<Self, PicaOutputMapError> {
        if output_count > OUTMAP_REGISTER_COUNT {
            return Err(PicaOutputMapError::OutputCountExceedsMap {
                count: output_count,
                maximum: OUTMAP_REGISTER_COUNT,
            });
        }

        let mut enabled_outputs = [0u8; SHADER_OUTPUT_REGISTERS];
        let mut enabled_count = 0usize;
        for output in 0..SHADER_OUTPUT_REGISTERS {
            if output_mask & (1u16 << output) != 0 {
                enabled_outputs[enabled_count] = output as u8;
                enabled_count += 1;
            }
        }
        if enabled_count < output_count {
            return Err(PicaOutputMapError::OutputMaskTooSmall {
                required: output_count,
                enabled: enabled_count,
            });
        }

        let mut slots = [None; SEMANTIC_SLOTS];
        for (row, register) in registers.iter().take(output_count).enumerate() {
            let shader_output = enabled_outputs[row];
            for lane in 0..4usize {
                let semantic = ((register >> (lane * 8)) & 0xFF) as u8;
                if semantic == UNUSED_SEMANTIC || usize::from(semantic) >= SEMANTIC_SLOTS {
                    continue;
                }
                let slot = &mut slots[usize::from(semantic)];
                if slot.is_some() {
                    return Err(PicaOutputMapError::DuplicateSemantic { semantic });
                }
                *slot = Some((shader_output, lane as u8));
            }
        }
        Ok(Self { slots })
    }

    /// The (output register, lane) that carries `semantic`, if mapped.
    pub fn lane_for_semantic(&self, semantic: u8) -> Option<(u8, u8)> {
        self.slots.get(usize::from(semantic)).copied().flatten()
    }

    fn group<const N: usize>(
        &self,
        outputs: &[[f32; 4]; SHADER_OUTPUT_REGISTERS],
        group: PicaOutputGroup,
        semantics: [u8; N],
    ) -> Result<Option<[f32; N]>, PicaOutputMapError> {
        let mapped = semantics
            .iter()
            .filter(|&&semantic| self.lane_for_semantic(semantic).is_some())
            .count();
        if mapped == 0 {
            return Ok(None);
        }
        if mapped != N {
            return Err(PicaOutputMapError::IncompleteGroup { group });
        }
        let mut values = [0.0f32; N];
        for (value, semantic) in values.iter_mut().zip(semantics) {
            let (output, lane) = self
                .lane_for_semantic(semantic)
                .expect("completeness was checked above");
            *value = outputs[usize::from(output)][usize::from(lane)];
        }
        Ok(Some(values))
    }

    /// Routes raw shader outputs into position, color and texture coordinates.
    pub fn resolve(
        &self,
        outputs: &[[f32; 4]; SHADER_OUTPUT_REGISTERS],
    ) -> Result<PicaResolvedVertex, PicaOutputMapError> {
        let position = self
            .group(outputs, PicaOutputGroup::Position, POSITION)?
            .ok_or(PicaOutputMapError::MissingPosition)?;
        Ok(PicaResolvedVertex {
            position,
            color: self.group(outputs, PicaOutputGroup::Color, COLOR)?,
            texcoords: [
                self.group(outputs, PicaOutputGroup::TexCoord0, TEXCOORD0)?,
                self.group(outputs, PicaOutputGroup::TexCoord1, TEXCOORD1)?,
                self.group(outputs, PicaOutputGroup::TexCoord2, TEXCOORD2)?,
            ],
        })
    }
}

impl PicaResolvedVertex {
    /// Converts to a geometry-stage vertex.
    ///
    /// Color channels are clamped to 0..=1 and rounded to 8 bits. A vertex with
    /// no mapped color takes `default_color`; the hardware behavior for that
    /// case is not modeled, so the caller chooses explicitly.
    pub fn to_geometry_vertex(&self, default_color: [u8; 4]) -> GeometryVertex {
        let color = match self.color {
            Some(channels) => channels.map(|channel| {
                if channel.is_nan() {
                    0
                } else {
                    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
                }
            }),
            None => default_color,
        };
        GeometryVertex::new(self.position, color)
    }
}

impl PicaShadedVertex {
    /// Routes this vertex's raw outputs through `map`.
    pub fn resolve(
        &self,
        map: &PicaVertexOutputMap,
    ) -> Result<PicaResolvedVertex, PicaOutputMapError> {
        map.resolve(&self.outputs)
    }
}

impl Pica200 {
    /// First shader instruction index from GPUREG_VSH_ENTRYPOINT bits 0..15.
    pub fn vertex_shader_entrypoint(&self) -> usize {
        let raw = self.register(REG_VSH_ENTRYPOINT).unwrap_or(0);
        (raw & 0xFFFF) as usize
    }

    /// Decodes the current output-map rows and guest output-register mask.
    pub fn vertex_output_map(&self) -> Result<PicaVertexOutputMap, PicaOutputMapError> {
        let mut registers = [0u32; OUTMAP_REGISTER_COUNT];
        for (index, register) in registers.iter_mut().enumerate() {
            *register = self.register(REG_SH_OUTMAP_O0 + index as u16).unwrap_or(0);
        }
        let output_count = (self.register(REG_SH_OUTMAP_TOTAL).unwrap_or(0) & 0x7) as usize;
        let output_mask = self.register(REG_VSH_OUTMAP_MASK).unwrap_or(0) as u16;
        PicaVertexOutputMap::from_registers_masked(registers, output_mask, output_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNUSED_ROW: u32 = 0x1F1F_1F1F;

    fn registers(rows: &[u32]) -> [u32; OUTMAP_REGISTER_COUNT] {
        let mut registers = [UNUSED_ROW; OUTMAP_REGISTER_COUNT];
        registers[..rows.len()].copy_from_slice(rows);
        registers
    }

    fn outputs_with(values: &[(usize, [f32; 4])]) -> [[f32; 4]; SHADER_OUTPUT_REGISTERS] {
        let mut outputs = [[0.0; 4]; SHADER_OUTPUT_REGISTERS];
        for &(index, value) in values {
            outputs[index] = value;
        }
        outputs
    }

    #[test]
    fn entrypoint_uses_only_the_low_sixteen_bits() {
        let mut gpu = Pica200::new();
        assert_eq!(gpu.vertex_shader_entrypoint(), 0);
        gpu.write_register(REG_VSH_ENTRYPOINT, 0x7FFF_0005).unwrap();
        assert_eq!(gpu.vertex_shader_entrypoint(), 5);
    }

    #[test]
    fn position_color_and_texcoord_are_routed_by_semantic() {
        let map = PicaVertexOutputMap::from_registers(registers(&[
            0x0302_0100, // o0 = position xyzw
            0x0B0A_0908, // o1 = color rgba
            0x1F1F_0D0C, // o2 = texcoord0 u, v
        ]))
        .unwrap();
        let outputs = outputs_with(&[
            (0, [1.0, 2.0, 3.0, 4.0]),
            (1, [0.1, 0.2, 0.3, 0.4]),
            (2, [0.5, 0.6, 9.0, 9.0]),
        ]);

        let vertex = map.resolve(&outputs).unwrap();
        assert_eq!(vertex.position, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(vertex.color, Some([0.1, 0.2, 0.3, 0.4]));
        assert_eq!(vertex.texcoords, [Some([0.5, 0.6]), None, None]);
    }

    #[test]
    fn lanes_can_be_permuted_across_registers() {
        // Position components are spread over two registers and reordered.
        let map = PicaVertexOutputMap::from_registers(registers(&[
            0x1F1F_0300, // o0.x = pos.x, o0.y = pos.w
            0x1F1F_0201, // o1.x = pos.y, o1.y = pos.z
        ]))
        .unwrap();
        assert_eq!(map.lane_for_semantic(0x00), Some((0, 0)));
        assert_eq!(map.lane_for_semantic(0x03), Some((0, 1)));
        assert_eq!(map.lane_for_semantic(0x01), Some((1, 0)));
        assert_eq!(map.lane_for_semantic(0x02), Some((1, 1)));

        let outputs = outputs_with(&[(0, [10.0, 40.0, 0.0, 0.0]), (1, [20.0, 30.0, 0.0, 0.0])]);
        assert_eq!(
            map.resolve(&outputs).unwrap().position,
            [10.0, 20.0, 30.0, 40.0]
        );
    }

    #[test]
    fn unused_and_untracked_semantics_are_ignored() {
        // 0x04 (quaternion), 0x12 (view) and 0x1F (unused) do not affect resolution.
        let map = PicaVertexOutputMap::from_registers(registers(&[
            0x0302_0100,
            0x1F12_0704, // o1: quat.x, quat.w, view.x, unused
        ]))
        .unwrap();
        let vertex = map
            .resolve(&outputs_with(&[(0, [1.0, 2.0, 3.0, 1.0])]))
            .unwrap();
        assert_eq!(vertex.color, None);
        assert_eq!(vertex.texcoords, [None, None, None]);
    }

    #[test]
    fn duplicate_semantics_are_rejected() {
        assert_eq!(
            PicaVertexOutputMap::from_registers(registers(&[0x0302_0100, 0x1F1F_1F00])),
            Err(PicaOutputMapError::DuplicateSemantic { semantic: 0x00 })
        );
    }

    #[test]
    fn partial_groups_and_missing_position_are_errors() {
        // Position with only x, y, z mapped.
        let map = PicaVertexOutputMap::from_registers(registers(&[0x1F02_0100])).unwrap();
        assert_eq!(
            map.resolve(&outputs_with(&[])),
            Err(PicaOutputMapError::IncompleteGroup {
                group: PicaOutputGroup::Position
            })
        );

        // Complete position but only the red channel of the color.
        let map =
            PicaVertexOutputMap::from_registers(registers(&[0x0302_0100, 0x1F1F_1F08])).unwrap();
        assert_eq!(
            map.resolve(&outputs_with(&[])),
            Err(PicaOutputMapError::IncompleteGroup {
                group: PicaOutputGroup::Color
            })
        );

        // Nothing mapped at all.
        let map = PicaVertexOutputMap::from_registers(registers(&[])).unwrap();
        assert_eq!(
            map.resolve(&outputs_with(&[])),
            Err(PicaOutputMapError::MissingPosition)
        );
    }

    #[test]
    fn geometry_vertex_color_is_clamped_rounded_and_defaulted() {
        let vertex = PicaResolvedVertex {
            position: [0.0, 0.0, 0.0, 1.0],
            color: Some([1.5, -0.5, 0.5, f32::NAN]),
            texcoords: [None; 3],
        };
        let geometry = vertex.to_geometry_vertex([9, 9, 9, 9]);
        // 0.5 * 255 = 127.5, which rounds away from zero to 128.
        assert_eq!(geometry.color, [255, 0, 128, 0]);
        assert_eq!(geometry.position, [0.0, 0.0, 0.0, 1.0]);

        let no_color = PicaResolvedVertex {
            color: None,
            ..vertex
        };
        assert_eq!(
            no_color.to_geometry_vertex([9, 8, 7, 6]).color,
            [9, 8, 7, 6]
        );
    }

    #[test]
    fn sparse_output_mask_routes_map_rows_to_enabled_shader_registers() {
        let map = PicaVertexOutputMap::from_registers_masked(
            registers(&[0x0302_0100, 0x0B0A_0908]),
            0x0011,
            2,
        )
        .unwrap();
        assert_eq!(map.lane_for_semantic(0x00), Some((0, 0)));
        assert_eq!(map.lane_for_semantic(0x08), Some((4, 0)));

        let outputs = outputs_with(&[(0, [1.0, 2.0, 3.0, 1.0]), (4, [0.25, 0.5, 0.75, 1.0])]);
        let resolved = map.resolve(&outputs).unwrap();
        assert_eq!(resolved.position, [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(resolved.color, Some([0.25, 0.5, 0.75, 1.0]));
    }

    #[test]
    fn rows_after_the_guest_output_count_are_ignored() {
        let map = PicaVertexOutputMap::from_registers_masked(
            registers(&[0x0302_0100, 0x0302_0100]),
            0x0001,
            1,
        )
        .unwrap();
        assert_eq!(map.lane_for_semantic(0x00), Some((0, 0)));
    }

    #[test]
    fn invalid_output_count_or_mask_is_reported() {
        let rows = [0x0302_0100; OUTMAP_REGISTER_COUNT];
        assert_eq!(
            PicaVertexOutputMap::from_registers_masked(rows, 0x0001, 2),
            Err(PicaOutputMapError::OutputMaskTooSmall {
                required: 2,
                enabled: 1
            })
        );
        assert_eq!(
            PicaVertexOutputMap::from_registers_masked(rows, 0xFFFF, 8),
            Err(PicaOutputMapError::OutputCountExceedsMap {
                count: 8,
                maximum: OUTMAP_REGISTER_COUNT
            })
        );
    }

    #[test]
    fn output_map_is_read_from_the_live_registers() {
        let mut gpu = Pica200::new();
        assert_eq!(
            gpu.vertex_output_map(),
            Err(PicaOutputMapError::MissingPosition)
        );

        for index in 0..OUTMAP_REGISTER_COUNT as u16 {
            gpu.write_register(REG_SH_OUTMAP_O0 + index, UNUSED_ROW)
                .unwrap();
        }
        gpu.write_register(REG_SH_OUTMAP_O0, 0x0302_0100).unwrap();
        gpu.write_register(REG_SH_OUTMAP_TOTAL, 1).unwrap();
        gpu.write_register(REG_VSH_OUTMAP_MASK, 1).unwrap();
        let map = gpu.vertex_output_map().unwrap();
        assert_eq!(map.lane_for_semantic(0x03), Some((0, 3)));
    }
}
