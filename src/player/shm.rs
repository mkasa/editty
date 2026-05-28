//! POSIX shared-memory transport for the kitty graphics protocol (`t=s`).
//!
//! Locally this is far cheaper than the direct transport: we write the raw RGBA
//! bytes into an shm object and hand kitty only the (short) object name, instead
//! of base64-encoding the whole frame and pushing it through the PTY. The
//! terminal reads the object and unlinks it itself — we never unlink it.

use std::ffi::CString;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// Create an shm object, write `data` into it, and return its POSIX name
/// (leading `/`). On any failure the partially-created object is unlinked and
/// the error returned so the caller can fall back to the direct transport.
pub fn write(data: &[u8]) -> io::Result<String> {
    // macOS caps shm names at 31 chars including the leading '/', so keep it
    // short: pid (low 16 bits) + a wrapping sequence, both hex.
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = format!("/ett{:x}-{:x}", std::process::id() & 0xffff, seq);
    let cname = CString::new(name.as_bytes()).expect("shm name has no NUL");

    unsafe {
        let fd = libc::shm_open(
            cname.as_ptr(),
            libc::O_CREAT | libc::O_RDWR | libc::O_EXCL,
            0o600,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let result = (|| {
            if libc::ftruncate(fd, data.len() as libc::off_t) != 0 {
                return Err(io::Error::last_os_error());
            }
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                data.len(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            if ptr == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
            libc::munmap(ptr, data.len());
            Ok(())
        })();

        libc::close(fd);
        match result {
            Ok(()) => Ok(name),
            Err(e) => {
                libc::shm_unlink(cname.as_ptr());
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_roundtrips_through_shared_memory() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let name = write(&data).expect("shm write should succeed locally");
        assert!(name.starts_with('/') && name.len() <= 31, "valid shm name");

        // Re-open the object and confirm kitty would read back exactly our bytes.
        let cname = CString::new(name.as_bytes()).unwrap();
        unsafe {
            let fd = libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0);
            assert!(fd >= 0, "reopen shm: {}", io::Error::last_os_error());
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                data.len(),
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );
            assert_ne!(ptr, libc::MAP_FAILED, "mmap shm");
            let slice = std::slice::from_raw_parts(ptr as *const u8, data.len());
            assert_eq!(slice, &data[..], "shm contents match what we wrote");
            libc::munmap(ptr, data.len());
            libc::close(fd);
            // We stand in for kitty here and unlink it.
            libc::shm_unlink(cname.as_ptr());
        }
    }
}
