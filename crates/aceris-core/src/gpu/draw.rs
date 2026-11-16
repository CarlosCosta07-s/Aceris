//! PICA200 draw triggers, primitive configuration and triangle assembly.
//!
//! Writing the DRAWARRAYS or DRAWELEMENTS register starts a draw. This module
//! captures the register state at that moment into a `PicaDrawRequest` so a
//! later pipeline stage can consume it without depending on register values
//! that the command list may overwrite afterwards.
//!
//! Only the documented fields are decoded: the primitive mode (bits 8..9 of
//! GPUREG_PRIMITIVE_CONFIG) and the existing index/count/offset registers. All
//! other bits stay raw. Address translation, vertex-offset application and the
//! actual rendering remain the responsibility of the draw-execution layer.

use super::index::{IndexBufferLayout, REG_INDEXBUFFER_CONFIG, REG_NUMVERTICES, REG_VERTEX_OFFSET};
use super::pica200::REGISTER_COUNT;
use std::fmt;

/// Trigger register for non-indexed draws.
pub const REG_DRAWARRAYS: u16 = 0x22E;
/// Trigger register for indexed draws.
pub const REG_DRAWELEMENTS: u16 = 0x22F;
/// Primitive configuration register; bits 8..9 select the primitive mode.
pub const REG_PRIMITIVE_CONFIG: u16 = 0x25E;
/// Maximum number of draw requests retained until the consumer drains them.
pub const MAX_PENDING_DRAW_REQUESTS: usize = 4096;

/// Primitive topology selected by GPUREG_PRIMITIVE_CONFIG bits 8..9.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaPrimitiveMode {
    Triangles,
    TriangleStrip,
    TriangleFan,
    /// Primitives produced by a geometry shader; not assembled by this module.
    GeometryPrimitive,
}

impl PicaPrimitiveMode {
    /// Decodes the mode from the raw primitive-configuration register.
    pub const fn from_register(value: u32) -> Self {
        match (value >> 8) & 0x3 {
            0 => Self::Triangles,
            1 => Self::TriangleStrip,
            2 => Self::TriangleFan,
            _ => Self::GeometryPrimitive,
        }
    }

    /// Builds triangle index triples for `vertex_count` consecutive vertices.
    ///
    /// Incomplete trailing primitives are dropped. Strip triangles alternate
    /// their first two indices so every triangle keeps the same winding.
    /// Geometry primitives need the geometry shader and are rejected.
    pub fn assemble_triangles(self, vertex_count: u32) -> Result<Vec<[u32; 3]>, PicaDrawError> {
        if vertex_count > MAX_ASSEMBLED_VERTICES {
            return Err(PicaDrawError::TooManyVertices { vertex_count });
        }
        let count = vertex_count;
        let triangles = match self {
            Self::Triangles => (0..count / 3)
                .map(|triangle| [triangle * 3, triangle * 3 + 1, triangle * 3 + 2])
                .collect(),
            Self::TriangleStrip => (0..count.saturating_sub(2))
                .map(|first| {
                    if first % 2 == 0 {
                        [first, first + 1, first + 2]
                    } else {
                        [first + 1, first, first + 2]
                    }
                })
                .collect(),
            Self::TriangleFan => (0..count.saturating_sub(2))
                .map(|second| [0, second + 1, second + 2])
                .collect(),
            Self::GeometryPrimitive => return Err(PicaDrawError::GeometryPrimitiveUnsupported),
        };
        Ok(triangles)
    }
}

/// Defensive upper bound for one assembly call.
pub const MAX_ASSEMBLED_VERTICES: u32 = 1_048_576;

/// Which trigger register started the draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaDrawKind {
    Arrays,
    Elements,
}

/// Register state captured when a draw trigger was written.
///
/// Values are raw register contents except for the decoded primitive mode and
/// index layout. `sequence` increases with every accepted trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaDrawRequest {
    pub kind: PicaDrawKind,
    pub primitive_mode: PicaPrimitiveMode,
    pub raw_primitive_config: u32,
    pub vertex_count: u32,
    pub raw_vertex_offset: u32,
    pub index_buffer: IndexBufferLayout,
    pub trigger_value: u32,
    pub sequence: u64,
}

