//! WebAssembly ABI v1 of the plugin sandbox (see `elephant-plugins`):
//! `elephant_alloc(len) -> ptr` and `elephant_import(ptr, len) -> i64`
//! returning `(out_ptr << 32) | out_len`.
#![allow(unsafe_code)]

/// Allocates `len` bytes for the host to write the request into. Leaked on
/// purpose: every call runs in a fresh instance.
#[unsafe(no_mangle)]
pub extern "C" fn elephant_alloc(len: i32) -> i32 {
    let len = usize::try_from(len).unwrap_or(0);
    let buf: &'static mut [u8] = Vec::leak(vec![0u8; len]);
    buf.as_mut_ptr() as i32
}

/// Runs one import. The output buffer is leaked for the host to read.
#[unsafe(no_mangle)]
pub extern "C" fn elephant_import(ptr: i32, len: i32) -> i64 {
    let len = usize::try_from(len).unwrap_or(0);
    // SAFETY: the host wrote `len` bytes at `ptr`, a buffer returned by
    // `elephant_alloc` in this instance.
    let bytes = unsafe { core::slice::from_raw_parts(ptr as usize as *const u8, len) };
    let input = String::from_utf8_lossy(bytes);
    let out: &'static mut str = String::leak(crate::import_json(&input));
    ((out.as_ptr() as i64) << 32) | (out.len() as i64)
}
