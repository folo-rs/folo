# Unsafe code

This chapter covers how to write safety comments for `unsafe` code in this
workspace. Unwind safety (`UnwindSafe`/`RefUnwindSafe`), the `!Sync` marker
pattern, and manual `Send`/`Sync` impls live in their own chapter:
[`docs/unwind-safety.md`](unwind-safety.md).

## Safety comments

Safety comments must explain how we satisfy the safety requirements of the unsafe
function we are calling. The API documentation of an unsafe function has a
"Safety" section that poses a challenge and the safety comment is the response —
the two must be paired and correspond to each other.

Safety comments are not there just to re-state the requirements or make generic
claims, they must specifically explain how we satisfy the safety requirements of
the function we are calling (e.g. by referencing an assertion, a type invariant,
earlier logic or other mechanism).

When creating a reference from a raw pointer (`unsafe { &*ptr }`,
`unsafe { &mut *ptr }`, `NonNull::as_ref`, `NonNull::as_mut`, etc.), the safety
comment must address BOTH:

1. **Validity** — the pointer is non-null, properly aligned, points to an
   initialized value of the correct type, and the pointee outlives the new
   reference.
2. **Aliasing** — explain why no conflicting reference exists for the duration of
   the new borrow:
   * For `&T`: no `&mut T` to the same memory may exist concurrently.
   * For `&mut T`: no other `&T` or `&mut T` to the same memory may exist
     concurrently. This includes references on other threads.

Documenting only validity is insufficient. Rust's aliasing rules are equally
strict and equally easy to violate, especially when constructing references from
raw pointers stored across function calls or threads. Justify aliasing by
referencing locks held, single-threaded ownership, `!Send`/`!Sync` markers,
scope-bounded borrows, atomic-only access, or other concrete mechanisms.

Safety comments are also required in examples and doctests that use `unsafe`
blocks.

Safety comments (whether single- or multiline) go above the line with the
`unsafe` block. To be clear, they are associated with a specific `unsafe` block,
not with a function call. Example:

```rust
/// SAFETY: We ensured above that the pointer is valid and aligned for the type `T`.
unsafe {
    unsafe_function(pointer);
}
```

Each `unsafe` block is expected to only have one call to an unsafe function, and
should not have nontrivial safe code inside it.

Good example — only unsafe code in the unsafe block:

```rust
/// SAFETY: We ensured above that the pointer is valid and aligned for the type `T`.
let entity = Entity::from(unsafe {
    unsafe_function(pointer)
});
```

Bad example — `unsafe` block includes safe code:

```rust
/// SAFETY: We ensured above that the pointer is valid and aligned for the type `T`.
let entity = unsafe {
    Entity::from(unsafe_function(pointer))
};
```
