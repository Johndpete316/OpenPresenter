//! Background media (images and video) via libmpv, rendered into the caller's
//! OpenGL framebuffer with mpv's render API. libmpv is loaded at runtime: if it
//! isn't installed, `Player::new` returns `MediaError::LibraryMissing` and the
//! rest of the app keeps working.
#![allow(unsafe_code)] // FFI wrapper; every unsafe block is commented.

mod ffi;

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MediaError {
    #[error("libmpv isn't available: {0}")]
    LibraryMissing(String),
    #[error("couldn't start the media player: {0}")]
    Create(String),
    #[error("media player option {name}: {message}")]
    Option { name: String, message: String },
    #[error("media command {what} failed: {message}")]
    Command { what: String, message: String },
    #[error("couldn't set up video drawing: {0}")]
    Render(String),
    #[error("{path}: can't be played: {message}", path = .path.display())]
    Playback { path: PathBuf, message: String },
}

impl MediaError {
    /// One plain sentence for the operator, plus what happens next.
    pub fn user_message(&self) -> String {
        match self {
            MediaError::LibraryMissing(_) => {
                "Video and image backgrounds are unavailable because mpv-2.dll isn't next to the program. Slides still work.".into()
            }
            MediaError::Create(_) | MediaError::Option { .. } | MediaError::Render(_) => {
                "The media player couldn't start, so backgrounds are off. Slides still work.".into()
            }
            MediaError::Command { .. } => "The media player didn't accept that command. The background is unchanged.".into(),
            MediaError::Playback { path, .. } => format!(
                "'{}' can't be played (missing or unsupported file). The background is cleared.",
                path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
            ),
        }
    }
}

/// An audio output device as mpv lists it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AudioDevice {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// How background media fills the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scaling {
    /// Cover the whole output, cropping what doesn't fit (no bars).
    #[default]
    Fill,
    /// Show all of the picture; black bars where the shapes differ.
    Fit,
    /// Stretch to the output's shape (distorts).
    Stretch,
}

impl Scaling {
    /// mpv (keepaspect, panscan) for this mode.
    fn mpv(self) -> (&'static str, &'static str) {
        match self {
            Scaling::Fill => ("yes", "1.0"),
            Scaling::Fit => ("yes", "0.0"),
            Scaling::Stretch => ("no", "0.0"),
        }
    }
}

/// Events from a player's event thread.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    Loaded(PathBuf),
    Failed(MediaError),
}

