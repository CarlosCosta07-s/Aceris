//! PICA200 graphics hardware model and its software bring-up primitives.

mod geometry;
mod index;
mod input_fetch;
mod physical_memory;
mod pica200;
mod rasterizer;
mod shader_input;
mod vertex;

pub use geometry::{
    ClipDepthRange, GeometryError, GeometryPipeline, GeometryReport, GeometryVertex, Matrix4,
    Viewport,
};
pub use index::{
    IndexBufferError, IndexBufferLayout, IndexElementWidth, MAX_INDICES_PER_READ,
    PicaIndexInputState, REG_INDEXBUFFER_CONFIG, REG_NUMVERTICES, REG_VERTEX_OFFSET,
};
pub use physical_memory::{
    FCRAM_BASE, FCRAM_SIZE, GpuPhysicalMemory, GpuPhysicalMemoryError, PHYSICAL_PAGE_SIZE,
    VRAM_BASE, VRAM_SIZE,
};
pub use pica200::{
    Pica200, PicaCommandBufferDescriptor, PicaCommandError, PicaCommandListReport,
    REG_ATTRIBUTE_BUFFERS, REG_COLOR_OPERATION, REG_COMMAND_BUFFER_ADDRESS_0,
    REG_COMMAND_BUFFER_ADDRESS_1, REG_COMMAND_BUFFER_JUMP_0, REG_COMMAND_BUFFER_JUMP_1,
    REG_COMMAND_BUFFER_SIZE_0, REG_COMMAND_BUFFER_SIZE_1, REGISTER_COUNT,
};
pub use shader_input::{
    PicaVertexShaderInputLayout, PicaVertexShaderInputState, VertexShaderAttributeBinding,
    VertexShaderInputBindings, VertexShaderInputError, MAX_VERTEX_SHADER_INPUTS,
    MAX_VERTEX_SHADER_REGISTERS, REG_VSH_ATTRIBUTES_PERMUTATION_HIGH,
    REG_VSH_ATTRIBUTES_PERMUTATION_LOW, REG_VSH_INPUTBUFFER_CONFIG,
};
pub use rasterizer::{
    DepthFunction, RasterVertex, RasterizationReport, RasterizerError, SoftwareRasterizer,
};
pub use vertex::{
    AttributeDataType, MAX_COMPONENTS_PER_ARRAY, MAX_COMPONENTS_PER_ATTRIBUTE, MAX_VERTEX_ARRAYS,
    MAX_VERTEX_ATTRIBUTES, NumericVertexAttribute, NumericVertexInput, PicaVertexInputLayout,
    PicaVertexInputState, REG_ATTRIBBUFFER0_OFFSET, REG_ATTRIBBUFFERS_FORMAT_HIGH,
    REG_ATTRIBBUFFERS_FORMAT_LOW, REG_ATTRIBBUFFERS_LOC, RawVertexAttribute, RawVertexInput,
    VertexArrayLayout, VertexAttributeFormat, VertexInputError,
};
