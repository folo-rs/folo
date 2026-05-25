use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::{LayoutDispatch, RawOpaquePool, RawOpaquePoolThreadSafe};

// These are the core data sets shared by the pool objects and the handle objects.
pub(crate) type BlindPoolCore = Arc<Mutex<BlindPoolInnerMap>>;
pub(crate) type LocalBlindPoolCore = Rc<RefCell<LocalBlindPoolInnerMap>>;

pub(crate) type BlindPoolInnerMap = LayoutDispatch<RawOpaquePoolThreadSafe>;
pub(crate) type LocalBlindPoolInnerMap = LayoutDispatch<RawOpaquePool>;
pub(crate) type RawBlindPoolInnerMap = LayoutDispatch<RawOpaquePool>;
