//! Android `NativeActivity` host over raw FFI (no NDK glue library, no crates).
//!
//! STATUS: written without an Android device or NDK at hand. It is type-checked
//! on Linux (`RUSTFLAGS="--cfg aceris_typecheck_native" cargo check`) and built
//! for `aarch64-linux-android` by the repository CI, but has never been run on a
//! device. Application logic (intro, library, gamepad overlay, touch) lives in
//! `app.rs` and is unit-tested on every platform.
//!
//! Threading follows `android_native_app_glue`: the activity callbacks run on
//! the UI thread and only record state under a mutex and wake the app thread's
//! `ALooper`; the app thread owns the emulator, the input queue attachment and
//! the window, and renders at ~60 Hz.

use crate::aaudio::AAudioOutput;
use crate::app::{App, Pointer, Screen};
use crate::performance::FramePerformanceMonitor;
use aceris_graphics::{GraphicsBackend, GraphicsPresenter};
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[repr(C)]
pub struct NativeActivityCallbacks {
    on_start: Option<unsafe extern "C" fn(*mut NativeActivity)>,
    on_resume: Option<unsafe extern "C" fn(*mut NativeActivity)>,
    on_save_instance_state:
        Option<unsafe extern "C" fn(*mut NativeActivity, *mut usize) -> *mut c_void>,
    on_pause: Option<unsafe extern "C" fn(*mut NativeActivity)>,
    on_stop: Option<unsafe extern "C" fn(*mut NativeActivity)>,
    on_destroy: Option<unsafe extern "C" fn(*mut NativeActivity)>,
    on_window_focus_changed: Option<unsafe extern "C" fn(*mut NativeActivity, c_int)>,
    on_native_window_created: Option<unsafe extern "C" fn(*mut NativeActivity, *mut c_void)>,
    on_native_window_resized: Option<unsafe extern "C" fn(*mut NativeActivity, *mut c_void)>,
    on_native_window_redraw_needed: Option<unsafe extern "C" fn(*mut NativeActivity, *mut c_void)>,
    on_native_window_destroyed: Option<unsafe extern "C" fn(*mut NativeActivity, *mut c_void)>,
    on_input_queue_created: Option<unsafe extern "C" fn(*mut NativeActivity, *mut c_void)>,
    on_input_queue_destroyed: Option<unsafe extern "C" fn(*mut NativeActivity, *mut c_void)>,
    on_content_rect_changed: Option<unsafe extern "C" fn(*mut NativeActivity, *const c_void)>,
    on_configuration_changed: Option<unsafe extern "C" fn(*mut NativeActivity)>,
    on_low_memory: Option<unsafe extern "C" fn(*mut NativeActivity)>,
}

#[repr(C)]
pub struct NativeActivity {
    callbacks: *mut NativeActivityCallbacks,
    vm: *mut c_void,
    env: *mut c_void,
    clazz: *mut c_void,
    internal_data_path: *const c_char,
    external_data_path: *const c_char,
    sdk_version: i32,
    instance: *mut c_void,
    asset_manager: *mut c_void,
    obb_path: *const c_char,
}

#[repr(C)]
struct NativeWindowBuffer {
    width: i32,
    height: i32,
    stride: i32,
    format: i32,
    bits: *mut c_void,
    reserved: [u32; 6],
}

#[cfg_attr(target_os = "android", link(name = "android"))]
unsafe extern "C" {
    fn ANativeWindow_getWidth(window: *mut c_void) -> i32;
    fn ANativeWindow_getHeight(window: *mut c_void) -> i32;
    fn ANativeWindow_setBuffersGeometry(
        window: *mut c_void,
        width: i32,
        height: i32,
        format: i32,
    ) -> i32;
    fn ANativeWindow_lock(
        window: *mut c_void,
        out_buffer: *mut NativeWindowBuffer,
        in_out_dirty_bounds: *mut c_void,
    ) -> i32;
    fn ANativeWindow_unlockAndPost(window: *mut c_void) -> i32;
    fn ANativeWindow_acquire(window: *mut c_void);
    fn ANativeWindow_release(window: *mut c_void);

    fn ALooper_prepare(options: c_int) -> *mut c_void;
    fn ALooper_pollOnce(
        timeout_millis: c_int,
        out_fd: *mut c_int,
        out_events: *mut c_int,
        out_data: *mut *mut c_void,
    ) -> c_int;
    fn ALooper_wake(looper: *mut c_void);

    fn AInputQueue_attachLooper(
        queue: *mut c_void,
        looper: *mut c_void,
        ident: c_int,
        callback: *const c_void,
        data: *mut c_void,
    );
    fn AInputQueue_detachLooper(queue: *mut c_void);
    fn AInputQueue_getEvent(queue: *mut c_void, out_event: *mut *mut c_void) -> i32;
    fn AInputQueue_preDispatchEvent(queue: *mut c_void, event: *mut c_void) -> i32;
    fn AInputQueue_finishEvent(queue: *mut c_void, event: *mut c_void, handled: c_int);

    fn AInputEvent_getType(event: *const c_void) -> i32;
    fn AInputEvent_getSource(event: *const c_void) -> i32;
    fn AKeyEvent_getAction(event: *const c_void) -> i32;
    fn AKeyEvent_getKeyCode(event: *const c_void) -> i32;
    fn AKeyEvent_getUnicodeChar(event: *const c_void) -> i32;
    fn AMotionEvent_getAction(event: *const c_void) -> i32;
    fn AMotionEvent_getPointerCount(event: *const c_void) -> usize;
    fn AMotionEvent_getPointerId(event: *const c_void, index: usize) -> i32;
    fn AMotionEvent_getX(event: *const c_void, index: usize) -> f32;
    fn AMotionEvent_getY(event: *const c_void, index: usize) -> f32;
    fn AMotionEvent_getAxisValue(event: *const c_void, axis: i32, pointer_index: usize) -> f32;
}

