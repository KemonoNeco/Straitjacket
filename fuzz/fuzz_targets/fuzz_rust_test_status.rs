#![no_main]
//! Fuzz harness for `straightjacket::commands::run_new_tests::rust_test_status`.
//!
//! `rust_test_status` builds a `Regex` from a user-controlled `name` field
//! using `regex::escape` + `unwrap()`. The unwrap is exactly the place
//! fuzzing earns its keep: an adversarial name (regex metacharacters,
//! large unicode codepoints, embedded NULs) must not crash the function.
//!
//! Inputs are split deterministically by the `arbitrary` crate's
//! `(&str, &str)` impl: `(output, name)`.
//!
//! Watching for:
//!   - Regex::new panic on adversarial names (regex-escape edge cases).
//!   - Any non-{Pass, Fail, Unknown} result (impossible by type, but the
//!     exhaustive match locks the contract).
//!   - On empty `output`, status must be `Unknown` (no test results to scan).

use libfuzzer_sys::fuzz_target;
use straightjacket::commands::run_new_tests::{rust_test_status, TestStatus};

fuzz_target!(|input: (&str, &str)| {
    let (output, name) = input;
    let status = rust_test_status(output, name);

    // The result is a closed enum — any future widening that returns a
    // non-{Pass, Fail, Unknown} variant breaks this match.
    match status {
        TestStatus::Pass | TestStatus::Fail | TestStatus::Unknown => {}
    }

    // Empty test output cannot contain any test result line, so the only
    // sound status is Unknown regardless of name.
    if output.is_empty() {
        assert_eq!(
            status,
            TestStatus::Unknown,
            "empty output must yield Unknown; got {status:?} for name={name:?}"
        );
    }
});
