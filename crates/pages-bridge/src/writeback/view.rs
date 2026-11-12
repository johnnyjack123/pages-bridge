//! Resetting what the view state remembers about the old text.
//!
//! Pages caches the character range of every laid-out page and the text selection. After the
//! text changes, both point at the wrong characters and possibly past its end.

use iwork::{Edits, Encoder, Package};

/// Page layout cache: field 3 and 4 page counts, 5 the pages with their character ranges,
/// 6 the number of characters laid out.
const LAYOUT_CACHE: u32 = 10131;
/// A text selection: range in field 2 (location, length), location again in 10, range in 12.
const SELECTION: u32 = 10132;
/// `NSNotFound` as a 32-bit location: "no selection".
const NOT_FOUND: u64 = 4_294_967_295;

pub(crate) fn reset(pkg: &Package, edits: &mut Edits) {
    for id in pkg.ids_of_kind(LAYOUT_CACHE) {
        let Some(cache) = pkg.object(id) else { continue };
        let mut e = Encoder::new();
        e.copy_except(&cache, &[3, 4, 5, 6]).uint(3, 0).uint(4, 0).uint(6, 0);
        edits.replace_object(id, e.finish(), pkg.references(id).to_vec());
    }
    for id in pkg.ids_of_kind(SELECTION) {
        let Some(selection) = pkg.object(id) else { continue };
        let location = selection.msg(2).and_then(|r| r.uint(1));
        if location.is_none_or(|l| l == NOT_FOUND || l == 0) {
            continue;
        }
        let mut range = Encoder::new();
        range.uint(1, 0).uint(2, 0);
        let range = range.finish();
        let mut e = Encoder::new();
        e.copy_except(&selection, &[2, 10, 12]).bytes(2, &range).uint(10, 0);
        if selection.has(12) {
            e.bytes(12, &range);
        }
        edits.replace_object(id, e.finish(), pkg.references(id).to_vec());
    }
}
