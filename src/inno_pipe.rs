//! Nonblocking message-mode pipe shared by the helper and DLL.
#![allow(dead_code)] // The helper uses the server half; the DLL uses the client half.
use std::{io, ptr};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*, System::Pipes::*};

pub struct Pipe(pub HANDLE);
impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
// A handle has no thread affinity. Access remains serialized by its owner.
unsafe impl Send for Pipe {}

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

impl Pipe {
    pub fn server(name: &str) -> io::Result<Self> {
        use windows_sys::Win32::Security::{Authorization::*, SECURITY_ATTRIBUTES};
        let mut descriptor = ptr::null_mut();
        // Protected DACL: owner, administrators and SYSTEM only. Remote clients are rejected below.
        let sddl = wide("D:P(A;;GA;;;OW)(A;;GA;;;BA)(A;;GA;;;SY)");
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateNamedPipeW(
                wide(name).as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE
                    | PIPE_READMODE_MESSAGE
                    | PIPE_NOWAIT
                    | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                4096,
                0,
                &attributes,
            )
        };
        unsafe {
            LocalFree(descriptor);
        }
        if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
    pub fn client(name: &str) -> io::Result<Self> {
        let handle = unsafe {
            CreateFileW(
                wide(name).as_ptr(),
                GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let pipe = Self(handle);
        let mode = PIPE_NOWAIT;
        if unsafe { SetNamedPipeHandleState(handle, &mode, ptr::null(), ptr::null()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(pipe)
    }
    pub fn connect(&self) -> io::Result<bool> {
        if unsafe { ConnectNamedPipe(self.0, ptr::null_mut()) } != 0 {
            return Ok(true);
        }
        match io::Error::last_os_error().raw_os_error().map(|n| n as u32) {
            Some(ERROR_PIPE_CONNECTED) => Ok(true),
            Some(ERROR_PIPE_LISTENING | ERROR_NO_DATA) => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub fn read(&self) -> io::Result<Option<[u8; super::wire::SIZE]>> {
        let mut buffer = [0; super::wire::SIZE];
        let mut count = 0;
        let ok = unsafe {
            ReadFile(
                self.0,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut count,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_DATA as i32) {
                return Ok(None);
            }
            return Err(error);
        }
        if count != buffer.len() as u32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid telemetry length",
            ));
        }
        Ok(Some(buffer))
    }
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut written = 0;
        if unsafe {
            WriteFile(
                self.0,
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if written != bytes.len() as u32 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "telemetry buffer full",
            ));
        }
        Ok(())
    }
}
