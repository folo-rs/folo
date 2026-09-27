# Bootstrap binary command adapter

This nonpublished executable supports the existing `ReleaseBinaries.psm1` caller
until operational cutover. Its `plan` and `run` arguments, matrix JSON, item-array
`outcomes.json` and step summary remain the private bootstrap workflow protocol.
They are not unified publication manifests or a supported Rust library API.

The shell supplies process arguments, working directory, summary destination,
diagnostic sink and its group-aligned version to `crp_publication::legacy`.
That explicitly temporary adapter retains legacy parsing, runner/budget projection
and receipt formatting. It reuses the publication-owned binary identity, asset
completeness predicate, `Github`, `BinaryPublisher` and `execute_items`.
Before an uploading run observes assets or builds, its supervised `gh` adapter
resolves each exact tag and peels annotations to verify the frozen source commit.
The private matrix fields and one-batch-per-target shape do not change that check.

`BinaryPublisher` composes `crp_native` for immutable source worktrees, compiler
and artifact verification, archives, cancellation, deadlines and cleanup. Upload
uses native's actual controller directory and item deadline. The adapter must
not manufacture a new deadline, query current main in place of a frozen source,
or supply credentials to build children.

The no-upload option skips tag and release asset queries and uploads while retaining
native source/build/archive behavior, including repository acquisition when a
source object is missing. Successful, failed, unattempted and cleanup outcomes
remain visible; a cleanup failure prevents a successful command exit even when
an item uploaded successfully.

The executable-connected smoke suite covers the private protocol, GitHub adapter
behavior and a representative native wiring path. Full source, archive, feature-isolation
and cancellation scenarios stay with the permanent application and component owners.
A native fake `gh` executable exercises tag and asset process boundaries without
contacting GitHub; in-process tests cover parsing and deterministic decisions.

The cutover removes this package and `crp_publication::legacy` only after all old
publisher callers have been replaced. Removing the module also changes the
published implementation package's content and needs release reassessment.
