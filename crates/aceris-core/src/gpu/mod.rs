//! PICA200 graphics hardware model and its software bring-up primitives.

mod address_space;
mod depth_buffer;
mod draw;
mod draw_exec;
mod fixed_attribute;
mod fragment;
mod fragment_output;
mod fragment_pipeline;
mod fragment_rasterizer;
mod fragment_render;
mod geometry;
mod gsp_interrupt;
mod gsp_queue;
mod index;
mod input_fetch;
mod output_map;
mod physical_memory;
mod pica200;
mod rasterizer;
mod render;
mod render_target;
mod shader_exec;
mod shader_input;
mod texture;
mod texture_copy;
mod transfer;
mod vertex;
mod vertex_shader;

pub use address_space::{
    GpuAddressSpaceError, GpuDmaAddressSpace, GpuLinearMapping, LINEAR_PAGE_SIZE, NEW_LINEAR_BASE,
    NEW_LINEAR_END, OLD_LINEAR_BASE, OLD_LINEAR_END, ResolvedGpuRange,
};
pub use depth_buffer::{PicaDepthBufferConfig, PicaDepthBufferError, PicaDepthBufferFormat};
pub use draw::{
    MAX_ASSEMBLED_VERTICES, MAX_PENDING_DRAW_REQUESTS, PicaCommandListRunError, PicaDrawError,
    PicaDrawKind, PicaDrawRequest, PicaPrimitiveMode, REG_DRAWARRAYS, REG_DRAWELEMENTS,
    REG_PRIMITIVE_CONFIG,
};
pub use draw_exec::{PicaDrawExecError, PicaShadedDraw, PicaShadedVertex};
pub use fixed_attribute::{
    FIXED_ATTRIBUTE_COUNT, FixedVertexAttribute, Float24, IMMEDIATE_ATTRIBUTE_INDEX,
    REG_FIXEDATTRIB_DATA0, REG_FIXEDATTRIB_DATA1, REG_FIXEDATTRIB_DATA2, REG_FIXEDATTRIB_INDEX,
};
pub use fragment::{
    PICA_TEV_STAGE_COUNT, PicaFragmentInputs, PicaTevAlphaOperand, PicaTevChannel, PicaTevCombine,
    PicaTevError, PicaTevRgbOperand, PicaTevSource, PicaTevStage, PicaTextureEnvironment,
    REG_TEXENV0_COLOR, REG_TEXENV0_COMBINER, REG_TEXENV0_OPERAND, REG_TEXENV0_SCALE,
    REG_TEXENV0_SOURCE, TEXENV_STAGE_REGISTER_STRIDE, TEXENV_STAGE_SOURCE_REGISTERS,
};
pub use fragment_output::{
    PicaAlphaTestFunction, PicaBlendChannel, PicaBlendEquation, PicaBlendFactor,
    PicaColorOperationMode, PicaFragmentOperation, PicaFragmentOutputError,
    PicaFragmentOutputState, PicaLogicOperation, PicaStencilFunction, PicaStencilOperation,
    PicaStencilState, REG_BLEND_COLOR, REG_BLEND_FUNC, REG_FRAGOP_ALPHA_TEST, REG_LOGIC_OP,
    REG_STENCIL_OP, REG_STENCIL_TEST,
};
pub use fragment_pipeline::{
    PicaFragmentDraw, PicaFragmentDrawError, PicaFragmentInterpolationError, PicaFragmentPipeline,
    PicaFragmentPipelineError, PicaFragmentSampleError, PicaFragmentVaryings, PicaFragmentVertex,
    REG_FOG_COLOR, REG_FOG_LINEAR_OFFSET, REG_FOG_LINEAR_SLOPE, REG_TEXENV_UPDATE_BUFFER,
};
pub use fragment_rasterizer::{
    PicaFragmentRasterError, PicaFragmentRasterReport, PicaFragmentRasterState,
    PicaFragmentRasterizer, PicaFragmentVertexDefaults,
};
pub use fragment_render::{PicaFragmentRenderSession, PicaFragmentRenderSessionError};
pub use geometry::{
    ClipDepthRange, GeometryError, GeometryPipeline, GeometryReport, GeometryVertex, Matrix4,
    Viewport,
};
pub use gsp_interrupt::{
    GSP_INTERRUPT_QUEUE_CLIENT_COUNT, GSP_INTERRUPT_QUEUE_CLIENT_STRIDE,
    GSP_INTERRUPT_QUEUE_FULL_ERROR, GSP_INTERRUPT_QUEUE_IGNORE_PDC,
    GSP_INTERRUPT_QUEUE_PDC_THRESHOLD, GSP_INTERRUPT_QUEUE_SIZE, GspInterruptId,
    GspInterruptQueueError, GspInterruptQueuePushResult, GspInterruptRelayQueue,
};
pub use gsp_queue::{
    GSP_GX_COMMAND_DISPLAY_TRANSFER, GSP_GX_COMMAND_ENTRY_SIZE, GSP_GX_COMMAND_FLUSH_CACHE_REGIONS,
    GSP_GX_COMMAND_MEMORY_FILL, GSP_GX_COMMAND_PROCESS_COMMAND_LIST, GSP_GX_COMMAND_QUEUE_CAPACITY,
    GSP_GX_COMMAND_QUEUE_CLIENT_COUNT, GSP_GX_COMMAND_QUEUE_CLIENT_STRIDE,
    GSP_GX_COMMAND_QUEUE_HEADER_SIZE, GSP_GX_COMMAND_QUEUE_OFFSET, GSP_GX_COMMAND_QUEUE_SIZE,
    GSP_GX_COMMAND_REQUEST_DMA, GSP_GX_COMMAND_TEXTURE_COPY, GSP_INVALID_ADDRESS_RESULT,
    GspCacheRegion, GspDisplayTransfer, GspDmaRequest, GspFlushCacheRegions, GspGxCommand,
    GspGxCommandQueue, GspGxCommandQueueError, GspMemoryFill, GspMemoryFillBuffer,
    GspProcessCommandList, GspTextureCopy, MAX_GSP_DMA_BYTES,
};
pub use index::{
    IndexBufferError, IndexBufferLayout, IndexElementWidth, MAX_INDICES_PER_READ,
    PicaIndexInputState, REG_INDEXBUFFER_CONFIG, REG_NUMVERTICES, REG_VERTEX_OFFSET,
};
pub use output_map::{
    OUTMAP_REGISTER_COUNT, PicaOutputGroup, PicaOutputMapError, PicaResolvedVertex,
    PicaVertexOutputMap, REG_SH_OUTMAP_O0, REG_VSH_ENTRYPOINT,
};
pub use physical_memory::{
    FCRAM_BASE, FCRAM_SIZE, GpuPhysicalMemory, GpuPhysicalMemoryError, PHYSICAL_PAGE_SIZE,
    VRAM_BASE, VRAM_SIZE,
};
pub use pica200::{
    FOG_LUT_ENTRY_COUNT, GAS_LUT_ENTRY_COUNT, Pica200, PicaCommandBufferDescriptor,
    PicaCommandError, PicaCommandListReport, PicaCullMode, PicaDepthBuffering, PicaDepthState,
    PicaDepthTestState, PicaScissorMode, PicaScissorState, PicaViewportState,
    REG_ATTRIBUTE_BUFFERS, REG_COLOR_OPERATION, REG_COLORBUFFER_WRITE,
    REG_COMMAND_BUFFER_ADDRESS_0, REG_COMMAND_BUFFER_ADDRESS_1, REG_COMMAND_BUFFER_JUMP_0,
    REG_COMMAND_BUFFER_JUMP_1, REG_COMMAND_BUFFER_SIZE_0, REG_COMMAND_BUFFER_SIZE_1,
    REG_DEPTH_COLOR_MASK, REG_DEPTHBUFFER_FORMAT, REG_DEPTHBUFFER_LOC, REG_DEPTHBUFFER_READ,
    REG_DEPTHBUFFER_WRITE, REG_DEPTHMAP_ENABLE, REG_DEPTHMAP_OFFSET, REG_DEPTHMAP_SCALE,
    REG_FACECULLING_CONFIG, REG_FOG_LUT_DATA0, REG_FOG_LUT_INDEX, REG_GAS_LUT_DATA,
    REG_GAS_LUT_INDEX, REG_SCISSORTEST_DIM, REG_SCISSORTEST_MODE, REG_SCISSORTEST_POS,
    REG_VIEWPORT_HEIGHT, REG_VIEWPORT_INV_HEIGHT, REG_VIEWPORT_INV_WIDTH, REG_VIEWPORT_WIDTH,
    REG_VIEWPORT_XY, REG_VSH_CODETRANSFER_DATA0, REG_VSH_CODETRANSFER_END,
    REG_VSH_CODETRANSFER_INDEX, REG_VSH_FLOATUNIFORM_DATA0, REG_VSH_FLOATUNIFORM_INDEX,
    REG_VSH_INTUNIFORM_I0, REG_VSH_OPDESCS_DATA0, REG_VSH_OPDESCS_INDEX, REGISTER_COUNT,
    VSH_CODE_TRANSFER_WORDS, VSH_FLOAT_UNIFORM_COUNT, VSH_OPDESC_TRANSFER_WORDS,
};
pub use rasterizer::{
    DepthFunction, PicaDepthMap, PicaDepthMode, RasterVertex, RasterizationReport, RasterizerError,
    SoftwareRasterizer,
};
pub use render::{
    PicaRenderError, PicaRenderReport, PicaRenderSession, PicaRenderSessionError,
    PicaRenderSettings, PicaRenderingRunError,
};
pub use render_target::{
    PicaColorBufferFormat, PicaRenderBlockMode, PicaRenderTargetConfig, PicaRenderTargetError,
    PicaRenderTargetWriteReport, REG_COLORBUFFER_FORMAT, REG_COLORBUFFER_LOC,
    REG_FRAMEBUFFER_BLOCK32, REG_FRAMEBUFFER_DIM,
};
pub use shader_exec::{
    MAX_SHADER_EXECUTION_INSTRUCTIONS, PicaShaderExecError, PicaShaderOpcode,
    PicaVertexShaderOutput, PicaVertexShaderProgram, SHADER_INPUT_REGISTERS,
    SHADER_OUTPUT_REGISTERS, SHADER_TEMPORARY_REGISTERS,
};
pub use shader_input::{
    MAX_VERTEX_SHADER_INPUTS, MAX_VERTEX_SHADER_REGISTERS, PicaVertexShaderInputLayout,
    PicaVertexShaderInputState, REG_VSH_ATTRIBUTES_PERMUTATION_HIGH,
    REG_VSH_ATTRIBUTES_PERMUTATION_LOW, REG_VSH_INPUTBUFFER_CONFIG, VertexShaderAttributeBinding,
    VertexShaderInputBindings, VertexShaderInputError,
};
pub use texture::{
    PICA_TEXTURE_UNIT_COUNT, PicaTextureAxis, PicaTextureError, PicaTextureFilter,
    PicaTextureFormat, PicaTextureUnit, PicaTextureWrap, REG_TEXUNIT_CONFIG,
};
pub use texture_copy::{
    MAX_TEXTURE_COPY_BYTES, TextureCopyError, TextureCopyReport, TextureCopyRequest,
    execute_texture_copy,
};
pub use transfer::{
    DISPLAY_TRANSFER_BLOCKS_32X32, DISPLAY_TRANSFER_CROP, DISPLAY_TRANSFER_FLIP_VERTICAL,
    DISPLAY_TRANSFER_LINEAR_TO_TILED, DISPLAY_TRANSFER_TEXTURE_COPY,
    DISPLAY_TRANSFER_TILED_TO_TILED, DisplayTransferError, DisplayTransferFormat,
    DisplayTransferLayout, DisplayTransferReport, DisplayTransferRequest,
    MAX_DISPLAY_TRANSFER_BYTES, display_transfer_buffer_sizes, execute_display_transfer,
};
pub use vertex::{
    AttributeBitWidth, AttributeDataType, MAX_COMPONENTS_PER_ARRAY, MAX_COMPONENTS_PER_ATTRIBUTE,
    MAX_VERTEX_ARRAYS, MAX_VERTEX_ATTRIBUTES, NumericVertexAttribute, NumericVertexInput,
    PicaVertexInputLayout, PicaVertexInputState, REG_ATTRIBBUFFER0_OFFSET,
    REG_ATTRIBBUFFERS_FORMAT_HIGH, REG_ATTRIBBUFFERS_FORMAT_LOW, REG_ATTRIBBUFFERS_LOC,
    RawVertexAttribute, RawVertexInput, VertexArrayLayout, VertexAttributeFormat, VertexInputError,
};
pub use vertex_shader::{
    PicaShaderFormat1, PicaShaderFormat1Compare, PicaShaderFormat1Inverted, PicaShaderFormat1Unary,
    PicaShaderFormat2Flow, PicaShaderFormat3BooleanFlow, PicaShaderFormat4Emit,
    PicaShaderFormat5InvertedMultiplyAdd, PicaShaderFormat5MultiplyAdd,
    PicaShaderOperandDescriptor, PicaVertexShaderInstruction, PicaVertexShaderInstructionFormat,
};
