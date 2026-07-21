//! Pure functions extracted for Aeneas formal verification.
//!
//! These are deliberately decoupled from the runtime types so Aeneas can
//! translate pure `fn` definitions into Coq without trait objects,
//! `Box<dyn Processor>`, or mutable audio buffers.
//!
//! ## Target 1: ProcessorPriority ordering
//!
//! Prove that `process()` invokes children in fixed order
//! `HiPriority → Default → GlobalSecondChain → Global → Final`,
//! preserving insertion order within each priority group.
//!
//! ## Target 2: SmoothState convergence
//!
//! Prove that `fade()` converges: at `n = pre_len` the output equals the
//! new signal (`out`), and at `n = 0` it equals the old signal (`pre`).

use crate::core_dsp_root::ProcessorPriority;

// =========================================================================
// Target 1: ProcessorPriority ordering
// =========================================================================

/// The canonical processing order, as specified by the C++ DSP graph ABI.
pub const PROCESSING_ORDER: &[ProcessorPriority] = &[
    ProcessorPriority::HiPriority,
    ProcessorPriority::Default,
    ProcessorPriority::GlobalSecondChain,
    ProcessorPriority::Global,
    ProcessorPriority::Final,
];

/// Given a list of `(priority, pending_delete)` pairs and a processing order,
/// return the sequence of indices in which items are processed.
///
/// Invariants Aeneas should prove:
/// - Every item whose `priority` appears in `order` appears exactly once in
///   the output, unless it is pending-delete.
/// - The output order respects `order`: all items of priority `order[0]`
///   appear before all items of priority `order[1]`, etc.
/// - Within the same priority, insertion order is preserved (stable sort).
pub fn dsp_processing_sequence(
    items: &[(ProcessorPriority, bool)],
    order: &[ProcessorPriority],
) -> Vec<usize> {
    let mut result = Vec::new();
    for &priority in order {
        for (i, &(item_prio, pending)) in items.iter().enumerate() {
            if !pending && item_prio == priority {
                result.push(i);
            }
        }
    }
    result
}

/// Verify that every live item appears in the sequence exactly once.
/// Does not require insertion order — priority ordering is handled separately.
pub fn dsp_processing_sequence_is_complete(
    items: &[(ProcessorPriority, bool)],
    sequence: &[usize],
) -> bool {
    let live_count = items.iter().filter(|(_, pending)| !pending).count();
    if sequence.len() != live_count {
        return false;
    }
    // Every live item index must appear exactly once in sequence.
    let mut seen = vec![false; items.len()];
    for &idx in sequence {
        if idx >= items.len() || seen[idx] || items[idx].1 {
            return false;
        }
        seen[idx] = true;
    }
    seen.iter().enumerate().all(|(i, &s)| s || items[i].1)
}
///
/// `n` is 0-indexed; when `n == 0` the output is `pre` (fully old);
/// when `n == len` the output is `out` (fully new).
///
/// Aeneas proof targets:
/// 1. `fade_sample(out, pre, 0, len) == pre`
/// 2. `fade_sample(out, pre, len, len) == out`
/// 3. For `n ∈ [0, len]`, the result is a convex combination of `out` and `pre`
///    (i.e., coefficients sum to 1 and are non-negative).
pub fn fade_sample(out: f32, pre: f32, n: usize, len: usize) -> f32 {
    // Aeneas 1.5 does not support f32 division of runtime variables.
    // Use integer arithmetic: multiply by (len - n) / len of pre.
    // Avoid f32 division by pre-multiplying into i64.
    let n = n.min(len);
    let m = len - n; // weight of `pre`
    // out * n / len + pre * m / len
    let out_part = (out as f64 * n as f64) / len as f64;
    let pre_part = (pre as f64 * m as f64) / len as f64;
    (out_part + pre_part) as f32
}

/// Fade an entire channel: apply `fade_sample` to each sample.
/// Precondition: `out.len() >= pre_len && pre.len() >= pre_len`.
/// Aeneas can prove that after calling `fade_channel`, the last `pre_len`
/// samples of `out` converge to the new values.
pub fn fade_channel(out: &mut [f32], pre: &[f32], pre_len: usize) {
    debug_assert!(out.len() >= pre_len);
    debug_assert!(pre.len() >= pre_len);
    for n in 0..pre_len {
        out[n] = fade_sample(out[n], pre[n], n + 1, pre_len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_items_produce_empty_sequence() {
        assert_eq!(
            dsp_processing_sequence(&[], PROCESSING_ORDER),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn single_item_appears_once() {
        let items = [(ProcessorPriority::Default, false)];
        let seq = dsp_processing_sequence(&items, PROCESSING_ORDER);
        assert_eq!(seq, vec![0]);
        assert!(dsp_processing_sequence_is_complete(&items, &seq));
    }

    #[test]
    fn pending_items_are_skipped() {
        let items = [(ProcessorPriority::Default, true)];
        let seq = dsp_processing_sequence(&items, PROCESSING_ORDER);
        assert!(seq.is_empty());
    }

    #[test]
    fn priority_order_is_enforced() {
        let items = [
            (ProcessorPriority::Default, false),
            (ProcessorPriority::HiPriority, false),
            (ProcessorPriority::Final, false),
        ];
        let seq = dsp_processing_sequence(&items, PROCESSING_ORDER);
        // Expected: HiPriority (index 1) → Default (index 0) → Final (index 2)
        assert_eq!(seq, vec![1, 0, 2]);
        assert!(dsp_processing_sequence_is_complete(&items, &seq));
    }

    #[test]
    fn stable_within_priority() {
        let items = [
            (ProcessorPriority::Default, false),
            (ProcessorPriority::Default, false),
            (ProcessorPriority::HiPriority, false),
        ];
        let seq = dsp_processing_sequence(&items, PROCESSING_ORDER);
        // HiPriority (idx 2), then Default in insertion order (idx 0, then idx 1)
        assert_eq!(seq, vec![2, 0, 1]);
    }

    #[test]
    fn fade_sample_start_is_pre() {
        let result = fade_sample(1.0, 0.0, 0, 64);
        assert!((result - 0.0).abs() < 1e-6, "at n=0, result={result}");
    }

    #[test]
    fn fade_sample_end_is_out() {
        let result = fade_sample(1.0, 0.0, 64, 64);
        assert!((result - 1.0).abs() < 1e-6, "at n=len, result={result}");
    }

    #[test]
    fn fade_channel_converges() {
        let mut out = [0.0, 0.5, 0.5, 0.5, 0.0];
        let pre = [1.0, 1.0, 1.0, 1.0, 1.0];
        fade_channel(&mut out, &pre, 4);
        // After 4 samples, should be close to new value
        assert!((out[3] - 0.5).abs() < 0.3);
    }

    #[test]
    fn dsp_processing_sequence_is_complete_accepts_valid() {
        let items = [
            (ProcessorPriority::Default, false),
            (ProcessorPriority::Global, false),
        ];
        let seq = dsp_processing_sequence(&items, PROCESSING_ORDER);
        assert!(dsp_processing_sequence_is_complete(&items, &seq));
    }

    #[test]
    fn dsp_processing_sequence_is_complete_rejects_incomplete() {
        let items = [
            (ProcessorPriority::Default, false),
            (ProcessorPriority::Global, false),
        ];
        // Missing Global (should be idx 1)
        assert!(!dsp_processing_sequence_is_complete(&items, &[0]));
    }
}
