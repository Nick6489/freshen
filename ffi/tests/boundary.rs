//! Garbage-argument tests for the C ABI.
//!
//! The rule for every hostile call here: the library must return a status code. It may
//! not crash, hang, or take its panic-catching path (`FRESHEN_ERR_PANIC` means an
//! `unwrap`, index or arithmetic overflow was reachable from bad input, which is a bug even
//! though the boundary contained it). Every failure must also leave a message.
//!
//! What this can and cannot cover. It covers anything the library can detect: null pointers,
//! invalid UTF-8, undersized structs, unknown enum values, absurd counts and lengths,
//! out-of-range numbers, hostile transport callbacks, misuse of consumed handles, and
//! corrupt on-disk state. It cannot cover what no C API can detect: a non-null pointer that
//! does not point at what the caller claims, a count that lies about a real buffer, double
//! frees, or using a sink after its callback returned. Those are the caller's contract and
//! are documented in the header.
//!
//! Tests run in a debug build, so arithmetic overflow panics and would surface as a
//! `FRESHEN_ERR_PANIC`.

mod support;

use freshen_ffi::*;
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    fs,
    path::{Path, PathBuf},
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicPtr, AtomicUsize, Ordering},
    },
};

// ---------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------

const KNOWN_STATUSES: [i32; 17] = [
    FRESHEN_OK,
    FRESHEN_ERR_CANCELLED,
    FRESHEN_ERR_SIGNATURE,
    FRESHEN_ERR_INVALID,
    FRESHEN_ERR_UNSAFE_PATH,
    FRESHEN_ERR_SIZE_LIMIT,
    FRESHEN_ERR_HASH_MISMATCH,
    FRESHEN_ERR_BUSY,
    FRESHEN_ERR_HELPER,
    FRESHEN_ERR_RECOVERY,
    FRESHEN_ERR_IO,
    FRESHEN_ERR_JSON,
    FRESHEN_ERR_HTTP,
    FRESHEN_ERR_ZIP,
    FRESHEN_ERR_UNKNOWN,
    FRESHEN_ERR_INVALID_ARGUMENT,
    FRESHEN_ERR_PANIC,
];

fn message() -> String {
    let text = freshen_last_error_message();
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// Asserts a status is a code this ABI defines, is not the caught-panic code, and that a
/// failure explains itself.
#[track_caller]
fn sane(status: i32, what: &str) -> i32 {
    assert_ne!(
        status,
        FRESHEN_ERR_PANIC,
        "{what}: reached the panic-catching path ({})",
        message()
    );
    assert!(
        KNOWN_STATUSES.contains(&status),
        "{what}: returned undefined status {status}"
    );
    if status != FRESHEN_OK {
        assert!(!message().is_empty(), "{what}: failed without a message");
    }
    status
}

#[track_caller]
fn expect(status: i32, wanted: i32, what: &str) {
    sane(status, what);
    assert_eq!(status, wanted, "{what}: {}", message());
}

fn cs(text: &str) -> CString {
    CString::new(text).unwrap()
}

/// A C string that is not valid UTF-8.
fn bad_utf8() -> CString {
    CString::new(vec![0xff, 0xfe, 0x80]).unwrap()
}

fn take(text: *mut c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let owned = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    unsafe { freshen_string_free(text) };
    Some(owned)
}

// ---------------------------------------------------------------------------------
// A hostile host transport
// ---------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Behavior {
    /// Serve the fixtures properly.
    Serve,
    /// Return this status without writing anything.
    Status(i32),
    /// Claim success without writing a byte.
    SucceedEmpty,
    /// Pass a null buffer with a nonzero length to the sink.
    NullBufferWithLength,
    /// Pass an absurd length to the sink.
    AbsurdLength,
    /// Declare a total larger than any limit.
    HugeTotal,
    /// Call back into the library (a nested check) before serving.
    Reenter,
}

struct Host {
    files: HashMap<&'static str, Vec<u8>>,
    behavior: Mutex<Behavior>,
    updater: AtomicPtr<FreshenUpdater>,
    calls: AtomicUsize,
    sink_statuses: Mutex<Vec<i32>>,
    nested_status: Mutex<Option<i32>>,
}

impl Host {
    fn new(fixtures: &Path) -> Self {
        let read = |name: &str| fs::read(fixtures.join(name)).unwrap();
        let mut files = HashMap::new();
        files.insert("index", read("index.json"));
        files.insert("m/1.5.0", read("v1.5.0.manifest"));
        files.insert("s/1.5.0", read("v1.5.0.sig"));
        files.insert("m/2.0.0", read("v2.0.0.manifest"));
        files.insert("s/2.0.0", read("v2.0.0.sig"));
        files.insert("bad-sig", read("tampered.sig"));
        files.insert("package", read("package.zip"));
        Host {
            files,
            behavior: Mutex::new(Behavior::Serve),
            updater: AtomicPtr::new(ptr::null_mut()),
            calls: AtomicUsize::new(0),
            sink_statuses: Mutex::new(Vec::new()),
            nested_status: Mutex::new(None),
        }
    }

    fn set(&self, behavior: Behavior) {
        *self.behavior.lock().unwrap() = behavior;
    }
}

unsafe extern "C" fn host_download(
    user: *mut c_void,
    url: *const c_char,
    sink: *mut FreshenSink,
    _limit: u64,
) -> i32 {
    let host = unsafe { &*(user as *const Host) };
    host.calls.fetch_add(1, Ordering::SeqCst);
    let behavior = *host.behavior.lock().unwrap();
    let record = |status: i32| {
        host.sink_statuses.lock().unwrap().push(status);
        status
    };
    match behavior {
        Behavior::Status(status) => return status,
        Behavior::SucceedEmpty => return FRESHEN_OK,
        Behavior::NullBufferWithLength => {
            let status = record(unsafe { freshen_sink_write(sink, ptr::null(), 10) });
            // A careless host ignores the failure and reports success.
            let _ = status;
            return FRESHEN_OK;
        }
        Behavior::AbsurdLength => {
            let byte = 0u8;
            let status = record(unsafe { freshen_sink_write(sink, &byte, usize::MAX) });
            let _ = status;
            return FRESHEN_OK;
        }
        Behavior::HugeTotal => {
            let status = record(unsafe { freshen_sink_set_total(sink, u64::MAX) });
            return status;
        }
        Behavior::Reenter => {
            // Nested use of the same updater from inside a transport callback.
            let updater = host.updater.load(Ordering::SeqCst);
            host.set(Behavior::Serve);
            let index = cs("https://test.invalid/index");
            let mut source: FreshenSource = unsafe { std::mem::zeroed() };
            source.struct_size = std::mem::size_of::<FreshenSource>();
            source.kind = FRESHEN_SOURCE_INDEX;
            source.a = index.as_ptr();
            let mut candidate: *mut FreshenCandidate = ptr::null_mut();
            let nested = unsafe {
                freshen_check(
                    updater,
                    &source,
                    ptr::null(),
                    None,
                    ptr::null_mut(),
                    &mut candidate,
                )
            };
            *host.nested_status.lock().unwrap() = Some(nested);
            unsafe { freshen_candidate_free(candidate) };
        }
        Behavior::Serve => {}
    }
    let text = unsafe { CStr::from_ptr(url) }.to_str().unwrap();
    let Some(path) = text.strip_prefix("https://test.invalid/") else {
        return FRESHEN_ERR_IO;
    };
    let Some(bytes) = host.files.get(path) else {
        return FRESHEN_ERR_IO;
    };
    let status = unsafe { freshen_sink_set_total(sink, bytes.len() as u64) };
    if status != FRESHEN_OK {
        return status;
    }
    for chunk in bytes.chunks(4096) {
        let status = unsafe { freshen_sink_write(sink, chunk.as_ptr(), chunk.len()) };
        if status != FRESHEN_OK {
            return status;
        }
    }
    FRESHEN_OK
}

// ---------------------------------------------------------------------------------
// Fixture, config and source builders
// ---------------------------------------------------------------------------------

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    key: [u8; 32],
    host: Box<Host>,
}

