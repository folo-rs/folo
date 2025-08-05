mod abstractions;
mod facade;

pub(crate) use abstractions::*;
pub(crate) use facade::*;

#[cfg(all(unix, not(miri)))]
mod unix;
#[cfg(all(unix, not(miri)))]
pub(crate) use unix::*;

#[cfg(all(windows, not(miri)))]
mod windows;
#[cfg(all(windows, not(miri)))]
pub(crate) use windows::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
pub(crate) use mock::*;

// We do not cfg(miri) this simply because that disables IDE editor support, which is annoying.
mod rust;
#[cfg_attr(not(miri), expect(unused_imports, reason = "conditional"))]
pub(crate) use rust::*;
