use freewheeling_plus::block::*;
use freewheeling_plus::mem::Preallocated;
use std::io;

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
fn serialization_round_trip_preserves_stereo_and_chain() {
    let mut source = AudioBlock::new(2);
    source.samples.copy_from_slice(&[1.0, 2.0]);
    source.extra = Some(ExtraChannel {
        samples: vec![-1.0, -2.0],
    });
    let mut tail = AudioBlock::new(1);
    tail.samples[0] = 3.0;
    tail.extra = Some(ExtraChannel {
        samples: vec![-3.0],
    });
    source.link(tail);

    let mut bytes = Vec::new();
    source.serialize(&mut bytes).unwrap();
    let restored = AudioBlock::deserialize(&mut bytes.as_slice()).unwrap();
    assert_eq!(restored.samples, vec![1.0, 2.0]);
    assert_eq!(restored.extra.as_ref().unwrap().samples, vec![-1.0, -2.0]);
    let restored_tail = restored.next.as_deref().unwrap();
    assert_eq!(restored_tail.samples, vec![3.0]);
    assert_eq!(restored_tail.extra.as_ref().unwrap().samples, vec![-3.0]);
}

#[test]
fn serialization_rejects_an_absurd_chain_length() {
    let mut bytes = b"FWB2".to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.push(0);
    let error = AudioBlock::deserialize(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn subchain_generation_rejects_an_out_of_range_start() {
    let block = AudioBlock::new(3);
    assert_eq!(
        block.generate_subchain(2, 5, false).unwrap().samples.len(),
        1
    );
    assert!(block.generate_subchain(4, 5, false).is_none());
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
    // A position before every marker has no marker at or before it: the search
    // must not wrap around to the end of the loop.
    assert_eq!(markers.nth_before(0, 0), None);
    assert_eq!(markers.nth_before(0, 1).unwrap().data, 7);
    assert_eq!(markers.nth_before(1, 3).unwrap().data, 7);
    assert_eq!(markers.nth_before(2, 3), None);
}
