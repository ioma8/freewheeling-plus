//! ALSA mixer control interface.
//!
//! The small backend boundary is intentional: the control protocol is useful
//! without an ALSA device (and is consequently straightforward to test), while
//! `AlsaMixerBackend` remains the production implementation.

use std::process::Command;

/// Production backend.  `amixer` is ALSA's supported command-line interface;
/// each backend instance owns the selected card, just like the old cset handle.
#[derive(Default)]
pub struct AlsaMixerBackend {
    card: Option<String>,
}

impl AlsaMixerBackend {
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
pub struct HardwareMixerInterface {
    backend: AlsaMixerBackend,
    prev_hwid: Option<i32>,
}

impl HardwareMixerInterface {
    pub fn new(backend: AlsaMixerBackend) -> Self {
        Self {
            backend,
            prev_hwid: None,
        }
    }

    pub fn backend(&self) -> &AlsaMixerBackend {
        &self.backend
    }
    pub fn backend_mut(&mut self) -> &mut AlsaMixerBackend {
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

impl Drop for HardwareMixerInterface {
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
    #[test]
    fn validates_and_maps() {
        let mut m = HardwareMixerInterface::new(AlsaMixerBackend::default());
        assert!(m.alsa_mixer_control_set(0, -1, 1, -1, -1, -1).is_err());
        assert!(m.alsa_mixer_control_set(0, 1, -1, -1, -1, -1).is_err());
        assert_eq!(percent_to_value(50.0, 0, 101), 51);
    }
}
