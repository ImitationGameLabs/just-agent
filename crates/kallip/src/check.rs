//! `kallip check`: machine checks that stand alone from any task.
//!
//! `check message` runs the commit-message battery the team applies by
//! hand — subject shape and width, body length, bullet consistency, line
//! width, and the process-word/numbering/timestamp scans — reporting each
//! check with its measured value. Numbers are re-derived from git at run
//! time; the command never reads a report's claims, only the object.
//!
//! Relation to the written standard (hygiene charter / team-protocol
//! message rules): where the two overlap, the battery follows the
//! charter — intro up to 3 lines, and the numbering scan covers the
//! N/B/E/R/T/Q/M review-artifact shape family. `ascii_only` is a
//! battery addition with no charter counterpart, kept intentionally:
//! messages are ASCII-only in practice.

use anyhow::{Result, anyhow};
use clap::Args as ClapArgs;
use serde::Serialize;
use std::process::Command;

/// One check result: the verdict plus the measured value it came from.
#[derive(Serialize)]
pub struct CheckOutcome {
    pub name: String,
    pub pass: bool,
    pub value: String,
}

/// The battery for one commit message.
#[derive(Serialize)]
pub struct MessageReport {
    pub commit: String,
    pub subject: String,
    pub pass: bool,
    pub checks: Vec<CheckOutcome>,
}

#[derive(ClapArgs)]
pub struct CheckMessageArgs {
    /// Emit the machine face (JSON) instead of text.
    #[arg(long)]
    pub json: bool,
    /// Commit range or single ref to check (default HEAD).
    pub range: Option<String>,
}

fn git(args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// A paragraph is a run of consecutive non-empty body lines. A
/// bullet paragraph's bullets are its lines starting "- "; every
/// following line belongs to the bullet above it. Prose-only
/// paragraphs are not bullet paragraphs and are skipped.
fn bullet_paragraphs(body: &str) -> Vec<(usize, Vec<usize>)> {
    let mut paragraphs: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut current: Option<(usize, Vec<usize>)> = None;
    for line in body.lines() {
        if line.trim().is_empty() {
            if let Some(p) = current.take() {
                paragraphs.push(p);
            }
            continue;
        }
        if line.trim_start().starts_with("- ") {
            match &mut current {
                Some((count, bullets)) => {
                    *count += 1;
                    bullets.push(1);
                }
                None => current = Some((1, vec![1])),
            }
        } else if let Some((_, bullets)) = &mut current
            && let Some(last) = bullets.last_mut()
        {
            *last += 1;
        }
    }
    if let Some(p) = current {
        paragraphs.push(p);
    }
    paragraphs
}

fn check_message(commit: &str, message: &str) -> MessageReport {
    let mut checks: Vec<CheckOutcome> = Vec::new();
    let mut record = |name: &str, pass: bool, value: String| {
        checks.push(CheckOutcome {
            name: name.to_string(),
            pass,
            value,
        })
    };

    let mut lines = message.lines();
    let subject = lines.next().unwrap_or("").trim_end().to_string();
    let body: String = message.lines().skip(1).collect::<Vec<_>>().join("\n");

    // Subject shape.
    let subject_starts_lowercase = subject
        .chars()
        .next()
        .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .unwrap_or(false);
    record(
        "subject_lowercase_start",
        subject_starts_lowercase,
        subject.chars().next().map(String::from).unwrap_or_default(),
    );
    let no_period = !subject.ends_with('.');
    record(
        "subject_no_period",
        no_period,
        format!("ends_with_dot={}", !no_period),
    );
    let width = subject.len();
    record("subject_width", width <= 72, width.to_string());

    // Intro length: prose lines before the first blank line.
    let intro: Vec<&str> = message
        .lines()
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .collect();
    record("intro_lines", intro.len() <= 3, intro.len().to_string());

    // Body length: non-empty body lines. Bullets may be grouped into
    // paragraphs; each group past the first buys three more lines than
    // the single-group budget of 15.
    let body_nonempty = body.lines().filter(|l| !l.trim().is_empty()).count();
    let groups = bullet_paragraphs(&body).len();
    let body_budget = 15 + 3 * groups.saturating_sub(1);
    record(
        "body_nonempty",
        body_nonempty <= body_budget,
        format!("{body_nonempty}/{body_budget} groups={groups}"),
    );

    // Bullets: every bullet paragraph holds 3+ bullets and no bullet
    // wraps past 3 lines. Wrap counts may differ between bullets —
    // the team standard is shape consistency, not equal wrap counts
    // (reviewed history holds mixed-wrap paragraphs).
    let paragraphs = bullet_paragraphs(&body);
    let mut bullet_summary = String::new();
    let mut bullets_ok = true;
    for (count, lines_per_bullet) in &paragraphs {
        let wraps: Vec<usize> = lines_per_bullet.iter().map(|n| n - 1).collect();
        let bounded = wraps.iter().all(|w| *w <= 3);
        if *count < 3 || !bounded {
            bullets_ok = false;
        }
        let max_wrap = wraps.iter().max().copied().unwrap_or(0);
        bullet_summary.push_str(&format!("{count}b/{max_wrap}w "));
    }
    record("bullets", bullets_ok, bullet_summary.trim_end().to_string());

    // Line width: the hardest line in the whole message.
    let max_width = message.lines().map(|l| l.len()).max().unwrap_or(0);
    record("max_line_width", max_width <= 75, max_width.to_string());

    // Vocabulary scan: review-process references. Bare "review" is
    // excluded on purpose — it is task-domain vocabulary (review seats,
    // review receipts, the review status), not a process reference.
    let lower = message.to_ascii_lowercase();
    let process_hit = [
        "reviewer",
        "review findings",
        "review cycle",
        "finding",
        "blocking",
        "verified",
        "tested",
    ]
    .iter()
    .any(|w| lower.contains(w));
    record("process_words", !process_hit, format!("hit={process_hit}"));

    let numbering = ["N", "B", "E", "R", "T", "Q", "M"].iter().any(|p| {
        message
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| t.len() > 1)
            .any(|t| t.starts_with(*p) && t[1..].chars().all(|c| c.is_ascii_digit()))
    });
    record("internal_numbering", !numbering, format!("hit={numbering}"));

    let timestamp = message
        .split_whitespace()
        .any(|t| t.len() == 10 && t.as_bytes()[4] == b'-' && t.as_bytes()[7] == b'-');
    record("timestamp", !timestamp, format!("hit={timestamp}"));

    let non_ascii = !message.is_ascii();
    record("ascii_only", !non_ascii, format!("hit={non_ascii}"));

    let pass = checks.iter().all(|c| c.pass);
    MessageReport {
        commit: commit.to_string(),
        subject,
        pass,
        checks,
    }
}

