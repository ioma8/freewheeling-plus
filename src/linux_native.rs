//! Direct ALSA mixer backend (Linux only).
//!
//! Provides a simple wrapper around ALSA high-level controls for volume
//! and mute management. JACK audio/MIDI has been extracted into `crate::jack`.

#[cfg(target_os = "linux")]
use crate::amixer::MixerBackend;
#[cfg(target_os = "linux")]
use alsa::ctl::{ElemId, ElemIface};
#[cfg(target_os = "linux")]
use alsa::hctl::HCtl;

#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct DirectAlsaMixerBackend {
    ctl: Option<HCtl>,
}

#[cfg(target_os = "linux")]
impl MixerBackend for DirectAlsaMixerBackend {
    fn open(&mut self, card: &str) -> Result<(), String> {
        self.close();
        let ctl = HCtl::new(card, false)
            .map_err(|e| format!("cannot open ALSA control {card}: {e}"))?;
        ctl.load()
            .map_err(|e| format!("cannot load ALSA controls for {card}: {e}"))?;
        self.ctl = Some(ctl);
        Ok(())
    }
    fn set_control(&mut self, numid: i32, values: &[i32]) -> Result<(), String> {
        let ctl = self.ctl.as_ref().ok_or("ALSA mixer is not open")?;
        let mut id = ElemId::new(ElemIface::Mixer);
        id.set_numid(numid as u32);
        let elem = ctl
            .find_elem(&id)
            .ok_or_else(|| format!("ALSA numid {numid} does not exist"))?;
        let mut value = elem
            .read()
            .map_err(|e| format!("cannot read ALSA numid {numid}: {e}"))?;
        for (index, raw) in values.iter().enumerate() {
            value
                .set_integer(index as u32, *raw)
                .ok_or_else(|| format!("ALSA numid {numid} value {index} is not integer"))?;
        }
        elem.write(&value)
            .map(|_| ())
            .map_err(|e| format!("cannot write ALSA numid {numid}: {e}"))
    }
    fn close(&mut self) {
        self.ctl = None;
    }
}