fn mpv_err(api: &ffi::Api, code: c_int) -> String {
    // SAFETY: mpv_error_string returns a static C string for any code.
    unsafe { CStr::from_ptr((api.error_string)(code)) }
        .to_string_lossy()
        .into_owned()
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

struct SendPtr(*mut c_void);
// SAFETY: mpv handles are thread-safe for the client API calls made from the event thread.
unsafe impl Send for SendPtr {}

type GlLoader = Box<dyn Fn(&CStr) -> *const c_void>;
type UpdateCallback = Box<dyn Fn() + Send + Sync>;

/// One mpv instance. Create, `attach_gl` and drop it on the thread that owns the
/// GL context (render calls must happen with that context current).
pub struct Player {
    api: &'static ffi::Api,
    handle: *mut ffi::mpv_handle,
    render: *mut ffi::mpv_render_context,
    gl_loader: Option<Box<GlLoader>>,
    update: Option<Box<UpdateCallback>>,
    stop: Arc<AtomicBool>,
    events: Option<JoinHandle<()>>,
    current: Arc<Mutex<Option<PathBuf>>>,
}

impl Player {
    /// Start an mpv instance for background playback (looping, no on-screen UI).
    /// `on_event` runs on the player's event thread.
    pub fn new(
        audio_device: Option<&str>,
        scaling: Scaling,
        on_event: impl Fn(MediaEvent) + Send + 'static,
    ) -> Result<Player, MediaError> {
        let api = ffi::api().map_err(|e| MediaError::LibraryMissing(e.to_owned()))?;
        // SAFETY: plain constructor; null on failure.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err(MediaError::Create("mpv_create returned null".into()));
        }
        let mut options = vec![
            ("vo", "libmpv"),
            ("idle", "yes"),
            ("keep-open", "yes"),
            ("loop-file", "inf"),
            ("image-display-duration", "inf"),
            ("hwdec", "auto-safe"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("osc", "no"),
            ("terminal", "no"),
            ("keepaspect", scaling.mpv().0),
            ("panscan", scaling.mpv().1),
        ];
        if let Some(d) = audio_device {
            options.push(("audio-device", d));
        }
        for (k, v) in options {
            let (ck, cv) = (cstr(k), cstr(v));
            // SAFETY: valid handle and NUL-terminated strings.
            let rc = unsafe { (api.set_option_string)(handle, ck.as_ptr(), cv.as_ptr()) };
            if rc < 0 {
                let message = mpv_err(api, rc);
                // SAFETY: handle is valid and not used afterwards.
                unsafe { (api.terminate_destroy)(handle) };
                return Err(MediaError::Option {
                    name: k.into(),
                    message,
                });
            }
        }
        // SAFETY: valid, un-initialised handle.
        let rc = unsafe { (api.initialize)(handle) };
        if rc < 0 {
            // SAFETY: handle is valid and not used afterwards.
            unsafe { (api.terminate_destroy)(handle) };
            return Err(MediaError::Create(mpv_err(api, rc)));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let current: Arc<Mutex<Option<PathBuf>>> = Arc::default();
        let (h, s, cur) = (SendPtr(handle), stop.clone(), current.clone());
        let events = std::thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || event_loop(api, &h, &s, &cur, &on_event))
            .map_err(|e| MediaError::Create(e.to_string()))?;
        Ok(Player {
            api,
            handle,
            render: std::ptr::null_mut(),
            gl_loader: None,
            update: None,
            stop,
            events: Some(events),
            current,
        })
    }

    /// Create the GL render context. Call with the target GL context current.
    /// `on_update` runs on an mpv thread whenever a new frame is ready: it must only
    /// signal (e.g. send a message), never call back into the player.
    pub fn attach_gl(
        &mut self,
        loader: impl Fn(&CStr) -> *const c_void + 'static,
        on_update: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), MediaError> {
        unsafe extern "C" fn get_proc(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
            // SAFETY: ctx is the Box<GlLoader> kept alive in `Player::gl_loader`; name is a C string from mpv.
            unsafe {
                let loader = &*(ctx as *const GlLoader);
                loader(CStr::from_ptr(name)) as *mut c_void
            }
        }
        unsafe extern "C" fn update_cb(ctx: *mut c_void) {
            // SAFETY: ctx is the Box<UpdateCallback> kept alive in `Player::update`.
            unsafe { (*(ctx as *const UpdateCallback))() }
        }
        let loader: Box<GlLoader> = Box::new(Box::new(loader));
        let mut init = ffi::mpv_opengl_init_params {
            get_proc_address: Some(get_proc),
            get_proc_address_ctx: &*loader as *const GlLoader as *mut c_void,
        };
        let api_type = cstr("opengl");
        let mut params = [
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut init as *mut _ as *mut c_void,
            },
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let mut render = std::ptr::null_mut();
        // SAFETY: valid handle, params terminated by INVALID, loader outlives the context.
        let rc = unsafe {
            (self.api.render_context_create)(&mut render, self.handle, params.as_mut_ptr())
        };
        if rc < 0 {
            return Err(MediaError::Render(mpv_err(self.api, rc)));
        }
        let update: Box<UpdateCallback> = Box::new(Box::new(on_update));
        // SAFETY: render is valid; the callback box outlives it (freed after render_context_free).
        unsafe {
            (self.api.render_context_set_update_callback)(
                render,
                Some(update_cb),
                &*update as *const UpdateCallback as *mut c_void,
            );
        }
        self.render = render;
        self.gl_loader = Some(loader);
        self.update = Some(update);
        Ok(())
    }

    fn command(&self, args: &[&str]) -> Result<(), MediaError> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: valid handle, NULL-terminated array of C strings that outlive the call.
        let rc = unsafe { (self.api.command)(self.handle, ptrs.as_mut_ptr()) };
        if rc < 0 {
            return Err(MediaError::Command {
                what: args.join(" "),
                message: mpv_err(self.api, rc),
            });
        }
        Ok(())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), MediaError> {
        let (n, v) = (cstr(name), cstr(value));
        // SAFETY: valid handle and C strings.
        let rc = unsafe { (self.api.set_property_string)(self.handle, n.as_ptr(), v.as_ptr()) };
        if rc < 0 {
            return Err(MediaError::Command {
                what: format!("set {name}={value}"),
                message: mpv_err(self.api, rc),
            });
        }
        Ok(())
    }

    /// Load a file (image or video). `paused` = decode the first frame and hold it
    /// (used to pre-load the next background so switching is instant).
    pub fn load(&self, path: &Path, paused: bool) -> Result<(), MediaError> {
        self.set("pause", if paused { "yes" } else { "no" })?;
        if let Ok(mut c) = self.current.lock() {
            *c = Some(path.to_path_buf());
        }
        self.command(&["loadfile", &path.to_string_lossy(), "replace"])
    }

    pub fn play(&self) -> Result<(), MediaError> {
        self.set("pause", "no")
    }

    pub fn pause(&self) -> Result<(), MediaError> {
        self.set("pause", "yes")
    }

    pub fn stop(&self) -> Result<(), MediaError> {
        if let Ok(mut c) = self.current.lock() {
            *c = None;
        }
        self.command(&["stop"])
    }

    pub fn current(&self) -> Option<PathBuf> {
        self.current.lock().ok().and_then(|c| c.clone())
    }

    pub fn set_audio_device(&self, name: Option<&str>) -> Result<(), MediaError> {
        self.set("audio-device", name.unwrap_or("auto"))
    }

    pub fn set_scaling(&self, scaling: Scaling) -> Result<(), MediaError> {
        let (keepaspect, panscan) = scaling.mpv();
        self.set("keepaspect", keepaspect)?;
        self.set("panscan", panscan)
    }

    pub fn set_mute(&self, mute: bool) -> Result<(), MediaError> {
        self.set("mute", if mute { "yes" } else { "no" })
    }

    /// Whether mpv has a new frame to render (call after the update callback fired).
    pub fn needs_render(&self) -> bool {
        if self.render.is_null() {
            return false;
        }
        // SAFETY: valid render context, called on the GL thread.
        unsafe { (self.api.render_context_update)(self.render) & ffi::MPV_RENDER_UPDATE_FRAME != 0 }
    }

    /// Render the current frame into framebuffer `fbo` of size `w × h`
    /// (kept in aspect, letterboxed). `flip_y` puts row 0 at the top.
    pub fn render(&self, fbo: u32, w: u32, h: u32, flip_y: bool) -> Result<(), MediaError> {
        if self.render.is_null() {
            return Err(MediaError::Render("no GL attached".into()));
        }
        let mut target = ffi::mpv_opengl_fbo {
            fbo: fbo as c_int,
            w: w as c_int,
            h: h as c_int,
            internal_format: 0,
        };
        let mut flip: c_int = c_int::from(flip_y);
        // Never wait for the frame's display time: the output thread must stay responsive
        // to slide changes; vsync on swap paces presentation instead.
        let mut block: c_int = 0;
        let mut params = [
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                data: &mut block as *mut _ as *mut c_void,
            },
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_OPENGL_FBO,
                data: &mut target as *mut _ as *mut c_void,
            },
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_FLIP_Y,
                data: &mut flip as *mut _ as *mut c_void,
            },
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        // SAFETY: valid render context on its GL thread; params valid for the call.
        let rc = unsafe { (self.api.render_context_render)(self.render, params.as_mut_ptr()) };
        if rc < 0 {
            return Err(MediaError::Render(mpv_err(self.api, rc)));
        }
        Ok(())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Stop first so freeing the render context isn't reported as a playback failure.
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.command(&["stop"]);
        if !self.render.is_null() {
            // SAFETY: freed on the GL thread (documented contract) before the handle.
            unsafe { (self.api.render_context_free)(self.render) };
            self.render = std::ptr::null_mut();
        }
        self.stop.store(true, Ordering::SeqCst);
        // SAFETY: valid handle; wakes the event thread's mpv_wait_event.
        unsafe { (self.api.wakeup)(self.handle) };
        if let Some(t) = self.events.take() {
            let _ = t.join();
        }
        // SAFETY: no other thread uses the handle any more.
        unsafe { (self.api.terminate_destroy)(self.handle) };
    }
}

