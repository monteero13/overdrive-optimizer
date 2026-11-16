//! Who is running the service, and who is sitting at the console.
//!
//! The service runs as LocalSystem; per-user settings (Game DVR, GPU preference, Steam
//! path…) live in the interactive user's hive under `HKEY_USERS\<SID>`. These helpers
//! find that SID. Read-only.

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub const LOCAL_SYSTEM_SID: &str = "S-1-5-18";

fn token_sid(token: HANDLE) -> Option<String> {
    unsafe {
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len)
            .ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        out
    }
}

/// SID of the account this process runs as.
pub fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let sid = token_sid(token);
        let _ = CloseHandle(token);
        sid
    }
}

pub fn is_local_system() -> bool {
    current_user_sid().as_deref() == Some(LOCAL_SYSTEM_SID)
}

/// SID of the user logged on at the physical console, if any.
/// `WTSQueryUserToken` only works from LocalSystem; elsewhere this returns `None`.
pub fn console_user_sid() -> Option<String> {
    unsafe {
        let session = WTSGetActiveConsoleSessionId();
        if session == u32::MAX {
            return None;
        }
        let mut token = HANDLE::default();
        WTSQueryUserToken(session, &mut token).ok()?;
        let sid = token_sid(token);
        let _ = CloseHandle(token);
        sid
    }
}

/// Session id of the physical console (`None` while nobody is attached).
pub fn console_session_id() -> Option<u32> {
    let s = unsafe { WTSGetActiveConsoleSessionId() };
    (s != u32::MAX).then_some(s)
}