const WINDOW_FORMAT_RGBA_8888: i32 = 1;
const ALOOPER_PREPARE_ALLOW_NON_CALLBACKS: c_int = 1;
const LOOPER_ID_INPUT: c_int = 1;
const INPUT_EVENT_TYPE_KEY: i32 = 1;
const INPUT_EVENT_TYPE_MOTION: i32 = 2;
const KEY_ACTION_DOWN: i32 = 0;
const KEY_ACTION_UP: i32 = 1;
const MOTION_ACTION_MASK: i32 = 0xFF;
const MOTION_ACTION_POINTER_INDEX_MASK: i32 = 0xFF00;
const MOTION_ACTION_POINTER_INDEX_SHIFT: i32 = 8;
const MOTION_ACTION_UP: i32 = 1;
const MOTION_ACTION_MOVE: i32 = 2;
const MOTION_ACTION_CANCEL: i32 = 3;
const MOTION_ACTION_POINTER_UP: i32 = 6;
const INPUT_SOURCE_CLASS_MASK: u32 = 0x0000_00FF;
const INPUT_SOURCE_CLASS_JOYSTICK: u32 = 0x0000_0010;
const INPUT_SOURCE_GAMEPAD: u32 = 0x0000_0401;
const INPUT_SOURCE_DPAD: u32 = 0x0000_0201;
const MOTION_AXIS_X: i32 = 0;
const MOTION_AXIS_Y: i32 = 1;
const MOTION_AXIS_LTRIGGER: i32 = 17;
const MOTION_AXIS_RTRIGGER: i32 = 18;
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

/// A raw pointer owned by the Android framework, stored as an address so it
/// can live in the shared state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Raw(usize);

impl Raw {
    fn new(pointer: *mut c_void) -> Self {
        Self(pointer as usize)
    }

    fn get(self) -> *mut c_void {
        self.0 as *mut c_void
    }

    fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// State written by the UI-thread callbacks and consumed by the app thread.
#[derive(Debug, Default)]
struct Shared {
    window: Raw,
    queue: Raw,
    resumed: bool,
    destroyed: bool,
    looper: Raw,
}

type JniEnv = *mut c_void;
type JniObject = *mut c_void;
type JniMethod = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
union JniValue {
    int_value: i32,
    object_value: JniObject,
}

fn jni_function<F: Copy>(handle: *mut c_void, index: usize) -> Option<F> {
    if handle.is_null() {
        return None;
    }
    // JNIEnv and JavaVM are pointers to pointers to their respective function tables.
    let table = unsafe { *(handle as *const *const *const c_void) };
    if table.is_null() {
        return None;
    }
    let address = unsafe { *table.add(index) };
    if address.is_null() {
        return None;
    }
    assert_eq!(size_of::<F>(), size_of::<*const c_void>());
    Some(unsafe { std::mem::transmute_copy::<*const c_void, F>(&address) })
}

type NewGlobalRefFn = unsafe extern "system" fn(JniEnv, JniObject) -> JniObject;
type DeleteGlobalRefFn = unsafe extern "system" fn(JniEnv, JniObject);
type AttachCurrentThreadFn =
    unsafe extern "system" fn(*mut c_void, *mut JniEnv, *mut c_void) -> i32;
type DetachCurrentThreadFn = unsafe extern "system" fn(*mut c_void) -> i32;

#[repr(C)]
struct JavaVmAttachArgs {
    version: c_int,
    name: *const c_char,
    group: JniObject,
}
type GetObjectClassFn = unsafe extern "system" fn(JniEnv, JniObject) -> JniObject;
type GetMethodIdFn =
    unsafe extern "system" fn(JniEnv, JniObject, *const c_char, *const c_char) -> JniMethod;
type NewStringUtfFn = unsafe extern "system" fn(JniEnv, *const c_char) -> JniObject;
type CallObjectMethodAFn =
    unsafe extern "system" fn(JniEnv, JniObject, JniMethod, *const JniValue) -> JniObject;
type CallIntMethodAFn =
    unsafe extern "system" fn(JniEnv, JniObject, JniMethod, *const JniValue) -> i32;
type CallVoidMethodAFn = unsafe extern "system" fn(JniEnv, JniObject, JniMethod, *const JniValue);
type DeleteLocalRefFn = unsafe extern "system" fn(JniEnv, JniObject);
type ExceptionCheckFn = unsafe extern "system" fn(JniEnv) -> u8;
type ExceptionClearFn = unsafe extern "system" fn(JniEnv);

/// Owns a JavaVM attachment on the app thread and polls anonymous playback
/// attributes. No notification-listener permission or track metadata is read.
struct AttachedMediaMonitor {
    java_vm: *mut c_void,
    env: JniEnv,
    activity: JniObject,
    detach_current_thread: DetachCurrentThreadFn,
}

impl AttachedMediaMonitor {
    fn attach(java_vm: Raw, activity: Raw, _sdk_version: i32) -> Option<Self> {
        // Keep JNI attachment available on the app thread for volume routing.
        // External-media queries are only made when AAudio exposes USAGE_GAME
        // (API 28+), so older Android versions never call that newer API.
        if java_vm.is_null() || activity.is_null() {
            return None;
        }
        let vm = java_vm.get();
        let attach: AttachCurrentThreadFn = jni_function(vm, 4)?;
        let detach: DetachCurrentThreadFn = jni_function(vm, 5)?;
        let mut env: JniEnv = ptr::null_mut();
        let mut attach_args = JavaVmAttachArgs {
            version: 0x0001_0006,
            name: b"aceris-android-app\0".as_ptr().cast(),
            group: ptr::null_mut(),
        };
        // Match the named JavaVMAttachArgs pattern used by PPSSPP. This makes
        // the native worker's JNI lifetime explicit and identifiable in dumps.
        // SAFETY: this worker thread is not attached yet; the VM owns the returned JNIEnv.
        if unsafe {
            attach(
                vm,
                &mut env,
                (&mut attach_args as *mut JavaVmAttachArgs).cast(),
            )
        } != 0
            || env.is_null()
        {
            return None;
        }
        Some(Self {
            java_vm: vm,
            env,
            activity: activity.get(),
            detach_current_thread: detach,
        })
    }

    fn exception_pending(&self) -> bool {
        let Some(check): Option<ExceptionCheckFn> = jni_function(self.env, 228) else {
            return false;
        };
        // SAFETY: self.env belongs to this attached thread.
        unsafe { check(self.env) != 0 }
    }

    fn clear_exception(&self) {
        let clear: Option<ExceptionClearFn> = jni_function(self.env, 17);
        if let Some(clear) = clear {
            // SAFETY: clearing a pending JNI exception on this attached thread.
            unsafe { clear(self.env) };
        }
    }

    fn delete_local(&self, object: JniObject) {
        if !object.is_null() {
            let delete: Option<DeleteLocalRefFn> = jni_function(self.env, 23);
            if let Some(delete) = delete {
                // SAFETY: object is a local reference created during this JNI call.
                unsafe { delete(self.env, object) };
            }
        }
    }

