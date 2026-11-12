//! PICA200 graphics hardware model and its software bring-up primitives.

mod geometry;
mod physical_memory;
mod pica200;
mod rasterizer;

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
