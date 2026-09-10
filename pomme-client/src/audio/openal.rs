use std::collections::VecDeque;
use std::ffi::{CStr, CString, c_char, c_float, c_int, c_void};
use std::rc::Rc;

use libloading::Library;

use super::decoder::PcmFormat;

type AlBoolean = c_char;
type AlEnum = c_int;
type AlInt = c_int;
type AlSize = c_int;
type AlUint = u32;
type AlcBoolean = c_char;
type AlcEnum = c_int;
type AlcInt = c_int;
type AlcDevice = c_void;
type AlcContext = c_void;

const AL_NO_ERROR: AlEnum = 0;
const AL_FALSE: AlInt = 0;
const AL_TRUE: AlInt = 1;
const AL_POSITION: AlEnum = 0x1004;
const AL_ORIENTATION: AlEnum = 0x100f;
const AL_PITCH: AlEnum = 0x1003;
const AL_LOOPING: AlEnum = 0x1007;
const AL_BUFFER: AlEnum = 0x1009;
const AL_GAIN: AlEnum = 0x100a;
const AL_SOURCE_STATE: AlEnum = 0x1010;
const AL_PLAYING: AlEnum = 0x1012;
const AL_PAUSED: AlEnum = 0x1013;
const AL_STOPPED: AlEnum = 0x1014;
const AL_BUFFERS_QUEUED: AlEnum = 0x1015;
const AL_BUFFERS_PROCESSED: AlEnum = 0x1016;
const AL_REFERENCE_DISTANCE: AlEnum = 0x1020;
const AL_ROLLOFF_FACTOR: AlEnum = 0x1021;
const AL_MAX_DISTANCE: AlEnum = 0x1023;
const AL_FORMAT_MONO16: AlEnum = 0x1101;
const AL_FORMAT_STEREO16: AlEnum = 0x1103;
const AL_SOURCE_RELATIVE: AlEnum = 0x0202;
const AL_INITIAL: AlEnum = 0x1011;
/// `AL_EXT_source_distance_model` capability enabled globally with `alEnable`.
const AL_SOURCE_DISTANCE_MODEL: AlEnum = 0x0200;
/// Per-source property used with `alSourcei` once `AL_SOURCE_DISTANCE_MODEL` is
/// enabled.
const AL_DISTANCE_MODEL: AlEnum = 0xd000;
const AL_LINEAR_DISTANCE: AlEnum = 0xd003;
const AL_VERSION: AlEnum = 0xb002;

const ALC_FALSE: AlcBoolean = 0;
const ALC_MAJOR_VERSION: AlcEnum = 0x1000;
const ALC_MINOR_VERSION: AlcEnum = 0x1001;
const ALC_ATTRIBUTES_SIZE: AlcEnum = 0x1002;
const ALC_ALL_ATTRIBUTES: AlcEnum = 0x1003;
const ALC_MONO_SOURCES: AlcEnum = 0x1010;
const ALC_HRTF_SOFT: AlcInt = 0x1992;
const ALC_NUM_HRTF_SPECIFIERS_SOFT: AlcEnum = 0x1994;
const ALC_HRTF_ID_SOFT: AlcInt = 0x1996;
const ALC_OUTPUT_LIMITER_SOFT: AlcInt = 0x199a;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceState {
    Initial,
    Playing,
    Paused,
    Stopped,
    Unknown(AlInt),
}