    fn method(
        &self,
        class: JniObject,
        name: &'static [u8],
        signature: &'static [u8],
    ) -> Option<JniMethod> {
        let get: GetMethodIdFn = jni_function(self.env, 33)?;
        // SAFETY: both static byte strings are NUL-terminated JNI method descriptors.
        let method = unsafe {
            get(
                self.env,
                class,
                name.as_ptr().cast(),
                signature.as_ptr().cast(),
            )
        };
        if method.is_null() || self.exception_pending() {
            self.clear_exception();
            None
        } else {
            Some(method)
        }
    }

    fn class_of(&self, object: JniObject) -> Option<JniObject> {
        let get: GetObjectClassFn = jni_function(self.env, 31)?;
        // SAFETY: object is a live local or global reference on this thread.
        let class = unsafe { get(self.env, object) };
        if class.is_null() || self.exception_pending() {
            self.clear_exception();
            None
        } else {
            Some(class)
        }
    }

    /// Associates the physical volume rocker with game output after the first
    /// game frame, rather than calling into Java during Activity startup.
    fn set_volume_control_stream(&self) {
        const STREAM_MUSIC: i32 = 3;
        let Some(activity_class) = self.class_of(self.activity) else {
            return;
        };
        let Some(method) = self.method(activity_class, b"setVolumeControlStream\0", b"(I)V\0")
        else {
            self.delete_local(activity_class);
            return;
        };
        let call: Option<CallVoidMethodAFn> = jni_function(self.env, 63);
        if let Some(call) = call {
            let args = [JniValue {
                int_value: STREAM_MUSIC,
            }];
            // SAFETY: method and argument match Activity.setVolumeControlStream(int).
            unsafe { call(self.env, self.activity, method, args.as_ptr()) };
            if self.exception_pending() {
                self.clear_exception();
            }
        }
        self.delete_local(activity_class);
    }

    /// Returns true when an active external playback session has USAGE_MEDIA.
    fn external_media_active(&self) -> Option<bool> {
        const AUDIO_USAGE_MEDIA: i32 = 1;
        let get_string: NewStringUtfFn = jni_function(self.env, 167)?;
        let call_object: CallObjectMethodAFn = jni_function(self.env, 36)?;
        let call_int: CallIntMethodAFn = jni_function(self.env, 51)?;

        let activity_class = self.class_of(self.activity)?;
        let get_system_service = self.method(
            activity_class,
            b"getSystemService\0",
            b"(Ljava/lang/String;)Ljava/lang/Object;\0",
        )?;
        // SAFETY: the service name is static NUL-terminated UTF-8.
        let service_name = unsafe { get_string(self.env, b"audio\0".as_ptr().cast()) };
        if service_name.is_null() {
            self.delete_local(activity_class);
            self.clear_exception();
            return None;
        }
        let service_args = [JniValue {
            object_value: service_name,
        }];
        // SAFETY: method ID and arguments match Context.getSystemService(String).
        let audio_manager = unsafe {
            call_object(
                self.env,
                self.activity,
                get_system_service,
                service_args.as_ptr(),
            )
        };
        self.delete_local(service_name);
        self.delete_local(activity_class);
        if audio_manager.is_null() || self.exception_pending() {
            self.clear_exception();
            return None;
        }

        let manager_class = self.class_of(audio_manager)?;
        let get_active = self.method(
            manager_class,
            b"getActivePlaybackConfigurations\0",
            b"()Ljava/util/List;\0",
        )?;
        // SAFETY: method ID is obtained from the AudioManager object's class.
        let configurations =
            unsafe { call_object(self.env, audio_manager, get_active, ptr::null()) };
        self.delete_local(manager_class);
        self.delete_local(audio_manager);
        if configurations.is_null() || self.exception_pending() {
            self.clear_exception();
            return None;
        }

        let list_class = self.class_of(configurations)?;
        let size_method = self.method(list_class, b"size\0", b"()I\0")?;
        let get_method = self.method(list_class, b"get\0", b"(I)Ljava/lang/Object;\0")?;
        // SAFETY: List.size() takes no arguments and returns an int.
        let size = unsafe { call_int(self.env, configurations, size_method, ptr::null()) };
        if self.exception_pending() {
            self.clear_exception();
            self.delete_local(list_class);
            self.delete_local(configurations);
            return None;
        }

        let mut found = false;
        for index in 0..size.clamp(0, 64) {
            let args = [JniValue { int_value: index }];
            // SAFETY: List.get(int) returns a local object reference for this index.
            let configuration =
                unsafe { call_object(self.env, configurations, get_method, args.as_ptr()) };
            if configuration.is_null() || self.exception_pending() {
                self.clear_exception();
                continue;
            }
            let Some(configuration_class) = self.class_of(configuration) else {
                self.delete_local(configuration);
                continue;
            };
            let Some(attributes_method) = self.method(
                configuration_class,
                b"getAudioAttributes\0",
                b"()Landroid/media/AudioAttributes;\0",
            ) else {
                self.delete_local(configuration_class);
                self.delete_local(configuration);
                continue;
            };
            // SAFETY: method ID matches AudioPlaybackConfiguration.getAudioAttributes().
            let attributes =
                unsafe { call_object(self.env, configuration, attributes_method, ptr::null()) };
            self.delete_local(configuration_class);
            self.delete_local(configuration);
            if attributes.is_null() || self.exception_pending() {
                self.clear_exception();
                continue;
            }

            let Some(attributes_class) = self.class_of(attributes) else {
                self.delete_local(attributes);
                continue;
            };
            let Some(usage_method) = self.method(attributes_class, b"getUsage\0", b"()I\0") else {
                self.delete_local(attributes_class);
                self.delete_local(attributes);
                continue;
            };
            // SAFETY: AudioAttributes.getUsage() returns an int.
            let usage = unsafe { call_int(self.env, attributes, usage_method, ptr::null()) };
            self.delete_local(attributes_class);
            self.delete_local(attributes);
            if self.exception_pending() {
                self.clear_exception();
                continue;
            }
            if usage == AUDIO_USAGE_MEDIA {
                found = true;
                break;
            }
        }
        self.delete_local(list_class);
        self.delete_local(configurations);
        Some(found)
    }
}

impl Drop for AttachedMediaMonitor {
    fn drop(&mut self) {
        // SAFETY: this is the same thread that was attached in attach().
        unsafe { (self.detach_current_thread)(self.java_vm) };
    }
}

fn create_activity_global_ref(env: *mut c_void, activity: *mut c_void) -> Raw {
    let Some(new_global): Option<NewGlobalRefFn> = jni_function(env, 21) else {
        return Raw::default();
    };
    // SAFETY: called on NativeActivity's UI thread with a live activity jobject.
    let global = unsafe { new_global(env, activity) };
    Raw::new(global)
}

struct Host {
    shared: Mutex<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    java_vm: Raw,
    activity_global: Raw,
    sdk_version: i32,
}

static HOST: Mutex<Option<std::sync::Arc<Host>>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn host() -> Option<std::sync::Arc<Host>> {
    lock(&HOST).clone()
}

fn update_shared(change: impl FnOnce(&mut Shared)) {
    if let Some(host) = host() {
        let looper = {
            let mut shared = lock(&host.shared);
            change(&mut shared);
            shared.looper
        };
        if !looper.is_null() {
            // SAFETY: the looper belongs to the live app thread; waking is thread-safe.
            unsafe { ALooper_wake(looper.get()) };
        }
    }
}

/// Entry point the Android runtime calls when the activity is created.
///
/// # Safety
/// Called by the framework with a valid `ANativeActivity` that outlives the
/// activity's callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ANativeActivity_onCreate(
    activity: *mut NativeActivity,
    _saved_state: *mut c_void,
    _saved_state_size: usize,
) {
    // SAFETY: `activity` and its callback table are valid for the activity's lifetime.
    unsafe {
        let callbacks = &mut *(*activity).callbacks;
        callbacks.on_resume = Some(on_resume);
        callbacks.on_pause = Some(on_pause);
        callbacks.on_destroy = Some(on_destroy);
        callbacks.on_native_window_created = Some(on_window_created);
        callbacks.on_native_window_destroyed = Some(on_window_destroyed);
        callbacks.on_input_queue_created = Some(on_queue_created);
        callbacks.on_input_queue_destroyed = Some(on_queue_destroyed);

        eprintln!("Aceris Android startup: NativeActivity created");
        let games_dir = games_directory(&*activity);
        let host = std::sync::Arc::new(Host {
            shared: Mutex::new(Shared::default()),
            thread: Mutex::new(None),
            java_vm: Raw::new((*activity).vm),
            activity_global: create_activity_global_ref((*activity).env, (*activity).clazz),
            sdk_version: (*activity).sdk_version,
        });
        *lock(&HOST) = Some(host.clone());
        let worker = host.clone();
        let handle = std::thread::spawn(move || run_app_thread(&worker, games_dir));
        *lock(&host.thread) = Some(handle);
    }
}