pub fn run_message_check(args: &CheckMessageArgs) -> Result<()> {
    let range = args.range.as_deref().unwrap_or("HEAD");
    // A bare ref names one commit: checking "HEAD" must not walk
    // the whole history behind it; only a..b ranges walk.
    let hashes: Vec<String> = if range.contains("..") {
        git(&["log", "--format=%H", range])?
            .lines()
            .map(str::to_string)
            .collect()
    } else {
        vec![
            git(&["log", "-1", "--format=%H", range])?
                .trim_end()
                .to_string(),
        ]
    };
    if hashes.is_empty() {
        return Err(anyhow!("no commits in range '{range}'"));
    }

    let mut reports = Vec::new();
    for hash in &hashes {
        let message = git(&["log", "-1", "--format=%B", hash])?;
        reports.push(check_message(hash, &message));
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for r in &reports {
            println!(
                "commit {} — {}",
                &r.commit[..7.min(r.commit.len())],
                r.subject
            );
            for c in &r.checks {
                println!(
                    "  {:<22} {}  {}",
                    c.name,
                    if c.pass { "PASS" } else { "FAIL" },
                    c.value
                );
            }
            println!("  verdict: {}", if r.pass { "PASS" } else { "FAIL" });
        }
    }

    let all_pass = reports.iter().all(|r| r.pass);
    if !all_pass {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict_of(message: &str) -> Vec<CheckOutcome> {
        let report = check_message("test", message);
        report.checks
    }

    fn find<'a>(checks: &'a [CheckOutcome], name: &str) -> &'a CheckOutcome {
        checks.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn grouped_bullets_extend_the_body_budget() {
        // intro(2) + two bullet groups of 8: 18 non-empty lines, exactly
        // the two-group budget of 15+3(g-1)=18 but over the 15-line
        // single-group budget; one line more must fail.
        let m = "subject line\n\nIntro prose stands alone here, two lines.\nand one more so it is two.\n\n- alpha wraps twice\n  a\n  b\n- beta wraps once\n  like so\n- gamma wraps twice\n  a\n  b\n\n- delta wraps twice\n  a\n  b\n- epsilon wraps once\n  like so\n- zeta wraps twice\n  a\n  b\n";
        let checks = verdict_of(m);
        let body = find(&checks, "body_nonempty");
        assert!(body.pass, "{}", body.value);
    }

    #[test]
    fn body_one_line_past_grouped_budget_fails() {
        let m = "subject line\n\nIntro prose stands alone here, two lines.\nand one more so it is two.\n\n- alpha wraps twice\n  a\n  b\n- beta wraps once\n  like so\n- gamma wraps three times\n  a\n  b\n  c\n\n- delta wraps twice\n  a\n  b\n- epsilon wraps once\n  like so\n- zeta wraps twice\n  a\n  b\n";
        let checks = verdict_of(m);
        assert!(!find(&checks, "body_nonempty").pass);
    }

    const THREE_BULLETS_ONE_WRAP: &str = "subject line\n\n\"Prose intro stands before the bullets and must not\nbe counted against them.\n\n- first bullet, wrapped once\n  like this\n- second bullet, wrapped once\n  like this\n- third bullet, wrapped once\n  like this\n\nTrailing prose is fine too.\n";

    #[test]
    fn prose_paragraphs_do_not_break_bullet_consistency() {
        let checks = verdict_of(THREE_BULLETS_ONE_WRAP);
        let bullets = find(&checks, "bullets");
        assert!(bullets.pass, "{}", bullets.value);
    }

    #[test]
    fn differing_wrap_counts_are_consistent_shape() {
        let m = "subject line\n\n- one\n- two wraps\n  once more\n- three\n";
        let checks = verdict_of(m);
        let bullets = find(&checks, "bullets");
        assert!(bullets.pass, "{}", bullets.value);
    }

    #[test]
    fn bullets_beyond_three_wraps_fail() {
        let m = "subject line\n\n- one\n- two\n  wraps\n  three\n  four\n  five\n- three\n- four\n";
        let checks = verdict_of(m);
        assert!(!find(&checks, "bullets").pass);
    }

    #[test]
    fn domain_review_vocabulary_is_not_a_process_hit() {
        let m = "feat(task): dispatch review seats\n\n- records review\n  receipts per seat\n- registers the\n  dispatch roster\n- gates close on\n  the receipt count\n";
        let checks = verdict_of(m);
        assert!(
            find(&checks, "process_words").pass,
            "bare domain review must not hit"
        );
        let m2 = "x: y\n\n- per the reviewer\n  findings we\n- did fix the\n  three spots\n- and blocked\n  regression\n";
        let checks2 = verdict_of(m2);
        assert!(!find(&checks2, "process_words").pass);
    }
}