fn event_loop(
    api: &'static ffi::Api,
    handle: &SendPtr,
    stop: &AtomicBool,
    current: &Mutex<Option<PathBuf>>,
    on_event: &dyn Fn(MediaEvent),
) {
    let path = || {
        current
            .lock()
            .ok()
            .and_then(|c| c.clone())
            .unwrap_or_default()
    };
    while !stop.load(Ordering::SeqCst) {
        // SAFETY: handle stays valid until this thread is joined.
        let ev = unsafe { &*(api.wait_event)(handle.0, -1.0) };
        match ev.event_id {
            ffi::MPV_EVENT_SHUTDOWN => return,
            _ if stop.load(Ordering::SeqCst) => return,
            ffi::MPV_EVENT_FILE_LOADED => {
                tracing::debug!(path = %path().display(), "media loaded");
                on_event(MediaEvent::Loaded(path()));
            }
            ffi::MPV_EVENT_END_FILE if !ev.data.is_null() => {
                // SAFETY: END_FILE events carry an mpv_event_end_file.
                let end = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
                if end.reason == ffi::MPV_END_FILE_REASON_ERROR {
                    let e = MediaError::Playback {
                        path: path(),
                        message: mpv_err(api, end.error),
                    };
                    tracing::warn!(error = %e, "media playback failed");
                    on_event(MediaEvent::Failed(e));
                }
            }
            _ => {}
        }
    }
}

