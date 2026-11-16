//! C ABI ownership and JSON transport for the native store.
use super::{BatchRequest, GraphBatchRequest, NativeStore, QueryRequest};
use crate::CwpdbResult;
use std::{
    any::Any,
    ffi::{c_char, c_int, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    ptr,
};

/// Opaque handle allocated by [`cwpdb_open`] and consumed by [`cwpdb_close`].
pub struct FfiStore {
    store: NativeStore,
}

impl FfiStore {
    pub fn is_read_only(&self) -> CwpdbResult<bool> {
        self.store.is_read_only()
    }
    /// Dispatch only the closed Desktop operation set without exposing the store or a callback.
    pub fn desktop_operation(
        &mut self,
        catalog: &crate::declared::Catalog,
        operation: crate::desktop::operation::DesktopOperation,
    ) -> crate::desktop::operation::DesktopResult {
        use crate::desktop::operation::DesktopResult;
        if operation.is_write() && self.store.is_read_only().unwrap_or(true) {
            return DesktopResult::Rejected { code: "read_only" };
        }
        crate::desktop::operation::execute(&mut self.store, catalog, operation)
    }
}

pub(super) const FFI_OK: c_int = 0;
pub(super) const FFI_ERROR: c_int = 1;

/// Open a read/write store from a UTF-8 path.
///
/// On success `out_store` receives an opaque handle. On failure `out_error`
/// receives a caller-freed UTF-8 message when it is non-null. The caller must
/// close the handle with [`cwpdb_close`] and free any returned string with
/// [`cwpdb_string_free`].
#[no_mangle]
pub unsafe extern "C" fn cwpdb_open(
    path: *const c_char,
    out_store: *mut *mut FfiStore,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the C caller promises each pointer satisfies the C ABI described above.
    unsafe { open_store(path, out_store, out_error, |path| NativeStore::open(path)) }
}

/// Open an existing store for read-only queries from a UTF-8 path.
///
/// On success `out_store` receives an opaque handle. It cannot create a
/// database or write to one. Callers close the handle with [`cwpdb_close`].
#[no_mangle]
pub unsafe extern "C" fn cwpdb_open_readonly(
    path: *const c_char,
    out_store: *mut *mut FfiStore,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the C caller promises each pointer satisfies the C ABI described above.
    unsafe {
        open_store(path, out_store, out_error, |path| {
            NativeStore::open_read_only(path)
        })
    }
}

unsafe fn open_store(
    path: *const c_char,
    out_store: *mut *mut FfiStore,
    out_error: *mut *mut c_char,
    open: impl FnOnce(&Path) -> CwpdbResult<NativeStore>,
) -> c_int {
    // SAFETY: the C caller promises each non-null out pointer is writable.
    unsafe {
        clear_out(out_store);
        clear_out(out_error);
    }
    if out_store.is_null() {
        // SAFETY: the C caller promises a non-null error out pointer is writable.
        unsafe { write_error(out_error, "cwpDb open requires an output handle") };
        return FFI_ERROR;
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> CwpdbResult<()> {
        // SAFETY: the C caller promises path is a valid NUL-terminated UTF-8 string.
        let path = unsafe { read_input(path, "database path")? };
        let store = open(Path::new(&path))?;
        // SAFETY: out_store was checked non-null above and the caller promises it is writable.
        unsafe { *out_store = Box::into_raw(Box::new(FfiStore { store })) };
        Ok(())
    }));
    // SAFETY: the C caller promises a non-null error out pointer is writable.
    unsafe { finish(result, out_error) }
}

/// Execute a JSON [`BatchRequest`] and return a JSON [`BatchResponse`].
#[no_mangle]
pub unsafe extern "C" fn cwpdb_execute_batch(
    handle: *mut FfiStore,
    request_json: *const c_char,
    out_response: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the C caller promises each non-null out pointer is writable.
    unsafe {
        clear_out(out_response);
        clear_out(out_error);
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> CwpdbResult<()> {
        // Validate the required result channel before the batch can commit.
        require_response_output(out_response)?;
        // SAFETY: the C caller promises valid arguments according to this function's ABI.
        // SAFETY: ffi_store validates the opaque pointer before returning it.
        let store = unsafe { &mut *ffi_store(handle)? };
        // SAFETY: the C caller promises request_json is a valid NUL-terminated UTF-8 string.
        let request = unsafe { read_input(request_json, "batch request")? };
        let request: BatchRequest = serde_json::from_str(&request)?;
        let response = serde_json::to_string(&store.execute_batch(&request)?)?;
        // SAFETY: the C caller promises out_response is writable when non-null.
        unsafe { write_response(out_response, response) }
    }));
    // SAFETY: the C caller promises a non-null error out pointer is writable.
    unsafe { finish(result, out_error) }
}

