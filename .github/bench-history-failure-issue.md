---
title: "{{ env.FAILURE_ISSUE_TITLE }}"
labels: bench-history, ci-failure
---

The **bench-history** workflow failed (collection or analysis). The
benchmark history may be missing the pushed `main` commit until this is fixed.

Failed run: {{ env.RUN_URL }}

This issue is updated, not duplicated, on each failing run, and is closed
automatically once the workflow runs green again.