struct Api {
    _library: Library,
    al_get_error: unsafe extern "C" fn() -> AlEnum,
    al_get_string: unsafe extern "C" fn(AlEnum) -> *const c_char,
    al_is_extension_present: unsafe extern "C" fn(*const c_char) -> AlBoolean,
    al_enable: unsafe extern "C" fn(AlEnum),
    al_listener3f: unsafe extern "C" fn(AlEnum, c_float, c_float, c_float),
    al_listenerfv: unsafe extern "C" fn(AlEnum, *const c_float),
    al_gen_sources: unsafe extern "C" fn(AlSize, *mut AlUint),
    al_delete_sources: unsafe extern "C" fn(AlSize, *const AlUint),
    al_source_play: unsafe extern "C" fn(AlUint),
    al_source_stop: unsafe extern "C" fn(AlUint),
    al_sourcef: unsafe extern "C" fn(AlUint, AlEnum, c_float),
    al_sourcei: unsafe extern "C" fn(AlUint, AlEnum, AlInt),
    al_source3f: unsafe extern "C" fn(AlUint, AlEnum, c_float, c_float, c_float),
    al_get_sourcei: unsafe extern "C" fn(AlUint, AlEnum, *mut AlInt),
    al_gen_buffers: unsafe extern "C" fn(AlSize, *mut AlUint),
    al_delete_buffers: unsafe extern "C" fn(AlSize, *const AlUint),
    al_buffer_data: unsafe extern "C" fn(AlUint, AlEnum, *const c_void, AlSize, AlSize),
    al_source_queue_buffers: unsafe extern "C" fn(AlUint, AlSize, *const AlUint),
    al_source_unqueue_buffers: unsafe extern "C" fn(AlUint, AlSize, *mut AlUint),
    alc_open_device: unsafe extern "C" fn(*const c_char) -> *mut AlcDevice,
    alc_close_device: unsafe extern "C" fn(*mut AlcDevice) -> AlcBoolean,
    alc_create_context: unsafe extern "C" fn(*mut AlcDevice, *const AlcInt) -> *mut AlcContext,
    alc_destroy_context: unsafe extern "C" fn(*mut AlcContext),
    alc_make_context_current: unsafe extern "C" fn(*mut AlcContext) -> AlcBoolean,
    alc_get_error: unsafe extern "C" fn(*mut AlcDevice) -> AlcEnum,
    alc_is_extension_present: unsafe extern "C" fn(*mut AlcDevice, *const c_char) -> AlcBoolean,
    alc_get_integerv: unsafe extern "C" fn(*mut AlcDevice, AlcEnum, AlSize, *mut AlcInt),
}

impl Api {
    fn load() -> Result<Self, String> {
        let mut candidates = Vec::new();
        if let Ok(executable) = std::env::current_exe()
            && let Some(directory) = executable.parent()
        {
            candidates.extend(library_names().iter().map(|name| directory.join(name)));
        }
        candidates.extend(library_names().iter().map(std::path::PathBuf::from));

        let mut errors = Vec::new();
        for path in candidates {
            let display = path.display().to_string();
            // SAFETY: Loading a shared library is inherently unsafe because its
            // initialization code is outside Rust's control. We only try
            // packaged/system OpenAL library names and keep the returned Library
            // alive for the lifetime of every copied symbol pointer.
            let library = match unsafe { Library::new(&path) } {
                Ok(library) => library,
                Err(e) => {
                    errors.push(format!("{display}: {e}"));
                    continue;
                }
            };
            match Self::from_library(library) {
                Ok(api) => return Ok(api),
                // A stub system library must not shadow the staged one.
                Err(e) => {
                    errors.push(format!("{display}: OpenAL symbols were incomplete: {e}"));
                }
            }
        }
        Err(format!("failed to load OpenAL ({})", errors.join("; ")))
    }