/// `<external files>/games`, falling back to the internal data directory.
///
/// # Safety
/// The activity's path pointers must be null or valid C strings.
unsafe fn games_directory(activity: &NativeActivity) -> PathBuf {
    let from = |pointer: *const c_char| {
        if pointer.is_null() {
            return None;
        }
        // SAFETY: non-null pointers from the framework are NUL-terminated.
        let text = unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned();
        (!text.is_empty()).then(|| PathBuf::from(text))
    };
    let base = from(activity.external_data_path)
        .or_else(|| from(activity.internal_data_path))
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp/aceris"));
    let games = base.join("games");
    // Best effort: the library simply shows an empty list if this fails.
    let _ = std::fs::create_dir_all(&games);
    games
}

/// Enables Android host timing logs when
/// `.aceris/android_performance_log_v1.txt` contains exactly `1`.
/// This is opt-in to avoid timing overhead in normal gameplay.
fn performance_logging_enabled(games_dir: &Path) -> bool {
    std::fs::read_to_string(
        games_dir
            .join(".aceris")
            .join("android_performance_log_v1.txt"),
    )
    .is_ok_and(|value| value.trim() == "1")
}

unsafe extern "C" fn on_resume(_activity: *mut NativeActivity) {
    update_shared(|shared| shared.resumed = true);
}

unsafe extern "C" fn on_pause(_activity: *mut NativeActivity) {
    update_shared(|shared| shared.resumed = false);
}

unsafe extern "C" fn on_destroy(activity: *mut NativeActivity) {
    update_shared(|shared| shared.destroyed = true);
    let host = lock(&HOST).take();
    if let Some(host) = host {
        let handle = lock(&host.thread).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        // The Activity owns this global reference; release it on the callback
        // thread after the worker detached, even when the monitor was unavailable.
        if !host.activity_global.is_null() {
            let env = unsafe { (*activity).env };
            let delete_global: Option<DeleteGlobalRefFn> = jni_function(env, 22);
            if let Some(delete_global) = delete_global {
                // SAFETY: the app thread has joined, so no monitor call can use this reference.
                unsafe { delete_global(env, host.activity_global.get()) };
            }
        }
    }
}

unsafe extern "C" fn on_window_created(_activity: *mut NativeActivity, window: *mut c_void) {
    // SAFETY: the window is valid until `on_window_destroyed`; the extra
    // reference keeps it alive until the app thread has dropped it.
    unsafe { ANativeWindow_acquire(window) };
    update_shared(|shared| {
        let stale = std::mem::replace(&mut shared.window, Raw::new(window));
        if !stale.is_null() {
            // SAFETY: balances the acquire taken for the replaced window.
            unsafe { ANativeWindow_release(stale.get()) };
        }
    });
}

unsafe extern "C" fn on_window_destroyed(_activity: *mut NativeActivity, window: *mut c_void) {
    update_shared(|shared| {
        if shared.window == Raw::new(window) {
            shared.window = Raw::default();
        }
    });
    // SAFETY: balances the acquire in `on_window_created`. Rendering takes the
    // shared lock, so no draw is in flight once `update_shared` returned.
    unsafe { ANativeWindow_release(window) };
}

unsafe extern "C" fn on_queue_created(_activity: *mut NativeActivity, queue: *mut c_void) {
    update_shared(|shared| shared.queue = Raw::new(queue));
}

unsafe extern "C" fn on_queue_destroyed(_activity: *mut NativeActivity, queue: *mut c_void) {
    update_shared(|shared| {
        if shared.queue == Raw::new(queue) {
            shared.queue = Raw::default();
        }
    });
}

/// Owns the extra native-window reference required by the GPU surface.
struct AndroidGpuPresenter {
    presenter: Option<GraphicsPresenter>,
    window: NonNull<c_void>,
}

impl Drop for AndroidGpuPresenter {
    fn drop(&mut self) {
        // Destroy the WGPU surface before releasing the ANativeWindow reference.
        self.presenter.take();
        // SAFETY: create_android_gpu_presenter acquired this reference once.
        unsafe { ANativeWindow_release(self.window.as_ptr()) };
    }
}

