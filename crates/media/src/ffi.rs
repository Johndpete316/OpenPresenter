//! Minimal libmpv client + OpenGL render API, loaded at runtime.
//!
//! Struct layouts and constants mirror mpv's `client.h`, `render.h` and
//! `render_gl.h` (client API 2.x). Loading at runtime means a missing
//! `mpv-2.dll` is a recoverable error (slides keep working), and building for
//! Windows needs no import library.
#![allow(unsafe_code)]
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

pub type mpv_handle = c_void;
pub type mpv_render_context = c_void;

pub const MPV_RENDER_PARAM_INVALID: c_int = 0;
pub const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
pub const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;
pub const MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME: c_int = 12;
pub const MPV_RENDER_UPDATE_FRAME: u64 = 1;

pub const MPV_EVENT_SHUTDOWN: c_int = 1;
pub const MPV_EVENT_END_FILE: c_int = 7;
pub const MPV_EVENT_FILE_LOADED: c_int = 8;
pub const MPV_END_FILE_REASON_ERROR: c_int = 4;

#[repr(C)]
pub struct mpv_render_param {
    pub type_: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_init_params {
    pub get_proc_address:
        Option<unsafe extern "C" fn(ctx: *mut c_void, name: *const c_char) -> *mut c_void>,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_fbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

#[allow(clippy::type_complexity)]
pub struct Api {
    _lib: libloading::Library,
    pub client_api_version: unsafe extern "C" fn() -> u64,
    pub create: unsafe extern "C" fn() -> *mut mpv_handle,
    pub initialize: unsafe extern "C" fn(*mut mpv_handle) -> c_int,
    pub terminate_destroy: unsafe extern "C" fn(*mut mpv_handle),
    pub set_option_string:
        unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
    pub set_property_string:
        unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
    pub get_property_string: unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> *mut c_char,
    pub command: unsafe extern "C" fn(*mut mpv_handle, *mut *const c_char) -> c_int,
    pub free: unsafe extern "C" fn(*mut c_void),
    pub error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    pub wait_event: unsafe extern "C" fn(*mut mpv_handle, f64) -> *mut mpv_event,
    pub wakeup: unsafe extern "C" fn(*mut mpv_handle),
    pub render_context_create: unsafe extern "C" fn(
        *mut *mut mpv_render_context,
        *mut mpv_handle,
        *mut mpv_render_param,
    ) -> c_int,
    pub render_context_set_update_callback: unsafe extern "C" fn(
        *mut mpv_render_context,
        Option<unsafe extern "C" fn(*mut c_void)>,
        *mut c_void,
    ),
    pub render_context_update: unsafe extern "C" fn(*mut mpv_render_context) -> u64,
    pub render_context_render:
        unsafe extern "C" fn(*mut mpv_render_context, *mut mpv_render_param) -> c_int,
    pub render_context_free: unsafe extern "C" fn(*mut mpv_render_context),
}

/// Library names tried in order.
const NAMES: &[&str] = if cfg!(windows) {
    &["mpv-2.dll", "libmpv-2.dll", "mpv-1.dll"]
} else if cfg!(target_os = "macos") {
    &["libmpv.2.dylib", "libmpv.dylib"]
} else {
    &["libmpv.so.2", "libmpv.so"]
};

static API: OnceLock<Result<Api, String>> = OnceLock::new();

/// The loaded library, or why it couldn't be loaded.
pub fn api() -> Result<&'static Api, &'static str> {
    API.get_or_init(load).as_ref().map_err(String::as_str)
}

fn load() -> Result<Api, String> {
    let mut errors = Vec::new();
    for name in NAMES {
        // SAFETY: loading libmpv runs its initialisers, which are well-behaved.
        match unsafe { libloading::Library::new(*name) } {
            Ok(lib) => match bind(lib) {
                Ok(api) => {
                    // SAFETY: function pointer from the library we just bound.
                    let v = unsafe { (api.client_api_version)() };
                    if v >> 16 < 2 {
                        errors.push(format!(
                            "{name}: client API {}.{} is too old (need 2.x)",
                            v >> 16,
                            v & 0xffff
                        ));
                        continue;
                    }
                    tracing::info!(
                        library = name,
                        api = format!("{}.{}", v >> 16, v & 0xffff),
                        "libmpv loaded"
                    );
                    return Ok(api);
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            },
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Err(errors.join("; "))
}

fn bind(lib: libloading::Library) -> Result<Api, libloading::Error> {
    // SAFETY: each symbol is looked up by its exact C name and given the signature
    // from mpv's headers.
    unsafe {
        macro_rules! sym {
            ($name:literal) => {
                *lib.get(concat!($name, "\0").as_bytes())?
            };
        }
        Ok(Api {
            client_api_version: sym!("mpv_client_api_version"),
            create: sym!("mpv_create"),
            initialize: sym!("mpv_initialize"),
            terminate_destroy: sym!("mpv_terminate_destroy"),
            set_option_string: sym!("mpv_set_option_string"),
            set_property_string: sym!("mpv_set_property_string"),
            get_property_string: sym!("mpv_get_property_string"),
            command: sym!("mpv_command"),
            free: sym!("mpv_free"),
            error_string: sym!("mpv_error_string"),
            wait_event: sym!("mpv_wait_event"),
            wakeup: sym!("mpv_wakeup"),
            render_context_create: sym!("mpv_render_context_create"),
            render_context_set_update_callback: sym!("mpv_render_context_set_update_callback"),
            render_context_update: sym!("mpv_render_context_update"),
            render_context_render: sym!("mpv_render_context_render"),
            render_context_free: sym!("mpv_render_context_free"),
            _lib: lib,
        })
    }
}
