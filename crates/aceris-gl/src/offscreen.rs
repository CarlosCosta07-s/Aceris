//! An offscreen context: render into a framebuffer object, read pixels back.

use crate::egl::EglContextHandle;
use crate::gl::{self, GLuint, Gl};
use std::ffi::{CStr, c_char, c_void};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlError {
    /// No usable EGL/OpenGL stack (library missing, no context, no GPU).
    Unavailable(String),
    /// GL reported an error or an object failed to build.
    Failed(String),
}

impl fmt::Display for GlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(formatter, "OpenGL unavailable: {reason}"),
            Self::Failed(reason) => write!(formatter, "OpenGL error: {reason}"),
        }
    }
}

impl std::error::Error for GlError {}

/// A current offscreen GL context plus the function table. The context is
/// current on the thread that created it; keep the value on that thread.
pub struct OffscreenContext {
    gl: Gl,
    _egl: EglContextHandle,
}

impl OffscreenContext {
    pub fn new() -> Result<Self, GlError> {
        let egl = EglContextHandle::create_current().map_err(GlError::Unavailable)?;
        let gl = Gl::load(|name| egl.gl_proc(name)).map_err(GlError::Unavailable)?;
        Ok(Self { gl, _egl: egl })
    }

    pub fn gl(&self) -> &Gl {
        &self.gl
    }

    /// Returns `GL_VERSION` and `GL_RENDERER` for diagnostics.
    pub fn describe(&self) -> String {
        let text = |name| {
            // SAFETY: a context is current; glGetString returns a static
            // NUL-terminated string or null.
            let pointer = unsafe { (self.gl.get_string)(name) };
            if pointer.is_null() {
                "unknown".to_string()
            } else {
                // SAFETY: non-null, NUL-terminated per the GL spec.
                unsafe { CStr::from_ptr(pointer.cast::<c_char>()) }
                    .to_string_lossy()
                    .into_owned()
            }
        };
        format!("{} on {}", text(gl::VERSION), text(gl::RENDERER))
    }

    /// Turns any pending GL error into a `GlError`.
    pub fn check(&self, what: &str) -> Result<(), GlError> {
        // SAFETY: a context is current.
        let error = unsafe { (self.gl.get_error)() };
        if error == gl::NO_ERROR {
            Ok(())
        } else {
            Err(GlError::Failed(format!("{what}: GL error {error:#x}")))
        }
    }

    /// Compiles and links a program from GLSL sources.
    pub fn build_program(&self, vertex: &str, fragment: &str) -> Result<GLuint, GlError> {
        let vertex_shader = self.compile(gl::VERTEX_SHADER, vertex)?;
        let fragment_shader = self.compile(gl::FRAGMENT_SHADER, fragment)?;
        // SAFETY: a context is current; all handles come from the same context.
        unsafe {
            let program = (self.gl.create_program)();
            (self.gl.attach_shader)(program, vertex_shader);
            (self.gl.attach_shader)(program, fragment_shader);
            (self.gl.link_program)(program);
            (self.gl.delete_shader)(vertex_shader);
            (self.gl.delete_shader)(fragment_shader);
            let mut status = 0;
            (self.gl.get_program_iv)(program, gl::LINK_STATUS, &mut status);
            if status == 0 {
                let log = self.info_log(program, true);
                (self.gl.delete_program)(program);
                return Err(GlError::Failed(format!("program link failed: {log}")));
            }
            Ok(program)
        }
    }

    fn compile(&self, kind: gl::GLenum, source: &str) -> Result<GLuint, GlError> {
        let length = i32::try_from(source.len())
            .map_err(|_| GlError::Failed("shader source too long".to_string()))?;
        let pointer = source.as_ptr().cast::<c_char>();
        // SAFETY: a context is current; pointer/length describe `source`.
        unsafe {
            let shader = (self.gl.create_shader)(kind);
            (self.gl.shader_source)(shader, 1, &pointer, &length);
            (self.gl.compile_shader)(shader);
            let mut status = 0;
            (self.gl.get_shader_iv)(shader, gl::COMPILE_STATUS, &mut status);
            if status == 0 {
                let log = self.info_log(shader, false);
                (self.gl.delete_shader)(shader);
                return Err(GlError::Failed(format!("shader compile failed: {log}")));
            }
            Ok(shader)
        }
    }

