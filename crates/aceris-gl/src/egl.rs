//! EGL entry points and offscreen context creation.

use crate::dynlib::Library;
use std::ffi::{c_char, c_int, c_void};

pub type EglDisplay = *mut c_void;
pub type EglConfig = *mut c_void;
pub type EglContext = *mut c_void;
pub type EglSurface = *mut c_void;

const EGL_NO_SURFACE: EglSurface = std::ptr::null_mut();
const EGL_NO_CONTEXT: EglContext = std::ptr::null_mut();
const EGL_TRUE: u32 = 1;

const EGL_NONE: c_int = 0x3038;
const EGL_SURFACE_TYPE: c_int = 0x3033;
const EGL_PBUFFER_BIT: c_int = 0x0001;
const EGL_RENDERABLE_TYPE: c_int = 0x3040;
const EGL_OPENGL_BIT: c_int = 0x0008;
const EGL_OPENGL_ES3_BIT: c_int = 0x0040;
const EGL_OPENGL_API: u32 = 0x30A2;
const EGL_OPENGL_ES_API: u32 = 0x30A0;
const EGL_CONTEXT_MAJOR_VERSION: c_int = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: c_int = 0x30FB;
const EGL_CONTEXT_OPENGL_PROFILE_MASK: c_int = 0x30FD;
const EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT: c_int = 0x0001;
const EGL_WIDTH: c_int = 0x3057;
const EGL_HEIGHT: c_int = 0x3056;
const EGL_PLATFORM_SURFACELESS_MESA: u32 = 0x31DD;

type GetProcAddress = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type GetPlatformDisplay = unsafe extern "C" fn(u32, *mut c_void, *const isize) -> EglDisplay;
type GetDisplay = unsafe extern "C" fn(*mut c_void) -> EglDisplay;
type Initialize = unsafe extern "C" fn(EglDisplay, *mut c_int, *mut c_int) -> u32;
type BindApi = unsafe extern "C" fn(u32) -> u32;
type ChooseConfig =
    unsafe extern "C" fn(EglDisplay, *const c_int, *mut EglConfig, c_int, *mut c_int) -> u32;
type CreateContext =
    unsafe extern "C" fn(EglDisplay, EglConfig, EglContext, *const c_int) -> EglContext;
type CreatePbufferSurface = unsafe extern "C" fn(EglDisplay, EglConfig, *const c_int) -> EglSurface;
type MakeCurrent = unsafe extern "C" fn(EglDisplay, EglSurface, EglSurface, EglContext) -> u32;
type GetError = unsafe extern "C" fn() -> c_int;

/// A current EGL context with its resolved entry points.
pub struct EglContextHandle {
    get_proc_address: GetProcAddress,
    gl_library: Option<Library>,
    // Kept so the objects live as long as the handle.
    _display: EglDisplay,
    _context: EglContext,
    _surface: EglSurface,
}