/// Execute a JSON [`GraphBatchRequest`] and return a JSON [`BatchResponse`].
#[no_mangle]
pub unsafe extern "C" fn cwpdb_graph_batch(
    handle: *mut FfiStore,
    request_json: *const c_char,
    out_response: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the C caller promises each non-null out pointer is writable.
    unsafe {
        clear_out(out_response);
        clear_out(out_error);
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> CwpdbResult<()> {
        // Validate the required result channel before the batch can commit.
        require_response_output(out_response)?;
        // SAFETY: the C caller promises valid arguments according to this function's ABI.
        // SAFETY: ffi_store validates the opaque pointer before returning it.
        let store = unsafe { &mut *ffi_store(handle)? };
        // SAFETY: the C caller promises request_json is a valid NUL-terminated UTF-8 string.
        let request = unsafe { read_input(request_json, "batch request")? };
        let request: GraphBatchRequest = serde_json::from_str(&request)?;
        let response = serde_json::to_string(&store.graph_batch(&request)?)?;
        // SAFETY: the C caller promises out_response is writable when non-null.
        unsafe { write_response(out_response, response) }
    }));
    // SAFETY: the C caller promises a non-null error out pointer is writable.
    unsafe { finish(result, out_error) }
}

/// Execute a JSON [`QueryRequest`] and return a JSON [`QueryResponse`].
#[no_mangle]
pub unsafe extern "C" fn cwpdb_query(
    handle: *mut FfiStore,
    request_json: *const c_char,
    out_response: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the C caller promises each non-null out pointer is writable.
    unsafe {
        clear_out(out_response);
        clear_out(out_error);
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> CwpdbResult<()> {
        require_response_output(out_response)?;
        // SAFETY: the C caller promises valid arguments according to this function's ABI.
        // SAFETY: ffi_store validates the opaque pointer before returning it.
        let store = unsafe { &mut *ffi_store(handle)? };
        // SAFETY: the C caller promises request_json is a valid NUL-terminated UTF-8 string.
        let request = unsafe { read_input(request_json, "query request")? };
        let request: QueryRequest = serde_json::from_str(&request)?;
        let response = serde_json::to_string(&store.query(&request)?)?;
        // SAFETY: the C caller promises out_response is writable when non-null.
        unsafe { write_response(out_response, response) }
    }));
    // SAFETY: the C caller promises a non-null error out pointer is writable.
    unsafe { finish(result, out_error) }
}

/// Read the invocation-scoped performance projection. No SQL input is accepted.
#[no_mangle]
pub unsafe extern "C" fn cwpdb_invocation_performance(
    handle: *mut FfiStore,
    request_json: *const c_char,
    out_response: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the C caller promises each non-null out pointer is writable.
    unsafe {
        clear_out(out_response);
        clear_out(out_error);
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> CwpdbResult<()> {
        require_response_output(out_response)?;
        // SAFETY: the C caller promises valid arguments according to this function's ABI.
        // SAFETY: ffi_store validates the opaque pointer before returning it.
        let store = unsafe { &mut *ffi_store(handle)? };
        // SAFETY: the C caller promises request_json is a valid NUL-terminated UTF-8 string.
        let request = unsafe { read_input(request_json, "query request")? };
        let request: crate::metrics::projection::Request = serde_json::from_str(&request)?;
        let response = serde_json::to_string(&store.invocation_performance(&request)?)?;
        // SAFETY: the C caller promises out_response is writable when non-null.
        unsafe { write_response(out_response, response) }
    }));
    // SAFETY: the C caller promises a non-null error out pointer is writable.
    unsafe { finish(result, out_error) }
}

/// Limit database pages and disable cache spilling for bounded disposable writers.
/// Returns the effective byte ceiling as JSON. The caller reserves WAL headroom.
#[no_mangle]
pub unsafe extern "C" fn cwpdb_set_max_database_bytes(
    handle: *mut FfiStore,
    bytes: u64,
    out_response: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the caller provides writable outputs and a live, exclusive handle.
    unsafe {
        clear_out(out_response);
        clear_out(out_error);
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> CwpdbResult<()> {
        require_response_output(out_response)?;
        // SAFETY: ffi_store checks null; the caller keeps the handle live.
        let store = unsafe { &mut *ffi_store(handle)? };
        let response = serde_json::to_string(&store.set_max_database_bytes(bytes)?)?;
        // SAFETY: the caller provides a writable output.
        unsafe { write_response(out_response, response) }
    }));
    // SAFETY: the caller provides a writable error output when non-null.
    unsafe { finish(result, out_error) }
}

