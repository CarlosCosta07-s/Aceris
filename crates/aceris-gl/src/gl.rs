//! The OpenGL 3.3 core / ES 3.0 subset used by the renderer, as raw function
//! pointers resolved at run time.

use std::ffi::{c_char, c_void};

pub type GLenum = u32;
pub type GLuint = u32;
pub type GLint = i32;
pub type GLsizei = i32;
pub type GLfloat = f32;
pub type GLboolean = u8;
pub type GLbitfield = u32;

pub const FALSE: GLboolean = 0;
pub const TRUE: GLboolean = 1;
pub const NO_ERROR: GLenum = 0;
pub const TRIANGLES: GLenum = 0x0004;
pub const DEPTH_TEST: GLenum = 0x0B71;
pub const BLEND: GLenum = 0x0BE2;
pub const SCISSOR_TEST: GLenum = 0x0C11;
pub const UNSIGNED_BYTE: GLenum = 0x1401;
pub const FLOAT: GLenum = 0x1406;
pub const RGBA: GLenum = 0x1908;
pub const RGBA8: GLenum = 0x8058;
pub const DEPTH_COMPONENT24: GLenum = 0x81A6;
pub const TEXTURE_2D: GLenum = 0x0DE1;
pub const TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const NEAREST: GLint = 0x2600;
pub const ARRAY_BUFFER: GLenum = 0x8892;
pub const STATIC_DRAW: GLenum = 0x88E4;
pub const STREAM_DRAW: GLenum = 0x88E0;
pub const FRAMEBUFFER: GLenum = 0x8D40;
pub const RENDERBUFFER: GLenum = 0x8D41;
pub const COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const DEPTH_ATTACHMENT: GLenum = 0x8D00;
pub const FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
pub const COLOR_BUFFER_BIT: GLbitfield = 0x4000;
pub const DEPTH_BUFFER_BIT: GLbitfield = 0x0100;
pub const VERTEX_SHADER: GLenum = 0x8B31;
pub const FRAGMENT_SHADER: GLenum = 0x8B30;
pub const COMPILE_STATUS: GLenum = 0x8B81;
pub const LINK_STATUS: GLenum = 0x8B82;
pub const INFO_LOG_LENGTH: GLenum = 0x8B84;
pub const VERSION: GLenum = 0x1F02;
pub const RENDERER: GLenum = 0x1F01;
pub const PACK_ALIGNMENT: GLenum = 0x0D05;
pub const UNPACK_ALIGNMENT: GLenum = 0x0CF5;

macro_rules! gl_table {
    ($($field:ident: fn($($arg:ty),*) $(-> $ret:ty)? = $symbol:literal;)*) => {
        /// Resolved GL entry points. Every call is `unsafe`: a context must be
        /// current on the calling thread and arguments must satisfy the GL spec.
        pub struct Gl {
            $(pub $field: unsafe extern "system" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Gl {
            /// Resolves every function through `resolve`, failing on the first
            /// missing symbol.
            pub fn load(resolve: impl Fn(&str) -> *mut c_void) -> Result<Self, String> {
                Ok(Self {
                    $($field: {
                        let pointer = resolve($symbol);
                        if pointer.is_null() {
                            return Err(format!("OpenGL lacks {}", $symbol));
                        }
                        // SAFETY: the pointer was resolved by name for exactly
                        // this prototype from the OpenGL 3.3 / ES 3.0 API.
                        unsafe {
                            std::mem::transmute::<
                                *mut c_void,
                                unsafe extern "system" fn($($arg),*) $(-> $ret)?,
                            >(pointer)
                        }
                    },)*
                })
            }
        }
    };
}

gl_table! {
    get_error: fn() -> GLenum = "glGetError";
    get_string: fn(GLenum) -> *const u8 = "glGetString";
    pixel_store_i: fn(GLenum, GLint) = "glPixelStorei";
    viewport: fn(GLint, GLint, GLsizei, GLsizei) = "glViewport";
    scissor: fn(GLint, GLint, GLsizei, GLsizei) = "glScissor";
    clear_color: fn(GLfloat, GLfloat, GLfloat, GLfloat) = "glClearColor";
    clear: fn(GLbitfield) = "glClear";
    enable: fn(GLenum) = "glEnable";
    disable: fn(GLenum) = "glDisable";
    read_pixels: fn(GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *mut c_void) = "glReadPixels";
    gen_textures: fn(GLsizei, *mut GLuint) = "glGenTextures";
    delete_textures: fn(GLsizei, *const GLuint) = "glDeleteTextures";
    bind_texture: fn(GLenum, GLuint) = "glBindTexture";
    tex_parameter_i: fn(GLenum, GLenum, GLint) = "glTexParameteri";
    tex_image_2d: fn(GLenum, GLint, GLint, GLsizei, GLsizei, GLint, GLenum, GLenum, *const c_void) = "glTexImage2D";
    gen_framebuffers: fn(GLsizei, *mut GLuint) = "glGenFramebuffers";
    delete_framebuffers: fn(GLsizei, *const GLuint) = "glDeleteFramebuffers";
    bind_framebuffer: fn(GLenum, GLuint) = "glBindFramebuffer";
    framebuffer_texture_2d: fn(GLenum, GLenum, GLenum, GLuint, GLint) = "glFramebufferTexture2D";
    check_framebuffer_status: fn(GLenum) -> GLenum = "glCheckFramebufferStatus";
    create_shader: fn(GLenum) -> GLuint = "glCreateShader";
    delete_shader: fn(GLuint) = "glDeleteShader";
    shader_source: fn(GLuint, GLsizei, *const *const c_char, *const GLint) = "glShaderSource";
    compile_shader: fn(GLuint) = "glCompileShader";
    get_shader_iv: fn(GLuint, GLenum, *mut GLint) = "glGetShaderiv";
    get_shader_info_log: fn(GLuint, GLsizei, *mut GLsizei, *mut c_char) = "glGetShaderInfoLog";
    create_program: fn() -> GLuint = "glCreateProgram";
    delete_program: fn(GLuint) = "glDeleteProgram";
    attach_shader: fn(GLuint, GLuint) = "glAttachShader";
    link_program: fn(GLuint) = "glLinkProgram";
    get_program_iv: fn(GLuint, GLenum, *mut GLint) = "glGetProgramiv";
    get_program_info_log: fn(GLuint, GLsizei, *mut GLsizei, *mut c_char) = "glGetProgramInfoLog";
    use_program: fn(GLuint) = "glUseProgram";
    gen_vertex_arrays: fn(GLsizei, *mut GLuint) = "glGenVertexArrays";
    bind_vertex_array: fn(GLuint) = "glBindVertexArray";
    gen_buffers: fn(GLsizei, *mut GLuint) = "glGenBuffers";
    delete_buffers: fn(GLsizei, *const GLuint) = "glDeleteBuffers";
    bind_buffer: fn(GLenum, GLuint) = "glBindBuffer";
    buffer_data: fn(GLenum, isize, *const c_void, GLenum) = "glBufferData";
    enable_vertex_attrib_array: fn(GLuint) = "glEnableVertexAttribArray";
    vertex_attrib_pointer: fn(GLuint, GLint, GLenum, GLboolean, GLsizei, *const c_void) = "glVertexAttribPointer";
    draw_arrays: fn(GLenum, GLint, GLsizei) = "glDrawArrays";
}
