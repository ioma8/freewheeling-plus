//! ALSA mixer control interface.
//!
//! The small backend boundary is intentional: the card-cache/argv protocol is
//! useful without an ALSA device (and is consequently straightforward to test
//! through [`MixerBackend`]), while `AlsaMixerBackend` remains the production
//! implementation.

use std::process::Command;

/// The backend boundary used by [`HardwareMixerInterface`].
///
/// Production code uses [`AlsaMixerBackend`]; tests substitute a recording
/// backend so the card-reuse logic and the emitted control values can be
/// observed without touching real hardware.
pub trait MixerBackend {
    /// Select the card subsequent [`MixerBackend::set_control`] calls address.
    fn open(&mut self, card: &str) -> Result<(), String>;
    /// Write one to four control values to `numid`.
    fn set_control(&mut self, numid: i32, values: &[i32]) -> Result<(), String>;
    /// Release the selected card.
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
        let numid = format!("numid={numid}");
        // `--` terminates option scanning: a value list whose first element is
        // negative (`-1,7`) would otherwise be parsed by getopt as an option.
        let output = Command::new("amixer")
            .args(["-D", card, "cset", "--", &numid, &value])
            .output()
            .map_err(|e| format!("cannot run amixer: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            if stderr.is_empty() {
                Err(format!("amixer exited with {}", output.status))
            } else {
                Err(format!("amixer exited with {}: {stderr}", output.status))
            }
        }
    }

    fn close(&mut self) {
        self.card = None;
    }
}

/// Direct replacement for the C++ `HardwareMixerInterface`.
///
/// `prev_hwid` caches the card named by `hwid`; the backend is kept private so
/// nothing can re-target it behind the cache's back.
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
        // "-1,7"), leaving validation to ALSA's control type parser; the
        // backend terminates option scanning so that value list still reaches
        // the parser.
        let count = raw
            .iter()
            .rposition(|&value| value != -1)
            .map_or(0, |index| index + 1);
        if count == 0 {
            return Err("invalid ALSA mixer setting: no control values".into());
        }
        if self.prev_hwid != Some(hwid) {
            // Only a *previously opened* card is closed: closing on first use
            // would tear down a backend the caller had already prepared.
            if self.prev_hwid.take().is_some() {
                self.backend.close();
            }
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
///
/// Non-finite percentages cannot be mapped and yield `min`.
pub fn percent_to_value(percent: f64, min: i64, max: i64) -> i64 {
    if !percent.is_finite() {
        return min;
    }
    // Widen before subtracting: `max - min` overflows i64 for wide ranges.
    let range = (i128::from(max) - i128::from(min)) as f64;
    let p = percent.clamp(0.0, 100.0);
    (p * range * 0.01 + min as f64).ceil() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Calls recorded by [`RecordingBackend`].
    type Writes = Rc<RefCell<Vec<(i32, Vec<i32>)>>>;

    #[derive(Clone, Default)]
    struct Log {
        opened: Rc<RefCell<Vec<String>>>,
        closed: Rc<RefCell<usize>>,
        writes: Writes,
    }

    #[derive(Default)]
    struct RecordingBackend {
        log: Log,
    }

    impl MixerBackend for RecordingBackend {
        fn open(&mut self, card: &str) -> Result<(), String> {
            self.log.opened.borrow_mut().push(card.to_owned());
            Ok(())
        }

        fn set_control(&mut self, numid: i32, values: &[i32]) -> Result<(), String> {
            self.log.writes.borrow_mut().push((numid, values.to_vec()));
            Ok(())
        }

        fn close(&mut self) {
            *self.log.closed.borrow_mut() += 1;
        }
    }

    #[test]
    fn validates_and_maps() {
        let mut m = HardwareMixerInterface::new(AlsaMixerBackend::default());
        assert!(m.alsa_mixer_control_set(0, -1, 1, -1, -1, -1).is_err());
        assert!(m.alsa_mixer_control_set(0, 1, -1, -1, -1, -1).is_err());
        assert_eq!(percent_to_value(50.0, 0, 101), 51);
    }

    #[test]
    fn unchanged_hwid_reuses_the_open_card() {
        let log = Log::default();
        let mut m = HardwareMixerInterface::new(RecordingBackend { log: log.clone() });
        m.alsa_mixer_control_set(7, 3, 1, -1, -1, -1).unwrap();
        m.alsa_mixer_control_set(7, 4, 2, -1, -1, -1).unwrap();
        assert_eq!(&*log.opened.borrow(), &["hw:7".to_owned()]);
        assert_eq!(*log.closed.borrow(), 0);
        assert_eq!(log.writes.borrow().len(), 2);
    }

    #[test]
    fn switching_hwid_reopens_the_card() {
        let log = Log::default();
        let mut m = HardwareMixerInterface::new(RecordingBackend { log: log.clone() });
        m.alsa_mixer_control_set(0, 3, 1, -1, -1, -1).unwrap();
        m.alsa_mixer_control_set(1, 3, 1, -1, -1, -1).unwrap();
        m.alsa_mixer_control_set(1, 3, 1, -1, -1, -1).unwrap();
        assert_eq!(
            &*log.opened.borrow(),
            &["hw:0".to_owned(), "hw:1".to_owned()]
        );
        assert_eq!(*log.closed.borrow(), 1);
        assert_eq!(log.writes.borrow().len(), 3);
    }

    #[test]
    fn emits_only_the_trailing_values() {
        let log = Log::default();
        let mut m = HardwareMixerInterface::new(RecordingBackend { log: log.clone() });
        m.alsa_mixer_control_set(0, 5, -1, 7, -1, -1).unwrap();
        assert_eq!(&*log.writes.borrow(), &[(5, vec![-1, 7])]);
    }

    #[test]
    fn percent_is_clamped_and_survives_wide_ranges() {
        assert_eq!(percent_to_value(-10.0, 0, 100), 0);
        assert_eq!(percent_to_value(150.0, 0, 100), 100);
        assert_eq!(percent_to_value(f64::NAN, 12, 100), 12);
        assert_eq!(percent_to_value(f64::INFINITY, 12, 100), 12);
        assert_eq!(percent_to_value(0.0, i64::MIN, i64::MAX), i64::MIN);
        assert_eq!(percent_to_value(-5.0, -100, 100), -100);
        assert_eq!(percent_to_value(200.0, -100, 100), 100);
    }
}