/// Chooses an Android GPU backend with a persisted crash-recovery sequence.
///
/// PPSSPP selects a platform-appropriate backend and remembers backend startup
/// failures across process launches. Aceris uses an original Rust implementation
/// of the same recovery principle: Vulkan -> OpenGL ES -> software presentation.
fn android_backend_name(backend: GraphicsBackend) -> &'static str {
    match backend {
        GraphicsBackend::Auto => "auto",
        GraphicsBackend::OpenGl => "opengl",
        GraphicsBackend::Vulkan => "vulkan",
        GraphicsBackend::Software => "software",
    }
}

fn fallback_android_backend_after_failure(failed: &str) -> GraphicsBackend {
    match failed.trim() {
        "vulkan" | "auto" => GraphicsBackend::OpenGl,
        "opengl" => GraphicsBackend::Software,
        _ => GraphicsBackend::Software,
    }
}

fn preferred_android_backend(sdk_version: i32) -> GraphicsBackend {
    // PPSSPP defaults to Vulkan on Android 8.1+ for 64-bit builds and uses
    // OpenGL on older/32-bit devices. Aceris APKs currently target ARM64.
    #[cfg(target_arch = "aarch64")]
    let vulkan_default = sdk_version >= 27;
    #[cfg(not(target_arch = "aarch64"))]
    let vulkan_default = sdk_version >= 30;
    if vulkan_default {
        GraphicsBackend::Vulkan
    } else {
        GraphicsBackend::OpenGl
    }
}

fn select_android_backend(sdk_version: i32, recovery_file: &Path) -> GraphicsBackend {
    if let Ok(failed) = std::fs::read_to_string(recovery_file) {
        match failed.trim() {
            "vulkan" => {
                let fallback = fallback_android_backend_after_failure("vulkan");
                let _ = std::fs::write(recovery_file, android_backend_name(fallback));
                eprintln!(
                    "Aceris Android startup: previous Vulkan initialization did not complete; retrying with OpenGL ES"
                );
                return fallback;
            }
            "opengl" => {
                let fallback = fallback_android_backend_after_failure("opengl");
                let _ = std::fs::write(recovery_file, android_backend_name(fallback));
                eprintln!(
                    "Aceris Android startup: previous OpenGL ES initialization did not complete; using software presentation"
                );
                return fallback;
            }
            "software" => return GraphicsBackend::Software,
            _ => {
                let _ = std::fs::remove_file(recovery_file);
            }
        }
    }

    match GraphicsBackend::from_env() {
        Ok(GraphicsBackend::Auto) => preferred_android_backend(sdk_version),
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("GPU presentation configuration ignored: {error}");
            preferred_android_backend(sdk_version)
        }
    }
}

/// Attempts a native Vulkan or OpenGL ES presenter for the current ANativeWindow.
fn create_android_gpu_presenter(
    window: *mut c_void,
    width: u32,
    height: u32,
    sdk_version: i32,
    cache_directory: &Path,
    recovery_file: &Path,
) -> Option<AndroidGpuPresenter> {
    let backend = select_android_backend(sdk_version, recovery_file);
    if backend == GraphicsBackend::Software {
        return None;
    }
    if let Some(parent) = recovery_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Written before touching the driver. If the native driver terminates the
    // process during adapter/device/surface creation, the next launch chooses
    // the next backend instead of repeating the same crash indefinitely.
    if let Err(error) = std::fs::write(recovery_file, android_backend_name(backend)) {
        eprintln!("Could not persist Android GPU recovery state: {error}");
    }
    eprintln!(
        "Aceris Android startup: initializing {:?} presentation",
        backend
    );
    let window = NonNull::new(window)?;
    // SAFETY: this is called while the shared native-window lock is held.
    unsafe { ANativeWindow_acquire(window.as_ptr()) };

    let display = RawDisplayHandle::Android(AndroidDisplayHandle::new());
    let native_window = RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(window));
    // SAFETY: this object owns an extra ANativeWindow reference, released only
    // after its WGPU surface has been destroyed by Drop.
    match unsafe {
        GraphicsPresenter::new_with_cache_directory(
            Some(display),
            native_window,
            width,
            height,
            backend,
            Some(cache_directory),
        )
    } {
        Ok(presenter) => {
            let info = presenter.adapter_info();
            eprintln!(
                "Aceris Android GPU presentation: {} ({}, {})",
                info.name, info.backend, info.device_type
            );
            Some(AndroidGpuPresenter {
                presenter: Some(presenter),
                window,
            })
        }
        Err(error) => {
            eprintln!("Android GPU presentation unavailable; using CPU fallback: {error}");
            // SAFETY: balances the acquire above after surface creation failed.
            unsafe { ANativeWindow_release(window.as_ptr()) };
            None
        }
    }
}

