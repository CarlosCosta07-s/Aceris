//! Runs the vertex stage of a captured PICA200 draw request.
//!
//! `Pica200::shade_draw` turns a `PicaDrawRequest` into shaded vertices and
//! triangles: it resolves vertex indices, fetches and converts attributes,
//! binds them to shader registers v0-v15, and runs the reference vertex shader
//! once per distinct vertex.
//!
//! What this stage does not do: it applies no output map (o0-o15 are returned
//! raw, so the caller still decides which register is the position), no
//! clipping, culling or rasterization, and it does not translate the raw
//! GPUREG_ATTRIBBUFFERS_LOC; the caller passes the resolved physical base.
//! Vertex layout and shader-input registers are read from the live register
//! bank, so run this before the command list changes them. Draw-time state
//! (index buffer layout, vertex count, vertex offset, primitive mode) comes
//! from the request, which captured it at the trigger write.
//!
//! Two behaviors rest on public documentation and are not independently
//! verified: GPUREG_VERTEX_OFFSET is a signed offset added to every vertex
//! index (for both array and indexed draws), and attribute components the
//! vertex format does not supply read as (0, 0, 0, 1).

use std::collections::HashMap;

use super::{
    GpuPhysicalMemory, IndexBufferError, NumericVertexAttribute, Pica200, PicaDrawError,
    PicaDrawKind, PicaDrawRequest, PicaShaderExecError, PicaVertexShaderProgram,
    SHADER_INPUT_REGISTERS, SHADER_OUTPUT_REGISTERS, VertexInputError, VertexShaderInputError,
};

/// Value of an attribute component that the vertex format does not supply.
const MISSING_COMPONENT_DEFAULTS: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// One vertex after the vertex shader, with its raw o0-o15 outputs.
#[derive(Clone, Debug, PartialEq)]
pub struct PicaShadedVertex {
    /// Vertex index after the vertex offset was applied.
    pub source_index: u32,
    pub outputs: [[f32; 4]; SHADER_OUTPUT_REGISTERS],
}

/// Result of shading one draw. Triangle entries index into `vertices`.
#[derive(Clone, Debug, PartialEq)]
pub struct PicaShadedDraw {
    pub vertices: Vec<PicaShadedVertex>,
    pub triangles: Vec<[usize; 3]>,
    pub executed_instructions: u64,
}

/// Failure while shading a draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaDrawExecError {
    Draw(PicaDrawError),
    Index(IndexBufferError),
    Vertex(VertexInputError),
    ShaderInput(VertexShaderInputError),
    Shader {
        source_index: u32,
        error: PicaShaderExecError,
    },
    /// Index plus the signed vertex offset left the u32 range.
    VertexIndexOutOfRange {
        index: u32,
        offset: i32,
    },
    /// Fixed attributes that the fixed-attribute registers did not resolve.
    UnresolvedFixedAttributes {
        mask: u16,
    },
    /// Two attributes map to the same shader register; precedence is undocumented.
    DuplicateShaderRegister {
        register: u8,
    },
    /// The attribute-to-register permutation names a register above v15.
    ShaderRegisterOutOfRange {
        register: u8,
    },
}

impl std::fmt::Display for PicaDrawExecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Draw(error) => write!(formatter, "draw preparation failed: {error}"),
            Self::Index(error) => write!(formatter, "index fetch failed: {error}"),
            Self::Vertex(error) => write!(formatter, "vertex fetch failed: {error:?}"),
            Self::ShaderInput(error) => write!(formatter, "shader input setup failed: {error}"),
            Self::Shader {
                source_index,
                error,
            } => write!(
                formatter,
                "shader failed for vertex {source_index}: {error}"
            ),
            Self::VertexIndexOutOfRange { index, offset } => write!(
                formatter,
                "vertex index {index} with offset {offset} is outside the 32-bit range"
            ),
            Self::UnresolvedFixedAttributes { mask } => {
                write!(formatter, "fixed attributes {mask:#06x} have no value")
            }
            Self::DuplicateShaderRegister { register } => write!(
                formatter,
                "more than one attribute targets shader register v{register}"
            ),
            Self::ShaderRegisterOutOfRange { register } => {
                write!(formatter, "shader register v{register} does not exist")
            }
        }
    }
}

impl std::error::Error for PicaDrawExecError {}

