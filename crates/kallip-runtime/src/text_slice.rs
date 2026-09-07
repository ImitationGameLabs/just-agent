//! Budget-driven head+tail text slicing, shared by the tool-result cap
//! (`tool_execution::cap_tool_result`) and the compaction wedge slice
//! (`context::compact::slice_oversized_turn`).
//!
//! Both consumers cut oversized text down to a head+tail keep with an
//! explicit omission marker between. One implementation keeps the marker
//! shape and the cut semantics from drifting across the two faces.

use crate::config::DEFAULT_TOOL_RESULT_LINE_MAX_TOKENS;
use crate::context::estimate_text;

/// Head (60%) + tail (40%) slice of `text` within `cap_chars`, cut boundaries
/// aligned to whole lines where lines are small enough to keep whole.
///
/// Why align to lines at all: cuts through complete lines stay readable, and
/// the omission marker can never be mistaken for a continuation of a half
/// line. Why not *always* align: a line too big to keep whole (over
/// [`DEFAULT_TOOL_RESULT_LINE_MAX_TOKENS`] estimated tokens) has no line
/// semantics left to protect — a minified-JSON line is arbitrary at every
/// char — so there the cut degrades to the raw char position. Dropping such
/// a line outright would blind the agent to exactly the densest output.
///
/// Returns `(head_chars, tail_chars, sliced)`. The counts are the chars
/// actually kept from the start and end of `text` (the omission marker
/// between them is not counted), so callers can report them as-is.
pub(crate) fn head_tail_slice(text: &str, cap_chars: usize) -> (usize, usize, String) {
    let total = text.chars().count();
    if total <= cap_chars {
        return (total, 0, text.to_owned());
    }
    let head_cut = cap_chars * 3 / 5;
    let tail_cut = total - (cap_chars - head_cut);

    let head_end = aligned_head_end(text, head_cut, total);
    let tail_start = aligned_tail_start(text, tail_cut, total);
    // Whole-line extensions can collide when the text barely exceeds the cap
    // (both extensions would reach into the same line and erase the gap);
    // there the raw char positions are the only cut that leaves a gap.
    let (head_end, tail_start) = if head_end >= tail_start {
        (head_cut, tail_cut)
    } else {
        (head_end, tail_start)
    };
    marker_slice(text, head_end, tail_start)
}

/// Pure char-domain variant of [`head_tail_slice`]: no line alignment. Used by
/// the cap's convergence loop as the invariant-preserving fallback once
/// line-aligned cuts stop making progress (see `cap_tool_result`).
pub(crate) fn char_head_tail(text: &str, cap_chars: usize) -> (usize, usize, String) {
    let total = text.chars().count();
    if total <= cap_chars {
        return (total, 0, text.to_owned());
    }
    let head_end = cap_chars * 3 / 5;
    let tail_start = total - (cap_chars - head_end);
    marker_slice(text, head_end, tail_start)
}

/// Head side of the line alignment: the index just past the end of the line
/// containing `pos` when that line is small enough to keep whole, else `pos`.
fn aligned_head_end(text: &str, pos: usize, total: usize) -> usize {
    let (start, end) = line_bounds(text, pos, total);
    if line_est(text, start, end) <= DEFAULT_TOOL_RESULT_LINE_MAX_TOKENS {
        end
    } else {
        pos
    }
}

/// Tail side: the index of the start of the line containing `pos` when that
/// line is small enough to keep whole, else `pos`.
fn aligned_tail_start(text: &str, pos: usize, total: usize) -> usize {
    let (start, end) = line_bounds(text, pos, total);
    if line_est(text, start, end) <= DEFAULT_TOOL_RESULT_LINE_MAX_TOKENS {
        start
    } else {
        pos
    }
}

/// Char-index bounds of the line containing `pos`: `start` is just past the
/// previous `\n` (or 0), `end` just past the line's own `\n` (or `total`).
fn line_bounds(text: &str, pos: usize, total: usize) -> (usize, usize) {
    let mut start = 0;
    for (i, c) in text.chars().enumerate().take(pos) {
        if c == '\n' {
            start = i + 1;
        }
    }
    let mut end = total;
    for (i, c) in text.chars().enumerate().skip(pos) {
        if c == '\n' {
            end = i + 1;
            break;
        }
    }
    (start, end)
}

fn line_est(text: &str, start: usize, end: usize) -> usize {
    estimate_text(
        &text
            .chars()
            .skip(start)
            .take(end - start)
            .collect::<String>(),
    )
}