/// The app thread: owns the emulator, input-queue attachment and rendering.
fn run_app_thread(host: &Host, games_dir: PathBuf) {
    // SAFETY: preparing a looper for this thread; it lives as long as the thread.
    let looper = unsafe { ALooper_prepare(ALOOPER_PREPARE_ALLOW_NON_CALLBACKS) };
    lock(&host.shared).looper = Raw::new(looper);

    eprintln!("Aceris Android startup: creating application state");
    let mut app = App::new(&games_dir);
    eprintln!("Aceris Android startup: application state ready; showing intro/library");
    let settings_directory = games_dir.join(".aceris");
    let gpu_pipeline_cache_directory = settings_directory.join("gpu_pipeline_cache");
    let gpu_backend_recovery_file = settings_directory.join("android_gpu_backend_recovery_v1.txt");
    let mut frame = Vec::new();
    let mut performance_monitor =
        performance_logging_enabled(&games_dir).then(FramePerformanceMonitor::new);
    if performance_monitor.is_some() {
        eprintln!("Aceris Android performance logging enabled (5-second reports)");
    }
    // Audio is optional and is not needed by the launcher. Open it only after
    // a game has produced its first presented frame, like PPSSPP's post-graphics
    // initialization, so audio-device startup cannot block the Aceris interface.
    let mut audio: Option<AAudioOutput> = None;
    let mut audio_initialization_attempted = false;
    let mut media_monitor: Option<AttachedMediaMonitor> = None;
    app.set_external_media_detection_available(false);
    let mut external_media_playing = false;
    let mut last_media_poll = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let mut audio_pause_state = None;
    let mut last_audio_pause_error: Option<String> = None;
    let mut attached_queue = Raw::default();
    // Reuse touch-event storage across motion callbacks instead of allocating
    // a new pointer Vec for every movement event.
    let mut pointer_scratch = Vec::with_capacity(10);
    let mut size = (0u32, 0u32);
    let mut gpu_presenter: Option<AndroidGpuPresenter> = None;
    let mut gpu_window = Raw::default();
    let mut attempted_gpu_window = Raw::default();
    let mut cpu_geometry_size = (0u32, 0u32);
    let mut next_frame = Instant::now();

    loop {
        let (window, queue, resumed, destroyed) = {
            let shared = lock(&host.shared);
            (
                shared.window,
                shared.queue,
                shared.resumed,
                shared.destroyed,
            )
        };
        if destroyed {
            gpu_presenter.take();
            break;
        }

        // Follow the input queue the UI thread published.
        if queue != attached_queue {
            // SAFETY: the queue pointers come from the framework; detaching
            // happens on this thread before a new queue is attached.
            unsafe {
                if !attached_queue.is_null() {
                    AInputQueue_detachLooper(attached_queue.get());
                }
                if !queue.is_null() {
                    AInputQueue_attachLooper(
                        queue.get(),
                        looper,
                        LOOPER_ID_INPUT,
                        ptr::null(),
                        ptr::null_mut(),
                    );
                }
            }
            attached_queue = queue;
        }

        // Frame pacing doubles as the looper wait, so input wakes us early.
        let wait = if resumed && !window.is_null() {
            next_frame.saturating_duration_since(Instant::now())
        } else {
            Duration::from_millis(250)
        };
        poll_looper(wait, &mut app, attached_queue, &mut pointer_scratch);

        // Pause also when the Activity is backgrounded, so queued game audio
        // cannot continue playing behind another app or after the screen closes.
        let audio_should_pause =
            !resumed || app.is_game_menu_open() || !matches!(app.screen(), Screen::Game);
        if audio_pause_state != Some(audio_should_pause) {
            if let Some(audio) = audio.as_mut() {
                match audio.set_paused(audio_should_pause) {
                    Ok(()) => {
                        audio_pause_state = Some(audio_should_pause);
                        last_audio_pause_error = None;
                    }
                    Err(message) => {
                        if last_audio_pause_error.as_deref() != Some(message.as_str()) {
                            eprintln!("Android audio pause/flush failed: {message}");
                            last_audio_pause_error = Some(message);
                        }
                    }
                }
            } else {
                audio_pause_state = Some(audio_should_pause);
            }
        }

        if resumed && !window.is_null() && Instant::now() >= next_frame {
            let now = Instant::now();
            if now.duration_since(last_media_poll) >= Duration::from_millis(500) {
                last_media_poll = now;
                let output_is_classified_as_game =
                    audio.as_ref().is_some_and(AAudioOutput::uses_game_usage);
                let detected = if output_is_classified_as_game {
                    media_monitor
                        .as_ref()
                        .and_then(AttachedMediaMonitor::external_media_active)
                        .unwrap_or(false)
                } else {
                    false
                };
                if detected != external_media_playing {
                    external_media_playing = detected;
                    app.set_external_media_playing(detected);
                    if let Some(audio) = audio.as_mut() {
                        audio.set_muted(detected);
                    }
                }
            }
            next_frame += FRAME_INTERVAL;
            if next_frame < Instant::now() {
                // Fell behind (paused or slow frame): do not run catch-up frames.
                next_frame = Instant::now() + FRAME_INTERVAL;
            }
            let frame_presented = render(
                host,
                &mut app,
                &mut frame,
                &mut size,
                &mut gpu_presenter,
                &mut gpu_window,
                &mut attempted_gpu_window,
                &mut cpu_geometry_size,
                &mut performance_monitor,
                &gpu_pipeline_cache_directory,
                host.sdk_version,
                &gpu_backend_recovery_file,
            );
            if frame_presented
                && !audio_initialization_attempted
                && matches!(app.screen(), Screen::Game)
            {
                audio_initialization_attempted = true;
                eprintln!("Aceris Android: opening optional AAudio output after first game frame");
                audio = AAudioOutput::open();
                if audio.is_none() {
                    eprintln!(
                        "Aceris Android: AAudio unavailable; game audio remains disabled, UI/emulation continue"
                    );
                } else {
                    eprintln!(
                        "Aceris Android: attaching optional JNI helpers after audio initialization"
                    );
                    media_monitor = AttachedMediaMonitor::attach(
                        host.java_vm,
                        host.activity_global,
                        host.sdk_version,
                    );
                    if let Some(monitor) = media_monitor.as_ref() {
                        monitor.set_volume_control_stream();
                    }
                    let media_detection_available =
                        audio.as_ref().is_some_and(AAudioOutput::uses_game_usage)
                            && media_monitor.is_some();
                    app.set_external_media_detection_available(media_detection_available);
                }
            }
            let samples = app.take_audio();
            if let Some(audio) = audio.as_mut() {
                audio.feed(&samples);
            }
            app.recycle_audio_buffer(samples);
        }
    }

    // SAFETY: final detach on the thread that attached.
    unsafe {
        if !attached_queue.is_null() {
            AInputQueue_detachLooper(attached_queue.get());
        }
    }
    lock(&host.shared).looper = Raw::default();
}

/// Waits up to `timeout` for looper events and drains pending input.
fn poll_looper(timeout: Duration, app: &mut App, queue: Raw, pointer_scratch: &mut Vec<Pointer>) {
    let mut first = true;
    loop {
        let millis = if first {
            c_int::try_from(timeout.as_millis()).unwrap_or(c_int::MAX)
        } else {
            0
        };
        first = false;
        let mut fd = 0;
        let mut events = 0;
        let mut data = ptr::null_mut();
        // SAFETY: out-pointers are valid locals; the looper was prepared on this thread.
        let result = unsafe { ALooper_pollOnce(millis, &mut fd, &mut events, &mut data) };
        if result < 0 {
            // Timeout, wake or error: nothing more to drain.
            break;
        }
        if result == LOOPER_ID_INPUT && !queue.is_null() {
            drain_input(queue, app, pointer_scratch);
        }
    }
}

