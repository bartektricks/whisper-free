//! Preserving the whole clipboard across an insertion.
//!
//! Reached only as `platform::backend::clipboard`. The counterpart of
//! `platform/macos/clipboard.rs`, and it exists for the same reason: the
//! clipboard plugin carries text and images, which is not enough to put back
//! what a spreadsheet or a design tool puts on the clipboard, and reading only
//! the text mistakes a format that has not been rendered yet for an absent one.
//!
//! Windows renders formats on demand too. An application can advertise a format
//! and supply the bytes only when something asks, so asking for every format
//! here is what turns those promises into data while the owner is still around
//! to serve them. See `docs/decisions/0010-preserving-the-clipboard.md`.

use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

/// A ceiling on what one snapshot may hold in memory, matching the macOS side.
const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;

/// How long to keep trying to open the clipboard before giving up.
///
/// Only one process may hold it at a time, and clipboard managers open it the
/// moment its contents change, so a first attempt losing the race is normal.
const OPEN_TIMEOUT: Duration = Duration::from_millis(200);
const OPEN_RETRY: Duration = Duration::from_millis(10);

/// Formats whose handle is not an `HGLOBAL`, so `GlobalLock` would be wrong.
///
/// Bitmaps, palettes and metafiles are GDI objects, and owner-display formats
/// are not data at all. None can be copied as bytes, and all of them are
/// rendered from formats that can. Windows synthesises `CF_BITMAP` from
/// `CF_DIB`, so skipping them costs nothing that does not come back.
const NOT_GLOBAL_MEMORY: [u32; 8] = [
    2,      // CF_BITMAP
    3,      // CF_METAFILEPICT
    9,      // CF_PALETTE
    14,     // CF_ENHMETAFILE
    0x0080, // CF_OWNERDISPLAY
    0x0082, // CF_DSPBITMAP
    0x0083, // CF_DSPMETAFILEPICT
    0x008E, // CF_DSPENHMETAFILE
];

/// One clipboard format and the bytes the clipboard served for it.
struct Format {
    id: u32,
    bytes: Vec<u8>,
}

/// Everything the clipboard held, format by format.
pub struct Snapshot {
    formats: Vec<Format>,
    incomplete: bool,
}

impl Snapshot {
    /// Whether there was nothing on the clipboard to put back.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.formats.is_empty()
    }

    /// Whether a listed format could not be captured, so restoring this
    /// snapshot will not reproduce the clipboard exactly.
    #[must_use]
    pub const fn is_incomplete(&self) -> bool {
        self.incomplete
    }
}

/// Hold the clipboard open for the length of a closure.
///
/// Windows lets one process own it at a time, and every path out of here has to
/// close it, including the ones that fail part-way.
fn with_clipboard<T>(body: impl FnOnce() -> T) -> Option<T> {
    let started = Instant::now();
    loop {
        // Safety: a null window handle asks for the clipboard on behalf of the
        // current task, which is what a process with no window of its own wants.
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            break;
        }
        if started.elapsed() >= OPEN_TIMEOUT {
            tracing::debug!("the clipboard could not be opened");
            return None;
        }
        std::thread::sleep(OPEN_RETRY);
    }

    let result = body();

    // Safety: the clipboard is open, having just been opened above.
    unsafe { CloseClipboard() };
    Some(result)
}

/// Copy the bytes behind one clipboard handle.
///
/// Returns `None` for a handle that is null or cannot be locked. The handle
/// belongs to the clipboard and is deliberately not freed.
fn handle_bytes(handle: HANDLE) -> Option<Vec<u8>> {
    if handle.is_null() {
        return None;
    }
    let global: HGLOBAL = handle.cast();

    // Safety: `global` is non-null and came from `GetClipboardData`, so it is
    // the clipboard's own moveable memory for a format we have already checked
    // is stored that way.
    let size = unsafe { GlobalSize(global) };
    if size == 0 {
        return None;
    }

    // Safety: as above. A locked handle must be unlocked, which every path
    // below does before returning.
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        return None;
    }

    // Safety: `GlobalSize` is the length of the block `GlobalLock` just pinned,
    // and the copy ends before the unlock.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) }.to_vec();

    // Safety: balancing the lock above.
    unsafe { GlobalUnlock(global) };

    Some(bytes)
}

