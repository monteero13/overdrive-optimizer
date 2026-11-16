//! `overdrive.v1` transport (docs/architecture/ipc-protocol.md §1).
//!
//! - Named pipe `\\.\pipe\overdrive.v1`, byte mode, local clients only
//!   (`PIPE_REJECT_REMOTE_CLIENTS`), DACL from an SDDL string.
//! - Frames: `u32` little-endian length + UTF-8 JSON, at most [`MAX_FRAME`] bytes.
//!   Anything larger is refused and the connection is dropped.
//! - Overlapped I/O, so one thread can block in a read while another writes on the same
//!   handle. With synchronous handles Windows serializes both and the writer would wait
//!   for the next incoming frame.
//! - Clients open the pipe with `SECURITY_IDENTIFICATION`: the service can identify the
//!   caller but can never impersonate it.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND,
    ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL,
    WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::CreateEventW;

pub const PIPE_NAME: &str = r"\\.\pipe\overdrive.v1";
pub const MAX_FRAME: usize = 1 << 20;
/// SYSTEM and Administrators: full. Interactive users: read/write (connect and talk).
/// No access for network logons, services of other accounts or anonymous callers.
pub const DEFAULT_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

const BUF: u32 = 64 * 1024;

struct Handle(HANDLE);
// SAFETY: a kernel handle is just an index; Windows allows using it from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn win_err(e: windows::core::Error) -> io::Error {
    let code = WIN32_ERROR((e.code().0 as u32) & 0xFFFF);
    let kind = match code {
        ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA | ERROR_OPERATION_ABORTED => {
            io::ErrorKind::BrokenPipe
        }
        ERROR_FILE_NOT_FOUND => io::ErrorKind::NotFound,
        _ => return io::Error::from_raw_os_error(code.0 as i32),
    };
    io::Error::new(kind, e.message())
}

fn is(e: &windows::core::Error, code: WIN32_ERROR) -> bool {
    e.code() == code.to_hresult()
}

fn event() -> io::Result<Handle> {
    unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map(Handle)
        .map_err(win_err)
}

/// Wait for an overlapped call that may have returned `ERROR_IO_PENDING`.
fn finish(h: HANDLE, ov: &OVERLAPPED, started: windows::core::Result<()>) -> io::Result<u32> {
    if let Err(e) = started {
        if !is(&e, ERROR_IO_PENDING) {
            return Err(win_err(e));
        }
    }
    let mut n = 0u32;
    unsafe { GetOverlappedResult(h, ov, &mut n, true) }.map_err(win_err)?;
    Ok(n)
}

/// One end of a connection. Cheap to clone; reads and writes may run on different threads.
#[derive(Clone)]
pub struct Pipe {
    h: Arc<Handle>,
    write_lock: Arc<Mutex<()>>,
    server_side: bool,
}

impl Pipe {
    fn new(h: Handle, server_side: bool) -> Self {
        Self {
            h: Arc::new(h),
            write_lock: Arc::new(Mutex::new(())),
            server_side,
        }
    }

    fn raw(&self) -> HANDLE {
        self.h.0
    }

    fn read_exact(&self, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let ev = event()?;
            let ov = OVERLAPPED {
                hEvent: ev.0,
                ..Default::default()
            };
            let r = unsafe { ReadFile(self.raw(), Some(buf), None, Some(&ov as *const _ as *mut _)) };
            let n = finish(self.raw(), &ov, r)? as usize;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            buf = &mut buf[n..];
        }
        Ok(())
    }

    fn write_all(&self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let ev = event()?;
            let ov = OVERLAPPED {
                hEvent: ev.0,
                ..Default::default()
            };
            let r = unsafe { WriteFile(self.raw(), Some(buf), None, Some(&ov as *const _ as *mut _)) };
            let n = finish(self.raw(), &ov, r)? as usize;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            buf = &buf[n..];
        }
        Ok(())
    }

    /// Blocks until a whole frame arrives. Oversized or non-UTF-8 frames are an error
    /// (the caller must drop the connection).
    pub fn read_frame(&self) -> io::Result<String> {
        let mut len = [0u8; 4];
        self.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame of {len} bytes exceeds {MAX_FRAME}"),
            ));
        }
        let mut data = vec![0u8; len];
        self.read_exact(&mut data)?;
        String::from_utf8(data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn write_frame(&self, json: &str) -> io::Result<()> {
        if json.len() > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("frame of {} bytes exceeds {MAX_FRAME}", json.len()),
            ));
        }
        let mut buf = Vec::with_capacity(4 + json.len());
        buf.extend_from_slice(&(json.len() as u32).to_le_bytes());
        buf.extend_from_slice(json.as_bytes());
        let _g = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.write_all(&buf)
    }

    /// Writes raw bytes (tests: malformed or oversized frames).
    pub fn write_raw(&self, bytes: &[u8]) -> io::Result<()> {
        let _g = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.write_all(bytes)
    }

    /// Unblocks pending reads/writes on every clone and ends the connection.
    pub fn close(&self) {
        unsafe {
            let _ = CancelIoEx(self.raw(), None);
            if self.server_side {
                let _ = DisconnectNamedPipe(self.raw());
            }
        }
    }

    /// PID of the process on the other end (server side: the client; client side: the server).
    pub fn peer_pid(&self) -> io::Result<u32> {
        let mut pid = 0u32;
        unsafe {
            if self.server_side {
                GetNamedPipeClientProcessId(self.raw(), &mut pid)
            } else {
                GetNamedPipeServerProcessId(self.raw(), &mut pid)
            }
        }
        .map_err(win_err)?;
        Ok(pid)
    }
}