fn drain_input(queue: Raw, app: &mut App, pointer_scratch: &mut Vec<Pointer>) {
    loop {
        let mut event = ptr::null_mut();
        // SAFETY: the queue stays valid while attached on this thread.
        let status = unsafe { AInputQueue_getEvent(queue.get(), &mut event) };
        if status < 0 || event.is_null() {
            break;
        }
        // SAFETY: `event` was just returned by the queue and is finished exactly once.
        unsafe {
            if AInputQueue_preDispatchEvent(queue.get(), event) != 0 {
                continue;
            }
            let handled = match AInputEvent_getType(event) {
                INPUT_EVENT_TYPE_KEY => handle_key(event, app),
                INPUT_EVENT_TYPE_MOTION => {
                    handle_motion(event, app, pointer_scratch);
                    true
                }
                _ => false,
            };
            AInputQueue_finishEvent(queue.get(), event, c_int::from(handled));
        }
    }
}

/// # Safety
/// `event` must be a live key event.
unsafe fn handle_key(event: *mut c_void, app: &mut App) -> bool {
    // SAFETY: caller guarantees a key event.
    let (source, action, code, unicode) = unsafe {
        (
            AInputEvent_getSource(event as *const c_void) as u32,
            AKeyEvent_getAction(event),
            AKeyEvent_getKeyCode(event),
            AKeyEvent_getUnicodeChar(event),
        )
    };
    let gamepad_source = is_gamepad_key_source(source);
    match action {
        KEY_ACTION_DOWN => {
            let mut handled = if gamepad_source {
                app.gamepad_key(code, true)
            } else {
                app.key(code, true)
            };
            if unicode > 0
                && let Some(character) = char::from_u32(unicode as u32)
                && !character.is_control()
            {
                handled |= app.text_input(character);
            }
            handled
        }
        KEY_ACTION_UP => {
            if gamepad_source {
                app.gamepad_key(code, false)
            } else {
                app.key(code, false)
            }
        }
        _ => false,
    }
}

fn is_gamepad_key_source(source: u32) -> bool {
    source & INPUT_SOURCE_GAMEPAD == INPUT_SOURCE_GAMEPAD
        || source & INPUT_SOURCE_DPAD == INPUT_SOURCE_DPAD
}

fn is_joystick_source(source: u32) -> bool {
    source & INPUT_SOURCE_CLASS_MASK == INPUT_SOURCE_CLASS_JOYSTICK
}

/// # Safety
/// `event` must be a live motion event.
unsafe fn handle_motion(event: *mut c_void, app: &mut App, pointers: &mut Vec<Pointer>) {
    // SAFETY: caller guarantees a motion event; indices stay below the pointer count.
    let (source, action, count) = unsafe {
        (
            AInputEvent_getSource(event as *const c_void) as u32,
            AMotionEvent_getAction(event),
            AMotionEvent_getPointerCount(event),
        )
    };

    // Physical joysticks arrive as motion events, separate from touchscreen
    // pointers. Use the primary stick only; digital gamepad buttons continue
    // through the key-event path above.
    if is_joystick_source(source) {
        pointers.clear();
        match action & MOTION_ACTION_MASK {
            MOTION_ACTION_MOVE => {
                // SAFETY: joystick motion events expose at least one axis sample.
                let (x, y, left_trigger, right_trigger) = unsafe {
                    (
                        AMotionEvent_getAxisValue(event, MOTION_AXIS_X, 0),
                        AMotionEvent_getAxisValue(event, MOTION_AXIS_Y, 0),
                        AMotionEvent_getAxisValue(event, MOTION_AXIS_LTRIGGER, 0),
                        AMotionEvent_getAxisValue(event, MOTION_AXIS_RTRIGGER, 0),
                    )
                };
                app.set_gamepad_axes(x, y);
                app.set_gamepad_triggers(left_trigger, right_trigger);
            }
            MOTION_ACTION_CANCEL => {
                app.set_gamepad_axes(0.0, 0.0);
                app.set_gamepad_triggers(0.0, 0.0);
            }
            _ => {}
        }
        return;
    }

    pointers.clear();
    if action & MOTION_ACTION_MASK == MOTION_ACTION_CANCEL {
        app.set_pointers(&[]);
        return;
    }

    // SAFETY: the pointer indices are bounded by AMotionEvent_getPointerCount.
    unsafe {
        pointers.extend((0..count).map(|index| Pointer {
            id: AMotionEvent_getPointerId(event, index),
            x: AMotionEvent_getX(event, index),
            y: AMotionEvent_getY(event, index),
        }));
    }

    let pointer_index =
        ((action & MOTION_ACTION_POINTER_INDEX_MASK) >> MOTION_ACTION_POINTER_INDEX_SHIFT) as usize;
    match action & MOTION_ACTION_MASK {
        MOTION_ACTION_UP | MOTION_ACTION_POINTER_UP => {
            // The lifted pointer is still listed in the event; remove it in
            // place so this high-frequency path does not allocate a second Vec.
            if pointer_index < pointers.len() {
                pointers.remove(pointer_index);
            }
            app.set_pointers(pointers);
        }
        _ => app.set_pointers(pointers),
    }
}