/// Read every format currently on the clipboard.
///
/// Never fails: a clipboard that cannot be opened, and one that is empty, both
/// give an empty snapshot. The caller has nothing to put back either way.
#[must_use]
pub fn capture() -> Snapshot {
    let empty = || Snapshot {
        formats: Vec::new(),
        incomplete: false,
    };

    let Some(snapshot) = with_clipboard(|| {
        let mut formats = Vec::new();
        let mut budget = MAX_SNAPSHOT_BYTES;
        let mut incomplete = false;
        let mut id = 0;

        loop {
            // Safety: the clipboard is open, and enumeration starts from zero
            // and walks the list by feeding back the previous format.
            id = unsafe { EnumClipboardFormats(id) };
            if id == 0 {
                break;
            }
            if NOT_GLOBAL_MEMORY.contains(&id) {
                continue;
            }

            // Safety: the clipboard is open and `id` is a format it just said
            // it holds. The returned handle stays owned by the clipboard.
            let Some(bytes) = handle_bytes(unsafe { GetClipboardData(id) }) else {
                tracing::debug!(format = id, "a clipboard format could not be read");
                incomplete = true;
                continue;
            };
            let Some(remaining) = budget.checked_sub(bytes.len()) else {
                incomplete = true;
                continue;
            };
            budget = remaining;
            formats.push(Format { id, bytes });
        }

        if incomplete {
            tracing::debug!(
                formats = formats.len(),
                "the clipboard snapshot is incomplete"
            );
        }

        Snapshot {
            formats,
            incomplete,
        }
    }) else {
        return empty();
    };

    snapshot
}

/// Hand a copy of `bytes` to the clipboard under `id`.
///
/// On success the block belongs to the clipboard and must not be freed here.
fn put(id: u32, bytes: &[u8]) -> bool {
    // Safety: an allocation request; a failure comes back as null.
    let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
    if global.is_null() {
        return false;
    }

    // Safety: `global` is a live moveable block we own until `SetClipboardData`
    // succeeds. Every path below unlocks it, and frees it unless ownership has
    // passed to the clipboard.
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        unsafe { GlobalFree(global) };
        return false;
    }

    // Safety: the block was allocated at exactly `bytes.len()` and is pinned by
    // the lock above, so the two ranges cannot overlap or overrun.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len()) };

    // Safety: balancing the lock above.
    unsafe { GlobalUnlock(global) };

    // Safety: the clipboard is open and emptied, so it will take ownership of
    // the block. A null return means it did not, and the block is still ours.
    if unsafe { SetClipboardData(id, global.cast()) }.is_null() {
        unsafe { GlobalFree(global) };
        return false;
    }
    true
}

/// Put a captured clipboard back, replacing whatever is there now.
///
/// # Errors
///
/// A message naming the step that failed, for the log. Nothing here is worth
/// showing a user verbatim.
pub fn restore(snapshot: &Snapshot) -> Result<(), String> {
    let opened = with_clipboard(|| {
        // Emptying is what takes ownership away from whoever holds it, and has
        // to happen even with nothing to write: leaving our own transcription
        // behind would be worse than an empty clipboard the user can see.
        // Safety: the clipboard is open.
        unsafe { EmptyClipboard() };

        snapshot
            .formats
            .iter()
            .filter(|format| !put(format.id, &format.bytes))
            .inspect(|format| tracing::debug!(format = format.id, "could not be written back"))
            .count()
    });

    match opened {
        None => Err("the clipboard could not be opened to restore it".into()),
        Some(0) => Ok(()),
        Some(failed) => Err(format!("{failed} clipboard formats could not be written back")),
    }
}
