//! Physical framebuffer scanout through the Nintendo 3DS display controllers.

use crate::gpu::{GpuPhysicalMemory, GpuPhysicalMemoryError};
use crate::video::{
    BOTTOM_SCREEN_HEIGHT, BOTTOM_SCREEN_WIDTH, DualScreenDisplay, RGBA8_BYTES_PER_PIXEL,
    ScreenSurfaceError, ScreenTarget, TOP_SCREEN_HEIGHT, TOP_SCREEN_WIDTH,
};
use std::fmt;

/// Top or bottom physical display controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LcdPanel {
    Top,
    Bottom,
}

/// PDC framebuffer pixel format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LcdPixelFormat {
    Rgba8 = 0,
    Rgb8 = 1,
    Rgb565 = 2,
    Rgb5A1 = 3,
    Rgba4 = 4,
}

impl LcdPixelFormat {
    fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgb8 => 3,
            Self::Rgb565 | Self::Rgb5A1 | Self::Rgba4 => 2,
        }
    }

    fn from_register(value: u32) -> Result<Self, LcdControllerError> {
        match value & 0x7 {
            0 => Ok(Self::Rgba8),
            1 => Ok(Self::Rgb8),
            2 => Ok(Self::Rgb565),
            3 => Ok(Self::Rgb5A1),
            4 => Ok(Self::Rgba4),
            format => Err(LcdControllerError::UnsupportedPixelFormat(format as u8)),
        }
    }
}

/// LCD controller register offsets within one 0x100-byte PDC window.
pub const PDC_FRAMEBUFFER_A_FIRST: u32 = 0x68;
pub const PDC_FRAMEBUFFER_A_SECOND: u32 = 0x6C;
pub const PDC_FRAMEBUFFER_FORMAT: u32 = 0x70;
pub const PDC_CONTROL: u32 = 0x74;
pub const PDC_FRAMEBUFFER_SELECT: u32 = 0x78;
pub const PDC_FRAMEBUFFER_STRIDE: u32 = 0x90;
pub const PDC_FRAMEBUFFER_B_FIRST: u32 = 0x94;
pub const PDC_FRAMEBUFFER_B_SECOND: u32 = 0x98;

const PDC_ENABLE: u32 = 1;
const PDC_OUTPUT_ENABLE: u32 = 1 << 16;
const PDC_NEXT_BUFFER: u32 = 1;
const PDC_CURRENT_BUFFER: u32 = 1 << 4;

/// Scanout validation or memory-conversion error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LcdControllerError {
    InvalidRegister {
        panel: LcdPanel,
        offset: u32,
    },
    UnsupportedPixelFormat(u8),
    UnsupportedInterlaceMode(u8),
    InvalidStride {
        panel: LcdPanel,
        stride: u32,
        row_bytes: usize,
    },
    AddressOverflow {
        address: u32,
        offset: u64,
    },
    AllocationFailed,
    PhysicalMemory(GpuPhysicalMemoryError),
    Surface(ScreenSurfaceError),
}

impl fmt::Display for LcdControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRegister { panel, offset } => {
                write!(
                    formatter,
                    "invalid {panel:?} PDC register offset {offset:#x}"
                )
            }
            Self::UnsupportedPixelFormat(format) => {
                write!(formatter, "unsupported PDC framebuffer format {format}")
            }
            Self::UnsupportedInterlaceMode(mode) => {
                write!(formatter, "unsupported PDC interlace mode {mode}")
            }
            Self::InvalidStride {
                panel,
                stride,
                row_bytes,
            } => write!(
                formatter,
                "invalid {panel:?} PDC stride {stride:#x}; row requires {row_bytes} bytes and stride must be 8-byte aligned"
            ),
            Self::AddressOverflow { address, offset } => write!(
                formatter,
                "PDC framebuffer address {address:#010x} + {offset:#x} overflows"
            ),
            Self::AllocationFailed => formatter.write_str("could not allocate LCD scanout buffer"),
            Self::PhysicalMemory(error) => {
                write!(formatter, "LCD physical-memory access failed: {error}")
            }
            Self::Surface(error) => write!(formatter, "LCD output surface update failed: {error}"),
        }
    }
}

