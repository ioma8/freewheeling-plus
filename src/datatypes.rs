/*
   Copyright 2004-2011 Jan Pekau

   This file is part of Freewheeling.

   Freewheeling is free software: you can redistribute it and/or modify
   it under the terms of the GNU General Public License as published by
   the Free Software Foundation, either version 2 of the License, or
   (at your option) any later version.

   Freewheeling is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
   GNU General Public License for more details.

   You should have received a copy of the GNU General Public License
   along with Freewheeling.  If not, see <http://www.gnu.org/licenses/>.
*/

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Maximum number of reader and writer threads
pub const MAX_RW_THREADS: usize = 50;

/// System-wide total number of RT data structures allowed
pub const MAX_RT_STRUCTS: usize = 20;

/// Number of data bytes in one config variable
pub const CFG_VAR_SIZE: usize = 16;

/// Callback interface used by realtime data structures when the registered
/// reader/writer-thread count changes.  This mirrors the C++ abstract class;
/// implementations decide how to resize or otherwise update themselves.
pub trait RtDataStructUpdater {
    fn update_num_rw_threads(&mut self, new_num_writers: i32);
}

#[cfg(test)]
mod rt_data_struct_updater_tests {
    use super::RtDataStructUpdater;

    struct Probe(i32);
    impl RtDataStructUpdater for Probe {
        fn update_num_rw_threads(&mut self, n: i32) {
            self.0 = n;
        }
    }

    #[test]
    fn forwards_thread_count_to_implementation() {
        let mut probe = Probe(0);
        probe.update_num_rw_threads(7);
        assert_eq!(probe.0, 7);
    }
}

// ============================================================
// CoreDataType
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoreDataType {
    Char,
    Int,
    Long,
    Float,
    Range,
    Variable,
    VariableRef,
    Invalid,
}

impl CoreDataType {
    pub fn from_name(name: &str) -> Self {
        match name {
            "char" => CoreDataType::Char,
            "int" => CoreDataType::Int,
            "long" => CoreDataType::Long,
            "float" => CoreDataType::Float,
            "range" => CoreDataType::Range,
            _ => CoreDataType::Invalid,
        }
    }
}


// ============================================================
// Range
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub lo: i32,
    pub hi: i32,
}

impl Range {
    pub fn new(lo: i32, hi: i32) -> Self {
        Range { lo, hi }
    }
}

// ============================================================
// UserVariable
// ============================================================

#[derive(Clone, PartialEq)]
pub struct UserVariable {
    pub name: Option<String>,
    pub type_: CoreDataType,
    pub data: [u8; CFG_VAR_SIZE],
    pub is_system: bool,
    next: Option<Box<UserVariable>>,
}

impl UserVariable {
    pub fn new() -> Self {
        UserVariable {
            name: None,
            type_: CoreDataType::Invalid,
            data: [0u8; CFG_VAR_SIZE],
            is_system: false,
            next: None,
        }
    }

    pub fn with_name(name: &str, type_: CoreDataType) -> Self {
        let mut v = UserVariable::new();
        v.name = Some(name.to_string());
        v.type_ = type_;
        v
    }

    pub fn raise_precision(&mut self, src: &UserVariable) {
        let new_type = match (self.type_, src.type_) {
            (CoreDataType::Char, CoreDataType::Int)
            | (CoreDataType::Char, CoreDataType::Long)
            | (CoreDataType::Char, CoreDataType::Float) => src.type_,
            (CoreDataType::Int, CoreDataType::Long) | (CoreDataType::Int, CoreDataType::Float) => {
                src.type_
            }
            (CoreDataType::Long, CoreDataType::Float) => CoreDataType::Float,
            _ => return,
        };
        if new_type != self.type_ {
            let old_val = self.as_i64();
            self.type_ = new_type;
            self.set_from_i64(old_val);
        }
    }

    pub fn as_i64(&self) -> i64 {
        match self.type_ {
            // The original configuration ABI stores a C++ `char`, which is
            // signed on the supported macOS target.
            CoreDataType::Char => (self.data[0] as i8) as i64,
            CoreDataType::Int => i32::from_ne_bytes(self.data[..4].try_into().unwrap()) as i64,
            CoreDataType::Long => i64::from_ne_bytes(self.data[..8].try_into().unwrap()),
            CoreDataType::Float => f32::from_ne_bytes(self.data[..4].try_into().unwrap()) as i64,
            _ => 0,
        }
    }

    fn set_from_i64(&mut self, val: i64) {
        match self.type_ {
            CoreDataType::Char => self.data[0] = (val as i8) as u8,
            CoreDataType::Int => {
                self.data[..4].copy_from_slice(&(val as i32).to_ne_bytes());
            }
            CoreDataType::Long => {
                self.data[..8].copy_from_slice(&val.to_ne_bytes());
            }
            CoreDataType::Float => {
                self.data[..4].copy_from_slice(&(val as f32).to_ne_bytes());
            }
            _ => {}
        }
    }

