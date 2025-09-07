use std::alloc::Layout;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use foldhash::HashMap;

use crate::{RawOpaquePool, RawOpaquePoolSend};

// These are the core data sets shared by the pool objects and the handle objects.
pub(crate) type BlindPoolCore = Arc<Mutex<BlindPoolInnerMap>>;
pub(crate) type LocalBlindPoolCore = Rc<RefCell<LocalBlindPoolInnerMap>>;

pub(crate) type BlindPoolInnerMap = HashMap<Layout, RawOpaquePoolSend>;
pub(crate) type LocalBlindPoolInnerMap = HashMap<Layout, RawOpaquePool>;