    /// # Safety
    /// `object` must be a live shader (or program when `program`).
    unsafe fn info_log(&self, object: GLuint, program: bool) -> String {
        let mut length = 0;
        // SAFETY: caller guarantees `object` is live; out-pointer is a local.
        unsafe {
            if program {
                (self.gl.get_program_iv)(object, gl::INFO_LOG_LENGTH, &mut length);
            } else {
                (self.gl.get_shader_iv)(object, gl::INFO_LOG_LENGTH, &mut length);
            }
        }
        let mut buffer = vec![0u8; usize::try_from(length).unwrap_or(0).max(1)];
        let mut written = 0;
        // SAFETY: the buffer holds `length` bytes (min 1).
        unsafe {
            let destination = buffer.as_mut_ptr().cast::<c_char>();
            if program {
                (self.gl.get_program_info_log)(object, length.max(1), &mut written, destination);
            } else {
                (self.gl.get_shader_info_log)(object, length.max(1), &mut written, destination);
            }
        }
        buffer.truncate(usize::try_from(written).unwrap_or(0));
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// Creates a `width`x`height` RGBA8 framebuffer object. Returns
    /// `(framebuffer, texture)`.
    pub fn create_color_target(
        &self,
        width: i32,
        height: i32,
    ) -> Result<(GLuint, GLuint), GlError> {
        // SAFETY: a context is current; the texture has no initial data.
        unsafe {
            let mut texture = 0;
            (self.gl.gen_textures)(1, &mut texture);
            (self.gl.bind_texture)(gl::TEXTURE_2D, texture);
            (self.gl.tex_parameter_i)(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST);
            (self.gl.tex_parameter_i)(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST);
            (self.gl.tex_image_2d)(
                gl::TEXTURE_2D,
                0,
                gl::RGBA8 as i32,
                width,
                height,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                std::ptr::null::<c_void>(),
            );
            let mut framebuffer = 0;
            (self.gl.gen_framebuffers)(1, &mut framebuffer);
            (self.gl.bind_framebuffer)(gl::FRAMEBUFFER, framebuffer);
            (self.gl.framebuffer_texture_2d)(
                gl::FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                texture,
                0,
            );
            let status = (self.gl.check_framebuffer_status)(gl::FRAMEBUFFER);
            if status != gl::FRAMEBUFFER_COMPLETE {
                return Err(GlError::Failed(format!(
                    "framebuffer incomplete: {status:#x}"
                )));
            }
            Ok((framebuffer, texture))
        }
    }

    /// Reads the bound framebuffer back as tightly packed RGBA8 rows, bottom
    /// row first (GL origin).
    pub fn read_rgba(&self, width: i32, height: i32) -> Result<Vec<u8>, GlError> {
        let mut pixels = vec![0u8; (width.max(0) as usize) * (height.max(0) as usize) * 4];
        // SAFETY: the buffer holds width*height*4 bytes; alignment 1 means rows are packed.
        unsafe {
            (self.gl.pixel_store_i)(gl::PACK_ALIGNMENT, 1);
            (self.gl.read_pixels)(
                0,
                0,
                width,
                height,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                pixels.as_mut_ptr().cast::<c_void>(),
            );
        }
        self.check("glReadPixels")?;
        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a context, or `None` (test skipped) when this machine has no
    /// usable EGL/OpenGL stack.
    fn context() -> Option<OffscreenContext> {
        match OffscreenContext::new() {
            Ok(context) => Some(context),
            Err(error) => {
                eprintln!("skipping OpenGL test: {error}");
                None
            }
        }
    }

    #[test]
    fn clear_and_read_back_returns_the_clear_color() {
        let Some(context) = context() else { return };
        eprintln!("GL: {}", context.describe());
        let (framebuffer, _texture) = context.create_color_target(8, 4).unwrap();
        let gl = context.gl();
        // SAFETY: context is current; framebuffer is complete.
        unsafe {
            (gl.bind_framebuffer)(gl::FRAMEBUFFER, framebuffer);
            (gl.viewport)(0, 0, 8, 4);
            (gl.clear_color)(1.0, 0.0, 0.5, 1.0);
            (gl.clear)(gl::COLOR_BUFFER_BIT);
        }
        let pixels = context.read_rgba(8, 4).unwrap();
        assert_eq!(pixels.len(), 8 * 4 * 4);
        for pixel in pixels.chunks_exact(4) {
            assert_eq!(pixel, [255, 0, 128, 255]);
        }
    }

    #[test]
    fn shader_program_draws_a_triangle_into_the_framebuffer() {
        let Some(context) = context() else { return };
        let vertex = "#version 330 core\nlayout(location=0) in vec2 position;\nvoid main(){gl_Position=vec4(position,0.0,1.0);}\n";
        let fragment =
            "#version 330 core\nout vec4 color;\nvoid main(){color=vec4(0.0,1.0,0.0,1.0);}\n";
        let program = context.build_program(vertex, fragment).unwrap();
        let (framebuffer, _texture) = context.create_color_target(16, 16).unwrap();
        // A triangle covering the whole viewport (oversized clip triangle).
        let vertices: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];
        let gl = context.gl();
        // SAFETY: context is current; buffer data points at `vertices`.
        unsafe {
            let mut vao = 0;
            (gl.gen_vertex_arrays)(1, &mut vao);
            (gl.bind_vertex_array)(vao);
            let mut buffer = 0;
            (gl.gen_buffers)(1, &mut buffer);
            (gl.bind_buffer)(gl::ARRAY_BUFFER, buffer);
            (gl.buffer_data)(
                gl::ARRAY_BUFFER,
                std::mem::size_of_val(&vertices) as isize,
                vertices.as_ptr().cast::<c_void>(),
                gl::STATIC_DRAW,
            );
            (gl.enable_vertex_attrib_array)(0);
            (gl.vertex_attrib_pointer)(0, 2, gl::FLOAT, gl::FALSE, 8, std::ptr::null());
            (gl.bind_framebuffer)(gl::FRAMEBUFFER, framebuffer);
            (gl.viewport)(0, 0, 16, 16);
            (gl.clear_color)(0.0, 0.0, 0.0, 1.0);
            (gl.clear)(gl::COLOR_BUFFER_BIT);
            (gl.use_program)(program);
            (gl.draw_arrays)(gl::TRIANGLES, 0, 3);
        }
        let pixels = context.read_rgba(16, 16).unwrap();
        assert!(
            pixels
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 255, 0, 255])
        );
    }

    #[test]
    fn shader_errors_are_reported_with_the_driver_log() {
        let Some(context) = context() else { return };
        let error = context
            .build_program(
                "#version 330 core\nvoid main(){ this is not glsl }\n",
                "#version 330 core\nout vec4 c;void main(){c=vec4(1.0);}\n",
            )
            .unwrap_err();
        assert!(matches!(error, GlError::Failed(message) if message.contains("compile")));
    }
}
