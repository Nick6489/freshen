//! UTF-16/stdcall ABI for Inno Pascal Script. See docs/inno.md.
#![allow(non_snake_case)]
#[cfg(windows)]
#[path = "../../src/inno_pipe.rs"]
mod pipe;
#[path = "../../src/inno_wire.rs"]
mod wire;

#[cfg(windows)]
mod windows {
    use super::{pipe::Pipe, wire::Message};
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };
    struct Connection {
        pipe: Pipe,
        token: [u8; 32],
        last: Instant,
    }
    static CONNECTION: Mutex<Option<Connection>> = Mutex::new(None);

    unsafe fn text(pointer: *const u16, limit: usize) -> Option<String> {
        if pointer.is_null() {
            return None;
        }
        for length in 0..=limit {
            if unsafe { *pointer.add(length) } == 0 {
                return String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) })
                    .ok();
            }
        }
        None
    }
    /// Connect once, without waiting. Pointers must reference readable NUL-terminated UTF-16.
    /// Returns 0 on success; 1 on invalid input; 2 on unavailable pipe; 3 on lock failure.
    /// # Safety
    /// Both pointers must remain readable through their NUL terminators (at most 161/65 units).
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn FreshenInnoConnect(
        endpoint: *const u16,
        secret: *const u16,
    ) -> i32 {
        let Some(name) = (unsafe { text(endpoint, 160) }) else {
            return 1;
        };
        let Some(secret) = (unsafe { text(secret, 64) }) else {
            return 1;
        };
        if !name.starts_with(r"\\.\pipe\freshen-inno-") || secret.len() != 64 || !secret.is_ascii()
        {
            return 1;
        }
        let mut token = [0; 32];
        for (index, byte) in token.iter_mut().enumerate() {
            let Ok(value) = u8::from_str_radix(&secret[index * 2..index * 2 + 2], 16) else {
                return 1;
            };
            *byte = value;
        }
        let Ok(mut guard) = CONNECTION.try_lock() else {
            return 3;
        };
        let Ok(pipe) = Pipe::client(&name) else {
            return 2;
        };
        *guard = Some(Connection {
            pipe,
            token,
            last: Instant::now() - Duration::from_secs(1),
        });
        0
    }
    fn report(kind: u32, current: u32, total: u32) -> i32 {
        let Ok(mut guard) = CONNECTION.try_lock() else {
            return 3;
        };
        let Some(connection) = guard.as_mut() else {
            return 2;
        };
        let message = Message {
            token: connection.token,
            kind,
            current,
            total,
        };
        if Message::decode(&message.encode()).is_none() {
            return 1;
        }
        if connection.last.elapsed() < Duration::from_millis(100) && kind == 4 {
            return 0;
        }
        if kind == 4 {
            connection.last = Instant::now();
        }
        if connection.pipe.write(&message.encode()).is_err() {
            *guard = None;
            return 2;
        }
        0
    }
    /// Phase 1=preparing, 2=installing, 3=finalizing. Never reports successful completion.
    #[unsafe(no_mangle)]
    pub extern "system" fn FreshenInnoPhase(phase: u32) -> i32 {
        if !(1..=3).contains(&phase) {
            return 1;
        }
        report(phase, 0, 0)
    }
    /// Actual Inno callback values. Invalid totals are rejected; callbacks coalesce at 10 Hz.
    #[unsafe(no_mangle)]
    pub extern "system" fn FreshenInnoProgress(current: u32, total: u32) -> i32 {
        report(4, current, total)
    }
    /// Application-defined numeric warning code (no strings or executable commands).
    #[unsafe(no_mangle)]
    pub extern "system" fn FreshenInnoDiagnostic(code: u32) -> i32 {
        report(5, code, 0)
    }
    /// Idempotent close. No flushing or waiting for the peer.
    #[unsafe(no_mangle)]
    pub extern "system" fn FreshenInnoDisconnect() {
        if let Ok(mut guard) = CONNECTION.try_lock() {
            *guard = None;
        }
    }

    #[test]
    fn bridge_reports_real_values_and_never_waits_for_reader() {
        let name = format!(r"\\.\pipe\freshen-inno-dll-test-{}", std::process::id());
        let server = Pipe::server(&name).unwrap();
        let endpoint = super::pipe::wide(&name);
        let secret = super::pipe::wide(&"11".repeat(32));
        assert_eq!(
            unsafe { FreshenInnoConnect(endpoint.as_ptr(), secret.as_ptr()) },
            0
        );
        assert!(server.connect().unwrap());
        assert_eq!(FreshenInnoProgress(2, 3), 0);
        let message = Message::decode(&server.read().unwrap().unwrap()).unwrap();
        assert_eq!((message.current, message.total), (2, 3));
        assert_eq!(message.token, [0x11; 32]);
        assert_eq!(FreshenInnoProgress(4, 3), 1);
        let start = Instant::now();
        for _ in 0..10000 {
            let _ = FreshenInnoDiagnostic(42);
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        FreshenInnoDisconnect();
        assert_eq!(FreshenInnoPhase(2), 2);
        assert_eq!(
            unsafe { FreshenInnoConnect(std::ptr::null(), secret.as_ptr()) },
            1
        );
        drop(server);
        assert_eq!(
            unsafe { FreshenInnoConnect(endpoint.as_ptr(), secret.as_ptr()) },
            2
        );
    }
}
