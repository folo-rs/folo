# Bootstrap candidate command adapter

This nonpublished executable retains the command interface used by
`ReleasePublication.psm1`: a manifest path, full candidate and release-line
commits, repeated exact package/version requests, and optional verbose output.
Successful verification writes its result to stdout; failure writes diagnostics
to stderr and returns failure. Quiet successful checks do not emit verbose notes.

The shell selects stderr as the diagnostic destination.
`crp_publication::legacy::verify_candidate` parses the private bootstrap grammar
and invokes `crp_publication::publication::candidate::verify` using a typed
`CandidateRequest`. Candidate policy calls `crp_versioning` directly and obtains
source facts from `crp_workspace`; neither depends on the executable package.

The existing compatibility library exports candidate `Metadata`/`Repository`
and workspace `snapshot_command::{capture, git}` for its retained regression
suite. These are private maintainer interfaces, not a supported Rust library API.
Permanent implementation tests remain in their owning packages.

Native I/O cases acquire the shared Windows scheduling slot before starting
their conservative last-chance watchdog. Pure scheduling tests retain the normal
in-process watchdog, and mutation testing retains watchdog disablement.

The operational cutover removes this package and the temporary legacy parser
only after removing its publisher callers. Keep the unified candidate operation
and permanent tests intact.