    fn from_library(library: Library) -> Result<Self, String> {
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                // SAFETY: The requested symbols and ABI signatures are defined by OpenAL 1.1.
                // The owning Library is stored in Api, so copied function pointers cannot
                // outlive it.
                *unsafe { library.get::<$ty>(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| format!(concat!($name, ": {}"), e))?
            }};
        }
        Ok(Self {
            al_get_error: symbol!("alGetError", unsafe extern "C" fn() -> AlEnum),
            al_get_string: symbol!("alGetString", unsafe extern "C" fn(AlEnum) -> *const c_char),
            al_is_extension_present: symbol!(
                "alIsExtensionPresent",
                unsafe extern "C" fn(*const c_char) -> AlBoolean
            ),
            al_enable: symbol!("alEnable", unsafe extern "C" fn(AlEnum)),
            al_listener3f: symbol!(
                "alListener3f",
                unsafe extern "C" fn(AlEnum, c_float, c_float, c_float)
            ),
            al_listenerfv: symbol!("alListenerfv", unsafe extern "C" fn(AlEnum, *const c_float)),
            al_gen_sources: symbol!("alGenSources", unsafe extern "C" fn(AlSize, *mut AlUint)),
            al_delete_sources: symbol!(
                "alDeleteSources",
                unsafe extern "C" fn(AlSize, *const AlUint)
            ),
            al_source_play: symbol!("alSourcePlay", unsafe extern "C" fn(AlUint)),
            al_source_stop: symbol!("alSourceStop", unsafe extern "C" fn(AlUint)),
            al_sourcef: symbol!("alSourcef", unsafe extern "C" fn(AlUint, AlEnum, c_float)),
            al_sourcei: symbol!("alSourcei", unsafe extern "C" fn(AlUint, AlEnum, AlInt)),
            al_source3f: symbol!(
                "alSource3f",
                unsafe extern "C" fn(AlUint, AlEnum, c_float, c_float, c_float)
            ),
            al_get_sourcei: symbol!(
                "alGetSourcei",
                unsafe extern "C" fn(AlUint, AlEnum, *mut AlInt)
            ),
            al_gen_buffers: symbol!("alGenBuffers", unsafe extern "C" fn(AlSize, *mut AlUint)),
            al_delete_buffers: symbol!(
                "alDeleteBuffers",
                unsafe extern "C" fn(AlSize, *const AlUint)
            ),
            al_buffer_data: symbol!(
                "alBufferData",
                unsafe extern "C" fn(AlUint, AlEnum, *const c_void, AlSize, AlSize)
            ),
            al_source_queue_buffers: symbol!(
                "alSourceQueueBuffers",
                unsafe extern "C" fn(AlUint, AlSize, *const AlUint)
            ),
            al_source_unqueue_buffers: symbol!(
                "alSourceUnqueueBuffers",
                unsafe extern "C" fn(AlUint, AlSize, *mut AlUint)
            ),
            alc_open_device: symbol!(
                "alcOpenDevice",
                unsafe extern "C" fn(*const c_char) -> *mut AlcDevice
            ),
            alc_close_device: symbol!(
                "alcCloseDevice",
                unsafe extern "C" fn(*mut AlcDevice) -> AlcBoolean
            ),
            alc_create_context: symbol!(
                "alcCreateContext",
                unsafe extern "C" fn(*mut AlcDevice, *const AlcInt) -> *mut AlcContext
            ),
            alc_destroy_context: symbol!(
                "alcDestroyContext",
                unsafe extern "C" fn(*mut AlcContext)
            ),
            alc_make_context_current: symbol!(
                "alcMakeContextCurrent",
                unsafe extern "C" fn(*mut AlcContext) -> AlcBoolean
            ),
            alc_get_error: symbol!(
                "alcGetError",
                unsafe extern "C" fn(*mut AlcDevice) -> AlcEnum
            ),
            alc_is_extension_present: symbol!(
                "alcIsExtensionPresent",
                unsafe extern "C" fn(*mut AlcDevice, *const c_char) -> AlcBoolean
            ),
            alc_get_integerv: symbol!(
                "alcGetIntegerv",
                unsafe extern "C" fn(*mut AlcDevice, AlcEnum, AlSize, *mut AlcInt)
            ),
            _library: library,
        })
    }

    fn al_error(&self, operation: &str) -> Result<(), String> {
        // SAFETY: alGetError has no pointer arguments and the current thread owns a
        // live context.
        let error = unsafe { (self.al_get_error)() };
        if error == AL_NO_ERROR {
            Ok(())
        } else {
            Err(format!("{operation}: OpenAL error 0x{error:04x}"))
        }
    }

    fn alc_error(&self, device: *mut AlcDevice, operation: &str) -> Result<(), String> {
        // SAFETY: device is a live handle returned by alcOpenDevice and owned by
        // ContextInner.
        let error = unsafe { (self.alc_get_error)(device) };
        if error == 0 {
            Ok(())
        } else {
            Err(format!("{operation}: OpenALC error 0x{error:04x}"))
        }
    }
}

fn library_names() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &["OpenAL.dll", "OpenAL32.dll", "soft_oal.dll"]
    }
    #[cfg(target_os = "macos")]
    {
        &[
            "libopenal.1.dylib",
            "libopenal.dylib",
            "/System/Library/Frameworks/OpenAL.framework/OpenAL",
        ]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        &["libopenal.so.1", "libopenal.so"]
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        &[]
    }
}

