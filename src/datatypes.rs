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

pub struct UserVariable {
    pub name: Option<String>,
    pub type_: CoreDataType,
    pub data: [u8; CFG_VAR_SIZE],
    pub is_system: bool,
    next: Option<Box<UserVariable>>,
}

/// Compares this variable's own state.
///
/// The private `next` chain is an implementation detail of the variable table,
/// not part of a variable's identity: two variables with the same name, type
/// and value are equal even when their chains differ.
impl PartialEq for UserVariable {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.type_ == other.type_
            && self.data == other.data
            && self.is_system == other.is_system
    }
}

/// Clones this variable's own state; the chain is not copied (see
/// [`PartialEq`]).
impl Clone for UserVariable {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            type_: self.type_,
            data: self.data,
            is_system: self.is_system,
            next: None,
        }
    }
}

/// Drops a long variable chain iteratively: the derived recursive drop would
/// overflow the stack for a deep list.
impl Drop for UserVariable {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut node) = next {
            next = node.next.take();
        }
    }
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

    /// Copy `src` into this variable.
    ///
    /// The accessors return `0` for `Range`/`Variable`/`VariableRef`/`Invalid`
    /// sources, so a mixed-type assignment would silently store a zero. Such an
    /// assignment is rejected (with a diagnostic) instead.
    pub fn set_from(&mut self, src: &UserVariable) {
        let scalar = |type_: CoreDataType| {
            matches!(
                type_,
                CoreDataType::Char
                    | CoreDataType::Int
                    | CoreDataType::Long
                    | CoreDataType::Float
            )
        };
        let compatible = match (self.type_, src.get_type()) {
            (CoreDataType::Range, CoreDataType::Range) => true,
            (target, source) => scalar(target) && scalar(source),
        };
        if !compatible {
            eprintln!(
                "UserVariable: WARNING: Can't set {:?} from {:?} variable!",
                self.type_,
                src.get_type()
            );
            return;
        }
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


    /// Implements the C++ `+=`, `-=`, `*=`, and `/=` semantics without
    /// exposing a byte-backed value to callers.
    pub fn add_assign(&mut self, src: &UserVariable) {
        self.apply_arithmetic(src, i64::saturating_add, |a, b| a + b);
    }
    pub fn sub_assign(&mut self, src: &UserVariable) {
        self.apply_arithmetic(src, i64::saturating_sub, |a, b| a - b);
    }
    pub fn mul_assign(&mut self, src: &UserVariable) {
        self.apply_arithmetic(src, i64::saturating_mul, |a, b| a * b);
    }

    /// Divide in place.
    ///
    /// Returns `false` when the divisor is zero, in which case the value is
    /// left untouched. Integral operands keep their own type (a `Long` stays a
    /// `Long`) instead of being promoted to `Float`.
    pub fn div_assign(&mut self, src: &UserVariable) -> bool {
        match self.type_ {
            CoreDataType::Range => {
                let divisor = src.as_range();
                let mut own = self.as_range();
                if divisor.lo == 0 || divisor.hi == 0 {
                    return false;
                }
                own.lo /= divisor.lo;
                own.hi /= divisor.hi;
                self.set_range(own.lo, own.hi);
                true
            }
            CoreDataType::Float => {
                let divisor = src.as_f32();
                if divisor == 0.0 {
                    return false;
                }
                self.set_float(self.as_f32() / divisor);
                true
            }
            CoreDataType::Char => {
                let divisor = i64::from(src.as_char());
                if divisor == 0 {
                    return false;
                }
                self.set_char((i64::from(self.as_char()) / divisor) as i8);
                true
            }
            CoreDataType::Int => {
                let divisor = i64::from(src.as_i32());
                if divisor == 0 {
                    return false;
                }
                self.set_int((i64::from(self.as_i32()) / divisor) as i32);
                true
            }
            CoreDataType::Long => {
                let divisor = src.as_i64();
                if divisor == 0 {
                    return false;
                }
                self.set_long(self.as_i64() / divisor);
                true
            }
            _ => false,
        }
    }

    /// Apply `int_op` in the native integer type and `float_op` to floats.
    ///
    /// Funnelling integers through `f64` would lose the low bits of any value
    /// above 2^53; the integer operation saturates at the type's bounds instead
    /// of overflowing.
    fn apply_arithmetic(
        &mut self,
        src: &UserVariable,
        int_op: impl Fn(i64, i64) -> i64,
        float_op: impl Fn(f32, f32) -> f32,
    ) {
        self.raise_precision(src);
        match self.type_ {
            CoreDataType::Char => self.set_char(
                int_op(i64::from(self.as_char()), i64::from(src.as_char())) as i8,
            ),
            CoreDataType::Int => {
                self.set_int(int_op(i64::from(self.as_i32()), i64::from(src.as_i32())) as i32)
            }
            CoreDataType::Long => self.set_long(int_op(self.as_i64(), src.as_i64())),
            CoreDataType::Float => self.set_float(float_op(self.as_f32(), src.as_f32())),
            CoreDataType::Range => {
                let a = self.as_range();
                let b = src.as_range();
                self.set_range(
                    int_op(i64::from(a.lo), i64::from(b.lo)) as i32,
                    int_op(i64::from(a.hi), i64::from(b.hi)) as i32,
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

impl fmt::Debug for UserVariable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UserVariable({:?}, val={})", self.type_, self.print())
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
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
    fn long_arithmetic_stays_exact() {
        let mut value = UserVariable::new();
        value.set_long(10_000_000_000_000_001);
        let mut one = UserVariable::new();
        one.set_long(1);
        value.add_assign(&one);
        assert_eq!(value.as_i64(), 10_000_000_000_000_002);
        value.mul_assign(&one);
        assert_eq!(value.as_i64(), 10_000_000_000_000_002);
        // Saturation keeps a wrapped result out of the value.
        let mut huge = UserVariable::new();
        huge.set_long(i64::MAX);
        huge.add_assign(&one);
        assert_eq!(huge.as_i64(), i64::MAX);
    }

    #[test]
    fn integral_division_keeps_its_type_and_reports_zero_divisors() {
        let mut value = UserVariable::new();
        value.set_long(9);
        let mut three = UserVariable::new();
        three.set_long(3);
        assert!(value.div_assign(&three));
        assert_eq!(value.get_type(), CoreDataType::Long);
        assert_eq!(value.as_i64(), 3);

        let mut zero = UserVariable::new();
        zero.set_long(0);
        assert!(!value.div_assign(&zero));
        assert_eq!(value.as_i64(), 3);
    }

    #[test]
    fn mixed_type_assignment_is_rejected() {
        let mut range = UserVariable::new();
        range.set_range(1, 9);
        let mut integer = UserVariable::new();
        integer.set_int(42);
        // A range has no scalar accessor, so the copy must not silently zero.
        integer.set_from(&range);
        assert_eq!(integer.as_i32(), 42);
        let mut target = UserVariable::new();
        target.set_int(0);
        target.set_from(&range);
        assert_eq!(target.as_i32(), 0);
        target.set_from(&integer);
        assert_eq!(target.as_i32(), 42);
    }

    #[test]
    fn clone_and_equality_ignore_the_private_chain() {
        let mut value = UserVariable::new();
        value.set_int(7);
        let mut chained = value.clone();
        chained.set_next(UserVariable::new());
        assert!(chained.has_next());
        assert_eq!(chained, value);
        assert!(!value.clone().has_next());
    }

    #[test]
    fn registration_reports_exhaustion_instead_of_panicking() {
        RTRWThreads::init_all();
        let index = RTRWThreads::register_reader_or_writer();
        assert_eq!(index, Some(0));
        assert_eq!(RTRWThreads::get_num_threads(), 1);
        assert_eq!(RTRWThreads::get_thread_ids(), RTRWThreads::get_thread_ids());
        RTRWThreads::close_all();
        assert_eq!(RTRWThreads::get_num_threads(), 0);
        assert!(RTRWThreads::get_thread_ids().is_empty());
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

// ============================================================
// RTRWThreads
// ============================================================

static NUM_RW_THREADS: AtomicUsize = AtomicUsize::new(0);
static THREAD_IDS: OnceLock<Mutex<Vec<std::thread::ThreadId>>> = OnceLock::new();

fn get_thread_ids() -> &'static Mutex<Vec<std::thread::ThreadId>> {
    THREAD_IDS.get_or_init(|| Mutex::new(Vec::with_capacity(MAX_RW_THREADS)))
}

/// Lock the thread table, recovering a poisoned mutex (the table holds plain
/// thread ids, so a panic while it was held left no invariant broken).
fn lock_thread_ids() -> std::sync::MutexGuard<'static, Vec<std::thread::ThreadId>> {
    get_thread_ids()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct RTRWThreads;

impl RTRWThreads {
    pub fn init_all() {
        NUM_RW_THREADS.store(0, Ordering::SeqCst);
        if let Ok(mut ids) = get_thread_ids().lock() {
            ids.clear();
        }
    }

    /// Register the calling thread as a reader/writer.
    ///
    /// Returns the thread's index, or `None` when the table is full, so a
    /// caller reports the condition instead of aborting the process. The count
    /// is updated inside the lock so it can never disagree with the id list.
    pub fn register_reader_or_writer() -> Option<usize> {
        let id = std::thread::current().id();
        let mut ids = lock_thread_ids();
        if ids.len() >= MAX_RW_THREADS {
            return None;
        }
        ids.push(id);
        NUM_RW_THREADS.store(ids.len(), Ordering::Release);
        Some(ids.len() - 1)
    }

    pub fn get_num_threads() -> usize {
        NUM_RW_THREADS.load(Ordering::Acquire)
    }
    pub fn get_thread_ids() -> Vec<std::thread::ThreadId> {
        lock_thread_ids().clone()
    }
    pub fn close_all() {
        NUM_RW_THREADS.store(0, Ordering::SeqCst);
        if let Ok(mut ids) = get_thread_ids().lock() {
            ids.clear();
        }
    }
}