impl std::error::Error for LcdControllerError {}

impl From<GpuPhysicalMemoryError> for LcdControllerError {
    fn from(error: GpuPhysicalMemoryError) -> Self {
        Self::PhysicalMemory(error)
    }
}

impl From<ScreenSurfaceError> for LcdControllerError {
    fn from(error: ScreenSurfaceError) -> Self {
        Self::Surface(error)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PanelRegisters {
    framebuffer_a: [u32; 2],
    framebuffer_b: [u32; 2],
    format: u32,
    control: u32,
    stride: u32,
    current_buffer: u8,
    next_buffer: u8,
}

impl PanelRegisters {
    fn write(
        &mut self,
        panel: LcdPanel,
        offset: u32,
        value: u32,
    ) -> Result<(), LcdControllerError> {
        match offset {
            PDC_FRAMEBUFFER_A_FIRST => self.framebuffer_a[0] = value,
            PDC_FRAMEBUFFER_A_SECOND => self.framebuffer_a[1] = value,
            PDC_FRAMEBUFFER_B_FIRST => self.framebuffer_b[0] = value,
            PDC_FRAMEBUFFER_B_SECOND => self.framebuffer_b[1] = value,
            PDC_FRAMEBUFFER_FORMAT => self.format = value,
            PDC_CONTROL => self.control = value,
            PDC_FRAMEBUFFER_SELECT => self.next_buffer = (value & PDC_NEXT_BUFFER) as u8,
            PDC_FRAMEBUFFER_STRIDE => self.stride = value,
            _ => return Err(LcdControllerError::InvalidRegister { panel, offset }),
        }
        Ok(())
    }

    fn read(&self, panel: LcdPanel, offset: u32) -> Result<u32, LcdControllerError> {
        match offset {
            PDC_FRAMEBUFFER_A_FIRST => Ok(self.framebuffer_a[0]),
            PDC_FRAMEBUFFER_A_SECOND => Ok(self.framebuffer_a[1]),
            PDC_FRAMEBUFFER_B_FIRST => Ok(self.framebuffer_b[0]),
            PDC_FRAMEBUFFER_B_SECOND => Ok(self.framebuffer_b[1]),
            PDC_FRAMEBUFFER_FORMAT => Ok(self.format),
            PDC_CONTROL => Ok(self.control),
            PDC_FRAMEBUFFER_SELECT => Ok((u32::from(self.next_buffer) & PDC_NEXT_BUFFER)
                | (u32::from(self.current_buffer) << 4)),
            PDC_FRAMEBUFFER_STRIDE => Ok(self.stride),
            _ => Err(LcdControllerError::InvalidRegister { panel, offset }),
        }
    }

    fn advance_vblank(&mut self) {
        self.current_buffer = self.next_buffer;
    }

    fn enabled(&self) -> bool {
        self.control & (PDC_ENABLE | PDC_OUTPUT_ENABLE) == (PDC_ENABLE | PDC_OUTPUT_ENABLE)
    }
}

/// Number of LCD surfaces refreshed by one scanout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LcdScanoutReport {
    pub screens_updated: u8,
}

/// Original 3DS top and bottom display controller register state.
///
/// The first implementation supports progressive mode A, the documented
/// RGBA8/RGB8/RGB565/RGB5A1/RGBA4 formats, and positive row strides. AA/AB/BA
/// interlace modes and negative-stride vertical flips are reported as
/// unsupported rather than silently rendered incorrectly.
#[derive(Clone, Debug, Default)]
pub struct LcdController {
    top: PanelRegisters,
    bottom: PanelRegisters,
}

impl LcdController {
    pub fn new() -> Self {
        Self::default()
    }

    fn panel(&self, panel: LcdPanel) -> &PanelRegisters {
        match panel {
            LcdPanel::Top => &self.top,
            LcdPanel::Bottom => &self.bottom,
        }
    }

    fn panel_mut(&mut self, panel: LcdPanel) -> &mut PanelRegisters {
        match panel {
            LcdPanel::Top => &mut self.top,
            LcdPanel::Bottom => &mut self.bottom,
        }
    }