struct ContextInner {
    api: Api,
    device: *mut AlcDevice,
    context: *mut AlcContext,
}

impl Drop for ContextInner {
    fn drop(&mut self) {
        // SAFETY: This thread exclusively owns the current context; null detaches it
        // before destroy.
        unsafe { (self.api.alc_make_context_current)(std::ptr::null_mut()) };
        // SAFETY: context is a live handle created exactly once by alcCreateContext and
        // is dropped once.
        unsafe { (self.api.alc_destroy_context)(self.context) };
        // SAFETY: device is a live handle created exactly once by alcOpenDevice and is
        // closed once, after its dependent context has been destroyed.
        unsafe { (self.api.alc_close_device)(self.device) };
    }
}

pub(super) struct Context {
    inner: Rc<ContextInner>,
    pub static_source_limit: usize,
    pub streaming_source_limit: usize,
}

impl Context {
    pub fn open_default(use_hrtf: bool) -> Result<Self, String> {
        let api = Api::load()?;
        // SAFETY: A null name requests OpenAL's default playback device.
        let device = unsafe { (api.alc_open_device)(std::ptr::null()) };
        if device.is_null() {
            return Err("alcOpenDevice returned null".to_string());
        }
        if let Err(e) = api.alc_error(device, "open device") {
            // SAFETY: device was successfully opened above and no context depends on it
            // yet.
            unsafe { (api.alc_close_device)(device) };
            return Err(e);
        }

        Self::create_for_device(api, device, use_hrtf)
    }

