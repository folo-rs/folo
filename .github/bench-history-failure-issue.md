---
title: "{{ env.FAILURE_ISSUE_TITLE }}"
labels: bench-history, ci-failure
---

The nightly **bench-history** workflow failed (collection or analysis). The
benchmark history may be missing the latest `main` commit until this is fixed.

Failed run: {{ env.RUN_URL }}

This issue is updated, not duplicated, on each failing night, and is closed
automatically once the workflow runs green again.