    /// Writes one PDC register offset for the selected display controller.
    pub fn write_register(
        &mut self,
        panel: LcdPanel,
        offset: u32,
        value: u32,
    ) -> Result<(), LcdControllerError> {
        self.panel_mut(panel).write(panel, offset, value)
    }

    /// Reads one PDC register offset, including the current/next buffer status.
    pub fn read_register(&self, panel: LcdPanel, offset: u32) -> Result<u32, LcdControllerError> {
        self.panel(panel).read(panel, offset)
    }

    /// Applies the next-buffer selection at the next VBlank boundary.
    pub fn advance_vblank(&mut self) {
        self.top.advance_vblank();
        self.bottom.advance_vblank();
    }

    /// Reads enabled physical framebuffers and atomically updates the LCD surfaces.
    ///
    /// The entire source image for each enabled panel is converted before any
    /// display surface is changed. A bad right-eye or bottom framebuffer
    /// therefore cannot leave only the left eye updated for the same frame.
    pub fn scanout(
        &self,
        memory: &GpuPhysicalMemory,
        display: &mut DualScreenDisplay,
    ) -> Result<LcdScanoutReport, LcdControllerError> {
        let mut pending = Vec::with_capacity(3);

        if self.top.enabled() {
            let top_format = self.top.pixel_format()?;
            let top_left = self.top.framebuffer_a[usize::from(self.top.current_buffer)];
            let top_right = self.top.framebuffer_b[usize::from(self.top.current_buffer)];
            pending.push((
                ScreenTarget::TopLeft,
                decode_framebuffer(
                    memory,
                    top_left,
                    TOP_SCREEN_WIDTH,
                    TOP_SCREEN_HEIGHT,
                    self.top.stride,
                    top_format,
                    LcdPanel::Top,
                )?,
            ));
            pending.push((
                ScreenTarget::TopRight,
                decode_framebuffer(
                    memory,
                    top_right,
                    TOP_SCREEN_WIDTH,
                    TOP_SCREEN_HEIGHT,
                    self.top.stride,
                    top_format,
                    LcdPanel::Top,
                )?,
            ));
        }

        if self.bottom.enabled() {
            let bottom_format = self.bottom.pixel_format()?;
            let bottom_address = self.bottom.framebuffer_a[usize::from(self.bottom.current_buffer)];
            pending.push((
                ScreenTarget::Bottom,
                decode_framebuffer(
                    memory,
                    bottom_address,
                    BOTTOM_SCREEN_WIDTH,
                    BOTTOM_SCREEN_HEIGHT,
                    self.bottom.stride,
                    bottom_format,
                    LcdPanel::Bottom,
                )?,
            ));
        }

        for (target, pixels) in pending.iter() {
            display.surface_mut(*target).copy_from_rgba8(pixels)?;
        }

        Ok(LcdScanoutReport {
            screens_updated: pending.len() as u8,
        })
    }
}

impl PanelRegisters {
    fn pixel_format(&self) -> Result<LcdPixelFormat, LcdControllerError> {
        let interlace = ((self.format >> 4) & 0x3) as u8;
        if interlace != 0 {
            return Err(LcdControllerError::UnsupportedInterlaceMode(interlace));
        }
        LcdPixelFormat::from_register(self.format)
    }
}

fn decode_framebuffer(
    memory: &GpuPhysicalMemory,
    base_address: u32,
    width: usize,
    height: usize,
    stride: u32,
    format: LcdPixelFormat,
    panel: LcdPanel,
) -> Result<Vec<u8>, LcdControllerError> {
    let row_bytes = width
        .checked_mul(format.bytes_per_pixel())
        .ok_or(LcdControllerError::AllocationFailed)?;
    let stride_bytes = stride as usize;
    if stride != 0 && (stride & 0x7 != 0 || stride_bytes < row_bytes || stride & 0x8000_0000 != 0) {
        return Err(LcdControllerError::InvalidStride {
            panel,
            stride,
            row_bytes,
        });
    }

    let last_row_offset = if stride == 0 {
        0
    } else {
        (height - 1)
            .checked_mul(stride_bytes)
            .ok_or(LcdControllerError::AllocationFailed)?
    };
    let source_length = last_row_offset
        .checked_add(row_bytes)
        .ok_or(LcdControllerError::AllocationFailed)?;
    memory.validate_range(base_address, source_length)?;

    let output_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(RGBA8_BYTES_PER_PIXEL))
        .ok_or(LcdControllerError::AllocationFailed)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_len)
        .map_err(|_| LcdControllerError::AllocationFailed)?;
    output.resize(output_len, 0);

    let mut source_row = Vec::new();
    source_row
        .try_reserve_exact(row_bytes)
        .map_err(|_| LcdControllerError::AllocationFailed)?;
    source_row.resize(row_bytes, 0);

    for y in 0..height {
        let row_offset = if stride == 0 { 0 } else { y * stride_bytes };
        let row_address = u64::from(base_address) + row_offset as u64;
        let row_address =
            u32::try_from(row_address).map_err(|_| LcdControllerError::AddressOverflow {
                address: base_address,
                offset: row_offset as u64,
            })?;
        memory.read(row_address, &mut source_row)?;
        decode_row(format, &source_row, &mut output, y * width * 4)?;
    }

    Ok(output)
}