impl EglContextHandle {
    /// Creates a context and makes it current on the calling thread.
    pub fn create_current() -> Result<Self, String> {
        let egl = Library::open_any(&["libEGL.so.1", "libEGL.so"])
            .ok_or_else(|| "libEGL is not available".to_string())?;
        let gl_library = Library::open_any(if cfg!(target_os = "android") {
            &["libGLESv3.so", "libGLESv2.so"][..]
        } else {
            &["libGL.so.1", "libOpenGL.so.0"][..]
        });

        macro_rules! egl_fn {
            ($name:literal as $ty:ty) => {{
                let pointer = egl.symbol($name);
                if pointer.is_null() {
                    return Err(format!("libEGL lacks {}", $name));
                }
                // SAFETY: the symbol was resolved from libEGL for exactly this
                // prototype (EGL 1.4/1.5 specification).
                unsafe { std::mem::transmute::<*mut c_void, $ty>(pointer) }
            }};
        }
        let get_proc_address = egl_fn!("eglGetProcAddress" as GetProcAddress);
        let initialize = egl_fn!("eglInitialize" as Initialize);
        let bind_api = egl_fn!("eglBindAPI" as BindApi);
        let choose_config = egl_fn!("eglChooseConfig" as ChooseConfig);
        let create_context = egl_fn!("eglCreateContext" as CreateContext);
        let create_pbuffer = egl_fn!("eglCreatePbufferSurface" as CreatePbufferSurface);
        let make_current = egl_fn!("eglMakeCurrent" as MakeCurrent);
        let get_error = egl_fn!("eglGetError" as GetError);

        // Prefer the surfaceless platform (no display server needed); fall
        // back to the default display (Android, or a desktop session).
        let mut display: EglDisplay = std::ptr::null_mut();
        let platform_display = egl.symbol("eglGetPlatformDisplay");
        if !platform_display.is_null() {
            // SAFETY: resolved for the EGL 1.5 prototype.
            let get_platform_display =
                unsafe { std::mem::transmute::<*mut c_void, GetPlatformDisplay>(platform_display) };
            // SAFETY: surfaceless platform takes no native display or attributes.
            display = unsafe {
                get_platform_display(
                    EGL_PLATFORM_SURFACELESS_MESA,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
        }
        let (mut major, mut minor) = (0, 0);
        // SAFETY: out-pointers reference live locals.
        let initialized = !display.is_null()
            && unsafe { initialize(display, &mut major, &mut minor) } == EGL_TRUE;
        if !initialized {
            let get_display = egl_fn!("eglGetDisplay" as GetDisplay);
            // SAFETY: EGL_DEFAULT_DISPLAY is a null native display.
            display = unsafe { get_display(std::ptr::null_mut()) };
            // SAFETY: out-pointers reference live locals.
            if display.is_null()
                || unsafe { initialize(display, &mut major, &mut minor) } != EGL_TRUE
            {
                return Err("eglInitialize failed".to_string());
            }
        }

        let (api, renderable) = if cfg!(target_os = "android") {
            (EGL_OPENGL_ES_API, EGL_OPENGL_ES3_BIT)
        } else {
            (EGL_OPENGL_API, EGL_OPENGL_BIT)
        };
        // SAFETY: plain enum argument.
        if unsafe { bind_api(api) } != EGL_TRUE {
            return Err("eglBindAPI failed".to_string());
        }
        let config_attributes = [
            EGL_SURFACE_TYPE,
            EGL_PBUFFER_BIT,
            EGL_RENDERABLE_TYPE,
            renderable,
            EGL_NONE,
        ];
        let mut config: EglConfig = std::ptr::null_mut();
        let mut count = 0;
        // SAFETY: the attribute list is EGL_NONE-terminated; out-pointers are live.
        let chosen = unsafe {
            choose_config(
                display,
                config_attributes.as_ptr(),
                &mut config,
                1,
                &mut count,
            )
        };
        if chosen != EGL_TRUE || count < 1 {
            return Err(format!("no usable EGL config (error {:#x})", unsafe {
                // SAFETY: eglGetError has no arguments.
                get_error()
            }));
        }

        let context_attributes: &[c_int] = if cfg!(target_os = "android") {
            &[EGL_CONTEXT_MAJOR_VERSION, 3, EGL_NONE]
        } else {
            &[
                EGL_CONTEXT_MAJOR_VERSION,
                3,
                EGL_CONTEXT_MINOR_VERSION,
                3,
                EGL_CONTEXT_OPENGL_PROFILE_MASK,
                EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT,
                EGL_NONE,
            ]
        };
        // SAFETY: attribute list is EGL_NONE-terminated.
        let context =
            unsafe { create_context(display, config, EGL_NO_CONTEXT, context_attributes.as_ptr()) };
        if context.is_null() {
            return Err(format!("eglCreateContext failed (error {:#x})", unsafe {
                // SAFETY: eglGetError has no arguments.
                get_error()
            }));
        }

        // Surfaceless first; a 1x1 pbuffer if the driver needs a surface.
        let mut surface = EGL_NO_SURFACE;
        // SAFETY: valid display/context; no surface is allowed by
        // EGL_KHR_surfaceless_context.
        if unsafe { make_current(display, EGL_NO_SURFACE, EGL_NO_SURFACE, context) } != EGL_TRUE {
            let pbuffer_attributes = [EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE];
            // SAFETY: attribute list is EGL_NONE-terminated.
            surface = unsafe { create_pbuffer(display, config, pbuffer_attributes.as_ptr()) };
            // SAFETY: valid display/context/surface (null surface fails cleanly).
            if surface.is_null()
                || unsafe { make_current(display, surface, surface, context) } != EGL_TRUE
            {
                return Err("eglMakeCurrent failed".to_string());
            }
        }

        Ok(Self {
            get_proc_address,
            gl_library,
            _display: display,
            _context: context,
            _surface: surface,
        })
    }

    /// Resolves a GL function for the current context.
    pub fn gl_proc(&self, name: &str) -> *mut c_void {
        if let Ok(c_name) = std::ffi::CString::new(name) {
            // SAFETY: NUL-terminated name; a context is current.
            let pointer = unsafe { (self.get_proc_address)(c_name.as_ptr()) };
            if !pointer.is_null() {
                return pointer;
            }
        }
        self.gl_library
            .map_or(std::ptr::null_mut(), |library| library.symbol(name))
    }
}