impl PicaDrawRequest {
    /// Captures the draw-related registers for one accepted trigger.
    pub(super) fn capture(
        kind: PicaDrawKind,
        trigger_value: u32,
        registers: &[u32; REGISTER_COUNT],
        sequence: u64,
    ) -> Self {
        let raw_primitive_config = registers[usize::from(REG_PRIMITIVE_CONFIG)];
        Self {
            kind,
            primitive_mode: PicaPrimitiveMode::from_register(raw_primitive_config),
            raw_primitive_config,
            vertex_count: registers[usize::from(REG_NUMVERTICES)],
            raw_vertex_offset: registers[usize::from(REG_VERTEX_OFFSET)],
            index_buffer: IndexBufferLayout::from_register(
                registers[usize::from(REG_INDEXBUFFER_CONFIG)],
            ),
            trigger_value,
            sequence,
        }
    }

    /// Triangle index triples for this draw, numbered from the first vertex.
    pub fn assemble_triangles(&self) -> Result<Vec<[u32; 3]>, PicaDrawError> {
        self.primitive_mode.assemble_triangles(self.vertex_count)
    }
}

/// Failure while preparing a draw request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaDrawError {
    TooManyVertices { vertex_count: u32 },
    GeometryPrimitiveUnsupported,
}

impl std::fmt::Display for PicaDrawError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyVertices { vertex_count } => write!(
                formatter,
                "PICA200 draw of {vertex_count} vertices exceeds the assembly limit"
            ),
            Self::GeometryPrimitiveUnsupported => {
                write!(formatter, "geometry-shader primitives are not assembled")
            }
        }
    }
}

impl std::error::Error for PicaDrawError {}

/// Failure while executing a command list with a draw observer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaCommandListRunError<E> {
    /// The command list itself is malformed.
    Command(super::pica200::PicaCommandError),
    /// The draw observer rejected a draw and stopped the list.
    Draw(E),
}

impl<E> From<super::pica200::PicaCommandError> for PicaCommandListRunError<E> {
    fn from(error: super::pica200::PicaCommandError) -> Self {
        Self::Command(error)
    }
}