    pub fn set_from(&mut self, src: &UserVariable) {
        match self.type_ {
            CoreDataType::Char => self.set_char(src.as_char()),
            CoreDataType::Int => self.set_int(src.as_i32()),
            CoreDataType::Long => self.set_long(src.as_i64()),
            CoreDataType::Float => self.set_float(src.as_f32()),
            CoreDataType::Range => {
                let r = src.as_range();
                self.set_range(r.lo, r.hi);
            }
            _ => {
                eprintln!("UserVariable: WARNING: Can't set from invalid variable!");
            }
        }
    }

    pub fn as_char(&self) -> i8 {
        match self.type_ {
            CoreDataType::Char => self.data[0] as i8,
            CoreDataType::Int => i32::from_ne_bytes(self.data[..4].try_into().unwrap()) as i8,
            CoreDataType::Long => i64::from_ne_bytes(self.data[..8].try_into().unwrap()) as i8,
            CoreDataType::Float => f32::from_ne_bytes(self.data[..4].try_into().unwrap()) as i8,
            _ => 0,
        }
    }

    pub fn as_i32(&self) -> i32 {
        match self.type_ {
            CoreDataType::Char => (self.data[0] as i8) as i32,
            CoreDataType::Int => i32::from_ne_bytes(self.data[..4].try_into().unwrap()),
            CoreDataType::Long => i64::from_ne_bytes(self.data[..8].try_into().unwrap()) as i32,
            CoreDataType::Float => f32::from_ne_bytes(self.data[..4].try_into().unwrap()) as i32,
            _ => 0,
        }
    }

    pub fn as_f32(&self) -> f32 {
        match self.type_ {
            CoreDataType::Char => (self.data[0] as i8) as f32,
            CoreDataType::Int => i32::from_ne_bytes(self.data[..4].try_into().unwrap()) as f32,
            CoreDataType::Long => i64::from_ne_bytes(self.data[..8].try_into().unwrap()) as f32,
            CoreDataType::Float => f32::from_ne_bytes(self.data[..4].try_into().unwrap()),
            _ => 0.0,
        }
    }

    pub fn as_range(&self) -> Range {
        match self.type_ {
            CoreDataType::Range => Range::new(
                i32::from_ne_bytes(self.data[..4].try_into().unwrap()),
                i32::from_ne_bytes(self.data[4..8].try_into().unwrap()),
            ),
            _ => {
                let v = self.as_i32();
                Range::new(v, v)
            }
        }
    }

    pub fn set_char(&mut self, val: i8) {
        self.type_ = CoreDataType::Char;
        self.data[0] = val as u8;
    }

    pub fn set_int(&mut self, val: i32) {
        self.type_ = CoreDataType::Int;
        self.data[..4].copy_from_slice(&val.to_ne_bytes());
    }

    pub fn set_long(&mut self, val: i64) {
        self.type_ = CoreDataType::Long;
        self.data[..8].copy_from_slice(&val.to_ne_bytes());
    }

    pub fn set_float(&mut self, val: f32) {
        self.type_ = CoreDataType::Float;
        self.data[..4].copy_from_slice(&val.to_ne_bytes());
    }

    pub fn set_range(&mut self, lo: i32, hi: i32) {
        self.type_ = CoreDataType::Range;
        self.data[..4].copy_from_slice(&lo.to_ne_bytes());
        self.data[4..8].copy_from_slice(&hi.to_ne_bytes());
    }

    pub fn get_type(&self) -> CoreDataType {
        self.type_
    }
    pub fn is_system_variable(&self) -> bool {
        self.is_system
    }
    pub fn get_value(&self) -> &[u8] {
        &self.data
    }
    pub fn get_name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub fn set_next(&mut self, next: UserVariable) {
        self.next = Some(Box::new(next));
    }
    pub fn has_next(&self) -> bool {
        self.next.is_some()
    }
    pub fn take_next(&mut self) -> Option<Box<UserVariable>> {
        self.next.take()
    }

    pub fn print(&self) -> String {
        match self.type_ {
            CoreDataType::Char => format!("{}", self.as_char()),
            CoreDataType::Int => format!("{}", self.as_i32()),
            CoreDataType::Long => format!("{}", self.as_i64()),
            CoreDataType::Float => format!("{:.2}", self.as_f32()),
            CoreDataType::Range => {
                let r = self.as_range();
                format!("{}>{}", r.lo, r.hi)
            }
            CoreDataType::Variable => "(variable)".to_string(),
            CoreDataType::VariableRef => "(variableref)".to_string(),
            CoreDataType::Invalid => "(invalid)".to_string(),
        }
    }

    #[allow(dead_code)]
    pub fn get_delta(&self, arg: &UserVariable) -> UserVariable {
        let mut ret = UserVariable::new();
        ret.type_ = CoreDataType::Char;
        ret.raise_precision(self);
        ret.raise_precision(arg);
        match ret.type_ {
            CoreDataType::Char => {
                ret.set_char((arg.as_char() as i16 - self.as_char() as i16).unsigned_abs() as i8)
            }
            CoreDataType::Int => ret.set_int((arg.as_i32() - self.as_i32()).unsigned_abs() as i32),
            CoreDataType::Long => {
                ret.set_long((arg.as_i64() - self.as_i64()).unsigned_abs() as i64)
            }
            CoreDataType::Float => ret.set_float((arg.as_f32() - self.as_f32()).abs()),
            _ => eprintln!("UserVariable: WARNING: GetDelta() doesn't work on this type!"),
        }
        ret
    }

