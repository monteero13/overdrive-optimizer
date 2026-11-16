//! NVIDIA driver profiles (NVAPI DRS) — loaded dynamically from `nvapi64.dll`, so the
//! service runs fine on machines without an NVIDIA driver.
//!
//! Function ids, structure layouts and status codes are taken from NVIDIA's public SDK
//! headers (github.com/NVIDIA/nvapi: nvapi_interface.h, nvapi.h, nvapi_lite_common.h,
//! NvApiDriverSettings.h). Sizes are checked at compile time against `sizeof` in the headers.
//!
//! Semantics (mirrors the registry primitive):
//! - `get`: the *user* value of a setting in the profile the exe belongs to; a predefined
//!   (NVIDIA-shipped) or inherited value is `None`.
//! - `set`: if the exe has no profile, create `Overdrive - <exe>` and add the exe to it.
//! - `clear`: delete the user value (the predefined one comes back). An `Overdrive - <exe>`
//!   profile left with no settings is deleted, so revert leaves no trace.
//!
//! Every call opens its own DRS session (create → load → act → save → destroy): a crash
//! can never leave a half-saved session around, and the driver's own file stays the
//! single source of truth.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::core::{s, w};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use overdrive_core::engine::{DriverSetting, SysError, SysResult};

const UNICODE_MAX: usize = 2048;
type UString = [u16; UNICODE_MAX];

// nvapi_lite_common.h
const NVAPI_OK: i32 = 0;
const NVAPI_INVALID_USER_PRIVILEGE: i32 = -137;
const NVAPI_SETTING_NOT_FOUND: i32 = -160;
const NVAPI_PROFILE_NOT_FOUND: i32 = -163;
const NVAPI_EXECUTABLE_NOT_FOUND: i32 = -166;

// NvApiDriverSettings.h
const FRL_FPS_ID: u32 = 0x1083_5002;
const PREFERRED_PSTATE_ID: u32 = 0x1057_EB71;

// NVDRS_SETTING_LOCATION / NVDRS_SETTING_TYPE
const NVDRS_CURRENT_PROFILE_LOCATION: u32 = 0;
const NVDRS_DWORD_TYPE: u32 = 0;

pub const PROFILE_PREFIX: &str = "Overdrive - ";

fn setting_id(s: DriverSetting) -> u32 {
    match s {
        DriverSetting::MaxFrameRate => FRL_FPS_ID,
        DriverSetting::PowerMode => PREFERRED_PSTATE_ID,
    }
}

#[repr(C)]
struct NvdrsSetting {
    version: u32,
    setting_name: UString,
    setting_id: u32,
    setting_type: u32,
    setting_location: u32,
    is_current_predefined: u32,
    is_predefined_valid: u32,
    /// union { u32 | NVDRS_BINARY_SETTING (4 + 4096) | wstring | u64 | f32 }
    predefined: [u8; 4100],
    current: [u8; 4100],
}

#[repr(C)]
struct NvdrsApplicationV4 {
    version: u32,
    is_predefined: u32,
    app_name: UString,
    user_friendly_name: UString,
    launcher: UString,
    file_in_folder: UString,
    flags: u32, // isMetro:1, isCommandLine:1, reserved:30
    command_line: UString,
}

#[repr(C)]
struct NvdrsProfile {
    version: u32,
    profile_name: UString,
    gpu_support: u32,
    is_predefined: u32,
    num_of_apps: u32,
    num_of_settings: u32,
}

// MAKE_NVAPI_VERSION(type, ver) = sizeof(type) | ver << 16, with the header's sizes.
const _: () = assert!(std::mem::size_of::<NvdrsSetting>() == 12320);
const _: () = assert!(std::mem::size_of::<NvdrsApplicationV4>() == 20492);
const _: () = assert!(std::mem::size_of::<NvdrsProfile>() == 4116);
const SETTING_VER: u32 = 12320 | (1 << 16);
const APPLICATION_VER: u32 = 20492 | (4 << 16);
const PROFILE_VER: u32 = 4116 | (1 << 16);

type H = *mut c_void;
type FnInit = unsafe extern "C" fn() -> i32;
type FnSession = unsafe extern "C" fn(*mut H) -> i32;
type FnWithSession = unsafe extern "C" fn(H) -> i32;
type FnFindApp = unsafe extern "C" fn(H, *const u16, *mut H, *mut NvdrsApplicationV4) -> i32;
type FnCreateProfile = unsafe extern "C" fn(H, *mut NvdrsProfile, *mut H) -> i32;
type FnFindProfile = unsafe extern "C" fn(H, *const u16, *mut H) -> i32;
type FnDeleteProfile = unsafe extern "C" fn(H, H) -> i32;
type FnProfileInfo = unsafe extern "C" fn(H, H, *mut NvdrsProfile) -> i32;
type FnCreateApp = unsafe extern "C" fn(H, H, *mut NvdrsApplicationV4) -> i32;
type FnGetSetting = unsafe extern "C" fn(H, H, u32, *mut NvdrsSetting) -> i32;
type FnSetSetting = unsafe extern "C" fn(H, H, *mut NvdrsSetting) -> i32;
type FnDeleteSetting = unsafe extern "C" fn(H, H, u32) -> i32;

