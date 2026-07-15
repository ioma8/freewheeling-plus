//! ALSA mixer control interface.
//!
//! The small backend boundary is intentional: the control protocol is useful
//! without an ALSA device (and is consequently straightforward to test), while
//! `AlsaMixerBackend` remains the production implementation.

use std::process::Command;

/// Operations needed by [`HardwareMixerInterface`].
pub trait MixerBackend {
    fn open(&mut self, card: &str) -> Result<(), String>;
    fn set_control(&mut self, numid: i32, values: &[i32]) -> Result<(), String>;
    fn close(&mut self);
}

/// Production backend.  `amixer` is ALSA's supported command-line interface;
/// each backend instance owns the selected card, just like the old cset handle.
#[derive(Default)]
pub struct AlsaMixerBackend {
    card: Option<String>,
}

impl MixerBackend for AlsaMixerBackend {
    fn open(&mut self, card: &str) -> Result<(), String> {
        self.card = Some(card.to_owned());
        Ok(())
    }

    fn set_control(&mut self, numid: i32, values: &[i32]) -> Result<(), String> {
        let card = self.card.as_deref().ok_or("ALSA mixer is not open")?;
        let value = values
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let status = Command::new("amixer")
            .args(["-D", card, "cset", &format!("numid={numid}"), &value])
            .status()
            .map_err(|e| format!("cannot run amixer: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("amixer exited with {status}"))
        }
    }

    fn close(&mut self) {
        self.card = None;
    }
}

/// Direct replacement for the C++ `HardwareMixerInterface`.
pub struct HardwareMixerInterface<B: MixerBackend = AlsaMixerBackend> {
    backend: B,
    prev_hwid: Option<i32>,
}

impl<B: MixerBackend> HardwareMixerInterface<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            prev_hwid: None,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Set one to four ALSA values, retaining the old card-reuse optimization.
    pub fn alsa_mixer_control_set(
        &mut self,
        hwid: i32,
        numid: i32,
        val1: i32,
        val2: i32,
        val3: i32,
        val4: i32,
    ) -> Result<(), String> {
        if numid < 0 {
            return Err("invalid ALSA mixer setting: no numid".into());
        }
        let raw = [val1, val2, val3, val4];
        // C++ chooses the emitted arity from the last non--1 argument.  It
        // does not reject an intermediate -1 (`val1=-1,val2=7` becomes
        // "-1,7"), leaving validation to ALSA's control type parser.
        let count = raw
            .iter()
            .rposition(|&value| value != -1)
            .map_or(0, |index| index + 1);
        if count == 0 {
            return Err("invalid ALSA mixer setting: no control values".into());
        }
        if self.prev_hwid != Some(hwid) {
            self.backend.close();
            self.prev_hwid = None;
            self.backend.open(&format!("hw:{hwid}"))?;
            self.prev_hwid = Some(hwid);
        }
        self.backend.set_control(numid, &raw[..count])
    }

    pub fn close(&mut self) {
        self.backend.close();
        self.prev_hwid = None;
    }
}

impl<B: MixerBackend> Drop for HardwareMixerInterface<B> {
    fn drop(&mut self) {
        self.close();
    }
}

/// ALSA/amixer's 0--100 percentage mapping, rounded upward like `amixer cset`.
pub fn percent_to_value(percent: f64, min: i64, max: i64) -> i64 {
    let p = percent.clamp(0.0, 100.0);
    (p * (max - min) as f64 * 0.01 + min as f64).ceil() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        opened: Vec<String>,
        writes: Vec<(i32, Vec<i32>)>,
        closes: usize,
    }
    impl MixerBackend for Fake {
        fn open(&mut self, c: &str) -> Result<(), String> {
            self.opened.push(c.into());
            Ok(())
        }
        fn set_control(&mut self, n: i32, v: &[i32]) -> Result<(), String> {
            self.writes.push((n, v.into()));
            Ok(())
        }
        fn close(&mut self) {
            self.closes += 1;
        }
    }
    #[test]
    fn preserves_values_and_reuses_card() {
        let mut m = HardwareMixerInterface::new(Fake::default());
        m.alsa_mixer_control_set(2, 5, 1, 2, -1, -1).unwrap();
        m.alsa_mixer_control_set(2, 6, 3, -1, -1, -1).unwrap();
        assert_eq!(m.backend().opened, vec!["hw:2"]);
        assert_eq!(m.backend().writes, vec![(5, vec![1, 2]), (6, vec![3])]);
    }
    #[test]
    fn validates_and_maps() {
        let mut m = HardwareMixerInterface::new(Fake::default());
        assert!(m.alsa_mixer_control_set(0, -1, 1, -1, -1, -1).is_err());
        m.alsa_mixer_control_set(0, 1, -1, 2, -1, -1).unwrap();
        assert_eq!(m.backend().writes, vec![(1, vec![-1, 2])]);
        assert_eq!(percent_to_value(50.0, 0, 101), 51);
    }
}
