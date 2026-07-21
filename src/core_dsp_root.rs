//! Root DSP graph, ported from `fweelin_core_dsp_root.cc`.
//!
//! The traits in this file are deliberately small compatibility boundaries for
//! the not-yet-migrated app, audio-buffer, processor, and command-queue types.
//! They contain no fallback processing: callers must provide the real DSP
//! implementations.

pub type Sample = f32;
pub type Frames = usize;

/// Priority levels for child processors in the DSP graph.
/// The numeric value is used as an index into the per-priority child list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ProcessorPriority {
    Default = 0,
    Global = 1,
    GlobalSecondChain = 2,
    HiPriority = 3,
    Final = 4,
}


pub struct AudioBuffers<'a> {
    pub inputs: [&'a [Sample]; 2],
    pub outputs: [&'a mut [Sample]; 2],
    pub num_inputs: usize,
    pub num_outputs: usize,
}

pub trait Processor {
    fn process(&mut self, pre: bool, len: Frames, buffers: &mut AudioBuffers<'_>);
    fn halt(&mut self) {}
    fn preprocess(&mut self) {}
}

pub trait RootApp {
    fn fragment_size(&self) -> Frames;
    fn time_scale(&self) -> f32;
    fn cleanup(&mut self, processor: Box<dyn Processor>);
}

pub enum Command {
    Add {
        processor: Box<dyn Processor>,
        kind: ProcessorPriority,
        silent: bool,
    },
    Delete {
        processor: *mut dyn Processor,
    },
}

pub trait CommandQueue {
    fn push(&mut self, command: Command) -> bool;
    fn pop(&mut self) -> Option<Command>;
}

struct Item {
    processor: Box<dyn Processor>,
    kind: ProcessorPriority,
    silent: bool,
    status: Status,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Live,
    LivePendingDelete,
    PendingDelete,
}

pub struct RootProcessor<A: RootApp, Q: CommandQueue> {
    app: A,
    queue: Option<Q>,
    input_volume: f32,
    input_delta: f32,
    output_volume: f32,
    output_delta: f32,
    input_settings: Vec<(f32, f32)>,
    children: Vec<Item>,
    work: Vec<Sample>,
    /// `Processor::preab`: the already-rendered root output used to fade an
    /// abrupt graph change into the current fragment.
    prework: Vec<Sample>,
    /// Safe snapshots for C++ stages whose `ins` and `outs` point at the same
    /// storage.  The original alias is observable only to processors; a
    /// preallocated copy gives them the same incoming signal without mutable
    /// aliasing in Rust.
    alias_input: Vec<Sample>,
    stereo: bool,
    sample_count: Frames,
    prewritten: bool,
    prewriting: bool,
    pre_len: Frames,
}

const MIN_VOLUME: f32 = 0.01;
const MAX_VOLUME: f32 = 5.0;
const MAX_DELTA: f32 = 1.5;
const DEFAULT_SMOOTH_LENGTH: Frames = 64;

impl<A: RootApp, Q: CommandQueue> RootProcessor<A, Q> {
    pub fn new(app: A, input_settings: Vec<(f32, f32)>, channels: usize) -> Self {
        assert!(channels == 1 || channels == 2);
        let n = app.fragment_size();
        Self {
            app,
            queue: None,
            input_volume: 1.0,
            input_delta: 1.0,
            output_volume: 1.0,
            output_delta: 1.0,
            input_settings,
            children: Vec::new(),
            work: vec![0.0; n * channels],
            prework: vec![0.0; n * channels],
            alias_input: vec![0.0; n * channels],
            stereo: channels == 2,
            sample_count: 0,
            prewritten: false,
            prewriting: false,
            pre_len: 0,
        }
    }
    pub fn final_prep(&mut self, queue: Q) {
        self.queue = Some(queue);
    }
    pub fn output_volume(&self) -> f32 {
        self.output_volume
    }
    pub fn input_volume(&self) -> f32 {
        self.input_volume
    }
    pub fn sample_count(&self) -> Frames {
        self.sample_count
    }
    pub fn adjust_output_volume(&mut self, amount: f32) {
        self.output_delta =
            (self.output_delta + amount * self.app.time_scale()).clamp(0.0, MAX_DELTA);
    }
    pub fn adjust_input_volume(&mut self, amount: f32) {
        self.input_delta =
            (self.input_delta + amount * self.app.time_scale()).clamp(0.0, MAX_DELTA);
    }
    pub fn set_output_volume(&mut self, value: f32) {
        self.output_volume = value;
        self.output_delta = 1.0;
    }
    pub fn set_input_volume(&mut self, value: f32) {
        self.input_volume = value;
        self.input_delta = 1.0;
    }
    pub fn add_child(&mut self, processor: Box<dyn Processor>, kind: ProcessorPriority, silent: bool) -> bool {
        self.do_preprocess();
        self.queue.as_mut().is_some_and(|q| {
            q.push(Command::Add {
                processor,
                kind,
                silent,
            })
        })
    }
    pub fn del_child(&mut self, processor: *mut dyn Processor) -> bool {
        if processor.is_null() {
            return false;
        }
        self.do_preprocess();
        if let Some(item) = self
            .children
            .iter_mut()
            .find(|x| std::ptr::addr_eq(std::ptr::from_ref(x.processor.as_ref()), processor))
        {
            item.processor.halt();
        }
        self.queue
            .as_mut()
            .is_some_and(|q| q.push(Command::Delete { processor }))
    }
    /// C++ `Processor::dopreprocess` for the root processor.  It renders the
    /// existing graph once with `pre=1`, then `fadepreandcurrent` consumes the
    /// result during the next ordinary callback.
    fn do_preprocess(&mut self) {
        if self.prewritten || self.prewriting || self.children.is_empty() {
            return;
        }
        self.prewriting = true;
        let channels = if self.stereo { 2 } else { 1 };
        let len = self.app.fragment_size().min(DEFAULT_SMOOTH_LENGTH);
        let empty_left = [];
        let empty_right = [];
        // Move the prebuffer out temporarily: processing the graph needs an
        // exclusive `self`, while its output is the buffer we are filling.
        // This is a move of its allocation, not a callback allocation.
        let mut prework = std::mem::take(&mut self.prework);
        let split = if self.stereo {
            prework.len() / 2
        } else {
            prework.len()
        };
        let (left, right) = prework.split_at_mut(split);
        let mut pre = AudioBuffers {
            inputs: [&empty_left, &empty_right],
            outputs: [left, &mut []],
            num_inputs: 0,
            num_outputs: 1,
        };
        if channels == 2 {
            pre.outputs[1] = right;
        }
        self.process(true, len, &mut pre);
        self.prework = prework;
        self.pre_len = len;
        self.prewritten = true;
        self.prewriting = false;
    }

    fn fade_pre_and_current(&mut self, len: Frames, buffers: &mut AudioBuffers<'_>) {
        if !self.prewritten {
            return;
        }
        let count = len.min(self.pre_len).min(DEFAULT_SMOOTH_LENGTH);
        let split = if self.stereo {
            self.prework.len() / 2
        } else {
            self.prework.len()
        };
        let (pre_left, pre_right) = self.prework.split_at(split);
        for channel in 0..if self.stereo { 2 } else { 1 } {
            let previous = if channel == 0 { pre_left } else { pre_right };
            for (frame, prev) in previous.iter().enumerate().take(count) {
                let ramp = frame as f32 / DEFAULT_SMOOTH_LENGTH as f32;
                buffers.outputs[channel][frame] =
                    buffers.outputs[channel][frame] * ramp + prev * (1.0 - ramp);
            }
        }
        self.prewritten = false;
    }
    fn update(&mut self) {
        let Some(mut q) = self.queue.take() else {
            return;
        };
        while let Some(command) = q.pop() {
            match command {
                Command::Add {
                    processor,
                    kind,
                    silent,
                } => self.children.push(Item {
                    processor,
                    kind,
                    silent,
                    status: Status::Live,
                }),
                Command::Delete { processor } => {
                    if let Some(i) = self.children.iter_mut().find(|x| {
                        std::ptr::addr_eq(std::ptr::from_ref(x.processor.as_ref()), processor)
                    }) && i.status == Status::Live
                    {
                        i.status = Status::LivePendingDelete;
                    }
                }
            }
        }
        let mut i = 0;
        while i < self.children.len() {
            if self.children[i].status == Status::PendingDelete {
                let item = self.children.remove(i);
                self.app.cleanup(item.processor);
            } else {
                i += 1;
            }
        }
        self.queue = Some(q);
    }
    #[allow(clippy::too_many_arguments)]
    fn chain(
        children: &mut [Item],
        stereo: bool,
        pre: bool,
        len: Frames,
        mut main: Option<&mut AudioBuffers<'_>>,
        child: &mut AudioBuffers<'_>,
        kind: ProcessorPriority,
        mix: bool,
    ) {
        // Invariant (debug builds): all live items of `kind` are processed.
        #[cfg(debug_assertions)]
        let expected = children
            .iter()
            .filter(|i| i.status != Status::PendingDelete && i.kind == kind)
            .count();
        #[cfg(debug_assertions)]
        let mut processed = 0usize;

        for item in children.iter_mut() {
            if item.status != Status::PendingDelete && item.kind == kind {
                item.processor.process(pre, len, child);
                if !pre && item.status == Status::LivePendingDelete {
                    item.status = Status::PendingDelete;
                }
                if mix && !item.silent {
                    let main = main.as_deref_mut().expect("mixing needs main output");
                    for c in 0..if stereo { 2 } else { 1 } {
                        for n in 0..len {
                            main.outputs[c][n] += child.outputs[c][n];
                        }
                    }
                }
                #[cfg(debug_assertions)]
                {
                    processed += 1;
                }
            }
        }

        #[cfg(debug_assertions)]
        debug_assert_eq!(
            processed, expected,
            "chain({kind:?}): {processed}/{expected} items processed"
        );
    }
    pub fn process(&mut self, pre: bool, requested: Frames, buffers: &mut AudioBuffers<'_>) {
        let len = requested.min(self.app.fragment_size());
        for c in 0..if self.stereo { 2 } else { 1 } {
            buffers.outputs[c][..len].fill(0.0);
        }
        if !pre {
            if self.queue.is_none() {
                return;
            }
            self.update();
            self.output_volume = (self.output_volume * self.output_delta).min(MAX_VOLUME);
            self.input_volume =
                (self.input_volume * self.input_delta).clamp(MIN_VOLUME, MAX_VOLUME);
            for v in &mut self.input_settings {
                v.0 *= v.1;
            }
        }
        {
            let split = if self.stereo {
                self.work.len() / 2
            } else {
                self.work.len()
            };
            let (left, right) = self.work.split_at_mut(split);
            let mut child = AudioBuffers {
                inputs: [buffers.inputs[0], buffers.inputs[1]],
                outputs: [left, &mut []],
                num_inputs: 0,
                num_outputs: 1,
            };
            if self.stereo {
                child.outputs[1] = right;
            }
            Self::chain(
                &mut self.children,
                self.stereo,
                pre,
                len,
                Some(buffers),
                &mut child,
                ProcessorPriority::HiPriority,
                true,
            );
            Self::chain(
                &mut self.children,
                self.stereo,
                pre,
                len,
                Some(buffers),
                &mut child,
                ProcessorPriority::Default,
                true,
            );
        }
        // C++ applies the root gain immediately after the high-priority and
        // default mix, before its global/final post-processing chains.
        for c in 0..if self.stereo { 2 } else { 1 } {
            for x in &mut buffers.outputs[c][..len] {
                *x *= self.output_volume;
            }
        }
        if !pre {
            self.fade_pre_and_current(len, buffers);
            // `GLOBAL_SECOND_CHAIN` receives a copy of the already mixed,
            // gain-scaled signal in C++.  C++ aliases that copy as both the
            // child input and output; retain a preallocated input snapshot so
            // processors receive the same starting signal without unsafe
            // mutable aliasing.
            let alias_split = if self.stereo {
                self.alias_input.len() / 2
            } else {
                self.alias_input.len()
            };
            {
                let work_split = if self.stereo {
                    self.work.len() / 2
                } else {
                    self.work.len()
                };
                let (work_left, work_right) = self.work.split_at_mut(work_split);
                work_left[..len].copy_from_slice(&buffers.outputs[0][..len]);
                if self.stereo {
                    work_right[..len].copy_from_slice(&buffers.outputs[1][..len]);
                }
                let (alias_left, alias_right) = self.alias_input.split_at_mut(alias_split);
                alias_left[..len].copy_from_slice(&work_left[..len]);
                if self.stereo {
                    alias_right[..len].copy_from_slice(&work_right[..len]);
                }
                let mut child = AudioBuffers {
                    inputs: [alias_left, alias_right],
                    outputs: [work_left, work_right],
                    num_inputs: 0,
                    num_outputs: 1,
                };
                // These stages deliberately remain separate: their order and
                // whether they mix into the accumulated signal are graph ABI.
                Self::chain(
                    &mut self.children,
                    self.stereo,
                    false,
                    len,
                    None,
                    &mut child,
                    ProcessorPriority::GlobalSecondChain,
                    false,
                );
            }
            // C++ restores `abtmp`'s external inputs before running GLOBAL,
            // while leaving its output scratch buffer intact.
            {
                let work_split = if self.stereo {
                    self.work.len() / 2
                } else {
                    self.work.len()
                };
                let (work_left, work_right) = self.work.split_at_mut(work_split);
                let mut child = AudioBuffers {
                    inputs: [buffers.inputs[0], buffers.inputs[1]],
                    outputs: [work_left, work_right],
                    num_inputs: 0,
                    num_outputs: 1,
                };
                Self::chain(
                    &mut self.children,
                    self.stereo,
                    false,
                    len,
                    Some(buffers),
                    &mut child,
                    ProcessorPriority::Global,
                    true,
                );
            }
            // FINAL is another C++ input/output alias, this time the main
            // output.  Snapshot it before the in-place stage.
            let (alias_left, alias_right) = self.alias_input.split_at_mut(alias_split);
            alias_left[..len].copy_from_slice(&buffers.outputs[0][..len]);
            if self.stereo {
                alias_right[..len].copy_from_slice(&buffers.outputs[1][..len]);
            }
            let [out_left, out_right] = &mut buffers.outputs;
            let mut child = AudioBuffers {
                inputs: [alias_left, alias_right],
                outputs: [&mut **out_left, &mut **out_right],
                num_inputs: 0,
                num_outputs: 1,
            };
            Self::chain(
                &mut self.children,
                self.stereo,
                false,
                len,
                None,
                &mut child,
                    ProcessorPriority::Final,
                false,
            );
        }
        if !pre {
            self.sample_count += len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    struct App;
    impl RootApp for App {
        fn fragment_size(&self) -> Frames {
            4
        }
        fn time_scale(&self) -> f32 {
            1.0
        }
        fn cleanup(&mut self, _processor: Box<dyn Processor>) {}
    }

    #[derive(Default)]
    struct Queue(VecDeque<Command>);
    impl CommandQueue for Queue {
        fn push(&mut self, command: Command) -> bool {
            self.0.push_back(command);
            true
        }
        fn pop(&mut self) -> Option<Command> {
            self.0.pop_front()
        }
    }

    struct Constant;
    impl Processor for Constant {
        fn process(&mut self, _pre: bool, len: Frames, buffers: &mut AudioBuffers<'_>) {
            buffers.outputs[0][..len].fill(1.0);
        }
    }

    struct Value(f32);
    impl Processor for Value {
        fn process(&mut self, _pre: bool, len: Frames, buffers: &mut AudioBuffers<'_>) {
            buffers.outputs[0][..len].fill(self.0);
        }
    }

    struct CaptureInput(Arc<Mutex<Vec<f32>>>);
    impl Processor for CaptureInput {
        fn process(&mut self, _pre: bool, len: Frames, buffers: &mut AudioBuffers<'_>) {
            self.0
                .lock()
                .unwrap()
                .extend_from_slice(&buffers.inputs[0][..len]);
        }
    }

    #[test]
    fn mono_root_processes_full_fragment_and_counts_samples() {
        let mut root = RootProcessor::new(App, vec![], 1);
        root.final_prep(Queue::default());
        assert!(root.add_child(Box::new(Constant), ProcessorPriority::Default, false));

        let mut output = [0.0; 4];
        let mut empty_input_left = [];
        let mut empty_input_right = [];
        let mut empty_output_right = [];
        let mut buffers = AudioBuffers {
            inputs: [&mut empty_input_left, &mut empty_input_right],
            outputs: [&mut output, &mut empty_output_right],
            num_inputs: 0,
            num_outputs: 1,
        };
        root.process(false, 4, &mut buffers);

        assert_eq!(buffers.outputs[0], [1.0; 4]);
        assert_eq!(root.sample_count(), 4);
    }

    #[test]
    fn cpp_default_mix_is_gain_scaled_before_global_mix() {
        let mut root = RootProcessor::new(App, vec![], 1);
        root.final_prep(Queue::default());
        root.set_output_volume(2.0);
        assert!(root.add_child(Box::new(Value(1.0)), ProcessorPriority::Default, false));
        assert!(root.add_child(Box::new(Value(3.0)), ProcessorPriority::Global, false));

        let mut output = [0.0; 4];
        let mut empty_input_left = [];
        let mut empty_input_right = [];
        let mut empty_output_right = [];
        let mut buffers = AudioBuffers {
            inputs: [&mut empty_input_left, &mut empty_input_right],
            outputs: [&mut output, &mut empty_output_right],
            num_inputs: 0,
            num_outputs: 1,
        };
        root.process(false, 4, &mut buffers);

        // C++: default 1.0 → root gain 2.0 → global adds 3.0.
        assert_eq!(buffers.outputs[0], [5.0; 4]);
    }

    #[test]
    fn graph_change_fades_the_cpp_preprocessed_root_output() {
        let mut root = RootProcessor::new(App, vec![], 1);
        root.final_prep(Queue::default());
        assert!(root.add_child(Box::new(Value(1.0)), ProcessorPriority::Default, false));

        let mut output = [0.0; 4];
        let empty_input_left = [];
        let empty_input_right = [];
        let mut empty_output_right = [];
        let mut buffers = AudioBuffers {
            inputs: [&empty_input_left, &empty_input_right],
            outputs: [&mut output, &mut empty_output_right],
            num_inputs: 0,
            num_outputs: 1,
        };
        root.process(false, 4, &mut buffers);

        // `AddChild` runs RootProcessor::dopreprocess before queueing the
        // change. The next normal fragment fades old 1.0 into new 4.0 using
        // C++'s fixed 64-sample ramp.
        assert!(root.add_child(Box::new(Value(3.0)), ProcessorPriority::Default, false));
        root.process(false, 4, &mut buffers);
        assert_eq!(buffers.outputs[0][0], 1.0);
        assert!((buffers.outputs[0][1] - (1.0 + 3.0 / 64.0)).abs() < 1e-6);
    }

    #[test]
    fn global_second_chain_receives_post_gain_output_like_cpp_alias() {
        let mut root = RootProcessor::new(App, vec![], 1);
        root.final_prep(Queue::default());
        root.set_output_volume(2.0);
        let captured = Arc::new(Mutex::new(Vec::new()));
        assert!(root.add_child(Box::new(Value(1.0)), ProcessorPriority::Default, false));
        assert!(root.add_child(
            Box::new(CaptureInput(Arc::clone(&captured))),
            ProcessorPriority::GlobalSecondChain,
            true,
        ));

        let mut output = [0.0; 4];
        let empty_input_left = [];
        let empty_input_right = [];
        let mut empty_output_right = [];
        let mut buffers = AudioBuffers {
            inputs: [&empty_input_left, &empty_input_right],
            outputs: [&mut output, &mut empty_output_right],
            num_inputs: 0,
            num_outputs: 1,
        };
        root.process(false, 4, &mut buffers);

        assert_eq!(&*captured.lock().unwrap(), &[2.0; 4]);
        assert_eq!(buffers.outputs[0], [2.0; 4]);
    }
}