    /// Implements the C++ `+=`, `-=`, `*=`, and `/=` semantics without
    /// exposing a byte-backed value to callers.
    pub fn add_assign(&mut self, src: &UserVariable) {
        self.apply_arithmetic(src, |a, b| a + b);
    }
    pub fn sub_assign(&mut self, src: &UserVariable) {
        self.apply_arithmetic(src, |a, b| a - b);
    }
    pub fn mul_assign(&mut self, src: &UserVariable) {
        self.apply_arithmetic(src, |a, b| a * b);
    }
    pub fn div_assign(&mut self, src: &UserVariable) {
        if self.type_ == CoreDataType::Range {
            let r = src.as_range();
            let mut own = self.as_range();
            if r.lo != 0 {
                own.lo /= r.lo;
            }
            if r.hi != 0 {
                own.hi /= r.hi;
            }
            self.set_range(own.lo, own.hi);
        } else if src.as_f32() != 0.0 {
            self.set_float(self.as_f32() / src.as_f32());
        }
    }

    fn apply_arithmetic(&mut self, src: &UserVariable, op: impl Fn(f64, f64) -> f64) {
        self.raise_precision(src);
        match self.type_ {
            CoreDataType::Char => {
                self.set_char(op(self.as_char() as f64, src.as_char() as f64) as i8)
            }
            CoreDataType::Int => self.set_int(op(self.as_i32() as f64, src.as_i32() as f64) as i32),
            CoreDataType::Long => {
                self.set_long(op(self.as_i64() as f64, src.as_i64() as f64) as i64)
            }
            CoreDataType::Float => {
                self.set_float(op(self.as_f32() as f64, src.as_f32() as f64) as f32)
            }
            CoreDataType::Range => {
                let a = self.as_range();
                let b = src.as_range();
                self.set_range(
                    op(a.lo as f64, b.lo as f64) as i32,
                    op(a.hi as f64, b.hi as f64) as i32,
                );
            }
            _ => {}
        }
    }
}

impl Default for UserVariable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod user_variable_tests {
    use super::*;

    #[test]
    fn core_data_type_discriminants_match_cpp_storage_abi() {
        assert_eq!(CoreDataType::Char as u8, 0);
        assert_eq!(CoreDataType::Invalid as u8, 7);
    }

    #[test]
    fn char_is_signed_and_precision_promotion_preserves_its_value() {
        let mut value = UserVariable::new();
        value.set_char(-1);
        assert_eq!(value.as_char(), -1);
        assert_eq!(value.as_i32(), -1);

        let mut integer = UserVariable::new();
        integer.set_int(4);
        value.raise_precision(&integer);
        assert_eq!(value.get_type(), CoreDataType::Int);
        assert_eq!(value.as_i32(), -1);
    }

    #[test]
    fn print_uses_cpp_float_and_range_syntax() {
        let mut value = UserVariable::new();
        value.set_float(1.239);
        assert_eq!(value.print(), "1.24");
        value.set_range(-2, 9);
        assert_eq!(value.print(), "-2>9");
    }
}

impl fmt::Debug for UserVariable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UserVariable({:?}, val={})", self.type_, self.print())
    }
}

// ============================================================
// RTRWThreads
// ============================================================

static NUM_RW_THREADS: AtomicUsize = AtomicUsize::new(0);
static THREAD_IDS: OnceLock<Mutex<Vec<std::thread::ThreadId>>> = OnceLock::new();

fn get_thread_ids() -> &'static Mutex<Vec<std::thread::ThreadId>> {
    THREAD_IDS.get_or_init(|| Mutex::new(Vec::with_capacity(MAX_RW_THREADS)))
}

pub struct RTRWThreads;

impl RTRWThreads {
    pub fn init_all() {
        NUM_RW_THREADS.store(0, Ordering::SeqCst);
        if let Ok(mut ids) = get_thread_ids().lock() {
            ids.clear();
        }
    }

    pub fn register_reader_or_writer() -> usize {
        let id = std::thread::current().id();
        let mut ids = get_thread_ids().lock().unwrap();
        let idx = ids.len();
        assert!(
            idx < MAX_RW_THREADS,
            "Too many writer threads for Ring Buffer!"
        );
        ids.push(id);
        let count = idx + 1;
        NUM_RW_THREADS.store(count, Ordering::Release);
        count
    }

    pub fn get_num_threads() -> usize {
        NUM_RW_THREADS.load(Ordering::Acquire)
    }
    pub fn get_thread_ids() -> Vec<std::thread::ThreadId> {
        get_thread_ids().lock().unwrap().clone()
    }
    pub fn close_all() {
        NUM_RW_THREADS.store(0, Ordering::SeqCst);
        if let Ok(mut ids) = get_thread_ids().lock() {
            ids.clear();
        }
    }
}