/// Checkpoint and truncate WAL, returning WalCheckpoint as caller-freed JSON.
#[no_mangle]
pub unsafe extern "C" fn cwpdb_checkpoint_wal(
    handle: *mut FfiStore,
    out_response: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the caller provides writable outputs and a live, exclusive handle.
    unsafe {
        clear_out(out_response);
        clear_out(out_error);
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> CwpdbResult<()> {
        require_response_output(out_response)?;
        // SAFETY: ffi_store checks null; the caller keeps the handle live.
        let store = unsafe { &mut *ffi_store(handle)? };
        let response = serde_json::to_string(&store.checkpoint_wal()?)?;
        // SAFETY: the caller provides a writable output.
        unsafe { write_response(out_response, response) }
    }));
    // SAFETY: the caller provides a writable error output when non-null.
    unsafe { finish(result, out_error) }
}

/// Close an opaque store handle. Closing a null handle is a no-op.
#[no_mangle]
pub unsafe extern "C" fn cwpdb_close(handle: *mut FfiStore) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: a successful cwpdb_open created this allocation and the caller closes it once.
        unsafe { drop(Box::from_raw(handle)) };
    }));
}

/// Free a response or error string returned by this library. Null is a no-op.
#[no_mangle]
pub unsafe extern "C" fn cwpdb_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: this library allocated the returned string and the caller frees it once.
        unsafe { drop(CString::from_raw(value)) };
    }));
}

unsafe fn clear_out<T>(out: *mut *mut T) {
    if !out.is_null() {
        // SAFETY: this helper's caller verified the non-null pointer is writable.
        unsafe { *out = ptr::null_mut() };
    }
}

unsafe fn read_input(pointer: *const c_char, name: &str) -> CwpdbResult<String> {
    if pointer.is_null() {
        return Err(format!("cwpDb {name} is null").into());
    }
    // SAFETY: this helper's caller verified the pointer is NUL-terminated and readable.
    let text = unsafe { CStr::from_ptr(pointer) }.to_str()?;
    Ok(text.to_owned())
}

unsafe fn ffi_store(handle: *mut FfiStore) -> CwpdbResult<*mut NativeStore> {
    if handle.is_null() {
        return Err("cwpDb handle is null".into());
    }
    // SAFETY: the C caller keeps the opaque handle live and does not use it concurrently.
    Ok(unsafe { &mut (*handle).store })
}

fn require_response_output(out_response: *mut *mut c_char) -> CwpdbResult<()> {
    if out_response.is_null() {
        return Err("cwpDb response output is null".into());
    }
    Ok(())
}

unsafe fn write_response(out_response: *mut *mut c_char, response: String) -> CwpdbResult<()> {
    require_response_output(out_response)?;
    // SAFETY: this helper's caller verified the non-null output pointer is writable.
    unsafe { *out_response = ffi_string(response) };
    Ok(())
}

unsafe fn finish(
    result: std::thread::Result<CwpdbResult<()>>,
    out_error: *mut *mut c_char,
) -> c_int {
    match result {
        Ok(Ok(())) => FFI_OK,
        Ok(Err(error)) => {
            // SAFETY: this helper's caller verified a non-null error output pointer is writable.
            unsafe { write_error(out_error, &error.to_string()) };
            FFI_ERROR
        }
        Err(panic) => {
            // SAFETY: this helper's caller verified a non-null error output pointer is writable.
            unsafe { write_error(out_error, &panic_message(panic)) };
            FFI_ERROR
        }
    }
}

unsafe fn write_error(out_error: *mut *mut c_char, error: &str) {
    if !out_error.is_null() {
        // SAFETY: this helper's caller verified the non-null output pointer is writable.
        unsafe { *out_error = ffi_string(error.to_owned()) };
    }
}

fn ffi_string(value: String) -> *mut c_char {
    CString::new(value)
        .unwrap_or_else(|_| {
            CString::new("cwpDb FFI text contained a NUL byte").expect("static string")
        })
        .into_raw()
}

fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        format!("cwpDb native call panicked: {message}")
    } else if let Some(message) = panic.downcast_ref::<String>() {
        format!("cwpDb native call panicked: {message}")
    } else {
        "cwpDb native call panicked".into()
    }
}
