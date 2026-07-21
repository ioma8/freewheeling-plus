use freewheeling_plus::audioio::{
    AudioBackend, AudioCallback, AudioCallbackFn, AudioIO, AudioProcessor, BackendInfo,
};
use freewheeling_plus::event::{Event, EventListener, EventManager, EventType};
use freewheeling_plus::mem::{MemoryManager, Preallocated, PreallocatedTypeInner};
use freewheeling_plus::midiio::{MidiBackend, MidiIo, MidiMessage, MidiPortMessage};
use freewheeling_plus::processor_queue::{ProcessorCommand, ProcessorCommandQueue};
use freewheeling_plus::rcu::{Rcu, RcuRegistry};
use std::sync::atomic::AtomicPtr;
use std::sync::mpsc;
use std::sync::{Arc, Barrier, atomic::{AtomicUsize, Ordering}};
use std::thread;
use std::time::Duration;

struct CountListener(Arc<AtomicUsize>);
impl EventListener for CountListener {
    fn receive_event(
        &mut self,
        _: &Event,
        _: &dyn freewheeling_plus::event::EventProducer,
    ) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn event_dispatch_is_thread_safe_and_shutdown_joins_worker() {
    let manager = Arc::new(EventManager::new());
    let seen = Arc::new(AtomicUsize::new(0));
    manager.listen(
        Box::new(CountListener(seen.clone())),
        EventType::StartSession,
    );
    let barrier = Arc::new(Barrier::new(5));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let m = manager.clone();
        let b = barrier.clone();
        workers.push(thread::spawn(move || {
            b.wait();
            m.post_event(Event::StartSession);
        }));
    }
    barrier.wait();
    for w in workers {
        w.join().unwrap();
    }
    for _ in 0..1000 {
        if seen.load(Ordering::SeqCst) == 4 {
            break;
        }
        thread::yield_now();
    }
    assert_eq!(seen.load(Ordering::SeqCst), 4);
    drop(manager);
}

struct Item(Arc<AtomicUsize>);
impl Preallocated for Item {
    fn recycle(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn memory_manager_recycles_deferred_items_before_shutdown() {
    let manager = Arc::new(MemoryManager::new());
    let recycled = Arc::new(AtomicUsize::new(0));
    let ty = PreallocatedTypeInner::new(&manager, 3, true, {
        let r = recycled.clone();
        move || Box::new(Item(r.clone()))
    });
    let item = ty.rt_new().unwrap();
    ty.rt_delete(item);
    for _ in 0..1000 {
        if recycled.load(Ordering::SeqCst) == 1 {
            break;
        }
        thread::yield_now();
    }
    assert_eq!(recycled.load(Ordering::SeqCst), 1);
    assert!(ty.rt_new().is_some());
}

#[test]
fn rcu_waits_for_reader_from_before_update() {
    let registry = RcuRegistry::new();
    registry.register_current().unwrap();
    let slot = Arc::new(AtomicPtr::new(Box::into_raw(Box::new(1))));
    let gate = Arc::new(Barrier::new(2));
    let (reader_id_tx, reader_id_rx) = mpsc::channel();
    let (rcu_tx, rcu_rx) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let g = gate.clone();
    let reader = thread::spawn(move || {
        reader_id_tx.send(thread::current().id()).unwrap();
        let r: Arc<Rcu<u32>> = rcu_rx.recv().unwrap();
        r.registry().register_current().unwrap();
        r.read_lock().unwrap();
        g.wait();
        rx.recv().unwrap();
        r.read_unlock().unwrap();
    });
    registry.register(reader_id_rx.recv().unwrap()).unwrap();
    let rcu = Arc::new(Rcu::<u32>::new(registry));
    rcu_tx.send(rcu.clone()).unwrap();
    gate.wait();
    let old = unsafe { rcu.update(&slot, Box::into_raw(Box::new(2))) };
    let r = rcu.clone();
    let done = Arc::new(AtomicUsize::new(0));
    let d = done.clone();
    let waiter = thread::spawn(move || {
        r.synchronize(Duration::from_micros(1));
        d.store(1, Ordering::SeqCst);
    });
    assert!(done.load(Ordering::SeqCst) == 0);
    tx.send(()).unwrap();
    reader.join().unwrap();
    waiter.join().unwrap();
    unsafe {
        drop(Box::from_raw(old));
        drop(Box::from_raw(slot.load(Ordering::Acquire)));
    }
}

#[test]
fn processor_queue_preserves_all_commands_under_contention() {
    let q = Arc::new(ProcessorCommandQueue::new());
    let barrier = Arc::new(Barrier::new(5));
    let mut ts = Vec::new();
    for _ in 0..4 {
        let q = q.clone();
        let b = barrier.clone();
        ts.push(thread::spawn(move || {
            b.wait();
            q.enqueue_add(std::ptr::null_mut())
        }));
    }
    barrier.wait();
    let accepted: usize = ts.into_iter().map(|t| t.join().unwrap() as usize).sum();
    assert_eq!(q.pending_count(), accepted);
    let mut c = ProcessorCommand::default();
    let mut count = 0;
    while q.read_next(&mut c) {
        count += 1;
    }
    assert_eq!(count, accepted);
}

struct FakeAudio;
impl AudioBackend for FakeAudio {
    fn open(&mut self, _: &str) -> Result<BackendInfo, String> {
        Ok(BackendInfo {
            sample_rate: 48000,
            buffer_size: 4,
        })
    }
    fn activate(&mut self, mut cb: AudioCallbackFn) -> Result<(), String> {
        let i = vec![1.; 4];
        let mut l = vec![0.; 4];
        let mut r = vec![0.; 4];
        let mut c = AudioCallback {
            inputs: [&i, &i],
            outputs: [&mut l, &mut r],
            nframes: 4,
            position: Default::default(),
            transport_rolling: false,
        };
        cb(&mut c);
        assert_eq!(l, [2.; 4]);
        Ok(())
    }
    fn close(&mut self) {}
    fn relocate(&mut self, _: u32) {}
}
struct Gain;
impl AudioProcessor for Gain {
    fn process(&mut self, c: &mut AudioCallback<'_>) {
        for i in 0..4 {
            c.outputs[0][i] = c.inputs[0][i] * 2.;
        }
    }
}
#[test]
fn audio_callback_updates_state_on_callback_thread() {
    let mut io = AudioIO::new(FakeAudio);
    io.open("test").unwrap();
    io.activate(Gain).unwrap();
    assert_eq!(io.get_srate(), 48000);
    assert!(io.callback_thread().is_some());
}

struct MidiFake;
impl MidiBackend for MidiFake {
    fn open(&mut self, _: usize, _: usize) -> Result<(), String> {
        Ok(())
    }
    fn receive(&mut self) -> Result<Option<MidiPortMessage>, String> {
        Ok(None)
    }
    fn send(&mut self, _: MidiPortMessage) -> Result<(), String> {
        Ok(())
    }
    fn close(&mut self) {}
}
#[test]
fn midi_callback_state_survives_activation_and_shutdown() {
    let mut io = MidiIo::new(MidiFake);
    io.activate(1, 1).unwrap();
    assert_eq!(io.inputs, 1);
    assert!(io.send(0, MidiMessage::Clock).is_ok());
    io.shutdown();
}
