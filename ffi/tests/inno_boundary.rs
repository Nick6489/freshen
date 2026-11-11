use freshen_ffi::*;
use std::{
    ffi::{CStr, CString},
    fs, ptr,
};

#[test]
fn status_roundtrip_and_corrupt_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let name = CString::new(root.path().to_str().unwrap()).unwrap();
    let mut output = ptr::null_mut();
    assert_eq!(
        unsafe { freshen_inno_status(name.as_ptr(), &mut output) },
        FRESHEN_OK
    );
    assert!(output.is_null());
    let transaction = root.path().join(".freshen/transaction");
    fs::create_dir_all(&transaction).unwrap();
    let path = transaction.join("inno-status.json");
    fs::write(&path, r#"{"phase":"Installing","progress":[3,8],"cancellable":false,"telemetry_connected":true,"telemetry_observed":true,"progress_observed":true,"overdue":false,"diagnostic":null,"detail":"working","exit_code":null}"#).unwrap();
    assert_eq!(
        unsafe { freshen_inno_status(name.as_ptr(), &mut output) },
        FRESHEN_OK
    );
    let json: serde_json::Value =
        serde_json::from_slice(unsafe { CStr::from_ptr(output) }.to_bytes()).unwrap();
    assert_eq!(json["progress"], serde_json::json!([3, 8]));
    unsafe { freshen_string_free(output) };
    fs::write(path, b"bad json").unwrap();
    output = ptr::dangling_mut();
    assert_ne!(
        unsafe { freshen_inno_status(name.as_ptr(), &mut output) },
        FRESHEN_OK
    );
    assert!(output.is_null());
}
