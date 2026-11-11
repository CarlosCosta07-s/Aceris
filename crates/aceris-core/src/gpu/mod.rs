//! PICA200 graphics hardware model.

mod pica200;
mod physical_memory;
pub use pica200::{
    Pica200, PicaCommandBufferDescriptor, PicaCommandError, PicaCommandListReport,
    REG_ATTRIBUTE_BUFFERS, REG_COLOR_OPERATION, REG_COMMAND_BUFFER_ADDRESS_0,
    REG_COMMAND_BUFFER_ADDRESS_1, REG_COMMAND_BUFFER_JUMP_0, REG_COMMAND_BUFFER_JUMP_1,
    REG_COMMAND_BUFFER_SIZE_0, REG_COMMAND_BUFFER_SIZE_1, REGISTER_COUNT,
};
pub use physical_memory::{
    FCRAM_BASE, FCRAM_SIZE, GpuPhysicalMemory, GpuPhysicalMemoryError, PHYSICAL_PAGE_SIZE,
    VRAM_BASE, VRAM_SIZE,
};
