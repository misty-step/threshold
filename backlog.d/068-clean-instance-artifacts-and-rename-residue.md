# Clean instance artifacts and rename residue

Priority: P1 · Status: ready · Estimate: L

Epic. Created by the 2026-07-01 factory groom.

## Goal

Make the repository public-able and less misleading by separating product code
from instance artifacts, cleaning stale Daedalus-era references where they are
not historical evidence, and retiring dead local/planning surfaces.

## Oracle

- [ ] A run-artifact audit classifies every tracked `runs/` file as committed
      evidence to preserve, generated HTML/report surface to archive, or
      instance-local artifact that should move out of the product repo.
- [ ] No committed `runs/*.jsonl` history is edited or deleted; append-only run
      records stay intact.
- [ ] Live docs and commands use `threshold`; Daedalus references remain only in
      preserved historical approvals, deliveries, changelog entries, or run
      artifacts with an explicit reason.
- [ ] README Naming is rewritten for Threshold instead of mechanically carrying
      the old Daedalus rationale, and Crucible is no longer listed as an
      available candidate name.
- [ ] Stale docs and local-only surfaces from the groom report are either
      archived, deleted, or explicitly kept with a reason:
      `docs/threshold-ui-catalog.html`, `docs/threshold-ui-lab/`, old HTML plan
      files, `docs/rust-migration.md`, `.groom/lanes/`, local `jobs/`,
      `harbor-build/`, `.pytest_cache/`, and pycache directories.
- [ ] Pycompat parity tests are either retired or recast as historical
      compatibility tests that do not shape new schema choices.
- [ ] `bin/gate` passes.

## Verification System

- Claim: the product repo no longer looks like a personal instance dump or a
  half-renamed Daedalus repo.
- Falsifier: `rg -n "daedalus|Daedalus"` in non-historical surfaces finds live
  product docs/commands; tracked instance artifacts contain personal paths with
  no preservation rationale; or the cleanup deletes append-only run records.
- Driver: `find runs -maxdepth 2 -type f`, targeted `rg`, docs audit, and local
  disk cleanup.
- Grader: residual-reference allowlist, artifact classification table, git diff,
  and `bin/gate`.
- Evidence packet: cleanup report committed under docs or backlog notes, plus
  command output in the PR body.
- Cadence: once for the rename/groom cleanup; then repeat before public release.

## Children

1. **Run artifact classification.** Start from the current tracked count
   (`find runs -maxdepth 2 -type f`) and classify before moving or deleting.
2. **Rename residue pass.** DONE (2026-07-02, operator-ordered daedalus→threshold
   rename). Replaced the two live-code `daedalus` references in
   `crates/threshold-core/src/optimization_target.rs` (the `redact_local_paths`
   marker and its test) and the `.gitignore` build-artifacts comment.
   `~/Development/daedalus` moved to `~/Development/threshold`; GitHub repo
   `misty-step/daedalus` renamed to `misty-step/threshold`. Remaining
   `daedalus`/`Daedalus` hits are all under `runs/`, `approvals/`, `deliveries/`,
   `.groom/lanes/`, and `CHANGELOG.md` — preserved historical evidence per this
   ticket's own oracle, left untouched.
3. **README Naming rewrite.** DONE (2026-07-02). Rewrote the Naming section with
   a Threshold-specific rationale and dropped Crucible from the candidate list.
4. **Dead-surface retirement.** Not done in this pass — out of scope for the
   rename; still open.
5. **Local artifact flush.** Not done in this pass — out of scope for the
   rename; still open.
6. **Pycompat retirement decision.** Not done in this pass — out of scope for
   the rename; still open.

## Notes

- The groom report observed 210 tracked run files; the current live checkout
  reports 256 at shallow depth after the upstream v1.1.0 fast-forward. Verify
  the exact set again when executing.
- Do not rewrite published history to scrub committed evidence unless the
  operator explicitly authorizes it.
- 2026-07-02: children 2 and 3 (rename residue + README) closed as part of the
  operator-ordered daedalus→threshold rename. Children 1, 4, 5, 6 remain open;
  epic stays `status: ready` until those land.