/// Audio output devices mpv can use (for the settings UI).
pub fn audio_devices() -> Result<Vec<AudioDevice>, MediaError> {
    let api = ffi::api().map_err(|e| MediaError::LibraryMissing(e.to_owned()))?;
    // SAFETY: plain constructor.
    let handle = unsafe { (api.create)() };
    if handle.is_null() {
        return Err(MediaError::Create("mpv_create returned null".into()));
    }
    let result = (|| {
        for (k, v) in [("vo", "null"), ("terminal", "no")] {
            let (ck, cv) = (cstr(k), cstr(v));
            // SAFETY: valid handle and C strings.
            unsafe { (api.set_option_string)(handle, ck.as_ptr(), cv.as_ptr()) };
        }
        // SAFETY: valid handle.
        let rc = unsafe { (api.initialize)(handle) };
        if rc < 0 {
            return Err(MediaError::Create(mpv_err(api, rc)));
        }
        let name = cstr("audio-device-list");
        // SAFETY: valid handle; result must be freed with mpv_free.
        let raw = unsafe { (api.get_property_string)(handle, name.as_ptr()) };
        if raw.is_null() {
            return Err(MediaError::Command {
                what: "get audio-device-list".into(),
                message: "no value".into(),
            });
        }
        // SAFETY: non-null C string from mpv, freed right after copying.
        let json = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: raw came from mpv_get_property_string.
        unsafe { (api.free)(raw as *mut c_void) };
        serde_json::from_str(&json).map_err(|e| MediaError::Command {
            what: "parse audio-device-list".into(),
            message: e.to_string(),
        })
    })();
    // SAFETY: no other users of this handle.
    unsafe { (api.terminate_destroy)(handle) };
    result
}

/// Save one frame of `path` (about 10% in, for a video) as a PNG `width` pixels wide
/// at `out`. Runs a short-lived headless mpv; blocks until done (a few hundred ms).
pub fn thumbnail(path: &Path, out: &Path, width: u32) -> Result<(), MediaError> {
    let api = ffi::api().map_err(|e| MediaError::LibraryMissing(e.to_owned()))?;
    let dir = out
        .parent()
        .ok_or_else(|| MediaError::Create("thumbnail path has no folder".into()))?
        .join(format!("mpv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| MediaError::Create(e.to_string()))?;
    // SAFETY: plain constructor.
    let handle = unsafe { (api.create)() };
    if handle.is_null() {
        return Err(MediaError::Create("mpv_create returned null".into()));
    }
    let fail = |message: String| MediaError::Playback {
        path: path.to_path_buf(),
        message,
    };
    let result = (|| {
        let dir_s = dir.to_string_lossy().into_owned();
        let vf = format!("scale={width}:-2");
        for (k, v) in [
            ("config", "no"),
            ("terminal", "no"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
            ("osc", "no"),
            ("idle", "yes"),
            ("audio", "no"),
            ("sub", "no"),
            ("ao", "null"),
            ("vo", "image"),
            ("vo-image-format", "png"),
            ("vo-image-png-compression", "1"),
            ("vo-image-outdir", dir_s.as_str()),
            ("frames", "1"),
            ("start", "10%"),
            ("hr-seek", "no"),
            ("image-display-duration", "0"),
            ("vf", vf.as_str()),
        ] {
            let (ck, cv) = (cstr(k), cstr(v));
            // SAFETY: valid handle and C strings.
            let rc = unsafe { (api.set_option_string)(handle, ck.as_ptr(), cv.as_ptr()) };
            if rc < 0 {
                return Err(MediaError::Option {
                    name: k.into(),
                    message: mpv_err(api, rc),
                });
            }
        }
        // SAFETY: valid, un-initialised handle.
        let rc = unsafe { (api.initialize)(handle) };
        if rc < 0 {
            return Err(MediaError::Create(mpv_err(api, rc)));
        }
        let args = [cstr("loadfile"), cstr(&path.to_string_lossy())];
        let mut ptrs: Vec<*const c_char> = args.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: valid handle, NULL-terminated array of C strings that outlive the call.
        let rc = unsafe { (api.command)(handle, ptrs.as_mut_ptr()) };
        if rc < 0 {
            return Err(fail(mpv_err(api, rc)));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(fail("timed out".into()));
            }
            // SAFETY: valid handle; the event stays valid until the next wait_event.
            let ev = unsafe { &*(api.wait_event)(handle, left.as_secs_f64()) };
            match ev.event_id {
                ffi::MPV_EVENT_SHUTDOWN => return Err(fail("player shut down".into())),
                ffi::MPV_EVENT_END_FILE if !ev.data.is_null() => {
                    // SAFETY: END_FILE events carry an mpv_event_end_file.
                    let end = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
                    if end.reason == ffi::MPV_END_FILE_REASON_ERROR {
                        return Err(fail(mpv_err(api, end.error)));
                    }
                    break;
                }
                _ => {}
            }
        }
        Ok(())
    })();
    // SAFETY: no other users of this handle.
    unsafe { (api.terminate_destroy)(handle) };
    // The image VO wrote 00000001.png for its frame (flushed by now); take it.
    let result = result.and_then(|()| {
        let written = std::fs::read_dir(&dir)
            .ok()
            .and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.path())))
            .ok_or_else(|| fail("no frame decoded".into()))?;
        std::fs::rename(&written, out)
            .or_else(|_| std::fs::copy(&written, out).map(|_| ()))
            .map_err(|e| fail(e.to_string()))
    });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Whether libmpv could be loaded (and why not).
