//! Session management for allocation tracking.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{OnceLock, atomic};

use tracking_allocator::AllocationRegistry;

use crate::tracker::MemoryTracker;
use crate::operation::Operation;

/// Manages allocation tracking session state and contains operations.
///
/// This type ensures that allocation tracking is properly enabled and disabled,
/// and prevents multiple concurrent tracking sessions which would interfere with
/// each other. It also serves as a container for tracking operations.
///
/// # Examples
///
/// ```rust
/// use std::alloc::System;
///
/// use alloc_tracker::{Session, Allocator};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<s> = Allocator::system();
///
/// let mut session = Session::new();
/// let mut string_op = session.operation("do_stuff_with_strings");
///
/// for _ in 0..3 {
///     let _span = string_op.span();
///     // TODO: Some string stuff here that we want to analyze.    
/// }
///
/// // Output statistics of all operations to console.
/// println!("{session}");
/// ```
#[derive(Debug)]
pub struct Session {
    operations: HashMap<String, Operation>,
}

static TRACKING_SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
static TRACKER_INITIALIZED: OnceLock<()> = OnceLock::new();

impl Session {
    /// Creates a new allocation tracking session.
    ///
    /// This will automatically set up the global tracker (on first use) and enable
    /// allocation tracking. Only one session can be active at a time.
    ///
    /// # Panics
    ///
    /// Panics if another allocation tracking session is already active.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::alloc::System;
    ///
    /// use alloc_tracker::{Session, Allocator};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<s> = Allocator::system();
    ///
    /// let mut session = Session::new();
    /// // Allocation tracking is now enabled
    /// // Session will disable tracking when dropped
    /// ```
    #[expect(
        clippy::new_without_default,
        reason = "Default implementation would be inappropriate as new() can panic"
    )]
    pub fn new() -> Self {
        // Initialize the tracker on first use.
        TRACKER_INITIALIZED.get_or_init(|| {
            // If this fails, it might conceivably be another crate that is concurrently
            // using tracking_allocator, in which case we panic because there is nothing we can do.
            AllocationRegistry::set_global_tracker(MemoryTracker)
                .expect("global allocation tracker was already set by someone else");
        });

        // Try to acquire the session lock
        TRACKING_SESSION_ACTIVE
            .compare_exchange(
                false,
                true,
                atomic::Ordering::Acquire,
                atomic::Ordering::Relaxed,
            )
            .expect("another allocation tracking session is already active");

        AllocationRegistry::enable_tracking();

        Self {
            operations: HashMap::new(),
        }
    }

    /// Creates or retrieves an operation with the given name.
    ///
    /// This method exclusively borrows the session, ensuring that operations
    /// cannot be created concurrently. If an operation with the given name
    /// already exists, its existing statistics are preserved.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::alloc::System;
    ///
    /// use alloc_tracker::{Session, Allocator};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<s> = Allocator::system();
    ///
    /// let mut session = Session::new();
    /// let mut string_op = session.operation("string_operations");
    /// 
    /// for _ in 0..3 {
    ///     let _span = string_op.span();
    ///     let _s = String::from("test"); // This allocation will be tracked
    /// }
    /// ```
    pub fn operation(&mut self, name: impl Into<String>) -> &mut Operation {
        let name = name.into();
        
        // Get or create the operation
        self.operations.entry(name).or_insert_with_key(|name| Operation::new(name.clone()))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        AllocationRegistry::disable_tracking();
        TRACKING_SESSION_ACTIVE.store(false, atomic::Ordering::Release);
    }
}

impl fmt::Display for Session {
    #[cfg_attr(test, mutants::skip)] // No API contract.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operations.is_empty() {
            writeln!(f, "No operations recorded.")?;
        } else {
            writeln!(f, "Allocation statistics:")?;
            
            // Sort operations by name for consistent output
            let mut sorted_ops: Vec<_> = self.operations.iter().collect();
            sorted_ops.sort_by_key(|(name, _)| *name);
            
            for (_, operation) in sorted_ops {
                writeln!(f, "  {operation}")?;
            }
        }
        
        Ok(())
    }
}
