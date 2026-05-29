#![no_main]
//! Fuzz harness for `straightjacket::commands::hook::decide_pre_adversarial`.
//!
//! This is a security boundary — the function decides whether a prompt
//! delivered to an adversarial-* specialist is allowed through. A bypass
//! means a sandbox-escape vector. The contract we assert here is the
//! bidirectional substring invariant:
//!
//!   - If ANY string in `FORBIDDEN_ADVERSARIAL_STRINGS` is a substring of
//!     the prompt, the decision MUST be `Deny`.
//!   - If NONE of the forbidden strings is a substring, the decision MUST
//!     be `Allow`.
//!
//! Watching for: panics on multi-byte boundary inputs, Unicode-normalization
//! bypasses (where a NF-encoded variant of a forbidden marker would slip
//! through `.contains()`), and any drift between the documented invariant
//! and the implementation.

use libfuzzer_sys::fuzz_target;
use straightjacket::commands::hook::{
    decide_pre_adversarial, HookDecision, FORBIDDEN_ADVERSARIAL_STRINGS,
};

fuzz_target!(|prompt: &str| {
    let decision = decide_pre_adversarial(prompt);

    let contains_forbidden = FORBIDDEN_ADVERSARIAL_STRINGS
        .iter()
        .any(|f| prompt.contains(f));

    match (&decision, contains_forbidden) {
        (HookDecision::Deny(_), true) => {
            // Correct: forbidden marker present, denied. OK.
        }
        (HookDecision::Allow, false) => {
            // Correct: clean prompt, allowed. OK.
        }
        (HookDecision::Deny(_), false) => {
            panic!(
                "false-positive Deny: prompt does not contain any forbidden marker but \
                 decide_pre_adversarial denied. prompt={prompt:?}, decision={decision:?}"
            );
        }
        (HookDecision::Allow, true) => {
            panic!(
                "BYPASS: prompt contains a forbidden marker but decide_pre_adversarial \
                 allowed. prompt={prompt:?}, decision={decision:?}"
            );
        }
        (HookDecision::RunChecks(_), _) => {
            panic!(
                "decide_pre_adversarial must never return RunChecks; got: {decision:?} \
                 for prompt={prompt:?}"
            );
        }
    }
});
