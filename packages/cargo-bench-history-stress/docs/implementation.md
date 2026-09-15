# Implementation

The stress harness generates synthetic history, seeds storage, and invokes the real
analysis entry point. Its workload and output are described in the [README](../README.md).
The generated repository and stored benchmark objects must describe the same commit
topology so the measured analysis follows its ordinary Git and storage paths.

## Repository construction

Repository construction writes the dated branch history through Git fast-import,
reads Git's exported marks to obtain the assigned commit IDs, and restores a clean
main-branch checkout. Storage seeding uses those IDs rather than independently
inventing identities. Subprocess launch, input, completion, and unsuccessful exits
are errors; an absent marks file is not interchangeable with an empty one.

Library tests exercise the Git and filesystem helpers directly with temporary
directories and minimal history. They check observable repository creation, imported
branch identities, failed exits, and complete marks contents. Using real Git here
keeps command construction and pipe handling within the tested boundary without a
mock that duplicates Git's protocol. These tests participate in library-only mutation
testing; their last-chance watchdogs are disabled by the normal mutation environment.
Miri excludes them because it cannot execute the subprocess and filesystem paths.

The separate integration suite runs the stress binary and verifies the synthetic
dataset's findings through the complete analysis pipeline. That broader coverage
belongs to ordinary testing and coverage, not each mutation's execution budget.
