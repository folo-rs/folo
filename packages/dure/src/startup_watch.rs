//! Startup deadline coordination.

use std::sync::Mutex;

use crate::pal::ids::ConnId;

/// Indicates that the watchdog claimed connection teardown before settlement.
#[derive(Debug)]
pub(crate) struct StartupExpired;

/// Coordinates ownership of a connection with a startup deadline watchdog.
///
/// The blocking operation and its watchdog race through this state so exactly
/// one side owns connection teardown, and a response cannot be accepted after
/// the watchdog has committed the operation to failure.
#[derive(Debug, Default)]
pub(crate) struct StartupWatch {
    conn: Option<ConnId>,
    expired: bool,
}

impl StartupWatch {
    /// Start with an established connection owned by the watchdog.
    pub(crate) fn for_connection(conn: ConnId) -> Self {
        Self {
            conn: Some(conn),
            expired: false,
        }
    }

    /// Mark the operation expired and take the connection the watchdog must tear down.
    pub(crate) fn expire(watch: &Mutex<Self>) -> Option<ConnId> {
        let mut watch = watch
            .lock()
            .expect("startup watch is only read and written here, never across a panic");
        watch.expired = true;
        watch.conn.take()
    }

    /// Register an accepted connection and report whether the deadline already expired.
    pub(crate) fn register(watch: &Mutex<Self>, conn: ConnId) -> bool {
        let mut watch = watch
            .lock()
            .expect("startup watch is only read and written here, never across a panic");
        if watch.expired {
            return true;
        }
        watch.conn = Some(conn);
        false
    }

    /// Release the connection unless the watchdog already claimed its teardown.
    pub(crate) fn settle(watch: &Mutex<Self>) -> Result<(), StartupExpired> {
        let mut watch = watch
            .lock()
            .expect("startup watch is only read and written here, never across a panic");
        watch.conn = None;
        if watch.expired {
            Err(StartupExpired)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn a_connection_registered_before_expiry_is_owned_by_the_watchdog() {
        let watch = Mutex::new(StartupWatch::default());
        let conn = ConnId(7);
        assert!(!StartupWatch::register(&watch, conn));
        assert_eq!(StartupWatch::expire(&watch), Some(conn));
        assert_eq!(StartupWatch::expire(&watch), None);
    }

    #[test]
    fn a_connection_registered_after_expiry_is_owned_by_the_caller() {
        let watch = Mutex::new(StartupWatch::default());
        assert_eq!(StartupWatch::expire(&watch), None);
        assert!(StartupWatch::register(&watch, ConnId(7)));
        assert_eq!(StartupWatch::expire(&watch), None);
    }

    #[test]
    fn a_settled_connection_is_not_owned_by_the_watchdog() {
        let watch = Mutex::new(StartupWatch::default());
        assert!(!StartupWatch::register(&watch, ConnId(7)));
        StartupWatch::settle(&watch).unwrap();
        assert_eq!(StartupWatch::expire(&watch), None);
    }

    #[test]
    fn settling_reports_that_expiry_already_won() {
        let watch = Mutex::new(StartupWatch::for_connection(ConnId(7)));
        assert_eq!(StartupWatch::expire(&watch), Some(ConnId(7)));
        StartupWatch::settle(&watch).unwrap_err();
    }
}