fn decode_row(
    format: LcdPixelFormat,
    input: &[u8],
    output: &mut [u8],
    output_offset: usize,
) -> Result<(), LcdControllerError> {
    let bytes_per_pixel = format.bytes_per_pixel();
    let pixel_count = input.len() / bytes_per_pixel;

    for pixel_index in 0..pixel_count {
        let source = pixel_index * bytes_per_pixel;
        let destination = output_offset + pixel_index * 4;
        let rgba = match format {
            LcdPixelFormat::Rgba8 => [
                input[source + 3],
                input[source + 2],
                input[source + 1],
                input[source],
            ],
            LcdPixelFormat::Rgb8 => [input[source + 2], input[source + 1], input[source], 0xFF],
            LcdPixelFormat::Rgb565 => {
                let value = u16::from_le_bytes([input[source], input[source + 1]]);
                [
                    expand_5((value >> 11) & 0x1F),
                    expand_6((value >> 5) & 0x3F),
                    expand_5(value & 0x1F),
                    0xFF,
                ]
            }
            LcdPixelFormat::Rgb5A1 => {
                let value = u16::from_le_bytes([input[source], input[source + 1]]);
                [
                    expand_5((value >> 11) & 0x1F),
                    expand_5((value >> 6) & 0x1F),
                    expand_5((value >> 1) & 0x1F),
                    if value & 1 != 0 { 0xFF } else { 0 },
                ]
            }
            LcdPixelFormat::Rgba4 => {
                let value = u16::from_le_bytes([input[source], input[source + 1]]);
                [
                    expand_4((value >> 12) & 0xF),
                    expand_4((value >> 8) & 0xF),
                    expand_4((value >> 4) & 0xF),
                    expand_4(value & 0xF),
                ]
            }
        };
        output[destination..destination + 4].copy_from_slice(&rgba);
    }
    Ok(())
}

fn expand_4(value: u16) -> u8 {
    (value * 17) as u8
}

fn expand_5(value: u16) -> u8 {
    ((u32::from(value) * 255 + 15) / 31) as u8
}

