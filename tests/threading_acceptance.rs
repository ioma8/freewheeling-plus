use freewheeling_plus::event::{Event, EventListener, EventManager, EventType};
use freewheeling_plus::mem::{MemoryManager, Preallocated, PreallocatedTypeInner};
use freewheeling_plus::processor_queue::{ProcessorCommand, ProcessorCommandQueue};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;

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