impl From<PicaDrawError> for PicaDrawExecError {
    fn from(error: PicaDrawError) -> Self {
        Self::Draw(error)
    }
}

impl From<IndexBufferError> for PicaDrawExecError {
    fn from(error: IndexBufferError) -> Self {
        Self::Index(error)
    }
}

impl From<VertexInputError> for PicaDrawExecError {
    fn from(error: VertexInputError) -> Self {
        Self::Vertex(error)
    }
}

impl From<VertexShaderInputError> for PicaDrawExecError {
    fn from(error: VertexShaderInputError) -> Self {
        Self::ShaderInput(error)
    }
}

/// Expands an attribute to four lanes, filling missing ones with (0, 0, 0, 1).
fn shader_input_vector(attribute: &NumericVertexAttribute) -> [f32; 4] {
    let mut vector = MISSING_COMPONENT_DEFAULTS;
    let supplied = usize::from(attribute.component_count).min(4);
    vector[..supplied].copy_from_slice(&attribute.components[..supplied]);
    vector
}

impl Pica200 {
    /// Shades every distinct vertex referenced by the draw's triangles.
    ///
    /// `resolved_physical_byte_base` is the caller-translated physical address
    /// of GPUREG_ATTRIBBUFFERS_LOC. `program` is a snapshot of the uploaded
    /// shader (see `Pica200::vertex_shader_program`) and `shader_entry` is the
    /// instruction index to start from; the entry-point register is not
    /// decoded, so the caller supplies it.
    pub fn shade_draw(
        &self,
        request: &PicaDrawRequest,
        memory: &GpuPhysicalMemory,
        resolved_physical_byte_base: u32,
        program: &PicaVertexShaderProgram,
        shader_entry: usize,
    ) -> Result<PicaShadedDraw, PicaDrawExecError> {
        let triangles = request.assemble_triangles()?;
        let vertex_count = request.vertex_count as usize;
        let offset = request.raw_vertex_offset as i32;

        let raw_indices: Vec<u32> = match request.kind {
            PicaDrawKind::Arrays => (0..request.vertex_count).collect(),
            PicaDrawKind::Elements => request.index_buffer.read_indices(
                memory,
                resolved_physical_byte_base,
                0,
                vertex_count,
            )?,
        };
        let shaded_indices = raw_indices
            .iter()
            .map(|&index| {
                u32::try_from(i64::from(index) + i64::from(offset))
                    .map_err(|_| PicaDrawExecError::VertexIndexOutOfRange { index, offset })
            })
            .collect::<Result<Vec<u32>, _>>()?;

        let input_layout = self.vertex_shader_input_state()?.layout;

        let mut slots: HashMap<u32, usize> = HashMap::new();
        let mut vertices: Vec<PicaShadedVertex> = Vec::new();
        let mut executed_instructions = 0u64;
        let mut shaded_triangles = Vec::with_capacity(triangles.len());

        for triangle in triangles {
            let mut slot_triple = [0usize; 3];
            for (corner, &position) in triangle.iter().enumerate() {
                let source_index = shaded_indices[position as usize];
                if let Some(&slot) = slots.get(&source_index) {
                    slot_triple[corner] = slot;
                    continue;
                }

                let fetched = self.read_numeric_vertex_at_physical_base(
                    memory,
                    resolved_physical_byte_base,
                    source_index,
                )?;
                let bindings = input_layout.bind_vertex(&fetched);
                if bindings.unresolved_fixed_attribute_mask != 0 {
                    return Err(PicaDrawExecError::UnresolvedFixedAttributes {
                        mask: bindings.unresolved_fixed_attribute_mask,
                    });
                }

                let mut inputs = [[0.0f32; 4]; SHADER_INPUT_REGISTERS];
                let mut bound = [false; SHADER_INPUT_REGISTERS];
                for binding in &bindings.attributes {
                    let register = usize::from(binding.shader_register);
                    if register >= SHADER_INPUT_REGISTERS {
                        return Err(PicaDrawExecError::ShaderRegisterOutOfRange {
                            register: binding.shader_register,
                        });
                    }
                    if bound[register] {
                        return Err(PicaDrawExecError::DuplicateShaderRegister {
                            register: binding.shader_register,
                        });
                    }
                    bound[register] = true;
                    inputs[register] = shader_input_vector(&binding.value);
                }

                let output = program.run(shader_entry, &inputs).map_err(|error| {
                    PicaDrawExecError::Shader {
                        source_index,
                        error,
                    }
                })?;
                executed_instructions += output.executed_instructions as u64;

                let slot = vertices.len();
                vertices.push(PicaShadedVertex {
                    source_index,
                    outputs: output.outputs,
                });
                slots.insert(source_index, slot);
                slot_triple[corner] = slot;
            }
            shaded_triangles.push(slot_triple);
        }

        Ok(PicaShadedDraw {
            vertices,
            triangles: shaded_triangles,
            executed_instructions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{
        FCRAM_BASE, REG_ATTRIBBUFFER0_OFFSET, REG_ATTRIBBUFFERS_FORMAT_HIGH,
        REG_ATTRIBBUFFERS_FORMAT_LOW, REG_ATTRIBBUFFERS_LOC, REG_DRAWARRAYS, REG_DRAWELEMENTS,
        REG_INDEXBUFFER_CONFIG, REG_NUMVERTICES, REG_PRIMITIVE_CONFIG, REG_VERTEX_OFFSET,
        REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, REG_VSH_ATTRIBUTES_PERMUTATION_LOW,
        REG_VSH_CODETRANSFER_DATA0, REG_VSH_CODETRANSFER_INDEX, REG_VSH_INPUTBUFFER_CONFIG,
        REG_VSH_OPDESCS_DATA0, REG_VSH_OPDESCS_INDEX,
    };

    const BASE: u32 = FCRAM_BASE + 0x1000;
    /// MOV o0, v0 (destination 0 and source 0 contribute no bits).
    const MOV_O0_V0: u32 = 0x13 << 26;
    const END: u32 = 0x22 << 26;
    /// Full write mask with an identity swizzle on all three sources.
    const FULL_IDENTITY_DESCRIPTOR: u32 = 0b1111 | (0x1B << 5) | (0x1B << 14) | (0x1B << 23);

    /// GPU with one unsigned-byte attribute in v0 and `MOV o0, v0; END` uploaded.
    fn gpu_with_byte_vertices() -> Pica200 {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x0BAD_F000)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_LOW, 1).unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, 0)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET, 0).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 1, 0).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 2, (1 << 16) | (1 << 28))
            .unwrap();
        gpu.write_register(REG_VSH_INPUTBUFFER_CONFIG, 0).unwrap();
        gpu.write_register(REG_VSH_ATTRIBUTES_PERMUTATION_LOW, 0)
            .unwrap();
        gpu.write_register(REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, 0)
            .unwrap();
        gpu.write_register(REG_VSH_OPDESCS_INDEX, 0).unwrap();
        gpu.write_register(REG_VSH_OPDESCS_DATA0, FULL_IDENTITY_DESCRIPTOR)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0).unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, MOV_O0_V0)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, END).unwrap();
        gpu
    }

    fn memory_with(vertices: &[u8], indices: Option<(u32, &[u8])>) -> GpuPhysicalMemory {
        let mut memory = GpuPhysicalMemory::new();
        memory.write(BASE, vertices).unwrap();
        if let Some((offset, bytes)) = indices {
            memory.write(BASE + offset, bytes).unwrap();
        }
        memory
    }

    fn draw_arrays(gpu: &mut Pica200, count: u32) -> PicaDrawRequest {
        gpu.write_register(REG_NUMVERTICES, count).unwrap();
        gpu.write_register(REG_DRAWARRAYS, 1).unwrap();
        gpu.take_draw_requests()[0]
    }

    fn draw_elements(gpu: &mut Pica200, count: u32, index_config: u32) -> PicaDrawRequest {
        gpu.write_register(REG_INDEXBUFFER_CONFIG, index_config)
            .unwrap();
        gpu.write_register(REG_NUMVERTICES, count).unwrap();
        gpu.write_register(REG_DRAWELEMENTS, 1).unwrap();
        gpu.take_draw_requests()[0]
    }

    fn o0_x(draw: &PicaShadedDraw) -> Vec<f32> {
        draw.vertices
            .iter()
            .map(|vertex| vertex.outputs[0][0])
            .collect()
    }

    #[test]
    fn array_draw_shades_each_vertex_once_and_pads_missing_components() {
        let mut gpu = gpu_with_byte_vertices();
        let request = draw_arrays(&mut gpu, 3);
        let memory = memory_with(&[10, 20, 30], None);
        let program = gpu.vertex_shader_program();

        let draw = gpu
            .shade_draw(&request, &memory, BASE, &program, 0)
            .unwrap();

        assert_eq!(draw.triangles, vec![[0, 1, 2]]);
        assert_eq!(o0_x(&draw), vec![10.0, 20.0, 30.0]);
        // The format supplies only x; y, z, w default to (0, 0, 1).
        assert_eq!(draw.vertices[1].outputs[0], [20.0, 0.0, 0.0, 1.0]);
        assert_eq!(draw.vertices[2].source_index, 2);
        // MOV + END for each of the three vertices.
        assert_eq!(draw.executed_instructions, 6);
    }

    #[test]
    fn vertex_offset_shifts_array_draws() {
        let mut gpu = gpu_with_byte_vertices();
        gpu.write_register(REG_VERTEX_OFFSET, 1).unwrap();
        let request = draw_arrays(&mut gpu, 3);
        let memory = memory_with(&[5, 10, 20, 30], None);
        let program = gpu.vertex_shader_program();

        let draw = gpu
            .shade_draw(&request, &memory, BASE, &program, 0)
            .unwrap();
        assert_eq!(o0_x(&draw), vec![10.0, 20.0, 30.0]);
        assert_eq!(draw.vertices[0].source_index, 1);
    }

    #[test]
    fn indexed_draw_reuses_shaded_vertices_and_keeps_winding() {
        let mut gpu = gpu_with_byte_vertices();
        // u16 indices at +0x10: 0,1,2 then 2,1,0.
        let indices = [0u8, 0, 1, 0, 2, 0, 2, 0, 1, 0, 0, 0];
        let request = draw_elements(&mut gpu, 6, 0x8000_0010);
        let memory = memory_with(&[10, 20, 30], Some((0x10, &indices)));
        let program = gpu.vertex_shader_program();

        let draw = gpu
            .shade_draw(&request, &memory, BASE, &program, 0)
            .unwrap();
        assert_eq!(draw.vertices.len(), 3);
        assert_eq!(draw.triangles, vec![[0, 1, 2], [2, 1, 0]]);
        assert_eq!(draw.executed_instructions, 6);
    }

    #[test]
    fn indexed_draw_applies_a_negative_vertex_offset() {
        let mut gpu = gpu_with_byte_vertices();
        gpu.write_register(REG_VERTEX_OFFSET, 0xFFFF_FFFF).unwrap();
        let indices = [1u8, 0, 2, 0, 3, 0];
        let request = draw_elements(&mut gpu, 3, 0x8000_0010);
        let memory = memory_with(&[10, 20, 30, 40], Some((0x10, &indices)));
        let program = gpu.vertex_shader_program();

        let draw = gpu
            .shade_draw(&request, &memory, BASE, &program, 0)
            .unwrap();
        assert_eq!(o0_x(&draw), vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn vertex_index_below_zero_after_offset_is_rejected() {
        let mut gpu = gpu_with_byte_vertices();
        gpu.write_register(REG_VERTEX_OFFSET, 0xFFFF_FFFF).unwrap();
        let request = draw_arrays(&mut gpu, 3);
        let memory = memory_with(&[10, 20, 30], None);
        let program = gpu.vertex_shader_program();

        assert_eq!(
            gpu.shade_draw(&request, &memory, BASE, &program, 0),
            Err(PicaDrawExecError::VertexIndexOutOfRange {
                index: 0,
                offset: -1
            })
        );
    }

    #[test]
    fn strip_draw_shares_vertices_between_triangles() {
        let mut gpu = gpu_with_byte_vertices();
        gpu.write_register(REG_PRIMITIVE_CONFIG, 0x100).unwrap();
        let request = draw_arrays(&mut gpu, 4);
        let memory = memory_with(&[10, 20, 30, 40], None);
        let program = gpu.vertex_shader_program();

        let draw = gpu
            .shade_draw(&request, &memory, BASE, &program, 0)
            .unwrap();
        assert_eq!(draw.vertices.len(), 4);
        assert_eq!(draw.triangles, vec![[0, 1, 2], [2, 1, 3]]);
    }

    #[test]
    fn draw_uses_the_state_captured_at_the_trigger() {
        let mut gpu = gpu_with_byte_vertices();
        let request = draw_arrays(&mut gpu, 3);
        // Changing the registers after the trigger must not change the draw.
        gpu.write_register(REG_NUMVERTICES, 99).unwrap();
        gpu.write_register(REG_VERTEX_OFFSET, 7).unwrap();
        let memory = memory_with(&[10, 20, 30], None);
        let program = gpu.vertex_shader_program();

        let draw = gpu
            .shade_draw(&request, &memory, BASE, &program, 0)
            .unwrap();
        assert_eq!(o0_x(&draw), vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn incomplete_trailing_triangle_shades_nothing_extra() {
        let mut gpu = gpu_with_byte_vertices();
        let request = draw_arrays(&mut gpu, 4);
        let memory = memory_with(&[10, 20, 30, 40], None);
        let program = gpu.vertex_shader_program();

        let draw = gpu
            .shade_draw(&request, &memory, BASE, &program, 0)
            .unwrap();
        assert_eq!(draw.vertices.len(), 3);
        assert_eq!(draw.triangles.len(), 1);
    }

    #[test]
    fn shader_errors_report_the_failing_vertex() {
        let mut gpu = gpu_with_byte_vertices();
        let request = draw_arrays(&mut gpu, 3);
        let memory = memory_with(&[10, 20, 30], None);
        // Entry 5 was never uploaded.
        let program = gpu.vertex_shader_program();

        assert_eq!(
            gpu.shade_draw(&request, &memory, BASE, &program, 5),
            Err(PicaDrawExecError::Shader {
                source_index: 0,
                error: PicaShaderExecError::InstructionNotUploaded { index: 5 },
            })
        );
    }

    #[test]
    fn geometry_primitives_are_rejected_before_any_fetch() {
        let mut gpu = gpu_with_byte_vertices();
        gpu.write_register(REG_PRIMITIVE_CONFIG, 0x300).unwrap();
        let request = draw_arrays(&mut gpu, 3);
        let program = gpu.vertex_shader_program();

        assert_eq!(
            gpu.shade_draw(&request, &GpuPhysicalMemory::new(), BASE, &program, 0),
            Err(PicaDrawExecError::Draw(
                PicaDrawError::GeometryPrimitiveUnsupported
            ))
        );
    }

    #[test]
    fn missing_vertex_memory_reports_a_fetch_error() {
        let mut gpu = gpu_with_byte_vertices();
        let request = draw_arrays(&mut gpu, 3);
        let program = gpu.vertex_shader_program();

        // 0x1000 is not backed by any region.
        assert!(matches!(
            gpu.shade_draw(&request, &GpuPhysicalMemory::new(), 0x1000, &program, 0),
            Err(PicaDrawExecError::Vertex(_))
        ));
    }

    #[test]
    fn two_attributes_targeting_one_register_are_rejected() {
        let mut gpu = gpu_with_byte_vertices();
        // Two u8 attributes in one array, both permuted onto v0.
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_LOW, 0x11)
            .unwrap();
        // Two active attributes (bits 28..31 hold the count minus one).
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, 1 << 28)
            .unwrap();
        // Component 0 belongs to attribute 0, component 1 to attribute 1.
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 1, 0x10)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 2, (2 << 16) | (2 << 28))
            .unwrap();
        gpu.write_register(REG_VSH_INPUTBUFFER_CONFIG, 1).unwrap();
        let request = draw_arrays(&mut gpu, 3);
        let memory = memory_with(&[1, 2, 3, 4, 5, 6], None);
        let program = gpu.vertex_shader_program();

        assert_eq!(
            gpu.shade_draw(&request, &memory, BASE, &program, 0),
            Err(PicaDrawExecError::DuplicateShaderRegister { register: 0 })
        );
    }

    #[test]
    fn missing_components_default_helper_keeps_supplied_lanes() {
        use crate::gpu::{AttributeBitWidth, AttributeDataType, VertexAttributeFormat};
        let attribute = NumericVertexAttribute {
            format: VertexAttributeFormat {
                data_type: AttributeDataType::Float,
                bit_width: AttributeBitWidth::Bits32,
            },
            components: [7.0, 8.0, 0.0, 0.0],
            component_count: 2,
        };
        assert_eq!(shader_input_vector(&attribute), [7.0, 8.0, 0.0, 1.0]);
    }
}
