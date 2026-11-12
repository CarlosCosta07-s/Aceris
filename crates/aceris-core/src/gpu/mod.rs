//! PICA200 graphics hardware model and its software bring-up primitives.

mod geometry;
mod physical_memory;
mod pica200;
mod rasterizer;
mod vertex;

pub use geometry::{
    ClipDepthRange, GeometryError, GeometryPipeline, GeometryReport, GeometryVertex, Matrix4,
    Viewport,
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
pub use rasterizer::{
    DepthFunction, RasterVertex, RasterizationReport, RasterizerError, SoftwareRasterizer,
};
pub use vertex::{
    AttributeBitWidth, AttributeDataType, MAX_COMPONENTS_PER_ARRAY, MAX_COMPONENTS_PER_ATTRIBUTE,
    MAX_VERTEX_ARRAYS, MAX_VERTEX_ATTRIBUTES, PicaVertexInputLayout, RawVertexAttribute,
    RawVertexInput, VertexArrayLayout, VertexAttributeFormat, VertexInputError,
};
