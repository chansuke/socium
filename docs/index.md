# Documentation

Start with the [README](../README.md) for installation and everyday use.

## Current behavior

- [Reliability and recovery](daily-reliability.md): permissions, editing, cancellation, saved state, and recovery.
- [Security policy](../SECURITY.md): trust boundaries and private vulnerability reporting.
- [Public release review](public-release-review.md): cleanup, security findings, checks, and remaining limits.
- [Contributing](../CONTRIBUTING.md): development and verification.

## Work tracking

- [Improvement tickets](tickets/README.md) and [GitHub tracker #52](https://github.com/chansuke/socium/issues/52).
- [Issue conventions](tickets/github-workflow.md) and [machine-readable map](tickets/github-issues.json).
- [Earlier plan reconciliation](improvement-plan.md) and [earlier TODO reconciliation](todo.md) retain old task IDs without maintaining competing backlogs.

## Historical evidence

The [2026-11-11 comparison review](review-socium-pi-dsh-2026-11-11.md) and its
[reproduction files](review-evidence/2026-11-11/) describe the reviewed commit,
not the current release. The reproduction assertions intentionally demonstrate
old failures and are not part of the current test suite.

The [first reliability milestone](daily-reliability-validation.md) records what
PR #94 validated. Subsequent changes and test counts are documented in their PRs.
