//! OSC output used to mirror playing FreeWheeling loops in Qtractor.
//!
//! The transport is deliberately small and trait based: production uses UDP,
//! while callers and tests can provide any `OscBackend` implementation.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

pub const QTRACTOR_OSC_PORT: u16 = 5000;

#[derive(Clone, Debug, PartialEq)]
pub enum OscType {
    Int(i32),
    Float(f32),
    String(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OscMessage {
    pub path: String,
    pub args: Vec<OscType>,
}

impl OscMessage {
    pub fn new(path: impl Into<String>, args: Vec<OscType>) -> Result<Self, OscError> {
        let path = path.into();
        if !path.starts_with('/') || path.contains('\0') {
            return Err(OscError::InvalidPath(path));
        }
        Ok(Self { path, args })
    }

    pub fn type_tag(&self) -> String {
        let mut tags = String::from(",");
        for arg in &self.args {
            tags.push(match arg {
                OscType::Int(_) => 'i',
                OscType::Float(_) => 'f',
                OscType::String(_) => 's',
            });
        }
        tags
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        padded_string(&mut out, &self.path);
        padded_string(&mut out, &self.type_tag());
        for arg in &self.args {
            match arg {
                OscType::Int(v) => out.extend(v.to_be_bytes()),
                OscType::Float(v) => out.extend(v.to_bits().to_be_bytes()),
                OscType::String(v) => padded_string(&mut out, v),
            }
        }
        out
    }

    pub fn decode(packet: &[u8]) -> Result<Self, OscError> {
        let mut offset = 0;
        let path = read_padded_string(packet, &mut offset)?;
        let tags = read_padded_string(packet, &mut offset)?;
        if !tags.starts_with(',') {
            return Err(OscError::InvalidPacket("missing type tag"));
        }
        let mut args = Vec::with_capacity(tags.len().saturating_sub(1));
        for tag in tags.bytes().skip(1) {
            let arg = match tag {
                b'i' => OscType::Int(i32::from_be_bytes(read_four(packet, &mut offset)?)),
                b'f' => OscType::Float(f32::from_bits(u32::from_be_bytes(read_four(
                    packet,
                    &mut offset,
                )?))),
                b's' => OscType::String(read_padded_string(packet, &mut offset)?),
                _ => return Err(OscError::InvalidPacket("unsupported type tag")),
            };
            args.push(arg);
        }
        OscMessage::new(path, args)
    }
}

#[derive(Debug)]
pub enum OscError {
    InvalidPath(String),
    NotConnected,
    Io(io::Error),
    Poisoned,
    InvalidPacket(&'static str),
}
impl From<io::Error> for OscError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub trait OscBackend: Send {
    fn open(&mut self) -> Result<(), OscError>;
    fn send(&mut self, message: &OscMessage) -> Result<(), OscError>;
    fn close(&mut self);
}

pub struct UdpBackend {
    destination: SocketAddr,
    socket: Option<UdpSocket>,
}
impl UdpBackend {
    pub fn new(host: &str, port: u16) -> Result<Self, OscError> {
        let destination = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or(OscError::NotConnected)?;
        Ok(Self {
            destination,
            socket: None,
        })
    }
}
impl OscBackend for UdpBackend {
    fn open(&mut self) -> Result<(), OscError> {
        self.socket = Some(UdpSocket::bind("0.0.0.0:0")?);
        Ok(())
    }
    fn send(&mut self, message: &OscMessage) -> Result<(), OscError> {
        let socket = self.socket.as_ref().ok_or(OscError::NotConnected)?;
        socket.send_to(&message.encode(), self.destination)?;
        Ok(())
    }
    fn close(&mut self) {
        self.socket = None;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayingLoop {
    pub start: i32,
    pub length: i32,
    pub crossfade: i32,
    pub gain: f32,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayingLoops {
    pub tempo: Option<(f32, i32)>,
    pub loops: Vec<PlayingLoop>,
    pub range_end: i32,
}

pub struct OscClient<B: OscBackend> {
    backend: Arc<Mutex<B>>,
}
impl<B: OscBackend> OscClient<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend: Arc::new(Mutex::new(backend)),
        }
    }
    pub fn open(&self) -> Result<(), OscError> {
        self.backend.lock().map_err(|_| OscError::Poisoned)?.open()
    }
    pub fn close(&self) {
        if let Ok(mut b) = self.backend.lock() {
            b.close();
        }
    }
    pub fn send_playing_loops(&self, snapshot: &PlayingLoops) -> Result<(), OscError> {
        let mut b = self.backend.lock().map_err(|_| OscError::Poisoned)?;
        if let Some((tempo, beats)) = snapshot.tempo {
            b.send(&OscMessage::new(
                "/SetGlobalTempo",
                vec![OscType::Float(tempo), OscType::Int(beats)],
            )?)?;
        }
        for l in &snapshot.loops {
            b.send(&OscMessage::new(
                "/AddAudioClipOnUniqueTrack",
                vec![
                    OscType::Int(l.start),
                    OscType::Int(0),
                    OscType::Int(l.length),
                    OscType::Int(l.crossfade),
                    OscType::Float(l.gain),
                    OscType::String(l.path.clone()),
                ],
            )?)?;
        }
        b.send(&OscMessage::new(
            "/AdvanceLoopRange",
            vec![OscType::Int(0), OscType::Int(snapshot.range_end)],
        )?)
    }
    pub fn receive_event(
        &self,
        event: OscEvent,
        snapshot: Option<&PlayingLoops>,
    ) -> Result<(), OscError> {
        if matches!(event, OscEvent::TransmitPlayingLoopsToDaw)
            && let Some(s) = snapshot
        {
            self.send_playing_loops(s)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscEvent {
    TransmitPlayingLoopsToDaw,
}

/// Bounded UDP OSC receiver. Its worker never waits for a consumer: newest
/// packets are dropped when the queue is saturated.
pub struct OscReceiver {
    receiver: mpsc::Receiver<OscMessage>,
    running: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    local_addr: SocketAddr,
}

impl OscReceiver {
    pub fn bind(address: SocketAddr, capacity: usize) -> Result<Self, OscError> {
        let socket = UdpSocket::bind(address)?;
        socket.set_read_timeout(Some(std::time::Duration::from_millis(20)))?;
        let local_addr = socket.local_addr()?;
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        let running = Arc::new(AtomicBool::new(true));
        let dropped = Arc::new(AtomicU64::new(0));
        let worker_running = Arc::clone(&running);
        let worker_dropped = Arc::clone(&dropped);
        let worker = thread::Builder::new()
            .name("osc-receive".into())
            .spawn(move || {
                let mut packet = [0_u8; 65_507];
                while worker_running.load(Ordering::Acquire) {
                    match socket.recv_from(&mut packet) {
                        Ok((length, _)) => {
                            if let Ok(message) = OscMessage::decode(&packet[..length])
                                && sender.try_send(message).is_err()
                            {
                                worker_dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) => {}
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| OscError::Io(io::Error::other(error)))?;
        Ok(Self {
            receiver,
            running,
            dropped,
            worker: Some(worker),
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
    pub fn try_receive(&self) -> Option<OscMessage> {
        self.receiver.try_recv().ok()
    }
    pub fn dropped_messages(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for OscReceiver {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn padded_string(out: &mut Vec<u8>, value: &str) {
    out.extend(value.as_bytes());
    out.push(0);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn read_four(packet: &[u8], offset: &mut usize) -> Result<[u8; 4], OscError> {
    let bytes = packet
        .get(*offset..*offset + 4)
        .ok_or(OscError::InvalidPacket("truncated argument"))?;
    *offset += 4;
    Ok(bytes.try_into().expect("four byte slice"))
}

fn read_padded_string(packet: &[u8], offset: &mut usize) -> Result<String, OscError> {
    let tail = packet
        .get(*offset..)
        .ok_or(OscError::InvalidPacket("truncated string"))?;
    let length = tail
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(OscError::InvalidPacket("unterminated string"))?;
    let value = std::str::from_utf8(&tail[..length])
        .map_err(|_| OscError::InvalidPacket("invalid UTF-8"))?
        .to_owned();
    *offset += (length + 1).next_multiple_of(4);
    if *offset > packet.len() {
        return Err(OscError::InvalidPacket("truncated padding"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Recorder {
        opened: bool,
        messages: Vec<OscMessage>,
    }
    impl OscBackend for Recorder {
        fn open(&mut self) -> Result<(), OscError> {
            self.opened = true;
            Ok(())
        }
        fn send(&mut self, m: &OscMessage) -> Result<(), OscError> {
            if !self.opened {
                return Err(OscError::NotConnected);
            }
            self.messages.push(m.clone());
            Ok(())
        }
        fn close(&mut self) {
            self.opened = false;
        }
    }
    #[test]
    fn preserves_paths_types_and_order() {
        let r = Recorder {
            opened: false,
            messages: vec![],
        };
        let c = OscClient::new(r);
        c.open().unwrap();
        c.send_playing_loops(&PlayingLoops {
            tempo: Some((120.0, 4)),
            loops: vec![PlayingLoop {
                start: 0,
                length: 480,
                crossfade: 64,
                gain: 0.5,
                path: "/tmp/a.wav".into(),
            }],
            range_end: 480,
        })
        .unwrap();
        let b = c.backend.lock().unwrap();
        assert_eq!(b.messages[0].type_tag(), ",fi");
        assert_eq!(b.messages[1].type_tag(), ",iiiifs");
        assert_eq!(b.messages[2].path, "/AdvanceLoopRange");
    }
    #[test]
    fn rejects_bad_paths() {
        assert!(OscMessage::new("bad", vec![]).is_err());
    }

    #[test]
    fn packet_round_trip_and_udp_receipt() {
        let message = OscMessage::new(
            "/control",
            vec![
                OscType::Int(7),
                OscType::Float(0.5),
                OscType::String("ok".into()),
            ],
        )
        .unwrap();
        assert_eq!(OscMessage::decode(&message.encode()).unwrap(), message);
        let mut receiver = OscReceiver::bind("127.0.0.1:0".parse().unwrap(), 2).unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .send_to(&message.encode(), receiver.local_addr())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut received = None;
        while received.is_none() && std::time::Instant::now() < deadline {
            received = receiver.try_receive();
            std::thread::yield_now();
        }
        assert_eq!(received, Some(message));
        receiver.shutdown();
    }
}
