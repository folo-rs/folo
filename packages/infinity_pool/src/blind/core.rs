use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::{LayoutKey, RawOpaquePool, RawOpaquePoolSend};

// These are the core data sets shared by the pool objects and the handle objects.
pub(crate) type BlindPoolCore = Arc<Mutex<BlindPoolInnerMap>>;
pub(crate) type LocalBlindPoolCore = Rc<RefCell<LocalBlindPoolInnerMap>>;

pub(crate) type BlindPoolInnerMap = BTreeMap<LayoutKey, RawOpaquePoolSend>;
pub(crate) type LocalBlindPoolInnerMap = BTreeMap<LayoutKey, RawOpaquePool>;
pub(crate) type RawBlindPoolInnerMap = BTreeMap<LayoutKey, RawOpaquePool>;
