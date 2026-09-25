# Region-local storage

Region-local storage gives each memory region its own value while allowing threads in that
region to share updates. Its purpose is to support data sets whose writes need not cross memory
regions. Independent regions do not observe each other's local writes.

Values can be declared as static variables or created dynamically and distributed through linked
per-thread objects. Both forms use the same regional sharing behavior and accept initial values
that require runtime construction.

Writes are weakly consistent within a memory region. A thread that migrates between regions
can observe different local values; applications requiring stable region-local sequencing pin
their threads to the relevant region.
