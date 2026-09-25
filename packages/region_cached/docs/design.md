# Region-cached storage

Region-cached storage keeps regional copies of shared values so readers can use a copy local to
their memory region. Publishing a value updates the shared source and makes that value available
to every region; regional copies are caches rather than independent authorities.

Values can be declared as static variables or created dynamically and distributed through linked
per-thread objects. Both forms support runtime initialization and the same global publication
behavior.

Updates are weakly consistent. Concurrent writers have no defined resolution order, and thread
migration can break expectations about an immediately following read. Applications requiring
stable sequencing use a single writer and region-pinned readers.