    fn create_for_device(api: Api, device: *mut AlcDevice, use_hrtf: bool) -> Result<Self, String> {
        let mut major = 0;
        let mut minor = 0;
        // SAFETY: device is live; both output pointers reference valid AlcInt storage
        // for one element.
        unsafe { (api.alc_get_integerv)(device, ALC_MAJOR_VERSION, 1, &mut major) };
        // SAFETY: same invariant as the preceding version query.
        unsafe { (api.alc_get_integerv)(device, ALC_MINOR_VERSION, 1, &mut minor) };
        if let Err(e) = api.alc_error(device, "query OpenALC version") {
            // SAFETY: no context was created, so device may be closed directly.
            unsafe { (api.alc_close_device)(device) };
            return Err(e);
        }
        if major < 1 || (major == 1 && minor < 1) {
            // SAFETY: no context was created, so device may be closed directly.
            unsafe { (api.alc_close_device)(device) };
            return Err(format!("OpenALC 1.1 required, found {major}.{minor}"));
        }

        let hrtf_extension = CString::new("ALC_SOFT_HRTF").expect("static string has no nul");
        // SAFETY: device is live and the CString is NUL-terminated for the duration of
        // the call.
        let has_hrtf =
            unsafe { (api.alc_is_extension_present)(device, hrtf_extension.as_ptr()) } != ALC_FALSE;
        let limiter_extension =
            CString::new("ALC_SOFT_output_limiter").expect("static string has no nul");
        // SAFETY: device is live and the CString is NUL-terminated for the duration of
        // the call.
        let has_output_limiter =
            unsafe { (api.alc_is_extension_present)(device, limiter_extension.as_ptr()) }
                != ALC_FALSE;

        let mut attributes = Vec::with_capacity(7);
        if has_hrtf {
            let mut count = 0;
            // SAFETY: device is live and count points to one valid AlcInt output element.
            unsafe { (api.alc_get_integerv)(device, ALC_NUM_HRTF_SPECIFIERS_SOFT, 1, &mut count) };
            if count > 0 {
                attributes.extend_from_slice(&[
                    ALC_HRTF_SOFT,
                    AlcInt::from(use_hrtf),
                    ALC_HRTF_ID_SOFT,
                    0,
                ]);
            }
        }
        if has_output_limiter {
            attributes.extend_from_slice(&[ALC_OUTPUT_LIMITER_SOFT, 1]);
        }
        attributes.push(0);
        // SAFETY: device is live and attributes is terminated by 0 as required by
        // alcCreateContext.
        let context = unsafe { (api.alc_create_context)(device, attributes.as_ptr()) };
        if context.is_null() {
            let error = match api.alc_error(device, "create context") {
                Ok(()) => "alcCreateContext returned null".to_string(),
                Err(e) => e,
            };
            // SAFETY: context creation failed, so device has no dependent context.
            unsafe { (api.alc_close_device)(device) };
            return Err(error);
        }
        // SAFETY: context is live and owned exclusively by this thread.
        if unsafe { (api.alc_make_context_current)(context) } == ALC_FALSE {
            // SAFETY: context and device were created above and are not owned elsewhere
            // yet.
            unsafe { (api.alc_destroy_context)(context) };
            // SAFETY: context was destroyed immediately above, leaving no dependent object.
            unsafe { (api.alc_close_device)(device) };
            return Err("alcMakeContextCurrent failed".to_string());
        }
        if let Err(e) = api.alc_error(device, "make context current") {
            // SAFETY: this thread just made the context current; detach it before
            // destruction.
            unsafe { (api.alc_make_context_current)(std::ptr::null_mut()) };
            // SAFETY: this is still the sole owner of both handles on the initialization
            // path.
            unsafe { (api.alc_destroy_context)(context) };
            // SAFETY: dependent context was destroyed immediately above.
            unsafe { (api.alc_close_device)(device) };
            return Err(e);
        }

        let source_distance_model =
            CString::new("AL_EXT_source_distance_model").expect("static string has no nul");
        let linear_distance =
            CString::new("AL_EXT_LINEAR_DISTANCE").expect("static string has no nul");
        // SAFETY: a context is current and both extension-name CStrings are valid for
        // the call.
        let has_source_distance =
            unsafe { (api.al_is_extension_present)(source_distance_model.as_ptr()) } != 0;
        // SAFETY: same current-context and CString validity invariant as above.
        let has_linear_distance =
            unsafe { (api.al_is_extension_present)(linear_distance.as_ptr()) } != 0;
        if !has_source_distance || !has_linear_distance {
            // SAFETY: initialization has exclusive ownership of context and device.
            unsafe { (api.alc_make_context_current)(std::ptr::null_mut()) };
            // SAFETY: context remains live and is destroyed exactly once here.
            unsafe { (api.alc_destroy_context)(context) };
            // SAFETY: context was destroyed above, so device can be closed.
            unsafe { (api.alc_close_device)(device) };
            return Err("required OpenAL distance-model extensions are unavailable".to_string());
        }
        // SAFETY: a live context is current; Vanilla enables AL_SOURCE_DISTANCE_MODEL
        // globally.
        unsafe { (api.al_enable)(AL_SOURCE_DISTANCE_MODEL) };
        if let Err(e) = api.al_error("enable per-source distance models") {
            // SAFETY: initialization has exclusive ownership of context and device.
            unsafe { (api.alc_make_context_current)(std::ptr::null_mut()) };
            // SAFETY: context remains live and is destroyed exactly once here.
            unsafe { (api.alc_destroy_context)(context) };
            // SAFETY: context was destroyed above, so device can be closed.
            unsafe { (api.alc_close_device)(device) };
            return Err(e);
        }

        let total_sources = query_source_count(&api, device).unwrap_or(30);
        let streaming = ((total_sources as f32).sqrt() as usize).clamp(2, 8);
        let static_sources = total_sources.saturating_sub(streaming).clamp(8, 255);
        let inner = Rc::new(ContextInner {
            api,
            device,
            context,
        });
        Ok(Self {
            inner,
            static_source_limit: static_sources,
            streaming_source_limit: streaming,
        })
    }

