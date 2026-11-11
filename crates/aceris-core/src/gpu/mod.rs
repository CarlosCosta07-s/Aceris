//! PICA200 graphics hardware model.

mod pica200;
pub use pica200::{
    Pica200, PicaCommandError, PicaCommandListReport, REG_ATTRIBUTE_BUFFERS,
    REG_COLOR_OPERATION, REG_COMMAND_BUFFER_ADDRESS_0, REG_COMMAND_BUFFER_JUMP_0,
    REG_COMMAND_BUFFER_SIZE_0, REGISTER_COUNT,
};
