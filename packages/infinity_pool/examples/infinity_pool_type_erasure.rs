//! Example demonstrating type-erasure for lifetime extension.
//!
//! This example shows how to use `.erase()` to extend the lifetime of pooled objects
//! while discarding type information. This is useful when you need to keep objects alive
//! but do not need to access them through type-erased handles.

use infinity_pool::{OpaquePool, Pooled};

/// A resource that needs to be kept alive even after we stop needing typed access.
#[derive(Debug)]
struct Resource {
    id: usize,
    data: Vec<u8>,
}

impl Resource {
    fn new(id: usize, size: usize) -> Self {
        Self {
            id,
            data: vec![0; size],
        }
    }
}

/// Demonstrates lifetime extension using type-erased handles.
fn demonstrate_lifetime_extension() {
    let pool = OpaquePool::with_layout_of::<Resource>();

    // Insert multiple resources into the pool.
    let handle1 = pool.insert(Resource::new(1, 1024));
    let handle2 = pool.insert(Resource::new(2, 2048));
    let handle3 = pool.insert(Resource::new(3, 4096));

    println!("Created 3 resources in pool");
    println!("Pool length: {}", pool.len());

    // We need typed access to some resources but just want to keep others alive.
    let typed_handle: Pooled<Resource> = handle1.into_shared();
    println!("Resource 1 ID: {}", typed_handle.id);
    println!("Resource 1 data size: {}", typed_handle.data.len());

    // For handle2 and handle3, we do not need typed access anymore,
    // but we want to keep them alive. We can erase their types.
    let _erased_handle2 = handle2.into_shared().erase();
    let _erased_handle3 = handle3.into_shared().erase();

    println!("Erased type information for resources 2 and 3");
    println!("Pool length: {} (all resources still alive)", pool.len());

    // The typed_handle can still be used normally.
    println!("Can still access resource 1: ID={}", typed_handle.id);

    // The erased handles prevent the resources from being removed,
    // but we cannot access the data through them.

    // When all handles (typed and erased) are dropped, the resources are removed.
    drop(typed_handle);
    drop(_erased_handle2);
    drop(_erased_handle3);

    println!("All handles dropped, pool length: {}", pool.len());
}

/// Demonstrates mixing typed and type-erased handles.
fn demonstrate_mixed_handles() {
    let pool = OpaquePool::with_layout_of::<Resource>();

    let handle = pool.insert(Resource::new(42, 8192));

    // Create multiple shared handles - some typed, some erased.
    let shared_typed = handle.into_shared();
    let shared_typed_clone = shared_typed.clone();
    let shared_erased = shared_typed.clone().erase();

    println!();
    println!("Mixed handles demonstration:");
    println!("Created 1 typed handle + 2 clones (1 typed, 1 erased)");

    // Both typed handles can access the resource.
    println!("Typed handle 1: ID={}", shared_typed.id);
    println!("Typed handle 2: ID={}", shared_typed_clone.id);

    // Drop one typed handle, the resource remains accessible.
    drop(shared_typed);
    println!(
        "Dropped one typed handle, resource still accessible: ID={}",
        shared_typed_clone.id
    );

    // Drop the remaining typed handle.
    drop(shared_typed_clone);
    println!("Dropped last typed handle, but erased handle keeps resource alive");
    println!("Pool length: {}", pool.len());

    // When the erased handle is dropped, the resource is finally removed.
    drop(shared_erased);
    println!("Dropped erased handle, pool length: {}", pool.len());
}

fn main() {
    demonstrate_lifetime_extension();
    demonstrate_mixed_handles();
}