    pub fn version(&self) -> Option<String> {
        // SAFETY: the context is current on its owning audio thread; AL_VERSION returns
        // either null or a static NUL-terminated string owned by OpenAL.
        let ptr = unsafe { (self.inner.api.al_get_string)(AL_VERSION) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: OpenAL guarantees a NUL-terminated string for non-null alGetString
        // results.
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    pub fn set_listener(
        &self,
        position: [f32; 3],
        forward: [f32; 3],
        up: [f32; 3],
    ) -> Result<(), String> {
        // SAFETY: a live context is current and scalar listener coordinates need no
        // borrowed storage.
        unsafe {
            (self.inner.api.al_listener3f)(AL_POSITION, position[0], position[1], position[2])
        };
        let orientation = [forward[0], forward[1], forward[2], up[0], up[1], up[2]];
        // SAFETY: orientation points to six contiguous f32 values and remains valid for
        // the call.
        unsafe { (self.inner.api.al_listenerfv)(AL_ORIENTATION, orientation.as_ptr()) };
        self.inner.api.al_error("set listener transform")
    }

    pub fn create_source(&self) -> Result<Source, String> {
        let mut id = 0;
        // SAFETY: id points to one valid AlUint output slot and this context is
        // current.
        unsafe { (self.inner.api.al_gen_sources)(1, &mut id) };
        if let Err(error) = self.inner.api.al_error("allocate source") {
            if id != 0 {
                // SAFETY: alGenSources wrote this id during the failed allocation call;
                // deleting a non-zero returned id prevents a partial-initialization leak.
                unsafe { (self.inner.api.al_delete_sources)(1, &id) };
                let _ = self.inner.api.al_error("clean up failed source allocation");
            }
            return Err(error);
        }
        if id == 0 {
            return Err("alGenSources returned source 0".to_string());
        }
        Ok(Source {
            context: Rc::clone(&self.inner),
            id,
        })
    }

    pub fn create_buffer(&self, format: PcmFormat, samples: &[i16]) -> Result<Buffer, String> {
        let al_format = match format.channels {
            1 => AL_FORMAT_MONO16,
            2 => AL_FORMAT_STEREO16,
            channels => return Err(format!("unsupported OpenAL channel count {channels}")),
        };
        let byte_len = samples
            .len()
            .checked_mul(std::mem::size_of::<i16>())
            .and_then(|len| AlSize::try_from(len).ok())
            .ok_or_else(|| "PCM buffer is too large for OpenAL".to_string())?;
        let sample_rate = AlSize::try_from(format.sample_rate)
            .map_err(|_| "sample rate does not fit OpenAL integer range".to_string())?;
        let mut id = 0;
        // SAFETY: id points to one valid AlUint output slot and this context is
        // current.
        unsafe { (self.inner.api.al_gen_buffers)(1, &mut id) };
        if let Err(error) = self.inner.api.al_error("allocate buffer") {
            if id != 0 {
                // SAFETY: alGenBuffers wrote this id during the failed allocation call;
                // deleting a non-zero returned id prevents a partial-initialization leak.
                unsafe { (self.inner.api.al_delete_buffers)(1, &id) };
                let _ = self.inner.api.al_error("clean up failed buffer allocation");
            }
            return Err(error);
        }
        if id == 0 {
            return Err("alGenBuffers returned buffer 0".to_string());
        }
        // SAFETY: samples points to byte_len initialized bytes. OpenAL copies buffer
        // data before alBufferData returns, so the borrowed slice need not
        // outlive this call.
        unsafe {
            (self.inner.api.al_buffer_data)(
                id,
                al_format,
                samples.as_ptr().cast(),
                byte_len,
                sample_rate,
            )
        };
        if let Err(e) = self.inner.api.al_error("upload PCM buffer") {
            // SAFETY: id was generated above and has not been transferred elsewhere.
            unsafe { (self.inner.api.al_delete_buffers)(1, &id) };
            return Err(e);
        }
        Ok(Buffer {
            context: Rc::clone(&self.inner),
            id,
        })
    }
}

fn query_source_count(api: &Api, device: *mut AlcDevice) -> Option<usize> {
    let mut size = 0;
    // SAFETY: device is live and size points to one valid AlcInt output element.
    unsafe { (api.alc_get_integerv)(device, ALC_ATTRIBUTES_SIZE, 1, &mut size) };
    if api
        .alc_error(device, "query context attribute size")
        .is_err()
        || size <= 0
    {
        return None;
    }
    let mut attributes = vec![0; usize::try_from(size).ok()?];
    // SAFETY: attributes has exactly `size` writable AlcInt elements.
    unsafe { (api.alc_get_integerv)(device, ALC_ALL_ATTRIBUTES, size, attributes.as_mut_ptr()) };
    if api.alc_error(device, "query context attributes").is_err() {
        return None;
    }
    for pair in attributes.chunks_exact(2) {
        if pair[0] == 0 {
            break;
        }
        if pair[0] == ALC_MONO_SOURCES {
            return usize::try_from(pair[1]).ok();
        }
    }
    None
}

pub(super) struct Buffer {
    context: Rc<ContextInner>,
    id: AlUint,
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: id is owned by this Buffer, context is kept live by Rc, and Drop runs
        // once.
        unsafe { (self.context.api.al_delete_buffers)(1, &self.id) };
        if let Err(e) = self.context.api.al_error("delete buffer") {
            tracing::warn!("{e}");
        }
    }
}

pub(super) struct Source {
    context: Rc<ContextInner>,
    id: AlUint,
}

impl Source {
    pub fn configure(
        &self,
        gain: f32,
        pitch: f32,
        position: [f32; 3],
        relative: bool,
        looping: bool,
        attenuation_distance: Option<f32>,
    ) -> Result<(), String> {
        self.sourcef(AL_GAIN, gain)?;
        self.sourcef(AL_PITCH, pitch)?;
        self.sourcei(
            AL_SOURCE_RELATIVE,
            if relative { AL_TRUE } else { AL_FALSE },
        )?;
        self.sourcei(AL_LOOPING, if looping { AL_TRUE } else { AL_FALSE })?;
        // SAFETY: source id is live and position is passed by value.
        unsafe {
            (self.context.api.al_source3f)(
                self.id,
                AL_POSITION,
                position[0],
                position[1],
                position[2],
            )
        };
        self.context.api.al_error("set source position")?;
        match attenuation_distance {
            Some(max_distance) => {
                self.sourcei(AL_DISTANCE_MODEL, AL_LINEAR_DISTANCE)?;
                // Vanilla's Channel.linearAttenuation issues these without
                // checking alGetError, and accepts the negative
                // attenuation_distance that OpenAL rejects, so a pack setting
                // one must not lose the sound here.
                for (param, value) in [
                    (AL_MAX_DISTANCE, max_distance),
                    (AL_ROLLOFF_FACTOR, 1.0),
                    (AL_REFERENCE_DISTANCE, 0.0),
                ] {
                    if let Err(e) = self.sourcef(param, value) {
                        tracing::debug!("ignoring OpenAL attenuation rejection: {e}");
                    }
                }
            }
            None => self.sourcei(AL_DISTANCE_MODEL, 0)?,
        }
        Ok(())
    }

