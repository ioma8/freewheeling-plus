use freewheeling_plus::block::*;
use freewheeling_plus::mem::Preallocated;

#[test]
fn chain_owns_samples_and_recycles() {
    let mut first = AudioBlock::new(3);
    first.samples.copy_from_slice(&[1.0, 2.0, 3.0]);
    first.link(AudioBlock::new(2));
    first
        .next
        .as_mut()
        .unwrap()
        .samples
        .copy_from_slice(&[4.0, 5.0]);
    assert_eq!(first.total_len(), 5);
    assert_eq!(first.sample(4), Some(5.0));
    first.recycle();
    assert_eq!(first.total_len(), 3);
    assert!(first.samples.iter().all(|x| *x == 0.0));
}

#[test]
fn serialization_round_trip_preserves_audio() {
    let mut source = AudioBlock::new(4);
    source.samples.copy_from_slice(&[0.25, -1.0, 2.5, 0.0]);
    let mut bytes = Vec::new();
    source.serialize(&mut bytes).unwrap();
    let restored = AudioBlock::deserialize(&mut bytes.as_slice()).unwrap();
    assert_eq!(restored.samples, source.samples);
}

#[test]
fn iterator_writes_across_chain_and_markers_wrap() {
    let mut block = AudioBlock::new(2);
    block.link(AudioBlock::new(2));
    let mut it = AudioBlockIterator::new(&mut block, 3);
    assert_eq!(it.put_fragment(&[1.0, 2.0, 3.0, 4.0]), 4);
    assert_eq!(it.block.sample(3), Some(4.0));
    let markers = MarkerPoints {
        markers: vec![
            TimeMarker { offset: 1, data: 7 },
            TimeMarker { offset: 3, data: 9 },
        ],
    };
    assert_eq!(markers.nth_before(0, 0).unwrap().data, 9);
}
