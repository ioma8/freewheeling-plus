extern crate libcc2rs;
use libcc2rs::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::prelude::*;
use std::io::{Read, Seek, Write};
use std::os::fd::AsFd;
use std::rc::{Rc, Weak};
pub fn math_gcd_0(a: i32, b: i32) -> i32 {
    let a: Value<i32> = Rc::new(RefCell::new(a));
    let b: Value<i32> = Rc::new(RefCell::new(b));
    return (if ((*b.borrow()) == 0) {
        (*a.borrow())
    } else {
        ({
            let _a: i32 = (*b.borrow());
            let _b: i32 = ((*a.borrow()) % (*b.borrow()));
            math_gcd_0(_a, _b)
        })
    });
}
pub fn math_lcm_1(a: i32, b: i32) -> i32 {
    let a: Value<i32> = Rc::new(RefCell::new(a));
    let b: Value<i32> = Rc::new(RefCell::new(b));
    return (((*a.borrow()) * (*b.borrow())) / ({ math_gcd_0((*a.borrow()), (*b.borrow())) }));
}
pub fn iec_dB_to_fader_2(db: f32) -> f32 {
    let db: Value<f32> = Rc::new(RefCell::new(db));
    let def: Value<f32> = Rc::new(RefCell::new(0.0E+0));
    if ((*db.borrow()) < -7.0E+1) {
        (*def.borrow_mut()) = 0.0E+0;
    } else if ((*db.borrow()) < -6.0E+1) {
        (*def.borrow_mut()) = (((*db.borrow()) + 7.0E+1) * 2.5E-1);
    } else if ((*db.borrow()) < -5.0E+1) {
        (*def.borrow_mut()) = ((((*db.borrow()) + 6.0E+1) * 5.0E-1) + 2.5E+0);
    } else if ((*db.borrow()) < -4.0E+1) {
        (*def.borrow_mut()) = ((((*db.borrow()) + 5.0E+1) * 7.5E-1) + 7.5E+0);
    } else if ((*db.borrow()) < -3.0E+1) {
        (*def.borrow_mut()) = ((((*db.borrow()) + 4.0E+1) * 1.5E+0) + 1.5E+1);
    } else if ((*db.borrow()) < -2.0E+1) {
        (*def.borrow_mut()) = ((((*db.borrow()) + 3.0E+1) * 2.0E+0) + 3.0E+1);
    } else {
        (*def.borrow_mut()) = ((((*db.borrow()) + 2.0E+1) * 2.5E+0) + 5.0E+1);
    }
    return (*def.borrow());
}
pub fn iec_fader_to_dB_3(def: f32) -> f32 {
    let def: Value<f32> = Rc::new(RefCell::new(def));
    let db: Value<f32> = Rc::new(RefCell::new(0.0E+0));
    if ((*def.borrow()) >= 5.0E+1) {
        (*db.borrow_mut()) = ((((*def.borrow()) - 5.0E+1) / 2.5E+0) - 2.0E+1);
    } else if ((*def.borrow()) >= 3.0E+1) {
        (*db.borrow_mut()) = ((((*def.borrow()) - 3.0E+1) / 2.0E+0) - 3.0E+1);
    } else if ((*def.borrow()) >= 1.5E+1) {
        (*db.borrow_mut()) = ((((*def.borrow()) - 1.5E+1) / 1.5E+0) - 4.0E+1);
    } else if ((*def.borrow()) >= 7.5E+0) {
        (*db.borrow_mut()) = ((((*def.borrow()) - 7.5E+0) / 7.5E-1) - 5.0E+1);
    } else if ((*def.borrow()) >= 2.5E+0) {
        (*db.borrow_mut()) = ((((*def.borrow()) - 2.5E+0) / 5.0E-1) - 6.0E+1);
    } else {
        (*db.borrow_mut()) = (((*def.borrow()) / 2.5E-1) - 7.0E+1);
    }
    return (*db.borrow());
}
