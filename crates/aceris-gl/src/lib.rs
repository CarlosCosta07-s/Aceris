//! Offscreen OpenGL support for Aceris, written without external crates.
//!
//! The system's EGL and OpenGL libraries are opened with `dlopen` at run time
//! (so building needs no development packages and a machine without a GPU
//! stack still builds and runs the CPU renderer). On Linux the context is a
//! surfaceless EGL desktop-GL 3.3 core context; on Android it is an EGL
//! OpenGL ES 3.0 context. Everything here renders into framebuffer objects and
//! reads pixels back, because the emulated color buffers live in guest memory.

mod dynlib;
mod egl;
pub mod gl;
mod offscreen;

pub use offscreen::{GlError, OffscreenContext};
