extern crate libcc2rs;
use libcc2rs::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::prelude::*;
use std::io::{Read, Seek, Write};
use std::os::fd::AsFd;
use std::rc::{Rc, Weak};
thread_local!(
    pub static MAX_VOL_0: Value<f32> = Rc::new(RefCell::new(5.0E+0));
);
thread_local!(
    pub static MAX_DVOL_1: Value<f32> = Rc::new(RefCell::new(1.5E+0));
);
