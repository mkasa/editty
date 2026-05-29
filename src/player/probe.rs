//! Runtime detection of kitty-graphics support by *asking the terminal*, rather
//! than guessing from `$TERM`/env. Works for any terminal that implements the
//! protocol (kitty, Ghostty, WezTerm, Konsole, …) and also tells us whether the
//! shared-memory transport is usable here.
//!
//! Method: send a graphics query (`a=q`) over the direct transport, another over
//! shared memory, then a Primary Device Attributes request (`ESC [ c`) as a
//! sentinel. Replies arrive in order, so the DA reply (which every VT terminal
//! sends) marks the end — terminals without graphics support still answer DA
//! and so are detected almost instantly. A `i=<id>;OK` reply means support.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::shm;

const DIRECT_ID: u32 = 4242;
const SHM_ID: u32 = 4243;
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

pub struct GraphicsSupport {
    pub graphics: bool,
    pub shm: bool,
}

/// Probe the terminal. Must be called with the terminal already in raw mode.
/// `allow_shm` lets the caller (e.g. `EDITTY_NO_SHM`) skip the shm probe.
pub fn probe(allow_shm: bool) -> GraphicsSupport {
    let mut out = io::stdout();
    let mut shm_name: Option<String> = None;

    // 1x1 RGB query over the direct (escape-stream) transport.
    let _ = write!(out, "\x1b_Gi={DIRECT_ID},a=q,s=1,v=1,f=24,t=d;AAAA\x1b\\");

    // Same query over shared memory — only if we can create the object.
    if allow_shm {
        if let Ok(name) = shm::write(&[0u8, 0, 0]) {
            let b64 = STANDARD.encode(name.as_bytes());
            let _ = write!(out, "\x1b_Gi={SHM_ID},a=q,s=1,v=1,f=24,t=s,S=3;{b64}\x1b\\");
            shm_name = Some(name);
        }
    }

    // Sentinel: every VT terminal answers Primary Device Attributes.
    let _ = out.write_all(b"\x1b[c");
    let _ = out.flush();

    let buf = read_until_da(PROBE_TIMEOUT);

    if let Some(name) = shm_name {
        shm::unlink(&name); // in case the terminal didn't consume it
    }

    let supported = |id: u32| find(&buf, format!("i={id};OK").as_bytes()).is_some();
    let graphics = supported(DIRECT_ID);
    GraphicsSupport {
        graphics,
        shm: graphics && allow_shm && supported(SHM_ID),
    }
}

/// Read stdin until the Device Attributes reply arrives or the timeout elapses.
fn read_until_da(timeout: Duration) -> Vec<u8> {
    let mut buf = Vec::new();
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let mut fd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(&mut fd, 1, ms) };
        if ready <= 0 || fd.revents & libc::POLLIN == 0 {
            break; // timeout or error
        }
        let mut tmp = [0u8; 1024];
        let n = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                tmp.as_mut_ptr() as *mut libc::c_void,
                tmp.len(),
            )
        };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n as usize]);
        if has_da_reply(&buf) {
            break;
        }
    }
    buf
}

/// A Primary Device Attributes reply looks like `ESC [ ? … c`.
fn has_da_reply(buf: &[u8]) -> bool {
    match find(buf, b"\x1b[?") {
        Some(i) => buf[i..].contains(&b'c'),
        None => false,
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_supporting_reply() {
        // direct + shm OK, then the DA sentinel.
        let buf = b"\x1b_Gi=4242;OK\x1b\\\x1b_Gi=4243;OK\x1b\\\x1b[?62;1c";
        assert!(find(buf, b"i=4242;OK").is_some());
        assert!(find(buf, b"i=4243;OK").is_some());
        assert!(has_da_reply(buf));
    }

    #[test]
    fn detects_no_graphics_when_only_da_replies() {
        let buf = b"\x1b[?62;1c"; // a terminal that answered DA but no graphics OK
        assert!(find(buf, b"i=4242;OK").is_none());
        assert!(has_da_reply(buf));
    }

    #[test]
    fn da_reply_requires_terminator() {
        assert!(!has_da_reply(b"\x1b[?62;1")); // not yet terminated by 'c'
        assert!(!has_da_reply(b"random bytes"));
    }
}
