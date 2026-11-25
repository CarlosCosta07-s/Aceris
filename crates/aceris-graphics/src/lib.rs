//! Native GPU presentation backends shared by the Aceris frontends.
//!
//! This crate deliberately stays outside aceris-core: operating-system
//! windows, GPU instances, surfaces, swapchains and host API objects are not
//! guest PICA state. The first GPU path uploads a completed RGBA frame to a
//! host texture and presents it through Vulkan or OpenGL/OpenGL ES. The
//! software PICA renderer remains the compatibility reference and fallback.
//!
//! The raw-surface constructor is unsafe because the caller owns the native
//! display/window lifetime. The presenter must be dropped before those native
//! objects are destroyed.

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::env;

const PRESENT_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var coordinates = array<vec2<f32>, 3>(
        vec2<f32>(0.0,  1.0),
        vec2<f32>(2.0,  1.0),
        vec2<f32>(0.0, -1.0),
    );

    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    output.uv = coordinates[index];
    return output;
}

@group(0) @binding(0)
var frame_texture: texture_2d<f32>;

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let dimensions = textureDimensions(frame_texture);
    let upper = vec2<i32>(dimensions) - vec2<i32>(1, 1);
    let coordinates = clamp(
        vec2<i32>(input.uv * vec2<f32>(dimensions)),
        vec2<i32>(0, 0),
        upper,
    );
    return textureLoad(frame_texture, coordinates, 0);
}
"#;

/// Selects which native host API wgpu should initialize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsBackend {
    /// Try Vulkan and OpenGL/OpenGL ES; use Metal on macOS.
    Auto,
    /// Native OpenGL on desktop or OpenGL ES through EGL on Android.
    OpenGl,
    /// Vulkan through the platform Vulkan loader and surface extensions.
    Vulkan,
    /// Disable host GPU presentation and keep the software window path.
    Software,
}

impl GraphicsBackend {
    /// Reads ACERIS_GRAPHICS_BACKEND (auto, opengl, vulkan, software).
    /// An unset variable selects auto.
    pub fn from_env() -> Result<Self, String> {
        match env::var("ACERIS_GRAPHICS_BACKEND") {
            Ok(value) => Self::parse(&value),
            Err(env::VarError::NotPresent) => Ok(Self::Auto),
            Err(error) => Err(format!("could not read ACERIS_GRAPHICS_BACKEND: {error}")),
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "gl" | "opengl" | "gles" | "opengles" => Ok(Self::OpenGl),
            "vk" | "vulkan" => Ok(Self::Vulkan),
            "software" | "cpu" | "off" | "none" => Ok(Self::Software),
            _ => Err(format!(
                "unknown ACERIS_GRAPHICS_BACKEND value {value:?}; use auto, opengl, vulkan or software"
            )),
        }
    }

    fn enabled_backends(self) -> Result<wgpu::Backends, String> {
        match self {
            Self::Auto => {
                #[cfg(target_os = "macos")]
                {
                    Ok(wgpu::Backends::METAL)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Ok(wgpu::Backends::VULKAN | wgpu::Backends::GL)
                }
            }
            Self::OpenGl => Ok(wgpu::Backends::GL),
            Self::Vulkan => {
                #[cfg(target_os = "macos")]
                {
                    Err("Vulkan is not enabled on macOS; use auto (Metal) or software".to_string())
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Ok(wgpu::Backends::VULKAN)
                }
            }
            Self::Software => {
                Err("software presentation does not create a GPU presenter".to_string())
            }
        }
    }
}

/// Describes the GPU actually selected by the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphicsAdapterInfo {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub backend: String,
    pub device_type: String,
}

/// Presents an already-composed RGBA8 frame through the selected host GPU.
///
/// The guest emulation core and its software PICA renderer are not bypassed by
/// this first presenter. It accelerates host texture upload/composition and
/// display submission; it is not yet a PICA draw-call rasterizer.
pub struct GraphicsPresenter {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    frame_layout: wgpu::BindGroupLayout,
    frame_texture: Option<wgpu::Texture>,
    frame_bind_group: Option<wgpu::BindGroup>,
    frame_dimensions: (u32, u32),
    upload_rgba: Vec<u8>,
    adapter_info: GraphicsAdapterInfo,
}