impl<E: fmt::Display> fmt::Display for PicaCommandListRunError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => write!(formatter, "command list failed: {error:?}"),
            Self::Draw(error) => write!(formatter, "draw failed: {error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for PicaCommandListRunError<E> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{IndexElementWidth, Pica200};

    #[test]
    fn primitive_mode_uses_bits_8_and_9_only() {
        assert_eq!(
            PicaPrimitiveMode::from_register(0x0000_0000),
            PicaPrimitiveMode::Triangles
        );
        assert_eq!(
            PicaPrimitiveMode::from_register(0x0000_0100),
            PicaPrimitiveMode::TriangleStrip
        );
        assert_eq!(
            PicaPrimitiveMode::from_register(0x0000_0200),
            PicaPrimitiveMode::TriangleFan
        );
        assert_eq!(
            PicaPrimitiveMode::from_register(0x0000_0300),
            PicaPrimitiveMode::GeometryPrimitive
        );
        // Bits outside 8..9 never change the mode.
        assert_eq!(
            PicaPrimitiveMode::from_register(0xFFFF_FCFF),
            PicaPrimitiveMode::Triangles
        );
    }

    #[test]
    fn triangle_list_drops_incomplete_trailing_primitive() {
        assert_eq!(
            PicaPrimitiveMode::Triangles.assemble_triangles(7).unwrap(),
            vec![[0, 1, 2], [3, 4, 5]]
        );
        assert!(
            PicaPrimitiveMode::Triangles
                .assemble_triangles(2)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn triangle_strip_alternates_winding() {
        assert_eq!(
            PicaPrimitiveMode::TriangleStrip
                .assemble_triangles(5)
                .unwrap(),
            vec![[0, 1, 2], [2, 1, 3], [2, 3, 4]]
        );
        assert!(
            PicaPrimitiveMode::TriangleStrip
                .assemble_triangles(2)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn triangle_fan_shares_the_first_vertex() {
        assert_eq!(
            PicaPrimitiveMode::TriangleFan
                .assemble_triangles(5)
                .unwrap(),
            vec![[0, 1, 2], [0, 2, 3], [0, 3, 4]]
        );
    }

    #[test]
    fn geometry_primitives_and_huge_counts_are_rejected() {
        assert_eq!(
            PicaPrimitiveMode::GeometryPrimitive.assemble_triangles(3),
            Err(PicaDrawError::GeometryPrimitiveUnsupported)
        );
        assert_eq!(
            PicaPrimitiveMode::Triangles.assemble_triangles(MAX_ASSEMBLED_VERTICES + 1),
            Err(PicaDrawError::TooManyVertices {
                vertex_count: MAX_ASSEMBLED_VERTICES + 1
            })
        );
    }

    #[test]
    fn drawarrays_trigger_captures_registers_at_write_time() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_PRIMITIVE_CONFIG, 0x0100).unwrap();
        gpu.write_register(REG_NUMVERTICES, 6).unwrap();
        gpu.write_register(REG_VERTEX_OFFSET, 2).unwrap();
        gpu.write_register(REG_DRAWARRAYS, 1).unwrap();
        // Later register changes must not affect the captured request.
        gpu.write_register(REG_NUMVERTICES, 99).unwrap();

        let requests = gpu.take_draw_requests();
        assert_eq!(requests.len(), 1);
        let request = requests[0];
        assert_eq!(request.kind, PicaDrawKind::Arrays);
        assert_eq!(request.primitive_mode, PicaPrimitiveMode::TriangleStrip);
        assert_eq!(request.vertex_count, 6);
        assert_eq!(request.raw_vertex_offset, 2);
        assert_eq!(request.trigger_value, 1);
        assert_eq!(request.sequence, 0);
        assert_eq!(request.assemble_triangles().unwrap().len(), 4);
        assert!(gpu.take_draw_requests().is_empty());
    }

    #[test]
    fn drawelements_trigger_captures_index_layout() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_INDEXBUFFER_CONFIG, 0x8000_0040)
            .unwrap();
        gpu.write_register(REG_NUMVERTICES, 3).unwrap();
        gpu.write_register(REG_DRAWELEMENTS, 1).unwrap();

        let request = gpu.take_draw_requests()[0];
        assert_eq!(request.kind, PicaDrawKind::Elements);
        assert_eq!(request.primitive_mode, PicaPrimitiveMode::Triangles);
        assert_eq!(request.index_buffer.byte_offset(), 0x40);
        assert_eq!(
            request.index_buffer.element_width(),
            IndexElementWidth::UnsignedShort
        );
    }

    #[test]
    fn trigger_writes_without_bit_zero_do_not_draw() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_NUMVERTICES, 3).unwrap();
        gpu.write_register(REG_DRAWARRAYS, 0).unwrap();
        gpu.write_register(REG_DRAWELEMENTS, 0x2).unwrap();
        assert!(gpu.take_draw_requests().is_empty());
        assert_eq!(gpu.draw_requests_triggered(), 0);
    }

    #[test]
    fn command_list_draws_use_the_register_state_at_each_trigger() {
        // Two draws in one list with a different vertex count between them.
        // Each command is: parameter, header(register | mask 0xF << 16).
        let header = |register: u32| register | (0xF << 16);
        let list = [
            3,
            header(u32::from(REG_NUMVERTICES)),
            1,
            header(u32::from(REG_DRAWARRAYS)),
            6,
            header(u32::from(REG_NUMVERTICES)),
            1,
            header(u32::from(REG_DRAWARRAYS)),
        ];
        let mut gpu = Pica200::new();
        gpu.execute_command_list(&list).unwrap();

        let requests = gpu.take_draw_requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].vertex_count, 3);
        assert_eq!(requests[0].sequence, 0);
        assert_eq!(requests[1].vertex_count, 6);
        assert_eq!(requests[1].sequence, 1);
        assert_eq!(gpu.draw_requests_triggered(), 2);
    }

    #[test]
    fn command_list_trigger_with_empty_byte_mask_does_not_draw() {
        let list = [1, u32::from(REG_DRAWARRAYS)];
        let mut gpu = Pica200::new();
        gpu.execute_command_list(&list).unwrap();
        assert!(gpu.take_draw_requests().is_empty());
    }

    #[test]
    fn pending_draw_queue_is_bounded_and_counts_drops() {
        let mut gpu = Pica200::new();
        for _ in 0..MAX_PENDING_DRAW_REQUESTS + 5 {
            gpu.write_register(REG_DRAWARRAYS, 1).unwrap();
        }
        assert_eq!(gpu.take_draw_requests().len(), MAX_PENDING_DRAW_REQUESTS);
        assert_eq!(gpu.draw_requests_dropped(), 5);
        assert_eq!(
            gpu.draw_requests_triggered(),
            (MAX_PENDING_DRAW_REQUESTS + 5) as u64
        );
    }

    #[test]
    fn reset_clears_draw_state() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_DRAWARRAYS, 1).unwrap();
        gpu.reset();
        assert!(gpu.take_draw_requests().is_empty());
        assert_eq!(gpu.draw_requests_triggered(), 0);
        assert_eq!(gpu.draw_requests_dropped(), 0);
    }
}
