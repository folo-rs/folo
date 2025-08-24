//! Demonstrates partial initialization with `insert_with()`.
//!
//! This example shows the primary use case for `insert_with()`: creating objects with
//! some fields intentionally left uninitialized. This provides significant performance
//! benefits by avoiding unnecessary memory writes to `MaybeUninit` fields.

use std::mem::MaybeUninit;

use blind_pool::BlindPool;

/// A data structure where some fields start uninitialized by design.
/// This pattern is common in high-performance synchronization primitives and buffers.
#[derive(Debug)]
struct PartialBuffer {
    /// Always initialized - contains metadata about the buffer.
    capacity: usize,
    /// Always initialized - tracks how much of the buffer is used.
    len: usize,
    /// Starts uninitialized - will be filled on demand.
    /// Using `MaybeUninit` saves us from having to write 4KB of zeros.
    data: MaybeUninit<[u8; 4096]>,
    /// Starts uninitialized - checksum computed when buffer is complete.
    checksum: MaybeUninit<u32>,
}

impl PartialBuffer {
    /// Initializes a `PartialBuffer` in-place, leaving data and checksum uninitialized.
    /// This saves 4100 bytes of unnecessary writes compared to using `insert()`.
    fn new_in_place(uninit: &mut MaybeUninit<Self>, capacity: usize) {
        let ptr = uninit.as_mut_ptr();

        // SAFETY: We have exclusive access to this uninitialized memory.
        let capacity_ptr = unsafe { std::ptr::addr_of_mut!((*ptr).capacity) };
        // SAFETY: We have exclusive access to this uninitialized memory.
        let len_ptr = unsafe { std::ptr::addr_of_mut!((*ptr).len) };

        // SAFETY: We're writing to our own struct's capacity field.
        unsafe {
            capacity_ptr.write(capacity);
        }
        // SAFETY: We're writing to our own struct's len field.
        unsafe {
            len_ptr.write(0);
        }
    }

    /// Fills the buffer with data, initializing the data field.
    fn fill_data(&mut self, source: &[u8]) {
        let to_copy = source.len().min(4096);

        // Initialize the data field
        let mut buffer = [0_u8; 4096];
        buffer
            .get_mut(..to_copy)
            .unwrap()
            .copy_from_slice(source.get(..to_copy).unwrap());
        self.data.write(buffer);

        self.len = to_copy;
    }

    /// Computes and stores the checksum, initializing the checksum field.
    fn compute_checksum(&mut self) {
        // SAFETY: data must be initialized before calling this method
        let data = unsafe { self.data.assume_init_ref() };
        let checksum = data
            .get(..self.len)
            .unwrap()
            .iter()
            .map(|&b| u32::from(b))
            .sum();
        self.checksum.write(checksum);
    }

    /// Gets the checksum if it has been computed.
    fn get_checksum(&self) -> u32 {
        // SAFETY: In this example we assume checksum has been computed.
        unsafe { self.checksum.assume_init() }
    }
}

/// A simpler example: a cache entry that starts with only key initialized.
#[allow(
    dead_code,
    reason = "example struct to demonstrate partial initialization"
)]
struct CacheEntry {
    key: String,
    /// Value is computed/fetched later when needed
    value: MaybeUninit<Vec<u8>>,
    /// Timestamp set when value is computed
    computed_at: MaybeUninit<u64>,
}

impl CacheEntry {
    fn new_in_place(uninit: &mut MaybeUninit<Self>, key: String) {
        let ptr = uninit.as_mut_ptr();

        // SAFETY: We have exclusive access to this uninitialized memory.
        let key_ptr = unsafe { std::ptr::addr_of_mut!((*ptr).key) };

        // SAFETY: We're writing to our own struct's key field.
        unsafe {
            key_ptr.write(key);
        }
        // Note: Leave value and computed_at uninitialized
    }
}

fn main() {
    println!("=== BlindPool Partial Initialization Examples ===");
    println!();

    demonstrate_partial_buffer();
    demonstrate_cache_entry();
    demonstrate_performance_benefit();

    println!("Partial initialization completed successfully!");
}

/// Demonstrates the primary use case: partial buffer initialization.
fn demonstrate_partial_buffer() {
    println!("Example 1: Partial buffer initialization");
    println!("---------------------------------------");

    let pool = BlindPool::new();

    // Create a buffer with only metadata initialized.
    // This avoids writing 4100 bytes of uninitialized data that insert() would copy.
    // SAFETY: We properly initialize required fields.
    let buffer = unsafe {
        pool.insert_with(|uninit| {
            PartialBuffer::new_in_place(uninit, 4096);
        })
    };

    println!("Created buffer with capacity: {}", buffer.capacity);
    println!("Initial length: {}", buffer.len);

    // Later, when we have data to store, we initialize the data field
    let sample_data = b"Hello, this is some sample data for our buffer!";
    // SAFETY: We have exclusive access to the buffer
    unsafe {
        let buffer_mut = buffer.ptr().as_mut();
        buffer_mut.fill_data(sample_data);
        buffer_mut.compute_checksum();
    }

    println!("After filling: length = {}", buffer.len);
    println!("Checksum: {}", buffer.get_checksum());

    println!();
}

/// Demonstrates cache entry with lazy value computation.
fn demonstrate_cache_entry() {
    println!("Example 2: Cache entry with lazy initialization");
    println!("----------------------------------------------");

    let pool = BlindPool::new();

    // Create cache entries with only keys, values computed on demand
    // SAFETY: We initialize the key.
    let entry1 = unsafe {
        pool.insert_with(|uninit| {
            CacheEntry::new_in_place(uninit, "user:123".to_string());
        })
    };

    // SAFETY: We initialize the key.
    let entry2 = unsafe {
        pool.insert_with(|uninit| {
            CacheEntry::new_in_place(uninit, "session:abc".to_string());
        })
    };

    println!("Created cache entry with key: '{}'", entry1.key);
    println!("Created cache entry with key: '{}'", entry2.key);
    println!("Values and timestamps will be computed when needed");

    println!();
}

/// Demonstrates the performance benefit over regular `insert()`.
fn demonstrate_performance_benefit() {
    println!("Example 3: Performance benefit demonstration");
    println!("-------------------------------------------");

    // For comparison, let's show what regular insert() would do:
    println!("With insert(): Rust copies ALL fields from stack to pool");
    println!("  - 4096 bytes of uninitialized data in buffer");
    println!("  - 4 bytes of uninitialized checksum");
    println!("  - Total unnecessary writes: 4100 bytes");
    println!();

    println!("With insert_with(): Only initialized fields are written");
    println!("  - capacity: 8 bytes");
    println!("  - len: 8 bytes");
    println!("  - Total necessary writes: 16 bytes");
    println!("  - Savings: 4084 bytes per allocation!");
    println!();

    println!("Performance benefit increases with:");
    println!("  - Larger MaybeUninit fields");
    println!("  - More uninitialized fields");
    println!("  - Higher allocation frequency");
}
