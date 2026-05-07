<!-- claude-protocol-managed -->
# PR self-review checklist

Run through this before requesting human review. Skim items don't apply, but justify in the PR body.

- [ ] Diff size is justified by scope — no padding, no scope creep
- [ ] No phantom imports or dead code paths
- [ ] No empty function bodies or no-op stubs
- [ ] Tests assert real behavior, not tautologies (`expect(x).toBe(x)` and friends)
- [ ] No fabricated stack traces or invented error types
- [ ] Comments explain *why*, not *what*; no padding
- [ ] Any new dependency: registry-checked, age-checked, justified in PR body
- [ ] Lockfile changes audited line-by-line
- [ ] Author can defend every changed line in review