/// Terminal-services session of a process (0 = services), without opening it.
pub fn session_of(pid: u32) -> Option<u32> {
    let mut s = 0u32;
    unsafe { ProcessIdToSessionId(pid, &mut s) }.ok()?;
    Some(s)
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

/// Accepts connections. One instance is always listening, so the pipe name never
/// disappears between clients (another process cannot squat it).
pub struct Server {
    name: HSTRING,
    sd: SecurityDescriptor,
    next: Mutex<Option<Handle>>,
}

impl Server {
    /// Fails if another process already owns the pipe name (`FILE_FLAG_FIRST_PIPE_INSTANCE`).
    pub fn bind(name: &str, sddl: &str) -> io::Result<Self> {
        let mut psd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(sddl),
                SDDL_REVISION_1,
                &mut psd,
                None,
            )
        }
        .map_err(win_err)?;
        let s = Self {
            name: HSTRING::from(name),
            sd: SecurityDescriptor(psd),
            next: Mutex::new(None),
        };
        let first = s.instance(true)?;
        *s.next.lock().unwrap() = Some(first);
        Ok(s)
    }

    fn instance(&self, first: bool) -> io::Result<Handle> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.sd.0 .0,
            bInheritHandle: false.into(),
        };
        let mut mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let h = unsafe {
            CreateNamedPipeW(
                &self.name,
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUF,
                BUF,
                0,
                Some(&sa),
            )
        };
        if h.is_invalid() {
            return Err(win_err(unsafe { GetLastError() }.into()));
        }
        Ok(Handle(h))
    }

    /// Blocks until a client connects.
    pub fn accept(&self) -> io::Result<Pipe> {
        let h = match self.next.lock().unwrap().take() {
            Some(h) => h,
            None => self.instance(false)?,
        };
        loop {
            let ev = event()?;
            let ov = OVERLAPPED {
                hEvent: ev.0,
                ..Default::default()
            };
            let r = unsafe { ConnectNamedPipe(h.0, Some(&ov as *const _ as *mut _)) };
            match r {
                Err(e) if is(&e, ERROR_PIPE_CONNECTED) => break,
                // A client connected and already left: recycle the instance.
                Err(e) if is(&e, ERROR_NO_DATA) => unsafe {
                    let _ = DisconnectNamedPipe(h.0);
                },
                r => match finish(h.0, &ov, r) {
                    Ok(_) => break,
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => unsafe {
                        let _ = DisconnectNamedPipe(h.0);
                    },
                    Err(e) => return Err(e),
                },
            }
        }
        // Keep a listening instance before handing this one out.
        *self.next.lock().unwrap() = Some(self.instance(false)?);
        Ok(Pipe::new(h, true))
    }
}

/// Connect as a client. `NotFound` while the service isn't running.
pub fn connect(name: &str, timeout: Duration) -> io::Result<Pipe> {
    let wname = HSTRING::from(name);
    let deadline = Instant::now() + timeout;
    loop {
        let r = unsafe {
            CreateFileW(
                &wname,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED
                    | FILE_FLAGS_AND_ATTRIBUTES(SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0),
                None,
            )
        };
        match r {
            Ok(h) => return Ok(Pipe::new(Handle(h), false)),
            Err(e) if is(&e, ERROR_PIPE_BUSY) && Instant::now() < deadline => unsafe {
                let left = deadline.saturating_duration_since(Instant::now());
                let _ = WaitNamedPipeW(&wname, left.as_millis().clamp(1, 5000) as u32);
            },
            // Between two instances the name can vanish for a moment: retry until the deadline.
            Err(e) if is(&e, ERROR_FILE_NOT_FOUND) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => return Err(win_err(e)),
        }
    }
}
