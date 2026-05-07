# Contributing

Thanks for working on this project. The conventions below keep reviews quick and the codebase honest.

## Development tooling

This codebase is developed with the help of AI coding assistants (currently Claude Code) for exploration, scaffolding, and refactor proposals. The author reviews and is accountable for every committed line. Architecture decisions, dependency additions, security-sensitive code, and public API changes are reviewed by a human before landing.

## Disclosure conventions

Commits that used AI assistance carry an `Assisted-by:` trailer naming the model (per the Linux kernel's coding-assistants convention). Commits without the trailer were authored without AI assistance. We do not use `Co-Authored-By: Claude` (false authorship; AI agents cannot sign DCO).

## Self-review

Authors run through `.github/PR_SELF_REVIEW.md` before requesting review. PRs over ~500 lines should be split unless they are pure mechanical refactors.

## Branch + commit

- One purpose per PR; small diffs land faster.
- No `--no-verify` on commits unless you have a stated reason.
- No force-push to shared branches (`main`, `master`, `develop`, `release/*`).