struct Api {
    create_session: FnSession,
    destroy_session: FnWithSession,
    load: FnWithSession,
    save: FnWithSession,
    find_app: FnFindApp,
    create_profile: FnCreateProfile,
    find_profile: FnFindProfile,
    delete_profile: FnDeleteProfile,
    profile_info: FnProfileInfo,
    create_app: FnCreateApp,
    get_setting: FnGetSetting,
    set_setting: FnSetSetting,
    delete_setting: FnDeleteSetting,
}

// The function pointers are plain code addresses inside nvapi64.dll (never unloaded).
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

fn load_api() -> Option<Api> {
    unsafe {
        let module: HMODULE = LoadLibraryW(w!("nvapi64.dll")).ok()?;
        let qi = GetProcAddress(module, s!("nvapi_QueryInterface"))?;
        let qi: unsafe extern "C" fn(u32) -> *mut c_void = std::mem::transmute(qi);
        macro_rules! f {
            ($id:expr, $t:ty) => {{
                let p = qi($id);
                if p.is_null() {
                    return None;
                }
                std::mem::transmute::<*mut c_void, $t>(p)
            }};
        }
        // nvapi_interface.h
        let init: FnInit = f!(0x0150_E828, FnInit);
        if init() != NVAPI_OK {
            return None; // no NVIDIA GPU / driver
        }
        Some(Api {
            create_session: f!(0x0694_D52E, FnSession),
            destroy_session: f!(0xDAD9_CFF8, FnWithSession),
            load: f!(0x375D_BD6B, FnWithSession),
            save: f!(0xFCBC_7E14, FnWithSession),
            find_app: f!(0xEEE5_66B2, FnFindApp),
            create_profile: f!(0xCC17_6068, FnCreateProfile),
            find_profile: f!(0x7E4A_9A0B, FnFindProfile),
            delete_profile: f!(0x1709_3206, FnDeleteProfile),
            profile_info: f!(0x61CD_6FD6, FnProfileInfo),
            create_app: f!(0x4347_A9DE, FnCreateApp),
            get_setting: f!(0x73BF_8338, FnGetSetting),
            set_setting: f!(0x577D_D202, FnSetSetting),
            delete_setting: f!(0xE4A2_6362, FnDeleteSetting),
        })
    }
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(load_api).as_ref()
}

/// NVIDIA driver with DRS present.
pub fn available() -> bool {
    api().is_some()
}

fn ustr(s: &str) -> Box<UString> {
    let mut b = Box::new([0u16; UNICODE_MAX]);
    for (i, c) in s.encode_utf16().take(UNICODE_MAX - 1).enumerate() {
        b[i] = c;
    }
    b
}

fn from_ustr(u: &UString) -> String {
    let n = u.iter().position(|&c| c == 0).unwrap_or(u.len());
    String::from_utf16_lossy(&u[..n])
}

fn check(what: &str, st: i32) -> SysResult<()> {
    match st {
        NVAPI_OK => Ok(()),
        NVAPI_INVALID_USER_PRIVILEGE => Err(SysError::AccessDenied(format!("NVAPI {what}: needs administrator"))),
        NVAPI_SETTING_NOT_FOUND | NVAPI_PROFILE_NOT_FOUND | NVAPI_EXECUTABLE_NOT_FOUND => {
            Err(SysError::NotFound(format!("NVAPI {what}")))
        }
        e => Err(SysError::Other(format!("NVAPI {what}: status {e}"))),
    }
}

fn zeroed<T>() -> Box<T> {
    // Plain-old-data structs of integers: all-zero is a valid value.
    unsafe { Box::new(std::mem::zeroed()) }
}

/// One DRS session, destroyed on drop (even on error paths).
struct Session<'a> {
    api: &'a Api,
    h: H,
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.destroy_session)(self.h);
        }
    }
}

impl<'a> Session<'a> {
    fn open() -> SysResult<Self> {
        let api = api().ok_or_else(|| SysError::NotFound("NVIDIA driver profiles".into()))?;
        let mut h: H = std::ptr::null_mut();
        check("CreateSession", unsafe { (api.create_session)(&mut h) })?;
        let s = Session { api, h };
        check("LoadSettings", unsafe { (api.load)(s.h) })?;
        Ok(s)
    }