/// Head + omission marker + tail. The marker stands on its own lines between
/// the two kept ends: seamless concatenation can fuse the cut ends into
/// command or text sequences neither side contained (a prompt-injection
/// vector), and the reader must see where — and how much — was dropped. Same
/// shape as the shell capture face's `bytes omitted` marker.
fn marker_slice(text: &str, head_end: usize, tail_start: usize) -> (usize, usize, String) {
    let head: String = text.chars().take(head_end).collect();
    let tail: String = text.chars().skip(tail_start).collect();
    let omitted = tail_start - head_end;
    let sliced = format!("{head}\n[... {omitted} chars omitted ...]\n{tail}");
    (head_end, text.chars().count() - tail_start, sliced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_cap_returns_text_verbatim() {
        let (h, t, s) = head_tail_slice("hello\nworld\n", 100);
        assert_eq!((h, t, s.as_str()), (12, 0, "hello\nworld\n"));
    }

    #[test]
    fn whole_lines_are_kept_on_both_sides() {
        let mut text = String::new();
        for i in 0..100 {
            text.push_str(&format!("L{i:03} {}\n", "x".repeat(60)));
        }
        let (h, t, sliced) = head_tail_slice(&text, 1_000);
        assert!(sliced.contains("chars omitted"));
        // Head part ends at a line end (its last char is the line's own \n).
        let m = sliced.find("\n[... ").unwrap();
        assert_eq!(
            sliced.as_bytes()[m - 1],
            b'\n',
            "head must end at a line end"
        );
        assert_eq!(
            sliced[..m].chars().count(),
            h,
            "head count must match the kept prefix"
        );
        // Tail part starts at a line start, and that line is a whole source line.
        let after = sliced.find("chars omitted ...]\n").unwrap() + "chars omitted ...]\n".len();
        let tail = &sliced[after..];
        assert!(tail.starts_with('L'), "tail must start at a line start");
        let line_end = tail.find('\n').unwrap() + 1;
        assert!(
            text.contains(&tail[..line_end]),
            "tail must begin with a complete source line"
        );
        assert!(t >= tail.chars().count());
    }

    #[test]
    fn overlong_lines_degrade_to_char_cuts() {
        // 10K latin chars ≈ 2.5K estimated tokens: both crossing lines are
        // over the whole-line threshold, so no extension happens.
        let text = format!("{}\n{}\n", "x".repeat(10_000), "y".repeat(10_000));
        let (h, t, sliced) = head_tail_slice(&text, 1_000);
        assert_eq!((h, t), (600, 400));
        assert_eq!(
            sliced,
            format!(
                "{}\n[... 19002 chars omitted ...]\n{}\n",
                "x".repeat(600),
                "y".repeat(399)
            )
        );
    }

    #[test]
    fn cjk_lines_align_by_token_threshold_not_char_count() {
        // 1K CJK chars ≈ 1K estimated tokens: under the threshold despite the
        // shorter char run, so the head keeps that line whole.
        let text = format!("{}\n{}\n", "错".repeat(1_000), "错".repeat(20_000));
        let (h, _, sliced) = head_tail_slice(&text, 1_000);
        assert_eq!(h, 1_001, "the small CJK line must be kept whole");
        assert!(sliced.starts_with(&format!("{}\n", "错".repeat(1_000))));
    }

    #[test]
    fn extension_collision_falls_back_to_char_positions() {
        let mut text = String::new();
        for _ in 0..40 {
            text.push_str(&format!("{}\n", "x".repeat(99)));
        }
        let (h, t, sliced) = head_tail_slice(&text, 3_900);
        assert_eq!(sliced.find("chars omitted"), Some(2_350));
        assert_eq!((h, t), (2_340, 1_560));
    }

    #[test]
    fn single_line_without_newline_cuts_at_char_positions() {
        let text = "x".repeat(5_000);
        let (h, t, sliced) = head_tail_slice(&text, 1_000);
        assert_eq!((h, t), (600, 400));
        assert_eq!(
            sliced,
            format!(
                "{}\n[... 4000 chars omitted ...]\n{}",
                "x".repeat(600),
                "x".repeat(400)
            )
        );
    }

    #[test]
    fn degenerate_cap_still_leaves_a_declared_gap() {
        let (_, _, sliced) = head_tail_slice(&"x".repeat(100), 1);
        assert!(sliced.contains("chars omitted"));
    }

    #[test]
    fn char_cut_ignores_line_structure() {
        let text = format!("a{}\nb{}\n", "x".repeat(900), "y".repeat(900));
        let (h, t, sliced) = char_head_tail(&text, 100);
        assert_eq!((h, t), (60, 40));
        assert!(sliced.starts_with('a'));
        assert!(sliced.ends_with(&format!("{}\n", "y".repeat(39))));
        assert!(sliced.contains("chars omitted"));
    }
}