pub fn available() -> Result<(), MediaError> {
    ffi::api()
        .map(|_| ())
        .map_err(|e| MediaError::LibraryMissing(e.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs libmpv installed; skipped (passes) when it isn't, e.g. on CI.
    #[test]
    fn audio_devices_lists_auto() {
        if available().is_err() {
            eprintln!("libmpv not available ({:?}); skipping", available());
            return;
        }
        let devices = audio_devices().unwrap();
        assert!(devices.iter().any(|d| d.name == "auto"), "{devices:?}");
    }

    /// Playback itself needs a GL render context (tested end-to-end in the app);
    /// here: a bad file must produce a clear failure event.
    #[test]
    fn bad_file_reports_failure() {
        if available().is_err() {
            eprintln!("libmpv not available ({:?}); skipping", available());
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let p = Player::new(None, Scaling::Fill, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        p.load(Path::new("/definitely/not/a/file.mp4"), false)
            .unwrap();
        let failed = loop {
            match rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap() {
                MediaEvent::Failed(e) => break e,
                MediaEvent::Loaded(_) => {}
            }
        };
        assert!(
            failed.user_message().contains("file.mp4"),
            "{}",
            failed.user_message()
        );
        assert_eq!(
            p.current().as_deref(),
            Some(Path::new("/definitely/not/a/file.mp4"))
        );
        drop(p);
    }

    /// Needs libmpv and ffmpeg (to make a test clip); skipped when either is missing.
    #[test]
    fn thumbnail_grabs_a_video_frame() {
        if available().is_err() {
            eprintln!("libmpv not available ({:?}); skipping", available());
            return;
        }
        let dir = std::env::temp_dir().join(format!("pp6-thumb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // PP6_TEST_CLIP: a ready-made clip (e.g. under Wine, where ffmpeg isn't on PATH).
        let given = std::env::var_os("PP6_TEST_CLIP").map(PathBuf::from);
        let clip = given.clone().unwrap_or_else(|| dir.join("clip.mp4"));
        let made = given.is_some()
            || std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
                .arg("testsrc=duration=2:size=640x360:rate=10")
                .args(["-pix_fmt", "yuv420p"])
                .arg(&clip)
                .status()
                .is_ok_and(|s| s.success());
        if !made {
            eprintln!("ffmpeg not available; skipping");
            return;
        }
        let out = dir.join("thumb.png");
        thumbnail(&clip, &out, 256).unwrap();
        let png = std::fs::read(&out).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        // IHDR width/height (big-endian) right after the signature and chunk header.
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!(w, 256);
        if given.is_none() {
            assert_eq!(h, 144, "640×360 clip keeps its shape");
        }
        let err = thumbnail(&dir.join("missing.mp4"), &dir.join("x.png"), 256);
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