    fn save(&self) -> SysResult<()> {
        check("SaveSettings", unsafe { (self.api.save)(self.h) })
    }

    /// Profile the exe belongs to (`None` = none).
    fn app_profile(&self, exe: &str) -> SysResult<Option<H>> {
        let name = ustr(exe);
        let mut app = zeroed::<NvdrsApplicationV4>();
        app.version = APPLICATION_VER;
        let mut prof: H = std::ptr::null_mut();
        match check("FindApplicationByName", unsafe {
            (self.api.find_app)(self.h, name.as_ptr(), &mut prof, &mut *app)
        }) {
            Ok(()) => Ok(Some(prof)),
            Err(SysError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn profile_info(&self, p: H) -> SysResult<Box<NvdrsProfile>> {
        let mut info = zeroed::<NvdrsProfile>();
        info.version = PROFILE_VER;
        check("GetProfileInfo", unsafe { (self.api.profile_info)(self.h, p, &mut *info) })?;
        Ok(info)
    }

    fn user_value(&self, p: H, id: u32) -> SysResult<Option<u32>> {
        let mut s = zeroed::<NvdrsSetting>();
        s.version = SETTING_VER;
        match check("GetSetting", unsafe { (self.api.get_setting)(self.h, p, id, &mut *s) }) {
            Ok(()) => {
                let ours = s.setting_location == NVDRS_CURRENT_PROFILE_LOCATION && s.is_current_predefined == 0;
                Ok(ours.then(|| u32::from_le_bytes([s.current[0], s.current[1], s.current[2], s.current[3]])))
            }
            Err(SysError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn own_profile(&self, exe: &str) -> SysResult<H> {
        let pname = format!("{PROFILE_PREFIX}{exe}");
        let wname = ustr(&pname);
        let mut p: H = std::ptr::null_mut();
        match check("FindProfileByName", unsafe { (self.api.find_profile)(self.h, wname.as_ptr(), &mut p) }) {
            Ok(()) => {}
            Err(SysError::NotFound(_)) => {
                let mut info = zeroed::<NvdrsProfile>();
                info.version = PROFILE_VER;
                info.profile_name = *ustr(&pname);
                check("CreateProfile", unsafe { (self.api.create_profile)(self.h, &mut *info, &mut p) })?;
            }
            Err(e) => return Err(e),
        }
        let mut app = zeroed::<NvdrsApplicationV4>();
        app.version = APPLICATION_VER;
        app.app_name = *ustr(exe);
        check("CreateApplication", unsafe { (self.api.create_app)(self.h, p, &mut *app) })?;
        Ok(p)
    }
}

pub fn get(exe: &str, setting: DriverSetting) -> SysResult<Option<u32>> {
    let s = Session::open()?;
    match s.app_profile(exe)? {
        None => Ok(None),
        Some(p) => s.user_value(p, setting_id(setting)),
    }
}

pub fn set(exe: &str, setting: DriverSetting, value: u32) -> SysResult<()> {
    let s = Session::open()?;
    let p = match s.app_profile(exe)? {
        Some(p) => p,
        None => s.own_profile(exe)?,
    };
    let mut st = zeroed::<NvdrsSetting>();
    st.version = SETTING_VER;
    st.setting_id = setting_id(setting);
    st.setting_type = NVDRS_DWORD_TYPE;
    st.current[..4].copy_from_slice(&value.to_le_bytes());
    check("SetSetting", unsafe { (s.api.set_setting)(s.h, p, &mut *st) })?;
    s.save()
}

pub fn clear(exe: &str, setting: DriverSetting) -> SysResult<()> {
    let s = Session::open()?;
    let Some(p) = s.app_profile(exe)? else {
        return Ok(());
    };
    match check("DeleteProfileSetting", unsafe { (s.api.delete_setting)(s.h, p, setting_id(setting)) }) {
        Ok(()) | Err(SysError::NotFound(_)) => {}
        Err(e) => return Err(e),
    }
    let info = s.profile_info(p)?;
    if from_ustr(&info.profile_name) == format!("{PROFILE_PREFIX}{exe}") && info.num_of_settings == 0 {
        check("DeleteProfile", unsafe { (s.api.delete_profile)(s.h, p) })?;
    }
    s.save()
}

/// Name of the profile an exe belongs to (diagnostics and live tests).
pub fn profile_name(exe: &str) -> SysResult<Option<String>> {
    let s = Session::open()?;
    match s.app_profile(exe)? {
        None => Ok(None),
        Some(p) => Ok(Some(from_ustr(&s.profile_info(p)?.profile_name))),
    }
}