impl GraphicsPresenter {
    /// Creates a GPU surface from already-owned native display/window handles.
    ///
    /// # Safety
    ///
    /// The caller must keep the native display and window alive until this
    /// presenter has been dropped. The presenter must be dropped before either
    /// native object is released or destroyed. On Android, retain an additional
    /// ANativeWindow reference for the full presenter lifetime.
    pub unsafe fn new(
        display_handle: Option<RawDisplayHandle>,
        window_handle: RawWindowHandle,
        width: u32,
        height: u32,
        backend: GraphicsBackend,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("GPU surface dimensions must be non-zero".to_string());
        }
        let backends = backend.enabled_backends()?;
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = backends;
        let instance = wgpu::Instance::new(descriptor);

        // SAFETY: the caller guarantees that both raw handles remain valid
        // until this presenter (and its surface) has been dropped.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: display_handle,
                raw_window_handle: window_handle,
            })
        }
        .map_err(|error| format!("could not create GPU surface: {error}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|error| format!("no compatible {:?} GPU adapter: {error}", backend))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Aceris host presentation device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|error| format!("could not create GPU device: {error}"))?;

        let mut surface_config = surface
            .get_default_config(&adapter, width, height)
            .ok_or_else(|| "the selected GPU cannot present to this surface".to_string())?;
        surface_config.present_mode = if surface
            .get_capabilities(&adapter)
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            surface_config.present_mode
        };
        surface_config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &surface_config);

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Aceris frame texture layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Aceris host presentation pipeline layout"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Aceris original frame presentation shader"),
            source: wgpu::ShaderSource::Wgsl(PRESENT_SHADER.into()),
        });
        let color_targets = [Some(wgpu::ColorTargetState {
            format: surface_config.format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Aceris host presentation pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &color_targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        let info = adapter.get_info();
        let adapter_info = GraphicsAdapterInfo {
            name: info.name,
            vendor: info.vendor,
            device: info.device,
            backend: format!("{:?}", info.backend),
            device_type: format!("{:?}", info.device_type),
        };

        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            surface_config,
            pipeline,
            frame_layout,
            frame_texture: None,
            frame_bind_group: None,
            frame_dimensions: (0, 0),
            upload_rgba: Vec::new(),
            adapter_info,
        })
    }

    pub fn adapter_info(&self) -> &GraphicsAdapterInfo {
        &self.adapter_info
    }

    /// Reconfigures the swapchain after a native window resize.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err("GPU surface dimensions must be non-zero".to_string());
        }
        if self.surface_config.width == width && self.surface_config.height == height {
            return Ok(());
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        Ok(())
    }

    /// Uploads and presents a tightly packed top-to-bottom RGBA8 frame.
    pub fn present_rgba(&mut self, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
        let byte_len = frame_byte_len(width, height)?;
        if rgba.len() != byte_len {
            return Err(format!(
                "RGBA frame has {} bytes; {} were required for {width}x{height}",
                rgba.len(),
                byte_len
            ));
        }
        self.present_pixels(rgba, width, height)
    }

    /// Uploads a tightly packed BGRX8 frame, converting it into a reusable RGBA buffer.
    pub fn present_bgrx(&mut self, bgrx: &[u8], width: u32, height: u32) -> Result<(), String> {
        let byte_len = frame_byte_len(width, height)?;
        if bgrx.len() != byte_len {
            return Err(format!(
                "BGRX frame has {} bytes; {} were required for {width}x{height}",
                bgrx.len(),
                byte_len
            ));
        }
        self.upload_rgba.resize(byte_len, 0);
        convert_bgrx_to_rgba(bgrx, &mut self.upload_rgba);
        let upload = std::mem::take(&mut self.upload_rgba);
        let result = self.present_pixels(&upload, width, height);
        self.upload_rgba = upload;
        result
    }

    fn present_pixels(&mut self, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
        if width != self.surface_config.width || height != self.surface_config.height {
            self.resize(width, height)?;
        }
        self.ensure_frame_texture(width, height);
        let texture = self
            .frame_texture
            .as_ref()
            .ok_or_else(|| "GPU frame texture was not allocated".to_string())?;
        let bytes_per_row = width
            .checked_mul(4)
            .ok_or_else(|| "GPU frame row is too wide".to_string())?;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let mut configure_after_present = false;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                configure_after_present = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) => frame,
                    wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                        configure_after_present = true;
                        frame
                    }
                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
                    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                        return Err(
                            "GPU surface remained outdated or lost after reconfiguration"
                                .to_string(),
                        );
                    }
                    wgpu::CurrentSurfaceTexture::Validation => {
                        return Err("GPU surface acquisition failed validation".to_string());
                    }
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("GPU surface acquisition failed validation".to_string());
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Aceris frame presentation commands"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Aceris host presentation pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(
                0,
                self.frame_bind_group
                    .as_ref()
                    .ok_or_else(|| "GPU frame bind group was not created".to_string())?,
                &[],
            );
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        if configure_after_present {
            self.surface.configure(&self.device, &self.surface_config);
        }
        Ok(())
    }

    fn ensure_frame_texture(&mut self, width: u32, height: u32) {
        if self.frame_dimensions == (width, height) {
            return;
        }
        let format = if self.surface_config.format.is_srgb() {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Aceris completed frame texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Aceris completed frame bind group"),
            layout: &self.frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        self.frame_texture = Some(texture);
        self.frame_bind_group = Some(bind_group);
        self.frame_dimensions = (width, height);
    }
}