fn expand_6(value: u16) -> u8 {
    ((u32::from(value) * 255 + 31) / 63) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::FCRAM_BASE;

    const ENABLED: u32 = PDC_ENABLE | PDC_OUTPUT_ENABLE;

    fn configure_panel(
        controller: &mut LcdController,
        panel: LcdPanel,
        address_a: u32,
        address_b: u32,
        format: LcdPixelFormat,
        stride: u32,
    ) {
        controller
            .write_register(panel, PDC_FRAMEBUFFER_A_FIRST, address_a)
            .unwrap();
        controller
            .write_register(panel, PDC_FRAMEBUFFER_A_SECOND, address_a)
            .unwrap();
        controller
            .write_register(panel, PDC_FRAMEBUFFER_B_FIRST, address_b)
            .unwrap();
        controller
            .write_register(panel, PDC_FRAMEBUFFER_B_SECOND, address_b)
            .unwrap();
        controller
            .write_register(panel, PDC_FRAMEBUFFER_FORMAT, format as u32)
            .unwrap();
        controller
            .write_register(panel, PDC_FRAMEBUFFER_STRIDE, stride)
            .unwrap();
        controller
            .write_register(panel, PDC_CONTROL, ENABLED)
            .unwrap();
    }

    #[test]
    fn rgba8_physical_framebuffers_scan_out_to_stereo_top_and_bottom_surfaces() {
        let mut memory = GpuPhysicalMemory::new();
        let left = FCRAM_BASE;
        let right = FCRAM_BASE + 0x1000;
        let bottom = FCRAM_BASE + 0x2000;
        memory.write(left, &[0x44, 0x33, 0x22, 0x11]).unwrap();
        memory.write(right, &[0x88, 0x77, 0x66, 0x55]).unwrap();
        memory.write(bottom, &[0xCC, 0xBB, 0xAA, 0x99]).unwrap();

        let mut controller = LcdController::new();
        configure_panel(
            &mut controller,
            LcdPanel::Top,
            left,
            right,
            LcdPixelFormat::Rgba8,
            (TOP_SCREEN_WIDTH * 4) as u32,
        );
        configure_panel(
            &mut controller,
            LcdPanel::Bottom,
            bottom,
            0,
            LcdPixelFormat::Rgba8,
            (BOTTOM_SCREEN_WIDTH * 4) as u32,
        );

        let mut display = DualScreenDisplay::new().unwrap();
        let report = controller.scanout(&memory, &mut display).unwrap();

        assert_eq!(report.screens_updated, 3);
        assert_eq!(&display.top_left().pixels()[..4], &[0x11, 0x22, 0x33, 0x44]);
        assert_eq!(
            &display.top_right().pixels()[..4],
            &[0x55, 0x66, 0x77, 0x88]
        );
        assert_eq!(&display.bottom().pixels()[..4], &[0x99, 0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn scanout_converts_rgb565_and_repeats_first_row_when_stride_is_zero() {
        let mut memory = GpuPhysicalMemory::new();
        let framebuffer = FCRAM_BASE + 0x4000;
        memory.write(framebuffer, &[0x00, 0xF8]).unwrap();

        let mut controller = LcdController::new();
        configure_panel(
            &mut controller,
            LcdPanel::Bottom,
            framebuffer,
            0,
            LcdPixelFormat::Rgb565,
            0,
        );
        let mut display = DualScreenDisplay::new().unwrap();

        controller.scanout(&memory, &mut display).unwrap();
        assert_eq!(&display.bottom().pixels()[..4], &[0xFF, 0, 0, 0xFF]);
        assert_eq!(
            &display.bottom().pixels()[(BOTTOM_SCREEN_WIDTH * 4)..(BOTTOM_SCREEN_WIDTH * 4 + 4)],
            &[0xFF, 0, 0, 0xFF]
        );
    }

    #[test]
    fn unsupported_interlace_modes_are_rejected() {
        let memory = GpuPhysicalMemory::new();
        let mut controller = LcdController::new();
        controller
            .write_register(LcdPanel::Top, PDC_FRAMEBUFFER_FORMAT, 1 << 4)
            .unwrap();
        controller
            .write_register(LcdPanel::Top, PDC_CONTROL, ENABLED)
            .unwrap();
        let mut display = DualScreenDisplay::new().unwrap();

        assert_eq!(
            controller.scanout(&memory, &mut display),
            Err(LcdControllerError::UnsupportedInterlaceMode(1))
        );
    }

    #[test]
    fn invalid_second_eye_does_not_partially_update_the_left_eye() {
        let mut memory = GpuPhysicalMemory::new();
        let left = FCRAM_BASE + 0x6000;
        memory.write(left, &[1, 2, 3, 4]).unwrap();

        let mut controller = LcdController::new();
        configure_panel(
            &mut controller,
            LcdPanel::Top,
            left,
            0x1000,
            LcdPixelFormat::Rgba8,
            (TOP_SCREEN_WIDTH * 4) as u32,
        );
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([9, 8, 7, 6]);

        assert!(controller.scanout(&memory, &mut display).is_err());
        assert_eq!(&display.top_left().pixels()[..4], &[9, 8, 7, 6]);
    }
}