impl Fx {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = dir.path().join("fixtures");
        fs::create_dir(&fixtures).unwrap();
        support::write_fixtures(&fixtures, b"boundary test application");
        let work = dir.path().join("work");
        fs::create_dir(&work).unwrap();
        let root = dir.path().join("install");
        fs::create_dir(&root).unwrap();
        let key: [u8; 32] = fs::read(fixtures.join("pubkey"))
            .unwrap()
            .try_into()
            .unwrap();
        let host = Box::new(Host::new(&fixtures));
        Fx {
            _dir: dir,
            root,
            work,
            key,
            host,
        }
    }

    fn transport(&self) -> FreshenTransport {
        FreshenTransport {
            struct_size: std::mem::size_of::<FreshenTransport>(),
            user: &*self.host as *const Host as *mut c_void,
            download: Some(host_download),
        }
    }
}

/// C strings for a config, kept alive together.
struct Strings {
    product: CString,
    channel: CString,
    version: CString,
    target: CString,
}

impl Strings {
    fn standard() -> Self {
        Strings {
            product: cs("ffi-test"),
            channel: cs("stable"),
            version: cs("1.0.0"),
            target: cs("test-target"),
        }
    }
}

fn config(
    strings: &Strings,
    key: &[u8; 32],
    transport: *const FreshenTransport,
) -> FreshenUpdaterConfig {
    FreshenUpdaterConfig {
        struct_size: std::mem::size_of::<FreshenUpdaterConfig>(),
        product: strings.product.as_ptr(),
        channel: strings.channel.as_ptr(),
        current_version: strings.version.as_ptr(),
        target: strings.target.as_ptr(),
        trusted_keys: key.as_ptr(),
        trusted_key_count: 1,
        max_download: 512 * 1024 * 1024,
        max_unpacked: 64 * 1024 * 1024,
        http_timeout_secs: 0,
        transport,
    }
}

/// Creates a working updater against the fixture host.
fn updater(fx: &Fx) -> *mut FreshenUpdater {
    let strings = Strings::standard();
    let transport = fx.transport();
    let cfg = config(&strings, &fx.key, &transport);
    let mut out = ptr::null_mut();
    expect(
        unsafe { freshen_updater_new(&cfg, &mut out) },
        FRESHEN_OK,
        "updater_new",
    );
    assert!(!out.is_null());
    fx.host.updater.store(out, Ordering::SeqCst);
    out
}

fn source(kind: i32, a: *const c_char, b: *const c_char) -> FreshenSource {
    FreshenSource {
        struct_size: std::mem::size_of::<FreshenSource>(),
        kind,
        a,
        b,
        locations: ptr::null(),
        location_count: 0,
    }
}

fn check(u: *const FreshenUpdater, s: &FreshenSource) -> (i32, *mut FreshenCandidate) {
    let mut out: *mut FreshenCandidate = ptr::null_mut();
    let status = unsafe { freshen_check(u, s, ptr::null(), None, ptr::null_mut(), &mut out) };
    (status, out)
}

fn candidate_from(fx: &Fx, u: *const FreshenUpdater) -> *mut FreshenCandidate {
    let index = cs("https://test.invalid/index");
    let (status, candidate) = check(
        u,
        &source(FRESHEN_SOURCE_INDEX, index.as_ptr(), ptr::null()),
    );
    expect(status, FRESHEN_OK, "check");
    assert!(!candidate.is_null());
    let _ = fx;
    candidate
}

fn portable_policy(root: &CStr, exe: &CStr, owned: &[*const c_char]) -> FreshenInstallPolicy {
    FreshenInstallPolicy {
        struct_size: std::mem::size_of::<FreshenInstallPolicy>(),
        layout: FRESHEN_LAYOUT_PORTABLE,
        root: root.as_ptr(),
        executable: exe.as_ptr(),
        owned_files: owned.as_ptr(),
        owned_file_count: owned.len(),
        expandable_directories: ptr::null(),
        expandable_directory_count: 0,
        bundle_name: ptr::null(),
        bundle_id: ptr::null(),
        team_id: ptr::null(),
        exit_timeout_secs: 0,
        confirmation_timeout_secs: 0,
    }
}

// ---------------------------------------------------------------------------------
// Null pointers
// ---------------------------------------------------------------------------------

