//! Parameter-set banks and the state transitions used by the paramset display.
//!
//! This is the Rust counterpart of `ParamSetParam`, `ParamSetBank`, and the
//! non-visual behaviour of `FloDisplayParamSet`.  Event dispatch and drawing
//! belong to the application/rendering layers, which are not present in this
//! crate yet.

use crate::datatypes::{CoreDataType, UserVariable};

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSetParam {
    pub name: Option<String>,
    pub value: f32,
}

impl Default for ParamSetParam {
    fn default() -> Self {
        Self {
            name: None,
            value: 0.0,
        }
    }
}

impl ParamSetParam {
    pub fn new(name: Option<&str>, value: f32) -> Self {
        Self {
            name: name.map(str::to_owned),
            value,
        }
    }

    pub fn set_name(&mut self, name: Option<&str>) {
        self.name = name.map(str::to_owned);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSetBank {
    pub name: Option<String>,
    pub numparams: usize,
    pub firstparamidx: usize,
    pub maxvalue: f32,
    pub params: Vec<ParamSetParam>,
}

impl Default for ParamSetBank {
    fn default() -> Self {
        Self {
            name: None,
            numparams: 0,
            firstparamidx: 0,
            maxvalue: 1.0,
            params: Vec::new(),
        }
    }
}

impl ParamSetBank {
    pub fn new(name: Option<&str>, numparams: usize, maxvalue: f32) -> Self {
        let mut bank = Self::default();
        bank.setup(name, numparams, maxvalue);
        bank
    }

    pub fn setup(&mut self, name: Option<&str>, numparams: usize, maxvalue: f32) {
        self.name = name.map(str::to_owned);
        self.numparams = numparams;
        self.firstparamidx = 0;
        self.maxvalue = maxvalue;
        self.params = vec![ParamSetParam::default(); numparams];
    }

    pub fn set_name(&mut self, name: Option<&str>) {
        self.name = name.map(str::to_owned);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FloDisplayParamSet {
    pub name: String,
    pub iid: i32,
    pub id: i32,
    pub numactiveparams: usize,
    pub activeparam: Vec<UserVariable>,
    pub invalidparam: f32,
    pub sx: i32,
    pub sy: i32,
    pub numbanks: usize,
    pub curbank: usize,
    pub banks: Vec<ParamSetBank>,
}

impl FloDisplayParamSet {
    pub fn new(
        name: &str,
        iid: i32,
        id: i32,
        numactiveparams: usize,
        numbanks: usize,
        sx: i32,
        sy: i32,
    ) -> Self {
        Self {
            name: name.to_owned(),
            iid,
            id,
            numactiveparams,
            activeparam: vec![UserVariable::new(); numactiveparams],
            invalidparam: 0.0,
            sx,
            sy,
            numbanks,
            curbank: 0,
            banks: vec![ParamSetBank::default(); numbanks],
        }
    }

    pub fn current_bank(&self) -> Option<&ParamSetBank> {
        self.banks.get(self.curbank)
    }
    pub fn current_bank_mut(&mut self) -> Option<&mut ParamSetBank> {
        self.banks.get_mut(self.curbank)
    }

    pub fn show_bank(&mut self, delta: isize) {
        if self.numbanks == 0 {
            return;
        }
        self.curbank =
            (self.curbank as isize + delta).clamp(0, self.numbanks as isize - 1) as usize;
    }

    pub fn show_page(&mut self, page: isize) {
        let page_size = self.numactiveparams;
        let Some(bank) = self.current_bank_mut() else {
            return;
        };
        let proposed = bank.firstparamidx as isize + page_size as isize * page;
        bank.firstparamidx = if proposed < 0 {
            0
        } else if proposed >= bank.numparams as isize {
            bank.firstparamidx
        } else {
            proposed as usize
        };
    }

    pub fn absolute_param_index(&self, relative: isize) -> Option<usize> {
        let bank = self.current_bank()?;
        if bank.numparams == 0 {
            return None;
        }
        Some(
            (bank.firstparamidx as isize + relative).clamp(0, bank.numparams as isize - 1) as usize,
        )
    }

    pub fn set_param(&mut self, relative: isize, value: f32) -> bool {
        // `T_EV_ParamSetSetParam` does not use the clamped absolute-index
        // helper.  C++ rejects a relative index before the current page or
        // after the bank instead of silently writing the nearest parameter.
        let Some(bank) = self.current_bank() else {
            return false;
        };
        let idx = bank.firstparamidx as isize + relative;
        if idx < 0 || idx >= bank.numparams as isize {
            return false;
        }
        let idx = idx as usize;
        self.banks[self.curbank].params[idx].value = value;
        true
    }

    pub fn get_param(&self, relative: isize) -> f32 {
        // Same direct bounds check as `T_EV_ParamSetGetParam`; only
        // ParamSetGetAbsoluteParamIdxEvent clamps an index.
        let Some(bank) = self.current_bank() else {
            return 0.0;
        };
        let idx = bank.firstparamidx as isize + relative;
        if idx < 0 || idx >= bank.numparams as isize {
            0.0
        } else {
            bank.params[idx as usize].value
        }
    }

    pub fn link_active_params(&mut self) {
        for i in 0..self.numactiveparams {
            let value = self
                .current_bank()
                .and_then(|b| b.params.get(b.firstparamidx + i))
                .map_or(self.invalidparam, |p| p.value);
            self.activeparam[i].set_float(value);
        }
    }

    pub fn set_active_param(&mut self, index: usize, variable: UserVariable) -> bool {
        if index >= self.activeparam.len()
            || variable.get_type() != CoreDataType::Float
            || variable.is_system_variable()
        {
            return false;
        }
        self.activeparam[index] = variable;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_and_value_updates_match_relative_indices() {
        let mut d = FloDisplayParamSet::new("x", 1, 2, 2, 2, 10, 20);
        d.banks[0].setup(Some("a"), 4, 1.0);
        d.banks[1].setup(Some("b"), 3, 2.0);
        assert!(d.set_param(1, 4.5));
        assert_eq!(d.get_param(1), 4.5);
        d.show_page(1);
        assert_eq!(d.absolute_param_index(0), Some(2));
        d.show_bank(1);
        assert_eq!(d.current_bank().unwrap().name.as_deref(), Some("b"));
    }

    #[test]
    fn invalid_or_out_of_range_values_are_safe() {
        let mut d = FloDisplayParamSet::new("x", 0, 0, 2, 1, 0, 0);
        assert_eq!(d.get_param(0), 0.0);
        assert!(!d.set_param(0, 1.0));
        d.banks[0].setup(None, 1, 1.0);
        assert_eq!(d.absolute_param_index(-10), Some(0));
        d.banks[0].params[0].value = 4.0;
        assert_eq!(d.get_param(-10), 0.0);
        assert!(!d.set_param(-10, 8.0));
        assert_eq!(d.banks[0].params[0].value, 4.0);
    }
}
