//! Preserving the whole pasteboard across an insertion.
//!
//! Reached only as `platform::backend::clipboard`. The clipboard plugin can
//! read and write text and images and nothing else, which is not enough to put
//! a rich clipboard back: a table copied out of a spreadsheet, or a formatted
//! selection, arrives as RTF and HTML and an image rendering all at once, and
//! restoring only the text would throw the rest away.
//!
//! Reading every flavour is also what makes the *text* survive. macOS declares
//! flavours it can derive (plain text from RTF, TIFF from PNG) without
//! materialising them, and serves the data later by asking the application that
//! did the copying. Once that application has quit, an underived flavour can no
//! longer be produced and reads as absent even though the pasteboard still
//! lists it. Asking for it while the owner is alive both fulfils it and caches
//! it, so a snapshot taken here holds text that a read taken later would miss.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString};

/// A ceiling on what one snapshot may hold in memory.
///
/// A pasteboard has no size limit of its own, and a copied page of a design
/// tool can run to hundreds of megabytes. Past this the snapshot stops
/// collecting and reports itself incomplete, which costs the user the tail of a
/// very large clipboard rather than costing them the app.
const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;

/// One flavour of one pasteboard item: its uniform type identifier, and the
/// bytes the pasteboard served for it.
struct Flavour {
    kind: Retained<NSString>,
    bytes: Retained<NSData>,
}

/// Everything the pasteboard held, item by item and flavour by flavour.
pub struct Snapshot {
    items: Vec<Vec<Flavour>>,
    incomplete: bool,
}

impl Snapshot {
    /// Whether there was nothing on the pasteboard to put back.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether a declared flavour could not be captured, so restoring this
    /// snapshot will not reproduce the pasteboard exactly.
    #[must_use]
    pub const fn is_incomplete(&self) -> bool {
        self.incomplete
    }
}

/// Read every flavour of every item currently on the general pasteboard.
///
/// Never fails: a pasteboard that cannot be enumerated, and one that is empty,
/// both give an empty snapshot. The difference does not matter to the caller,
/// which has nothing to put back either way.
#[must_use]
pub fn capture() -> Snapshot {
    let pasteboard = NSPasteboard::generalPasteboard();
    let Some(items) = pasteboard.pasteboardItems() else {
        tracing::debug!("the pasteboard could not be enumerated; nothing will be restored");
        return Snapshot {
            items: Vec::new(),
            incomplete: false,
        };
    };

    let mut captured = Vec::new();
    let mut budget = MAX_SNAPSHOT_BYTES;
    let mut incomplete = false;

    for item in &items {
        let mut flavours = Vec::new();
        for kind in &item.types() {
            // A flavour macOS listed but cannot produce, because whoever put it
            // there has gone. Nothing can be done about it here; the point of
            // asking is that the ones which *can* still be served are cached by
            // the asking.
            let Some(bytes) = item.dataForType(&kind) else {
                tracing::debug!(kind = %kind, "a pasteboard flavour could not be read");
                incomplete = true;
                continue;
            };
            let Some(remaining) = budget.checked_sub(bytes.len()) else {
                incomplete = true;
                continue;
            };
            budget = remaining;
            flavours.push(Flavour { kind, bytes });
        }
        // An item every flavour of which was unreadable cannot be written back,
        // and an empty item would make `writeObjects` reject the whole lot.
        if !flavours.is_empty() {
            captured.push(flavours);
        }
    }

    if incomplete {
        tracing::debug!(
            items = captured.len(),
            "the pasteboard snapshot is incomplete"
        );
    }

    Snapshot {
        items: captured,
        incomplete,
    }
}

/// Put a captured pasteboard back, replacing whatever is there now.
///
/// # Errors
///
/// A message naming the step that failed, for the log. Nothing here is worth
/// showing a user verbatim.
pub fn restore(snapshot: &Snapshot) -> Result<(), String> {
    let pasteboard = NSPasteboard::generalPasteboard();

    // Clearing is what takes ownership away from whoever holds it, and has to
    // happen even when there is nothing to write: leaving our own transcription
    // behind would be worse than an empty pasteboard the user can see is empty.
    pasteboard.clearContents();
    if snapshot.items.is_empty() {
        return Ok(());
    }

    let mut items = Vec::with_capacity(snapshot.items.len());
    for flavours in &snapshot.items {
        let item = NSPasteboardItem::new();
        for flavour in flavours {
            if !item.setData_forType(&flavour.bytes, &flavour.kind) {
                tracing::debug!(kind = %flavour.kind, "a pasteboard flavour could not be written");
            }
        }
        items.push(ProtocolObject::from_retained(item));
    }

    let items: Retained<NSArray<ProtocolObject<dyn NSPasteboardWriting>>> =
        NSArray::from_retained_slice(&items);

    if pasteboard.writeObjects(&items) {
        Ok(())
    } else {
        Err("NSPasteboard rejected the restored items".into())
    }
}
