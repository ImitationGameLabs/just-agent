---
name: Test Discrimination
description: When you write or review a regression test that claims to lock a failure — how to empirically prove it discriminates by re-breaking the code and requiring a red, with the cargo test --exact silent-zero trap and safe backup/restore discipline
---

# Test Discrimination — prove the red before trusting the green

A regression test earns its place by failing when the behavior it guards
breaks. `code/testing` defines the criteria for that value; this skill is the
empirical procedure that verifies it — because a test whose red was never
observed may be green-forever decoration, passing on broken code and fixed code
alike, and "it should fail" is a prediction, not evidence.

## When to use

- You added a test — or a reviewer asked you to prove one — that claims to lock
  a specific regression: a fix, a guard, an error path.
- You are reviewing a change whose tests are described as regression locks and
  must judge whether the lock is real.
- A suite went green and you suspect some of it is vacuous.

## When NOT to use

- To judge what makes a test worth writing at all — behavior focus, naming,
  mocking — that is `code/testing`, the standard this procedure serves.
- To investigate a failing test you do not yet understand — use
  `code/debugging`; its Guard step produces exactly the locks this verifies.

## The sequence

**State the discrimination claim.** One sentence: the broken behavior, the
assertion that must fail against it, and what the broken code produces instead.
A claim you cannot state is a test that discriminates nothing.
Done when:

- the sentence names the failure mode and the specific expected red output

**Snapshot what you will re-break.** Copy each file you are about to mutate to
a scratch path outside the tree and record the paths — the verification
deliberately re-introduces the bug, so the working state must stay recoverable.
Done when:

- fresh copies exist at recorded scratch paths, tree otherwise untouched

**Re-introduce the old behavior, one variable.** Undo the fix minimally —
revert the single guard, restore the old branch, or substitute a sentinel
value. One change per probe round, because two simultaneous mutations leave
you unsure which one the test discriminates against.
Done when:

- the broken state differs from the working one by a minimal, single-concern hunk

**Run the test and require a named red.** Run the specific test against the
broken state. It must fail — and the output must show it actually ran. Never
trust the exit code alone: `cargo test --exact <name>` with a name that is not
the full test path silently runs 0 tests and exits 0, a green that proves
nothing. Omit `--exact` unless you pass the complete path, and read the
`test result` line's ran count, not just the exit status.
Done when:

- the test fails on the broken code, and the failure output names the test and
  the assertion, with a ran count of at least one

**Restore and prove the restoration.** Copy the fresh snapshots back, then
prove the tree is whole: the test is green again, `git status` shows no
residue, and a grep for any sentinel values you injected comes back zero.
Done when:

- test green, `git status` clean, sentinel and marker count zero

**Record the evidence.** The observed red — test name and failing assertion
against the old behavior — goes into the report or review notes next to the
test: the claim from the first step, now demonstrated rather than asserted.
Done when:

- the discrimination proof is written where this batch's reviewers will read it

## Key behaviors to remember

- **A probe that passes on broken code proves the opposite of its claim** — it
  is not locking the regression; rewrite the assertion or delete the test,
  because a test that cannot tell broken from fixed has no regression value.
- **Restore from the fresh snapshot, never an older one** — a stale backup
  silently drops tests written after it, turning a verification into an
  accidental deletion.
- **Count tests, not exit codes** — runners exit 0 for zero matches; the ran
  count is the only proof the probe touched its target.

## Anti-patterns

- **Asserting discrimination without running the red** — the assertion
  "clearly targets" the bug is a prediction; predictions about test failures
  are wrong often enough that the two-minute empirical check pays for itself.
- **Re-breaking several things at once** — the red then fails to identify
  which behavior the test discriminates; keep one variable per probe round.
- **Skipping the restoration proof** — leaving the bug or a sentinel behind
  converts a verification into a new incident; close with green, clean status,
  and zero markers.