    pub fn set_gain(&self, gain: f32) -> Result<(), String> {
        self.sourcef(AL_GAIN, gain)
    }

    pub fn set_position(&self, position: [f32; 3]) -> Result<(), String> {
        // SAFETY: source id is live and position is passed by value.
        unsafe {
            (self.context.api.al_source3f)(
                self.id,
                AL_POSITION,
                position[0],
                position[1],
                position[2],
            )
        };
        self.context.api.al_error("set source position")
    }

    pub fn attach_static(&self, buffer: &Buffer) -> Result<(), String> {
        if !Rc::ptr_eq(&self.context, &buffer.context) {
            return Err("source and buffer belong to different OpenAL contexts".to_string());
        }
        let id = AlInt::try_from(buffer.id).map_err(|_| "buffer id exceeds ALint".to_string())?;
        self.sourcei(AL_BUFFER, id)
    }

    pub fn queue_buffer(&self, buffer: &Buffer) -> Result<(), String> {
        if !Rc::ptr_eq(&self.context, &buffer.context) {
            return Err("source and buffer belong to different OpenAL contexts".to_string());
        }
        // SAFETY: both ids are live in the same current context; one buffer id is
        // readable here.
        unsafe { (self.context.api.al_source_queue_buffers)(self.id, 1, &buffer.id) };
        self.context.api.al_error("queue stream buffer")
    }