fn frame_byte_len(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("frame dimensions must be non-zero".to_string());
    }
    let width = usize::try_from(width).map_err(|_| "frame width is too large")?;
    let height = usize::try_from(height).map_err(|_| "frame height is too large")?;
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "frame dimensions overflow".to_string())
}

fn convert_bgrx_to_rgba(source: &[u8], destination: &mut [u8]) {
    for (source, destination) in source.chunks_exact(4).zip(destination.chunks_exact_mut(4)) {
        destination.copy_from_slice(&[source[2], source[1], source[0], 0xFF]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_names_are_parsed_case_insensitively() {
        assert_eq!(
            GraphicsBackend::parse("OpenGL").unwrap(),
            GraphicsBackend::OpenGl
        );
        assert_eq!(
            GraphicsBackend::parse("VK").unwrap(),
            GraphicsBackend::Vulkan
        );
        assert_eq!(
            GraphicsBackend::parse("SOFTWARE").unwrap(),
            GraphicsBackend::Software
        );
        assert_eq!(
            GraphicsBackend::parse("auto").unwrap(),
            GraphicsBackend::Auto
        );
    }

    #[test]
    fn unsupported_backend_names_return_a_useful_error() {
        assert!(
            GraphicsBackend::parse("directx")
                .unwrap_err()
                .contains("ACERIS_GRAPHICS_BACKEND")
        );
    }

    #[test]
    fn frame_dimensions_are_checked_before_allocation() {
        assert_eq!(frame_byte_len(320, 240).unwrap(), 320 * 240 * 4);
        assert!(frame_byte_len(0, 240).is_err());
        assert!(frame_byte_len(u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn bgrx_conversion_preserves_red_green_blue_order_and_sets_alpha() {
        let source = [0x11, 0x22, 0x33, 0x00, 0xAA, 0xBB, 0xCC, 0xDD];
        let mut output = [0; 8];
        convert_bgrx_to_rgba(&source, &mut output);
        assert_eq!(output, [0x33, 0x22, 0x11, 0xFF, 0xCC, 0xBB, 0xAA, 0xFF]);
    }
}
