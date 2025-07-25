# Changes at runtime

It is possible that a system will have processors added or removed at runtime, or for
constraints enforced by the operating system to change over time. Such changes will not be
represented in an existing processor set - once created, a processor set is static.

Changes to resource quotas can be applied by creating a new processor set (e.g. if the
processor time quota is lowered, building a new set will by default use the new quota).

This package will not detect more fundamental changes such as added/removed processors. Operations
attempted on removed processors may fail with an error or panic or silently misbehave (e.g.
threads never starting). Added processors will not be considered a member of any set.