    pub fn remove_processed(&self, queued: &mut VecDeque<Buffer>) -> Result<usize, String> {
        let processed = self.get_sourcei(AL_BUFFERS_PROCESSED)?.max(0);
        let processed =
            usize::try_from(processed).map_err(|_| "invalid processed count".to_string())?;
        for _ in 0..processed {
            let mut id = 0;
            // SAFETY: OpenAL reports this source has a processed buffer available to
            // unqueue.
            unsafe { (self.context.api.al_source_unqueue_buffers)(self.id, 1, &mut id) };
            self.context.api.al_error("unqueue stream buffer")?;
            let expected = queued
                .front()
                .ok_or_else(|| "OpenAL processed more buffers than Pomme queued".to_string())?;
            if expected.id != id {
                return Err(format!(
                    "OpenAL unqueued buffer {id}, expected {}",
                    expected.id
                ));
            }
            drop(queued.pop_front());
        }
        Ok(processed)
    }

    pub fn clear_queue(&self, queued: &mut VecDeque<Buffer>) -> Result<(), String> {
        self.stop()?;
        let count = self.get_sourcei(AL_BUFFERS_QUEUED)?.max(0);
        let count = usize::try_from(count).map_err(|_| "invalid queued count".to_string())?;
        for _ in 0..count {
            let mut id = 0;
            // SAFETY: after stopping a source, its queued buffers are available to unqueue.
            unsafe { (self.context.api.al_source_unqueue_buffers)(self.id, 1, &mut id) };
            self.context.api.al_error("clear stream buffer")?;
            let expected = queued
                .front()
                .ok_or_else(|| "OpenAL queued more buffers than Pomme tracks".to_string())?;
            if expected.id != id {
                return Err(format!(
                    "OpenAL cleared buffer {id}, expected {}",
                    expected.id
                ));
            }
            drop(queued.pop_front());
        }
        Ok(())
    }

    pub fn play(&self) -> Result<(), String> {
        // SAFETY: id is a live source owned by this Source and its context is current.
        unsafe { (self.context.api.al_source_play)(self.id) };
        self.context.api.al_error("play source")
    }

    pub fn stop(&self) -> Result<(), String> {
        // SAFETY: id is a live source owned by this Source and its context is current.
        unsafe { (self.context.api.al_source_stop)(self.id) };
        self.context.api.al_error("stop source")
    }

    pub fn state(&self) -> Result<SourceState, String> {
        Ok(match self.get_sourcei(AL_SOURCE_STATE)? {
            AL_PLAYING => SourceState::Playing,
            AL_PAUSED => SourceState::Paused,
            AL_STOPPED => SourceState::Stopped,
            AL_INITIAL => SourceState::Initial,
            other => SourceState::Unknown(other),
        })
    }

    fn sourcef(&self, param: AlEnum, value: f32) -> Result<(), String> {
        // SAFETY: id is live, param is an OpenAL source float property, and context is
        // current.
        unsafe { (self.context.api.al_sourcef)(self.id, param, value) };
        self.context.api.al_error("set source float")
    }

    fn sourcei(&self, param: AlEnum, value: AlInt) -> Result<(), String> {
        // SAFETY: id is live, param is an OpenAL source integer property, and context
        // is current.
        unsafe { (self.context.api.al_sourcei)(self.id, param, value) };
        self.context.api.al_error(&format!(
            "set source integer parameter 0x{param:04x} to 0x{value:x}"
        ))
    }

    fn get_sourcei(&self, param: AlEnum) -> Result<AlInt, String> {
        let mut value = 0;
        // SAFETY: id is live and value points to one writable AlInt output slot.
        unsafe { (self.context.api.al_get_sourcei)(self.id, param, &mut value) };
        self.context.api.al_error("query source integer")?;
        Ok(value)
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: id is owned by this Source, context remains live via Rc, and Drop
        // runs once.
        unsafe { (self.context.api.al_source_stop)(self.id) };
        // SAFETY: same ownership invariant; delete occurs exactly once after stopping.
        unsafe { (self.context.api.al_delete_sources)(1, &self.id) };
        if let Err(e) = self.context.api.al_error("delete source") {
            tracing::warn!("{e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_distance_model_capability_and_property_are_distinct() {
        assert_eq!(AL_SOURCE_DISTANCE_MODEL, 0x0200);
        assert_eq!(AL_DISTANCE_MODEL, 0xd000);
        assert_eq!(AL_LINEAR_DISTANCE, 0xd003);
        assert_ne!(AL_SOURCE_DISTANCE_MODEL, AL_DISTANCE_MODEL);
    }
}