/// Steps the app and presents its RGBA frame through the GPU when available.
fn render(
    host: &Host,
    app: &mut App,
    frame: &mut Vec<u8>,
    size: &mut (u32, u32),
    gpu_presenter: &mut Option<AndroidGpuPresenter>,
    gpu_window: &mut Raw,
    attempted_gpu_window: &mut Raw,
    cpu_geometry_size: &mut (u32, u32),
    performance_monitor: &mut Option<FramePerformanceMonitor>,
    cache_directory: &Path,
    sdk_version: i32,
    recovery_file: &Path,
) -> bool {
    let frame_started = performance_monitor.as_ref().map(|_| Instant::now());
    // Holding the lock keeps the window alive: `on_window_destroyed` takes it
    // before releasing the reference.
    let shared = lock(&host.shared);
    let window = shared.window;
    if window.is_null() {
        gpu_presenter.take();
        *gpu_window = Raw::default();
        *attempted_gpu_window = Raw::default();
        *size = (0, 0);
        *cpu_geometry_size = (0, 0);
        return false;
    }
    if *gpu_window != window {
        // Rotation, pause/resume, or surface recreation can replace ANativeWindow.
        gpu_presenter.take();
        *gpu_window = window;
        *attempted_gpu_window = Raw::default();
        *size = (0, 0);
        *cpu_geometry_size = (0, 0);
    }

    // SAFETY: `window` is valid while the shared lock is held. The GPU
    // presenter acquires an additional reference before this lock is released.
    unsafe {
        let width = ANativeWindow_getWidth(window.get()).max(0) as u32;
        let height = ANativeWindow_getHeight(window.get()).max(0) as u32;
        if width == 0 || height == 0 {
            return false;
        }
        if (width, height) != *size {
            app.resize(width, height);
            *size = (width, height);
        }
        let emulation_started = performance_monitor.as_ref().map(|_| Instant::now());
        app.step_into(frame);
        let emulation_elapsed = emulation_started
            .map(|started| started.elapsed())
            .unwrap_or_default();
        if frame.is_empty() {
            record_performance_sample(
                performance_monitor,
                app,
                frame_started,
                emulation_elapsed,
                None,
                false,
                false,
            );
            return false;
        }

        let presentation_started = performance_monitor.as_ref().map(|_| Instant::now());

        // The launcher (including the startup animation and library) uses the
        // original ANativeWindow CPU presenter. Host GPU initialization is not
        // required to display the UI, so a device-driver failure must not keep
        // the application from opening. Initialize the optional WGPU presenter
        // only after a guest game is running.
        let game_is_running = matches!(app.screen(), Screen::Game);
        if !game_is_running {
            gpu_presenter.take();
            *attempted_gpu_window = Raw::default();
        } else if gpu_presenter.is_none() && *attempted_gpu_window != window {
            *attempted_gpu_window = window;
            *gpu_presenter = create_android_gpu_presenter(
                window.get(),
                width,
                height,
                sdk_version,
                cache_directory,
                recovery_file,
            );
        }

        let gpu_result = if game_is_running {
            gpu_presenter.as_mut().map(|gpu| {
            gpu.presenter
                .as_mut()
                .expect("Android GPU presenter is present while its owner is alive")
                .present_rgba(frame, width, height)
            })
        } else {
            None
        };
        match gpu_result {
            Some(Ok(())) => {
                let _ = std::fs::remove_file(recovery_file);
                record_performance_sample(
                    performance_monitor,
                    app,
                    frame_started,
                    emulation_elapsed,
                    presentation_started,
                    true,
                    true,
                );
                return true;
            }
            Some(Err(error)) => {
                eprintln!("Android GPU surface lost; switching to CPU presentation: {error}");
                gpu_presenter.take();
            }
            None => {}
        }

        // The original RGBA ANativeWindow path remains the fallback.
        if *cpu_geometry_size != (width, height) {
            if ANativeWindow_setBuffersGeometry(window.get(), 0, 0, WINDOW_FORMAT_RGBA_8888) != 0 {
                eprintln!("ANativeWindow could not select RGBA_8888 fallback buffers");
                record_performance_sample(
                    performance_monitor,
                    app,
                    frame_started,
                    emulation_elapsed,
                    presentation_started,
                    false,
                    false,
                );
                return false;
            }
            *cpu_geometry_size = (width, height);
        }
        let mut buffer = NativeWindowBuffer {
            width: 0,
            height: 0,
            stride: 0,
            format: 0,
            bits: ptr::null_mut(),
            reserved: [0; 6],
        };
        if ANativeWindow_lock(window.get(), &mut buffer, ptr::null_mut()) != 0 {
            record_performance_sample(
                performance_monitor,
                app,
                frame_started,
                emulation_elapsed,
                presentation_started,
                false,
                false,
            );
            return false;
        }
        copy_frame(frame, width, height, &buffer);
        let presented = ANativeWindow_unlockAndPost(window.get()) == 0;
        record_performance_sample(
            performance_monitor,
            app,
            frame_started,
            emulation_elapsed,
            presentation_started,
            presented,
            false,
        );
        presented
    }
}

/// Records timing only when the optional Android diagnostics switch is enabled.
fn record_performance_sample(
    monitor: &mut Option<FramePerformanceMonitor>,
    app: &mut App,
    frame_started: Option<Instant>,
    emulation_elapsed: Duration,
    presentation_started: Option<Instant>,
    presented: bool,
    gpu_presented: bool,
) {
    let (Some(monitor), Some(frame_started)) = (monitor.as_mut(), frame_started) else {
        return;
    };
    let presentation_elapsed = presentation_started
        .map(|started| started.elapsed())
        .unwrap_or_default();
    if let Some(report) = monitor.record(
        frame_started.elapsed(),
        emulation_elapsed,
        presentation_elapsed,
        presented,
        gpu_presented,
    ) {
        let summary = report.to_string();
        app.set_performance_summary(summary.clone());
        eprintln!("Aceris Android performance: {summary}");
    }
}

/// Copies an RGBA frame into a locked window buffer row by row.
///
/// # Safety
/// `buffer.bits` must point to `buffer.height` rows of `buffer.stride` RGBA
/// pixels, as returned by `ANativeWindow_lock`.
unsafe fn copy_frame(frame: &[u8], width: u32, height: u32, buffer: &NativeWindowBuffer) {
    if buffer.bits.is_null() || buffer.stride <= 0 {
        return;
    }
    let rows = height.min(buffer.height.max(0) as u32) as usize;
    let columns = width.min(buffer.width.max(0) as u32) as usize;
    let stride = buffer.stride as usize;
    let stride_bytes = stride * 4;

    // A tightly packed native buffer can be copied with one bulk operation.
    // Keep the row-wise path below for padded or clipped buffers.
    if buffer.width == width as i32
        && buffer.height == height as i32
        && stride == width as usize
        && let Some(byte_len) = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
        && frame.len() >= byte_len
    {
        // SAFETY: the exact-size check guarantees both slices cover byte_len bytes.
        unsafe {
            ptr::copy_nonoverlapping(frame.as_ptr(), buffer.bits.cast::<u8>(), byte_len);
        }
        return;
    }

    for row in 0..rows {
        let source = &frame[row * width as usize * 4..][..columns * 4];
        // SAFETY: each destination row lies inside the locked buffer.
        unsafe {
            let destination = buffer.bits.cast::<u8>().add(row * stride_bytes);
            ptr::copy_nonoverlapping(source.as_ptr(), destination, source.len());
        }
    }
}

#[cfg(test)]
mod gamepad_source_tests {
    use super::*;

    #[test]
    fn android_graphics_recovery_advances_from_vulkan_to_gl_then_software() {
        assert_eq!(
            fallback_android_backend_after_failure("vulkan"),
            GraphicsBackend::OpenGl
        );
        assert_eq!(
            fallback_android_backend_after_failure("opengl"),
            GraphicsBackend::Software
        );
    }

    #[test]
    fn joystick_source_uses_android_source_class_mask() {
        assert!(is_joystick_source(0x0100_0010));
        assert!(is_joystick_source(0x0200_0010));
        assert!(!is_joystick_source(0x0000_0401));
        assert!(!is_joystick_source(0x0000_1002));
    }
}
