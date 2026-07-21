use freewheeling_plus::event::{Event, EventListener, EventManager, EventType};
use freewheeling_plus::mem::{MemoryManager, Preallocated, PreallocatedTypeInner};
use freewheeling_plus::processor_queue::{ProcessorCommand, ProcessorCommandQueue};
use freewheeling_plus::rcu::{Rcu, RcuRegistry};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

struct AcknowledgingListener(mpsc::Sender<()>);

impl EventListener for AcknowledgingListener {
    fn receive_event(
        &mut self,
        event: &Event,
        _: &dyn freewheeling_plus::event::EventProducer,
    ) {
        assert_eq!(event.get_type(), EventType::StartSession);
        self.0.send(()).unwrap();
    }
}

#[test]
fn event_delivery_acknowledges_every_concurrent_post() {
    const PRODUCERS: usize = 6;
    const EVENTS_PER_PRODUCER: usize = 8;
    let manager = Arc::new(EventManager::new());
    let (delivered_tx, delivered_rx) = mpsc::channel();
    manager.listen(
        Box::new(AcknowledgingListener(delivered_tx)),
        EventType::StartSession,
    );
    let gate = Arc::new(Barrier::new(PRODUCERS + 1));
    let mut workers = Vec::new();
    for _ in 0..PRODUCERS {
        let manager = Arc::clone(&manager);
        let gate = Arc::clone(&gate);
        workers.push(thread::spawn(move || {
            gate.wait();
            for _ in 0..EVENTS_PER_PRODUCER {
                manager.post_event(freewheeling_plus::event::Event::StartSession);
            }
        }));
    }
    gate.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    for _ in 0..PRODUCERS * EVENTS_PER_PRODUCER {
        delivered_rx.recv().unwrap();
    }
}

#[test]
fn rcu_grace_period_is_released_by_reader_unlock() {
    let registry = RcuRegistry::new();
    registry.register_current().unwrap();
    let slot = Arc::new(AtomicPtr::new(Box::into_raw(Box::new(1))));
    let reader_started = Arc::new(Barrier::new(2));
    let (reader_id_tx, reader_id_rx) = mpsc::channel();
    let (rcu_tx, rcu_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let reader_started_for_thread = Arc::clone(&reader_started);
    let reader = thread::spawn(move || {
        reader_id_tx.send(thread::current().id()).unwrap();
        let reader_rcu: Arc<Rcu<u32>> = rcu_rx.recv().unwrap();
        reader_rcu.registry().register_current().unwrap();
        reader_rcu.read_lock().unwrap();
        reader_started_for_thread.wait();
        release_rx.recv().unwrap();
        reader_rcu.read_unlock().unwrap();
    });
    registry.register(reader_id_rx.recv().unwrap()).unwrap();
    let rcu = Arc::new(Rcu::<u32>::new(registry));
    rcu_tx.send(Arc::clone(&rcu)).unwrap();
    reader_started.wait();
    let old = unsafe { rcu.update(&slot, Box::into_raw(Box::new(2))) };
    let waiter_rcu = Arc::clone(&rcu);
    let (done_tx, done_rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        waiter_rcu.synchronize(Duration::ZERO);
        done_tx.send(()).unwrap();
    });
    assert!(done_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    reader.join().unwrap();
    done_rx.recv().unwrap();
    waiter.join().unwrap();
    unsafe {
        drop(Box::from_raw(old));
        drop(Box::from_raw(slot.load(Ordering::Acquire)));
    }
}

#[test]
fn processor_queue_is_bounded_and_fifo_under_concurrent_producers() {
    const PRODUCERS: usize = 4;
    const EACH: usize = 32;
    let queue = Arc::new(ProcessorCommandQueue::new());
    let gate = Arc::new(Barrier::new(PRODUCERS + 1));
    let mut workers = Vec::new();
    for _ in 0..PRODUCERS {
        let queue = Arc::clone(&queue);
        let gate = Arc::clone(&gate);
        workers.push(thread::spawn(move || {
            gate.wait();
            (0..EACH)
                .filter(|_| queue.enqueue_add(std::ptr::null_mut()))
                .count()
        }));
    }
    gate.wait();
    let accepted: usize = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .sum();
    assert_eq!(queue.pending_count(), accepted);
    let mut command = ProcessorCommand::default();
    let mut drained = 0;
    while queue.read_next(&mut command) {
        assert_eq!(
            command.command_type,
            freewheeling_plus::processor_queue::ProcessorCommandType::Add
        );
        drained += 1;
    }
    assert_eq!(drained, accepted);
    assert_eq!(queue.pending_count(), 0);
}

struct Recycled(Arc<AtomicUsize>, mpsc::Sender<()>);

impl Preallocated for Recycled {
    fn recycle(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        self.1.send(()).unwrap();
    }
}

#[test]
fn preallocation_recycles_concurrent_deletes_before_reuse() {
    const INSTANCES: usize = 4;
    let manager = Arc::new(MemoryManager::new());
    let recycled = Arc::new(AtomicUsize::new(0));
    let (recycle_tx, recycle_rx) = mpsc::channel();
    let ty = PreallocatedTypeInner::new(&manager, INSTANCES + 1, true, {
        let recycled = Arc::clone(&recycled);
        let recycle_tx = recycle_tx.clone();
        move || Box::new(Recycled(Arc::clone(&recycled), recycle_tx.clone()))
    });
    let gate = Arc::new(Barrier::new(INSTANCES + 1));
    let mut workers = Vec::new();
    for _ in 0..INSTANCES {
        let ty = Arc::clone(&ty);
        let gate = Arc::clone(&gate);
        workers.push(thread::spawn(move || {
            gate.wait();
            let item = ty.rt_new().unwrap();
            ty.rt_delete(item);
        }));
    }
    gate.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    for _ in 0..INSTANCES {
        recycle_rx.recv().unwrap();
    }
    assert_eq!(recycled.load(Ordering::SeqCst), INSTANCES);
    assert_eq!(ty.get_block_size(), INSTANCES + 1);
    for _ in 0..INSTANCES {
        assert!(ty.rt_new().is_some());
    }
}
