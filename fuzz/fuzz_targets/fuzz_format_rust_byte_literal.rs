#![no_main]
//! Fuzz harness for `straightjacket::commands::reproducer_to_test::format_rust_byte_literal`.
//!
//! Invariants checked:
//!   1. Output starts with `&[` and ends with `]`.
//!   2. The number of `0x` hex-marker substrings equals the input length
//!      (every input byte is rendered exactly once).
//!   3. The byte count and wrap behavior agree: there must be exactly
//!      `input.len().saturating_sub(1) / 12` embedded newlines (wraps occur
//!      after every 12th rendered byte, starting at index 12).
//!
//! Watching for: panics on degenerate sizes (0, 1, very large), incorrect
//! wrap counts (off-by-one at the 12/13 boundary), and any drop/duplication
//! of input bytes in the emitted literal.

use libfuzzer_sys::fuzz_target;
use straightjacket::commands::reproducer_to_test::format_rust_byte_literal;

fuzz_target!(|data: &[u8]| {
    let out = format_rust_byte_literal(data);

    // Shape invariant: well-formed `&[ ... ]` literal envelope.
    assert!(
        out.starts_with("&["),
        "output must start with `&[`; got: {out:?}"
    );
    assert!(
        out.ends_with(']'),
        "output must end with `]`; got: {out:?}"
    );

    // Faithful encoding: one `0x` marker per input byte, no dropping or duplication.
    let marker_count = out.matches("0x").count();
    assert_eq!(
        marker_count,
        data.len(),
        "expected {} `0x` markers (one per input byte), got {} in: {out:?}",
        data.len(),
        marker_count,
    );

    // Wrap-count invariant: a newline is emitted before the 13th byte (index 12)
    // and every 12 bytes thereafter. For N input bytes, expected newlines =
    // (N - 1) / 12 when N >= 1, else 0.
    let expected_newlines = if data.is_empty() { 0 } else { (data.len() - 1) / 12 };
    let actual_newlines = out.matches('\n').count();
    assert_eq!(
        actual_newlines,
        expected_newlines,
        "wrap count mismatch for {} bytes: expected {} newlines, got {} in: {out:?}",
        data.len(),
        expected_newlines,
        actual_newlines,
    );
});
