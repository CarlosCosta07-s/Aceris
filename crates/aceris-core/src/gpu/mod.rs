//! PICA200 graphics hardware model and its software bring-up primitives.

mod address_space;
mod fixed_attribute;
mod geometry;
mod gsp_queue;
mod index;
mod input_fetch;
mod physical_memory;
mod pica200;
mod rasterizer;
mod shader_input;
mod vertex;
mod vertex_shader;

pub use address_space::{
    GpuAddressSpaceError, GpuDmaAddressSpace, GpuLinearMapping, LINEAR_PAGE_SIZE, NEW_LINEAR_BASE,
    NEW_LINEAR_END, OLD_LINEAR_BASE, OLD_LINEAR_END, ResolvedGpuRange,
};
pub use fixed_attribute::{
    FIXED_ATTRIBUTE_COUNT, FixedVertexAttribute, Float24, IMMEDIATE_ATTRIBUTE_INDEX,
    REG_FIXEDATTRIB_DATA0, REG_FIXEDATTRIB_DATA1, REG_FIXEDATTRIB_DATA2, REG_FIXEDATTRIB_INDEX,
};
pub use geometry::{
    ClipDepthRange, GeometryError, GeometryPipeline, GeometryReport, GeometryVertex, Matrix4,
    Viewport,
};
pub use gsp_queue::{
    GSP_GX_COMMAND_ENTRY_SIZE, GSP_GX_COMMAND_PROCESS_COMMAND_LIST, GSP_GX_COMMAND_QUEUE_CAPACITY,
    GSP_GX_COMMAND_QUEUE_CLIENT_COUNT, GSP_GX_COMMAND_QUEUE_CLIENT_STRIDE,
    GSP_GX_COMMAND_QUEUE_HEADER_SIZE, GSP_GX_COMMAND_QUEUE_OFFSET, GSP_GX_COMMAND_QUEUE_SIZE,
    GSP_INVALID_ADDRESS_RESULT, GspGxCommand, GspGxCommandQueue, GspGxCommandQueueError,
    GspProcessCommandList,
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
    REG_COMMAND_BUFFER_SIZE_0, REG_COMMAND_BUFFER_SIZE_1, REG_VSH_CODETRANSFER_DATA0,
    REG_VSH_CODETRANSFER_END, REG_VSH_CODETRANSFER_INDEX, REG_VSH_FLOATUNIFORM_DATA0,
    REG_VSH_FLOATUNIFORM_INDEX, REG_VSH_OPDESCS_DATA0, REG_VSH_OPDESCS_INDEX, REGISTER_COUNT,
    VSH_CODE_TRANSFER_WORDS, VSH_FLOAT_UNIFORM_COUNT, VSH_OPDESC_TRANSFER_WORDS,
};
pub use rasterizer::{
    DepthFunction, RasterVertex, RasterizationReport, RasterizerError, SoftwareRasterizer,
};
pub use shader_input::{
    MAX_VERTEX_SHADER_INPUTS, MAX_VERTEX_SHADER_REGISTERS, PicaVertexShaderInputLayout,
    PicaVertexShaderInputState, REG_VSH_ATTRIBUTES_PERMUTATION_HIGH,
    REG_VSH_ATTRIBUTES_PERMUTATION_LOW, REG_VSH_INPUTBUFFER_CONFIG, VertexShaderAttributeBinding,
    VertexShaderInputBindings, VertexShaderInputError,
};
pub use vertex_shader::{
    PicaShaderFormat1, PicaShaderFormat1Compare, PicaShaderFormat1Inverted, PicaShaderFormat1Unary,
    PicaShaderFormat2Flow, PicaShaderFormat3BooleanFlow, PicaShaderFormat4Emit,
    PicaShaderFormat5InvertedMultiplyAdd, PicaShaderFormat5MultiplyAdd,
    PicaVertexShaderInstruction, PicaVertexShaderInstructionFormat,
};
pub use vertex::{
    AttributeBitWidth, AttributeDataType, MAX_COMPONENTS_PER_ARRAY, MAX_COMPONENTS_PER_ATTRIBUTE,
    MAX_VERTEX_ARRAYS, MAX_VERTEX_ATTRIBUTES, NumericVertexAttribute, NumericVertexInput,
    PicaVertexInputLayout, PicaVertexInputState, REG_ATTRIBBUFFER0_OFFSET,
    REG_ATTRIBBUFFERS_FORMAT_HIGH, REG_ATTRIBBUFFERS_FORMAT_LOW, REG_ATTRIBBUFFERS_LOC,
    RawVertexAttribute, RawVertexInput, VertexArrayLayout, VertexAttributeFormat, VertexInputError,
};
