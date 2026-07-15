extern crate libcc2rs;
use libcc2rs::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::prelude::*;
use std::io::{Read, Seek, Write};
use std::os::fd::AsFd;
use std::rc::{Rc, Weak};
pub fn GetCoreDataType_0(name: Ptr<u8>) -> i32 {
    let name: Value<Ptr<u8>> = Rc::new(RefCell::new(name));
}
