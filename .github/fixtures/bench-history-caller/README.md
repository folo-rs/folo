# Reusable workflow caller fixture

This standalone virtual Cargo workspace supplies a tiny real collection target for the
repository's reusable-workflow integration check. The bench delegates engine output to the
existing faker library; it writes deterministic Criterion artifacts rather than measuring
wall-clock performance.

Its project and container isolate synthetic data in the existing test storage account.
The caller check creates the dedicated container through the test identity's data access.
Neither this fixture nor its workflow provisions Azure management resources or uses the
production history account.
