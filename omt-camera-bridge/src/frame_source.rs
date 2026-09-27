//! Reads UYVY frames from stdin on a background thread, keeping only the
//! latest one - mirrors the same "keep only latest, drop stale" pattern
//! c2/omtbridge.py already uses on the Python side.
//!
//! Without this, the main loop only got back to reading stdin after fully
//! encoding and sending the previous frame, so Python's pipe write couldn't
//! complete until that *entire* cycle had finished - not just until this
//! frame had been read out - even with a pipe sized to hold a whole frame.
//! Decoupling reading from encoding lets Python's write complete as soon as
//! this thread is back at `read_exact`, regardless of how long the main
//! loop's encode+send takes.

use std::io::{self, Read};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

enum Slot {
    Empty,
    Frame(Vec<u8>),
    Closed,
}

struct Shared {
    slot: Mutex<Slot>,
    cond: Condvar,
}

pub struct FrameSource {
    shared: Arc<Shared>,
}

impl FrameSource {
    /// Spawns the background reader thread immediately.
    pub fn spawn(frame_size: usize) -> Self {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot::Empty),
            cond: Condvar::new(),
        });

        let reader_shared = Arc::clone(&shared);
        thread::spawn(move || {
            let mut stdin = io::stdin().lock();
            loop {
                let mut buf = vec![0u8; frame_size];
                if stdin.read_exact(&mut buf).is_err() {
                    let mut guard = reader_shared.slot.lock().unwrap();
                    *guard = Slot::Closed;
                    reader_shared.cond.notify_one();
                    return;
                }
                let mut guard = reader_shared.slot.lock().unwrap();
                *guard = Slot::Frame(buf);
                reader_shared.cond.notify_one();
            }
        });

        Self { shared }
    }

    /// Blocks until a frame is available, returning `None` once stdin has
    /// closed (the reader thread hit EOF) and no frame is queued.
    pub fn next_frame(&self) -> Option<Vec<u8>> {
        let mut guard = self.shared.slot.lock().unwrap();
        loop {
            match &*guard {
                Slot::Frame(_) => {
                    let taken = std::mem::replace(&mut *guard, Slot::Empty);
                    let Slot::Frame(data) = taken else {
                        unreachable!()
                    };
                    return Some(data);
                }
                Slot::Closed => return None,
                Slot::Empty => {
                    guard = self.shared.cond.wait(guard).unwrap();
                }
            }
        }
    }
}