#[test]
fn null_pointers_are_rejected_everywhere() {
    let fx = Fx::new();
    let strings = Strings::standard();
    let transport = fx.transport();
    let good = config(&strings, &fx.key, &transport);
    let mut out: *mut FreshenUpdater = ptr::null_mut();

    // updater_new: the config, the out-pointer, and every required field.
    expect(
        unsafe { freshen_updater_new(ptr::null(), &mut out) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "config",
    );
    assert!(out.is_null());
    expect(
        unsafe { freshen_updater_new(&good, ptr::null_mut()) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "out",
    );
    type Edit = fn(&mut FreshenUpdaterConfig);
    let edits: [(&str, Edit); 6] = [
        ("product", |c| c.product = ptr::null()),
        ("channel", |c| c.channel = ptr::null()),
        ("current_version", |c| c.current_version = ptr::null()),
        ("target", |c| c.target = ptr::null()),
        ("trusted_keys", |c| c.trusted_keys = ptr::null()),
        ("trusted_key_count", |c| c.trusted_key_count = 0),
    ];
    for (name, edit) in edits {
        let mut cfg = config(&strings, &fx.key, &transport);
        edit(&mut cfg);
        let mut out: *mut FreshenUpdater = ptr::null_mut();
        expect(
            unsafe { freshen_updater_new(&cfg, &mut out) },
            FRESHEN_ERR_INVALID_ARGUMENT,
            name,
        );
        assert!(out.is_null(), "{name}: out must be null on failure");
    }
    let no_download = FreshenTransport {
        struct_size: std::mem::size_of::<FreshenTransport>(),
        user: ptr::null_mut(),
        download: None,
    };
    let cfg = config(&strings, &fx.key, &no_download);
    expect(
        unsafe { freshen_updater_new(&cfg, &mut out) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "transport.download",
    );

    // check
    let u = updater(&fx);
    let index = cs("https://test.invalid/index");
    let src = source(FRESHEN_SOURCE_INDEX, index.as_ptr(), ptr::null());
    let mut cand: *mut FreshenCandidate = ptr::null_mut();
    expect(
        unsafe {
            freshen_check(
                ptr::null(),
                &src,
                ptr::null(),
                None,
                ptr::null_mut(),
                &mut cand,
            )
        },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "check updater",
    );
    expect(
        unsafe {
            freshen_check(
                u,
                ptr::null(),
                ptr::null(),
                None,
                ptr::null_mut(),
                &mut cand,
            )
        },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "check source",
    );
    expect(
        unsafe { freshen_check(u, &src, ptr::null(), None, ptr::null_mut(), ptr::null_mut()) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "check out",
    );
    let no_url = source(FRESHEN_SOURCE_INDEX, ptr::null(), ptr::null());
    expect(
        check(u, &no_url).0,
        FRESHEN_ERR_INVALID_ARGUMENT,
        "index url",
    );
    let half = source(FRESHEN_SOURCE_MANIFEST, index.as_ptr(), ptr::null());
    expect(
        check(u, &half).0,
        FRESHEN_ERR_INVALID_ARGUMENT,
        "manifest signature url",
    );
    let mut listed = source(FRESHEN_SOURCE_MANIFESTS, ptr::null(), ptr::null());
    listed.location_count = 3;
    expect(
        check(u, &listed).0,
        FRESHEN_ERR_INVALID_ARGUMENT,
        "locations null with count",
    );
    let entry = FreshenLocation {
        document: ptr::null(),
        signature: index.as_ptr(),
    };
    listed.locations = &entry;
    listed.location_count = 1;
    expect(
        check(u, &listed).0,
        FRESHEN_ERR_INVALID_ARGUMENT,
        "location document null",
    );

    // prepare
    let good_candidate = candidate_from(&fx, u);
    let work = cs(fx.work.to_str().unwrap());
    let mut prep: *mut FreshenPrepared = ptr::null_mut();
    let prepare = |updater, candidate, parent, out| unsafe {
        freshen_prepare(
            updater,
            candidate,
            parent,
            ptr::null(),
            None,
            ptr::null_mut(),
            out,
        )
    };
    expect(
        prepare(ptr::null(), good_candidate, work.as_ptr(), &mut prep),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "prepare updater",
    );
    expect(
        prepare(u, ptr::null(), work.as_ptr(), &mut prep),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "prepare candidate",
    );
    expect(
        prepare(u, good_candidate, ptr::null(), &mut prep),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "prepare parent",
    );
    expect(
        prepare(u, good_candidate, work.as_ptr(), ptr::null_mut()),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "prepare out",
    );
    assert_eq!(
        fs::read_dir(&fx.work).unwrap().count(),
        0,
        "nothing staged on failure"
    );

    // arm
    expect(
        prepare(u, good_candidate, work.as_ptr(), &mut prep),
        FRESHEN_OK,
        "prepare",
    );
    let root = cs(fx.work.to_str().unwrap());
    let exe = cs("application");
    let owned = [exe.as_ptr()];
    let policy = portable_policy(&root, &exe, &owned);
    let helper = cs("/nonexistent-helper");
    let mut hand: *mut FreshenHandoff = ptr::null_mut();
    let arm = |prepared, policy, helper, args, count, out| unsafe {
        freshen_arm(prepared, policy, helper, args, count, out)
    };
    expect(
        arm(
            ptr::null_mut(),
            &policy,
            helper.as_ptr(),
            ptr::null(),
            0,
            &mut hand,
        ),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "arm prepared",
    );
    expect(
        arm(
            prep,
            ptr::null(),
            helper.as_ptr(),
            ptr::null(),
            0,
            &mut hand,
        ),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "arm policy",
    );
    // The two rejections above happen after the update is taken, so use a fresh one.
    let mut prep2: *mut FreshenPrepared = ptr::null_mut();
    expect(
        prepare(u, good_candidate, work.as_ptr(), &mut prep2),
        FRESHEN_OK,
        "prepare 2",
    );
    expect(
        arm(prep2, &policy, ptr::null(), ptr::null(), 0, &mut hand),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "arm helper",
    );
    let mut prep3: *mut FreshenPrepared = ptr::null_mut();
    expect(
        prepare(u, good_candidate, work.as_ptr(), &mut prep3),
        FRESHEN_OK,
        "prepare 3",
    );
    expect(
        arm(prep3, &policy, helper.as_ptr(), ptr::null(), 3, &mut hand),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "arm args null with count",
    );
    let mut prep4: *mut FreshenPrepared = ptr::null_mut();
    expect(
        prepare(u, good_candidate, work.as_ptr(), &mut prep4),
        FRESHEN_OK,
        "prepare 4",
    );
    expect(
        arm(
            prep4,
            &policy,
            helper.as_ptr(),
            ptr::null(),
            0,
            ptr::null_mut(),
        ),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "arm out",
    );
    let mut broken = portable_policy(&root, &exe, &owned);
    broken.root = ptr::null();
    let mut prep5: *mut FreshenPrepared = ptr::null_mut();
    expect(
        prepare(u, good_candidate, work.as_ptr(), &mut prep5),
        FRESHEN_OK,
        "prepare 5",
    );
    expect(
        arm(prep5, &broken, helper.as_ptr(), ptr::null(), 0, &mut hand),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "policy root",
    );
    let mut broken = portable_policy(&root, &exe, &owned);
    broken.owned_files = ptr::null();
    broken.owned_file_count = 2;
    let mut prep6: *mut FreshenPrepared = ptr::null_mut();
    expect(
        prepare(u, good_candidate, work.as_ptr(), &mut prep6),
        FRESHEN_OK,
        "prepare 6",
    );
    expect(
        arm(prep6, &broken, helper.as_ptr(), ptr::null(), 0, &mut hand),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "policy owned null with count",
    );
    let null_entry = [ptr::null::<c_char>()];
    let mut broken = portable_policy(&root, &exe, &null_entry);
    broken.owned_file_count = 1;
    let mut prep7: *mut FreshenPrepared = ptr::null_mut();
    expect(
        prepare(u, good_candidate, work.as_ptr(), &mut prep7),
        FRESHEN_OK,
        "prepare 7",
    );
    expect(
        arm(prep7, &broken, helper.as_ptr(), ptr::null(), 0, &mut hand),
        FRESHEN_ERR_INVALID_ARGUMENT,
        "policy owned entry null",
    );

    // status, recovery, discard, confirmation, helper dispatch
    let mut state = -1;
    let mut version: *mut c_char = ptr::null_mut();
    let mut detail: *mut c_char = ptr::null_mut();
    expect(
        unsafe { freshen_installation_status(ptr::null(), &mut state, &mut version, &mut detail) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "status root",
    );
    expect(
        unsafe {
            freshen_installation_status(root.as_ptr(), ptr::null_mut(), &mut version, &mut detail)
        },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "status out_state",
    );
    expect(
        unsafe { freshen_recover_installation(ptr::null(), &mut state, &mut version, &mut detail) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "recover root",
    );
    expect(
        unsafe { freshen_discard_finished_installation(ptr::null()) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "discard root",
    );
    let mut confirmed = false;
    expect(
        unsafe { freshen_confirm_startup(ptr::null(), &mut confirmed) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "confirm root",
    );
    expect(
        unsafe { freshen_confirm_startup(root.as_ptr(), ptr::null_mut()) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "confirm out",
    );
    let mut handled = false;
    expect(
        unsafe { freshen_run_helper(2, ptr::null(), &mut handled) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "run_helper argv null with count",
    );
    expect(
        unsafe { freshen_run_helper(0, ptr::null(), ptr::null_mut()) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "run_helper out",
    );
    let with_null = [ptr::null::<c_char>()];
    expect(
        unsafe { freshen_run_helper(1, with_null.as_ptr(), &mut handled) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "run_helper null argument",
    );

    // The sink outside a callback: null.
    expect(
        unsafe { freshen_sink_write(ptr::null_mut(), ptr::null(), 0) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "sink_write",
    );
    expect(
        unsafe { freshen_sink_set_total(ptr::null_mut(), 1) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "sink_set_total",
    );
    assert!(
        unsafe { freshen_sink_is_cancelled(ptr::null()) },
        "a null sink reads as cancelled"
    );

    // Accessors on null handles are harmless.
    unsafe {
        assert!(freshen_candidate_version(ptr::null()).is_null());
        assert!(freshen_candidate_notes(ptr::null()).is_null());
        assert_eq!(freshen_candidate_download_size(ptr::null()), 0);
        assert_eq!(freshen_candidate_history_count(ptr::null()), 0);
        assert!(freshen_candidate_history_version(ptr::null(), 0).is_null());
        assert!(freshen_candidate_history_notes(ptr::null(), 0).is_null());
        assert!(freshen_prepared_version(ptr::null()).is_null());
        assert!(freshen_prepared_notes(ptr::null()).is_null());
        assert_eq!(freshen_prepared_history_count(ptr::null()), 0);
        assert!(freshen_prepared_history_version(ptr::null(), 0).is_null());
        assert!(freshen_prepared_history_notes(ptr::null(), 0).is_null());
        assert_eq!(freshen_handoff_helper_pid(ptr::null()), 0);
        assert!(freshen_cancel_clone(ptr::null()).is_null());
        expect(
            freshen_handoff_cancel(ptr::null_mut()),
            FRESHEN_ERR_INVALID_ARGUMENT,
            "handoff_cancel",
        );
        // Freeing null is a documented no-op for every handle type.
        freshen_string_free(ptr::null_mut());
        freshen_updater_free(ptr::null_mut());
        freshen_candidate_free(ptr::null_mut());
        freshen_prepared_free(ptr::null_mut());
        freshen_handoff_free(ptr::null_mut());
        freshen_cancel_free(ptr::null_mut());
        freshen_cancel_cancel(ptr::null());
    }

    unsafe {
        freshen_prepared_free(prep);
        freshen_prepared_free(prep2);
        freshen_prepared_free(prep3);
        freshen_prepared_free(prep4);
        freshen_prepared_free(prep5);
        freshen_prepared_free(prep6);
        freshen_prepared_free(prep7);
        freshen_candidate_free(good_candidate);
        freshen_updater_free(u);
    }
    assert_eq!(
        fs::read_dir(&fx.work).unwrap().count(),
        0,
        "every staged update was released"
    );
}

// ---------------------------------------------------------------------------------
// Invalid UTF-8
// ---------------------------------------------------------------------------------

#[test]
fn invalid_utf8_is_rejected_in_every_string_parameter() {
    let fx = Fx::new();
    let bad = bad_utf8();
    let transport = fx.transport();

    // Each config string in turn.
    for field in 0..4 {
        let mut strings = Strings::standard();
        match field {
            0 => strings.product = bad.clone(),
            1 => strings.channel = bad.clone(),
            2 => strings.version = bad.clone(),
            _ => strings.target = bad.clone(),
        }
        let cfg = config(&strings, &fx.key, &transport);
        let mut out: *mut FreshenUpdater = ptr::null_mut();
        expect(
            unsafe { freshen_updater_new(&cfg, &mut out) },
            FRESHEN_ERR_INVALID_ARGUMENT,
            &format!("config string {field}"),
        );
        assert!(out.is_null());
    }

    let u = updater(&fx);
    // Sources.
    let good = cs("https://test.invalid/index");
    for (kind, a, b) in [
        (FRESHEN_SOURCE_INDEX, bad.as_ptr(), ptr::null()),
        (FRESHEN_SOURCE_MANIFEST, bad.as_ptr(), good.as_ptr()),
        (FRESHEN_SOURCE_MANIFEST, good.as_ptr(), bad.as_ptr()),
        (FRESHEN_SOURCE_GITHUB, bad.as_ptr(), good.as_ptr()),
        (FRESHEN_SOURCE_GITHUB, good.as_ptr(), bad.as_ptr()),
    ] {
        expect(
            check(u, &source(kind, a, b)).0,
            FRESHEN_ERR_INVALID_ARGUMENT,
            "source string",
        );
    }
    let entry = FreshenLocation {
        document: bad.as_ptr(),
        signature: good.as_ptr(),
    };
    let mut listed = source(FRESHEN_SOURCE_MANIFESTS, ptr::null(), ptr::null());
    listed.locations = &entry;
    listed.location_count = 1;
    expect(
        check(u, &listed).0,
        FRESHEN_ERR_INVALID_ARGUMENT,
        "location string",
    );

    // prepare / arm / status paths.
    let candidate = candidate_from(&fx, u);
    let mut prepared: *mut FreshenPrepared = ptr::null_mut();
    expect(
        unsafe {
            freshen_prepare(
                u,
                candidate,
                bad.as_ptr(),
                ptr::null(),
                None,
                ptr::null_mut(),
                &mut prepared,
            )
        },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "temporary_parent",
    );
    let work = cs(fx.work.to_str().unwrap());
    expect(
        unsafe {
            freshen_prepare(
                u,
                candidate,
                work.as_ptr(),
                ptr::null(),
                None,
                ptr::null_mut(),
                &mut prepared,
            )
        },
        FRESHEN_OK,
        "prepare",
    );
    let exe = cs("application");
    let owned = [exe.as_ptr()];
    let policy = portable_policy(&work, &exe, &owned);
    let mut hand: *mut FreshenHandoff = ptr::null_mut();
    let helper_bad =
        unsafe { freshen_arm(prepared, &policy, bad.as_ptr(), ptr::null(), 0, &mut hand) };
    expect(helper_bad, FRESHEN_ERR_INVALID_ARGUMENT, "helper path");
    let mut state = -1;
    let mut version: *mut c_char = ptr::null_mut();
    let mut detail: *mut c_char = ptr::null_mut();
    expect(
        unsafe { freshen_installation_status(bad.as_ptr(), &mut state, &mut version, &mut detail) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "status root",
    );
    expect(
        unsafe {
            freshen_recover_installation(bad.as_ptr(), &mut state, &mut version, &mut detail)
        },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "recover root",
    );
    expect(
        unsafe { freshen_discard_finished_installation(bad.as_ptr()) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "discard root",
    );
    let mut confirmed = false;
    expect(
        unsafe { freshen_confirm_startup(bad.as_ptr(), &mut confirmed) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "confirm root",
    );
    let mut handled = false;
    let args = [bad.as_ptr()];
    expect(
        unsafe { freshen_run_helper(1, args.as_ptr(), &mut handled) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "helper argument",
    );

    unsafe {
        freshen_prepared_free(prepared);
        freshen_candidate_free(candidate);
        freshen_updater_free(u);
    }
}

// ---------------------------------------------------------------------------------
// Struct sizes and unknown enum values
// ---------------------------------------------------------------------------------

#[test]
fn undersized_structs_and_unknown_enums_are_rejected() {
    let fx = Fx::new();
    let strings = Strings::standard();
    let transport = fx.transport();

    let full = std::mem::size_of::<FreshenUpdaterConfig>();
    for size in [0, 1, 8, full - 1] {
        let mut cfg = config(&strings, &fx.key, &transport);
        cfg.struct_size = size;
        let mut out: *mut FreshenUpdater = ptr::null_mut();
        expect(
            unsafe { freshen_updater_new(&cfg, &mut out) },
            FRESHEN_ERR_INVALID_ARGUMENT,
            "config struct_size",
        );
        assert!(out.is_null());
    }
    // A LARGER struct is what a newer host would pass; it must be accepted.
    let mut cfg = config(&strings, &fx.key, &transport);
    cfg.struct_size = full + 64;
    let mut out: *mut FreshenUpdater = ptr::null_mut();
    expect(
        unsafe { freshen_updater_new(&cfg, &mut out) },
        FRESHEN_OK,
        "larger struct_size",
    );
    unsafe { freshen_updater_free(out) };

    let mut short_transport = fx.transport();
    short_transport.struct_size = std::mem::size_of::<FreshenTransport>() - 1;
    let cfg = config(&strings, &fx.key, &short_transport);
    let mut out: *mut FreshenUpdater = ptr::null_mut();
    expect(
        unsafe { freshen_updater_new(&cfg, &mut out) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "transport struct_size",
    );

    let u = updater(&fx);
    let index = cs("https://test.invalid/index");
    for size in [0, 1, std::mem::size_of::<FreshenSource>() - 1] {
        let mut src = source(FRESHEN_SOURCE_INDEX, index.as_ptr(), ptr::null());
        src.struct_size = size;
        expect(
            check(u, &src).0,
            FRESHEN_ERR_INVALID_ARGUMENT,
            "source struct_size",
        );
    }
    for kind in [0, -1, 5, 99, i32::MAX, i32::MIN] {
        expect(
            check(u, &source(kind, index.as_ptr(), index.as_ptr())).0,
            FRESHEN_ERR_INVALID_ARGUMENT,
            &format!("source kind {kind}"),
        );
    }

    let candidate = candidate_from(&fx, u);
    let work = cs(fx.work.to_str().unwrap());
    let exe = cs("application");
    let owned = [exe.as_ptr()];
    let helper = cs("/nonexistent-helper");
    let mut variants: Vec<(&str, FreshenInstallPolicy)> = Vec::new();
    for size in [0, 1, std::mem::size_of::<FreshenInstallPolicy>() - 1] {
        let mut p = portable_policy(&work, &exe, &owned);
        p.struct_size = size;
        variants.push(("policy struct_size", p));
    }
    for layout in [0, -1, 3, 99, i32::MAX, i32::MIN] {
        let mut p = portable_policy(&work, &exe, &owned);
        p.layout = layout;
        variants.push(("policy layout", p));
    }
    let mut bundle = portable_policy(&work, &exe, &owned);
    bundle.layout = FRESHEN_LAYOUT_MAC_BUNDLE; // bundle_name/bundle_id/team_id are null
    variants.push(("mac bundle with null fields", bundle));
    for (name, policy) in variants {
        let mut prepared: *mut FreshenPrepared = ptr::null_mut();
        expect(
            unsafe {
                freshen_prepare(
                    u,
                    candidate,
                    work.as_ptr(),
                    ptr::null(),
                    None,
                    ptr::null_mut(),
                    &mut prepared,
                )
            },
            FRESHEN_OK,
            "prepare",
        );
        let mut hand: *mut FreshenHandoff = ptr::null_mut();
        expect(
            unsafe {
                freshen_arm(
                    prepared,
                    &policy,
                    helper.as_ptr(),
                    ptr::null(),
                    0,
                    &mut hand,
                )
            },
            FRESHEN_ERR_INVALID_ARGUMENT,
            name,
        );
        assert!(hand.is_null());
        unsafe { freshen_prepared_free(prepared) };
    }
    // Out-of-range timeouts are refused, not passed to the helper.
    for (exit, confirm) in [(3601u64, 0u64), (u64::MAX, 0), (0, 3601), (0, u64::MAX)] {
        let mut p = portable_policy(&work, &exe, &owned);
        p.exit_timeout_secs = exit;
        p.confirmation_timeout_secs = confirm;
        let mut prepared: *mut FreshenPrepared = ptr::null_mut();
        expect(
            unsafe {
                freshen_prepare(
                    u,
                    candidate,
                    work.as_ptr(),
                    ptr::null(),
                    None,
                    ptr::null_mut(),
                    &mut prepared,
                )
            },
            FRESHEN_OK,
            "prepare",
        );
        let mut hand: *mut FreshenHandoff = ptr::null_mut();
        let status =
            unsafe { freshen_arm(prepared, &p, helper.as_ptr(), ptr::null(), 0, &mut hand) };
        sane(status, "timeout");
        assert_ne!(
            status, FRESHEN_OK,
            "timeout {exit}/{confirm} must not be accepted"
        );
        unsafe { freshen_prepared_free(prepared) };
    }

    // Transport event kinds this ABI does not know are passed through, not rejected: the
    // host sees kind 0. (Covered by the C and .NET tests.)
    unsafe {
        freshen_candidate_free(candidate);
        freshen_updater_free(u);
    }
}

// ---------------------------------------------------------------------------------
// Absurd counts and lengths
// ---------------------------------------------------------------------------------

#[test]
fn absurd_counts_and_lengths_are_rejected_before_any_memory_is_read() {
    let fx = Fx::new();
    let strings = Strings::standard();
    let transport = fx.transport();

    // The buffers below are tiny; a count that claims otherwise must be refused up front
    // rather than trusted (reading that far would run off the end of the allocation).
    for count in [33usize, 1025, 1 << 20, usize::MAX / 32, usize::MAX] {
        let mut cfg = config(&strings, &fx.key, &transport);
        cfg.trusted_key_count = count;
        let mut out: *mut FreshenUpdater = ptr::null_mut();
        expect(
            unsafe { freshen_updater_new(&cfg, &mut out) },
            FRESHEN_ERR_INVALID_ARGUMENT,
            &format!("trusted_key_count {count}"),
        );
        assert!(out.is_null());
    }

    let u = updater(&fx);
    let index = cs("https://test.invalid/index");
    let entry = FreshenLocation {
        document: index.as_ptr(),
        signature: index.as_ptr(),
    };
    for count in [101usize, 1 << 20, usize::MAX / 16, usize::MAX] {
        let mut listed = source(FRESHEN_SOURCE_MANIFESTS, ptr::null(), ptr::null());
        listed.locations = &entry;
        listed.location_count = count;
        let status = check(u, &listed).0;
        sane(status, "location_count");
        assert!(
            status == FRESHEN_ERR_SIZE_LIMIT || status == FRESHEN_ERR_INVALID_ARGUMENT,
            "location_count {count} returned {status}"
        );
    }

    let candidate = candidate_from(&fx, u);
    let work = cs(fx.work.to_str().unwrap());
    let exe = cs("application");
    let owned = [exe.as_ptr()];
    let helper = cs("/nonexistent-helper");
    let entry_ptr = [exe.as_ptr()];
    for count in [1usize << 21, usize::MAX / 8, usize::MAX] {
        let mut prepared: *mut FreshenPrepared = ptr::null_mut();
        expect(
            unsafe {
                freshen_prepare(
                    u,
                    candidate,
                    work.as_ptr(),
                    ptr::null(),
                    None,
                    ptr::null_mut(),
                    &mut prepared,
                )
            },
            FRESHEN_OK,
            "prepare",
        );
        let mut policy = portable_policy(&work, &exe, &owned);
        policy.owned_file_count = count;
        let mut hand: *mut FreshenHandoff = ptr::null_mut();
        expect(
            unsafe {
                freshen_arm(
                    prepared,
                    &policy,
                    helper.as_ptr(),
                    ptr::null(),
                    0,
                    &mut hand,
                )
            },
            FRESHEN_ERR_INVALID_ARGUMENT,
            &format!("owned_file_count {count}"),
        );
        unsafe { freshen_prepared_free(prepared) };

        let mut prepared: *mut FreshenPrepared = ptr::null_mut();
        expect(
            unsafe {
                freshen_prepare(
                    u,
                    candidate,
                    work.as_ptr(),
                    ptr::null(),
                    None,
                    ptr::null_mut(),
                    &mut prepared,
                )
            },
            FRESHEN_OK,
            "prepare",
        );
        let mut policy = portable_policy(&work, &exe, &owned);
        policy.expandable_directories = entry_ptr.as_ptr();
        policy.expandable_directory_count = count;
        expect(
            unsafe {
                freshen_arm(
                    prepared,
                    &policy,
                    helper.as_ptr(),
                    ptr::null(),
                    0,
                    &mut hand,
                )
            },
            FRESHEN_ERR_INVALID_ARGUMENT,
            &format!("expandable_directory_count {count}"),
        );
        unsafe { freshen_prepared_free(prepared) };

        let mut prepared: *mut FreshenPrepared = ptr::null_mut();
        expect(
            unsafe {
                freshen_prepare(
                    u,
                    candidate,
                    work.as_ptr(),
                    ptr::null(),
                    None,
                    ptr::null_mut(),
                    &mut prepared,
                )
            },
            FRESHEN_OK,
            "prepare",
        );
        let policy = portable_policy(&work, &exe, &owned);
        expect(
            unsafe {
                freshen_arm(
                    prepared,
                    &policy,
                    helper.as_ptr(),
                    entry_ptr.as_ptr(),
                    count,
                    &mut hand,
                )
            },
            FRESHEN_ERR_INVALID_ARGUMENT,
            &format!("relaunch_arg_count {count}"),
        );
        unsafe { freshen_prepared_free(prepared) };

        let mut handled = false;
        expect(
            unsafe { freshen_run_helper(count, entry_ptr.as_ptr(), &mut handled) },
            FRESHEN_ERR_INVALID_ARGUMENT,
            &format!("argc {count}"),
        );
    }

    // A sink write claiming more bytes than any allocation can hold.
    fx.host.set(Behavior::AbsurdLength);
    let (status, cand) = check(
        u,
        &source(FRESHEN_SOURCE_INDEX, index.as_ptr(), ptr::null()),
    );
    sane(status, "absurd sink length");
    assert_ne!(status, FRESHEN_OK);
    assert!(cand.is_null());
    let statuses = fx.host.sink_statuses.lock().unwrap().clone();
    assert!(!statuses.is_empty(), "the transport was called");
    assert!(
        statuses.iter().all(|s| *s == FRESHEN_ERR_INVALID_ARGUMENT),
        "an absurd sink length must be rejected outright, got {statuses:?}"
    );

    unsafe {
        freshen_candidate_free(candidate);
        freshen_updater_free(u);
    }
    assert_eq!(fs::read_dir(&fx.work).unwrap().count(), 0);
}

// ---------------------------------------------------------------------------------
// Extreme numbers
// ---------------------------------------------------------------------------------

#[test]
fn extreme_numbers_never_panic() {
    let fx = Fx::new();
    let strings = Strings::standard();
    let transport = fx.transport();

    // Limits at both extremes are accepted (they only bound later work).
    for (download, unpacked) in [
        (0u64, 0u64),
        (u64::MAX, u64::MAX),
        (0, u64::MAX),
        (u64::MAX, 0),
    ] {
        let mut cfg = config(&strings, &fx.key, &transport);
        cfg.max_download = download;
        cfg.max_unpacked = unpacked;
        let mut out: *mut FreshenUpdater = ptr::null_mut();
        expect(
            unsafe { freshen_updater_new(&cfg, &mut out) },
            FRESHEN_OK,
            "limits",
        );
        // And using them must not panic either.
        let index = cs("https://test.invalid/index");
        let (status, cand) = check(
            out,
            &source(FRESHEN_SOURCE_INDEX, index.as_ptr(), ptr::null()),
        );
        sane(status, "check with extreme limits");
        if !cand.is_null() {
            let work = cs(fx.work.to_str().unwrap());
            let mut prepared: *mut FreshenPrepared = ptr::null_mut();
            let status = unsafe {
                freshen_prepare(
                    out,
                    cand,
                    work.as_ptr(),
                    ptr::null(),
                    None,
                    ptr::null_mut(),
                    &mut prepared,
                )
            };
            sane(status, "prepare with extreme limits");
            unsafe { freshen_prepared_free(prepared) };
            unsafe { freshen_candidate_free(cand) };
        }
        unsafe { freshen_updater_free(out) };
    }

    // Absurd HTTP timeouts are refused. They do not crash (a probe showed a refused connection
    // fails normally), but cancelling a blocked read is bounded by this timeout, so a
    // near-infinite one would make a stalled download impossible to cancel.
    for seconds in [86_401u64, u64::MAX / 2, u64::MAX] {
        let mut cfg = config(&strings, &fx.key, ptr::null());
        cfg.http_timeout_secs = seconds;
        let mut out: *mut FreshenUpdater = ptr::null_mut();
        expect(
            unsafe { freshen_updater_new(&cfg, &mut out) },
            FRESHEN_ERR_INVALID_ARGUMENT,
            &format!("http_timeout_secs {seconds}"),
        );
        assert!(out.is_null());
    }
    // The documented maximum (one day) is accepted and usable without panicking.
    let mut cfg = config(&strings, &fx.key, ptr::null());
    cfg.http_timeout_secs = 86_400;
    let mut out: *mut FreshenUpdater = ptr::null_mut();
    expect(
        unsafe { freshen_updater_new(&cfg, &mut out) },
        FRESHEN_OK,
        "one day timeout",
    );
    unsafe { freshen_updater_free(out) };

    // Odd but harmless version strings and identities.
    let long = "9".repeat(100_000);
    let odd = [
        "1.0.0-alpha.1+build.5",
        "0.0.0",
        "18446744073709551615.0.0",
        long.as_str(),
        "",
        "1.0",
        "v1.0.0",
        " 1.0.0",
        "1.0.0\n",
    ];
    for version in odd {
        let mut s = Strings::standard();
        s.version = cs(version);
        let cfg = config(&s, &fx.key, &transport);
        let mut out: *mut FreshenUpdater = ptr::null_mut();
        let status = unsafe { freshen_updater_new(&cfg, &mut out) };
        sane(status, "odd version");
        if status == FRESHEN_OK {
            unsafe { freshen_updater_free(out) };
        } else {
            assert!(out.is_null());
        }
    }
    let mut empty = Strings::standard();
    empty.product = cs("");
    empty.channel = cs("");
    empty.target = cs("");
    let cfg = config(&empty, &fx.key, &transport);
    let mut out: *mut FreshenUpdater = ptr::null_mut();
    let status = unsafe { freshen_updater_new(&cfg, &mut out) };
    sane(status, "empty identity");
    if status == FRESHEN_OK {
        unsafe { freshen_updater_free(out) };
    }
}

// ---------------------------------------------------------------------------------
// Hostile transport callbacks
// ---------------------------------------------------------------------------------

#[test]
fn hostile_transport_callbacks_are_contained() {
    let fx = Fx::new();
    let u = updater(&fx);
    let index = cs("https://test.invalid/index");
    let src = source(FRESHEN_SOURCE_INDEX, index.as_ptr(), ptr::null());

    // A transport may return any integer. Every one must map to a defined status.
    for returned in [
        i32::MIN,
        -1,
        1,
        2,
        5,
        7,
        10,
        13,
        100,
        101,
        102,
        999,
        1000,
        1001,
        1002,
        i32::MAX,
    ] {
        fx.host.set(Behavior::Status(returned));
        let (status, cand) = check(u, &src);
        sane(status, &format!("transport returned {returned}"));
        assert_ne!(
            status, FRESHEN_OK,
            "transport status {returned} must fail the check"
        );
        assert!(cand.is_null());
    }

    // Success without a single byte is a malformed document, not a crash.
    fx.host.set(Behavior::SucceedEmpty);
    let (status, cand) = check(u, &src);
    sane(status, "empty success");
    assert_ne!(status, FRESHEN_OK);
    assert!(cand.is_null());

    // A null buffer with a length is refused by the sink; a careless host that ignores that
    // and reports success still cannot get an update out of nothing.
    fx.host.sink_statuses.lock().unwrap().clear();
    fx.host.set(Behavior::NullBufferWithLength);
    let (status, cand) = check(u, &src);
    sane(status, "null buffer");
    assert_ne!(status, FRESHEN_OK);
    assert!(cand.is_null());
    assert!(
        fx.host
            .sink_statuses
            .lock()
            .unwrap()
            .contains(&FRESHEN_ERR_INVALID_ARGUMENT)
    );

    // A total larger than any limit fails immediately.
    fx.host.sink_statuses.lock().unwrap().clear();
    fx.host.set(Behavior::HugeTotal);
    let (status, cand) = check(u, &src);
    expect(status, FRESHEN_ERR_SIZE_LIMIT, "huge total");
    assert!(cand.is_null());
    assert!(
        fx.host
            .sink_statuses
            .lock()
            .unwrap()
            .contains(&FRESHEN_ERR_SIZE_LIMIT)
    );

    // Re-entrancy: the transport calls back into the same updater. It must neither deadlock
    // nor corrupt the outer operation.
    fx.host.set(Behavior::Reenter);
    let (status, cand) = check(u, &src);
    expect(status, FRESHEN_OK, "outer check after re-entrancy");
    assert!(!cand.is_null());
    assert_eq!(*fx.host.nested_status.lock().unwrap(), Some(FRESHEN_OK));
    unsafe { freshen_candidate_free(cand) };

    // And the updater is still fully usable afterwards.
    fx.host.set(Behavior::Serve);
    let (status, cand) = check(u, &src);
    expect(status, FRESHEN_OK, "check after hostile transports");
    assert!(!cand.is_null());
    unsafe {
        freshen_candidate_free(cand);
        freshen_updater_free(u);
    }
}

// ---------------------------------------------------------------------------------
// Misuse of handles that the API defines
// ---------------------------------------------------------------------------------

#[test]
fn defined_handle_misuse_reports_errors() {
    let fx = Fx::new();
    let u = updater(&fx);
    let candidate = candidate_from(&fx, u);
    let work = cs(fx.work.to_str().unwrap());

    // History accessors out of range.
    unsafe {
        assert!(freshen_candidate_history_version(candidate, 2).is_null());
        assert!(freshen_candidate_history_version(candidate, usize::MAX).is_null());
        assert!(freshen_candidate_history_notes(candidate, usize::MAX).is_null());
    }

    // A candidate from one updater used with another that has a stricter policy.
    let strings = Strings {
        version: cs("3.0.0"),
        ..Strings::standard()
    };
    let transport = fx.transport();
    let cfg = config(&strings, &fx.key, &transport);
    let mut newer: *mut FreshenUpdater = ptr::null_mut();
    expect(
        unsafe { freshen_updater_new(&cfg, &mut newer) },
        FRESHEN_OK,
        "newer updater",
    );
    let mut prepared: *mut FreshenPrepared = ptr::null_mut();
    let status = unsafe {
        freshen_prepare(
            newer,
            candidate,
            work.as_ptr(),
            ptr::null(),
            None,
            ptr::null_mut(),
            &mut prepared,
        )
    };
    expect(
        status,
        FRESHEN_ERR_INVALID,
        "candidate that does not satisfy the updater's policy",
    );
    assert!(prepared.is_null());
    assert_eq!(fs::read_dir(&fx.work).unwrap().count(), 0);

    // Arming consumes the prepared update even when it fails; every accessor then reports
    // "gone", and a second attempt says so.
    let mut prepared: *mut FreshenPrepared = ptr::null_mut();
    expect(
        unsafe {
            freshen_prepare(
                u,
                candidate,
                work.as_ptr(),
                ptr::null(),
                None,
                ptr::null_mut(),
                &mut prepared,
            )
        },
        FRESHEN_OK,
        "prepare",
    );
    assert!(take(unsafe { freshen_prepared_version(prepared) }).is_some());
    let exe = cs("application");
    let owned = [exe.as_ptr()];
    let install_root = cs(fx.root.to_str().unwrap());
    let policy = portable_policy(&install_root, &exe, &owned);
    let helper = cs("/nonexistent-helper");
    let mut hand: *mut FreshenHandoff = ptr::null_mut();
    let first = unsafe {
        freshen_arm(
            prepared,
            &policy,
            helper.as_ptr(),
            ptr::null(),
            0,
            &mut hand,
        )
    };
    sane(first, "arm with a helper that does not exist");
    assert_ne!(first, FRESHEN_OK);
    assert!(hand.is_null());
    unsafe {
        assert!(freshen_prepared_version(prepared).is_null());
        assert!(freshen_prepared_notes(prepared).is_null());
        assert_eq!(freshen_prepared_history_count(prepared), 0);
        assert!(freshen_prepared_history_notes(prepared, 0).is_null());
    }
    expect(
        unsafe {
            freshen_arm(
                prepared,
                &policy,
                helper.as_ptr(),
                ptr::null(),
                0,
                &mut hand,
            )
        },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "arm twice",
    );
    unsafe { freshen_prepared_free(prepared) };

    // Cancellation handles: clones share a flag, and cancelling twice is harmless.
    unsafe {
        let cancel = freshen_cancel_new();
        let alias = freshen_cancel_clone(cancel);
        freshen_cancel_cancel(alias);
        freshen_cancel_cancel(alias);
        let index = cs("https://test.invalid/index");
        let src = source(FRESHEN_SOURCE_INDEX, index.as_ptr(), ptr::null());
        let mut out: *mut FreshenCandidate = ptr::null_mut();
        expect(
            freshen_check(u, &src, cancel, None, ptr::null_mut(), &mut out),
            FRESHEN_ERR_CANCELLED,
            "cancelled",
        );
        freshen_cancel_free(alias);
        freshen_cancel_free(cancel);
        freshen_updater_free(newer);
        freshen_candidate_free(candidate);
        freshen_updater_free(u);
    }
}

// ---------------------------------------------------------------------------------
// The helper entry point and on-disk state
// ---------------------------------------------------------------------------------

#[test]
fn helper_dispatch_survives_garbage_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let garbage_plan = dir.path().join("plan.json");
    fs::write(&garbage_plan, b"{ this is not json").unwrap();
    let directory = dir.path().to_str().unwrap().to_string();
    let plan = garbage_plan.to_str().unwrap().to_string();
    let long = "x".repeat(1 << 20);

    let cases: Vec<Vec<String>> = vec![
        vec![],
        vec!["".into()],
        vec!["--not-a-helper".into()],
        vec!["--freshen-apply".into()],
        vec!["--freshen-apply".into(), "/nonexistent/plan.json".into()],
        vec![
            "--freshen-apply".into(),
            "/nonexistent/plan.json".into(),
            "token".into(),
        ],
        vec!["--freshen-apply".into(), plan.clone(), "token".into()],
        vec!["--freshen-apply".into(), directory.clone(), "token".into()],
        vec!["--freshen-apply".into(), "".into(), "".into()],
        vec!["--freshen-apply".into(), long.clone(), long.clone()],
        vec![
            "--freshen-apply".into(),
            plan.clone(),
            "token".into(),
            "extra".into(),
            "more".into(),
        ],
        vec![long.clone()],
    ];
    for arguments in cases {
        let owned: Vec<CString> = arguments.iter().map(|a| cs(a)).collect();
        let pointers: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        let mut handled = true;
        let status = unsafe {
            freshen_run_helper(
                pointers.len(),
                if pointers.is_empty() {
                    ptr::null()
                } else {
                    pointers.as_ptr()
                },
                &mut handled,
            )
        };
        sane(
            status,
            &format!(
                "run_helper {:?}",
                arguments.iter().map(|a| a.len()).collect::<Vec<_>>()
            ),
        );
        if status != FRESHEN_OK {
            assert!(!handled, "a failed dispatch must not report handled");
        }
    }
    // The ordinary-launch answer stays "not handled".
    let mut handled = true;
    let arg = cs("--something-else");
    let pointers = [arg.as_ptr()];
    expect(
        unsafe { freshen_run_helper(1, pointers.as_ptr(), &mut handled) },
        FRESHEN_OK,
        "ordinary launch",
    );
    assert!(!handled);
}

#[test]
fn status_recovery_and_discard_survive_corrupt_state() {
    let dir = tempfile::tempdir().unwrap();
    let call = |root: &Path| {
        let root = cs(root.to_str().unwrap());
        let mut state = -1;
        let mut version: *mut c_char = ptr::null_mut();
        let mut detail: *mut c_char = ptr::null_mut();
        let a = unsafe {
            freshen_installation_status(root.as_ptr(), &mut state, &mut version, &mut detail)
        };
        take(version);
        take(detail);
        sane(a, "installation_status");
        let mut state = -1;
        let mut version: *mut c_char = ptr::null_mut();
        let mut detail: *mut c_char = ptr::null_mut();
        let b = unsafe {
            freshen_recover_installation(root.as_ptr(), &mut state, &mut version, &mut detail)
        };
        take(version);
        take(detail);
        sane(b, "recover_installation");
        let c = unsafe { freshen_discard_finished_installation(root.as_ptr()) };
        sane(c, "discard_finished_installation");
        (a, b, c)
    };

    // A directory that does not exist, and a path that is a regular file.
    call(&dir.path().join("does-not-exist"));
    let file = dir.path().join("a-file");
    fs::write(&file, b"not a directory").unwrap();
    call(&file);

    // ".freshen" exists but is a file, not a directory.
    let as_file = dir.path().join("state-is-a-file");
    fs::create_dir(&as_file).unwrap();
    fs::write(as_file.join(".freshen"), b"oops").unwrap();
    call(&as_file);

    // A transaction directory with corrupt or hostile contents.
    let corrupt = |name: &str, journal: Option<&[u8]>, plan: Option<&[u8]>| {
        let root = dir.path().join(name);
        let transaction = root.join(".freshen/transaction");
        fs::create_dir_all(&transaction).unwrap();
        if let Some(bytes) = journal {
            fs::write(transaction.join("journal.json"), bytes).unwrap();
        }
        if let Some(bytes) = plan {
            fs::write(transaction.join("plan.json"), bytes).unwrap();
        }
        root
    };
    let huge = vec![b'{'; 5 * 1024 * 1024];
    let deep = "[".repeat(100_000);
    for (name, journal, plan) in [
        ("empty-transaction", None, None),
        ("garbage-journal", Some(&b"\xff\xfe not json"[..]), None),
        ("empty-journal", Some(&b""[..]), None),
        ("wrong-schema", Some(&b"{}"[..]), Some(&b"{}"[..])),
        ("null-journal", Some(&b"null"[..]), Some(&b"null"[..])),
        ("huge-journal", Some(&huge[..]), Some(&huge[..])),
        (
            "deeply-nested",
            Some(deep.as_bytes()),
            Some(deep.as_bytes()),
        ),
        (
            "garbage-plan",
            Some(&b"{\"status\":{}}"[..]),
            Some(&b"garbage"[..]),
        ),
    ] {
        let root = corrupt(name, journal, plan);
        call(&root);
    }
    // A journal that is a directory.
    let root = dir.path().join("journal-is-a-directory");
    fs::create_dir_all(root.join(".freshen/transaction/journal.json")).unwrap();
    call(&root);
}

// ---------------------------------------------------------------------------------
// The error message is thread-local
// ---------------------------------------------------------------------------------

#[test]
fn error_messages_do_not_leak_between_threads() {
    let mut out: *mut FreshenUpdater = ptr::null_mut();
    expect(
        unsafe { freshen_updater_new(ptr::null(), &mut out) },
        FRESHEN_ERR_INVALID_ARGUMENT,
        "main thread failure",
    );
    let here = message();
    assert!(!here.is_empty());
    let there = std::thread::spawn(|| {
        // A fresh thread has never failed, so it has no message; and a failure on it does
        // not disturb the message on the thread that started it.
        let before = message();
        let mut out: *mut FreshenUpdater = ptr::null_mut();
        let status = unsafe { freshen_updater_new(ptr::null(), &mut out) };
        (before, status, message())
    })
    .join()
    .unwrap();
    assert_eq!(there.0, "", "a new thread starts with no message");
    assert_eq!(there.1, FRESHEN_ERR_INVALID_ARGUMENT);
    assert!(!there.2.is_empty());
    assert_eq!(message(), here, "the main thread's message is unchanged");
}

// ---------------------------------------------------------------------------------
// Seeded random call sequences
// ---------------------------------------------------------------------------------

/// xorshift64*: small, deterministic, and good enough to explore call orderings.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

#[test]
fn random_call_sequences_never_break_the_boundary() {
    let fx = Fx::new();
    let work = cs(fx.work.to_str().unwrap());
    let helper = cs("/nonexistent-helper");
    let exe = cs("application");
    let owned = [exe.as_ptr()];
    let install_root = cs(fx.root.to_str().unwrap());
    let urls = [
        cs("https://test.invalid/index"),
        cs("https://test.invalid/m/2.0.0"),
        cs("https://test.invalid/s/2.0.0"),
        cs("https://test.invalid/bad-sig"),
        cs("https://test.invalid/nothing"),
        cs("http://test.invalid/index"),
        cs("not a url"),
        cs(""),
    ];
    let mut candidates: Vec<*mut FreshenCandidate> = Vec::new();
    let mut prepareds: Vec<*mut FreshenPrepared> = Vec::new();
    let mut cancels: Vec<*mut FreshenCancel> = vec![freshen_cancel_new()];

    // Several seeds, so a failure names the one that reproduces it.
    for seed in 1..=8u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let u = updater(&fx);
        for step in 0..400 {
            let what = format!("seed {seed} step {step}");
            match rng.below(12) {
                0..=2 => {
                    let a = &urls[rng.below(urls.len() as u64) as usize];
                    let b = &urls[rng.below(urls.len() as u64) as usize];
                    let kind = rng.pick(&[
                        FRESHEN_SOURCE_INDEX,
                        FRESHEN_SOURCE_MANIFEST,
                        FRESHEN_SOURCE_GITHUB,
                        FRESHEN_SOURCE_MANIFESTS,
                        0,
                        7,
                    ]);
                    let mut src = source(kind, a.as_ptr(), b.as_ptr());
                    let entries = [FreshenLocation {
                        document: a.as_ptr(),
                        signature: b.as_ptr(),
                    }];
                    if kind == FRESHEN_SOURCE_MANIFESTS {
                        src.locations = entries.as_ptr();
                        src.location_count = rng.below(3) as usize;
                    }
                    fx.host.set(rng.pick(&[
                        Behavior::Serve,
                        Behavior::Serve,
                        Behavior::Serve,
                        Behavior::Status(FRESHEN_ERR_IO),
                        Behavior::SucceedEmpty,
                    ]));
                    let cancel = if rng.below(4) == 0 {
                        rng.pick(&cancels) as *const FreshenCancel
                    } else {
                        ptr::null()
                    };
                    let mut out: *mut FreshenCandidate = ptr::null_mut();
                    let status =
                        unsafe { freshen_check(u, &src, cancel, None, ptr::null_mut(), &mut out) };
                    sane(status, &what);
                    if status != FRESHEN_OK {
                        assert!(out.is_null(), "{what}: out must be null on failure");
                    } else if !out.is_null() {
                        candidates.push(out);
                    }
                }
                3..=4 if !candidates.is_empty() => {
                    fx.host.set(Behavior::Serve);
                    let candidate = rng.pick(&candidates);
                    let cancel = if rng.below(3) == 0 {
                        rng.pick(&cancels) as *const FreshenCancel
                    } else {
                        ptr::null()
                    };
                    let mut out: *mut FreshenPrepared = ptr::null_mut();
                    let status = unsafe {
                        freshen_prepare(
                            u,
                            candidate,
                            work.as_ptr(),
                            cancel,
                            None,
                            ptr::null_mut(),
                            &mut out,
                        )
                    };
                    sane(status, &what);
                    if status == FRESHEN_OK {
                        prepareds.push(out);
                    } else {
                        assert!(out.is_null(), "{what}: out must be null on failure");
                    }
                }
                5..=6 if !prepareds.is_empty() => {
                    let index = rng.below(prepareds.len() as u64) as usize;
                    let prepared = prepareds[index];
                    let mut policy = portable_policy(&install_root, &exe, &owned);
                    policy.layout =
                        rng.pick(&[FRESHEN_LAYOUT_PORTABLE, FRESHEN_LAYOUT_PORTABLE, 9]);
                    policy.exit_timeout_secs = rng.pick(&[0, 1, 5000]);
                    let mut hand: *mut FreshenHandoff = ptr::null_mut();
                    let status = unsafe {
                        freshen_arm(
                            prepared,
                            &policy,
                            helper.as_ptr(),
                            ptr::null(),
                            0,
                            &mut hand,
                        )
                    };
                    sane(status, &what);
                    assert!(
                        hand.is_null(),
                        "{what}: the helper does not exist, so no handoff"
                    );
                }
                7 if !candidates.is_empty() => {
                    let candidate = rng.pick(&candidates);
                    let index = rng.below(5) as usize;
                    unsafe {
                        take(freshen_candidate_version(candidate));
                        take(freshen_candidate_notes(candidate));
                        take(freshen_candidate_history_version(candidate, index));
                        take(freshen_candidate_history_notes(candidate, index));
                        let _ = freshen_candidate_history_count(candidate);
                        let _ = freshen_candidate_download_size(candidate);
                    }
                }
                8 if !prepareds.is_empty() => {
                    let prepared = rng.pick(&prepareds);
                    let index = rng.below(5) as usize;
                    unsafe {
                        take(freshen_prepared_version(prepared));
                        take(freshen_prepared_notes(prepared));
                        take(freshen_prepared_history_version(prepared, index));
                        take(freshen_prepared_history_notes(prepared, index));
                        let _ = freshen_prepared_history_count(prepared);
                    }
                }
                9 => {
                    let cancel = rng.pick(&cancels);
                    unsafe { freshen_cancel_cancel(cancel) };
                }
                10 => {
                    let cancel =
                        unsafe { freshen_cancel_clone(rng.pick(&cancels) as *const FreshenCancel) };
                    assert!(!cancel.is_null());
                    cancels.push(cancel);
                }
                _ => {
                    // Drop a random prepared update (its staged files must go away).
                    if !prepareds.is_empty() {
                        let index = rng.below(prepareds.len() as u64) as usize;
                        let prepared = prepareds.swap_remove(index);
                        unsafe { freshen_prepared_free(prepared) };
                    }
                }
            }
        }
        unsafe { freshen_updater_free(u) };
    }

    for prepared in prepareds {
        unsafe { freshen_prepared_free(prepared) };
    }
    for candidate in candidates {
        unsafe { freshen_candidate_free(candidate) };
    }
    for cancel in cancels {
        unsafe { freshen_cancel_free(cancel) };
    }
    assert_eq!(
        fs::read_dir(&fx.work).unwrap().count(),
        0,
        "all staged updates were released"
    );
}
