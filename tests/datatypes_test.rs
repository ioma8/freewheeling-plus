use freewheeling_plus::datatypes::*;

#[test]
fn test_core_data_type_from_name() {
    assert_eq!(CoreDataType::from_name("char"), CoreDataType::Char);
    assert_eq!(CoreDataType::from_name("int"), CoreDataType::Int);
    assert_eq!(CoreDataType::from_name("long"), CoreDataType::Long);
    assert_eq!(CoreDataType::from_name("float"), CoreDataType::Float);
    assert_eq!(CoreDataType::from_name("range"), CoreDataType::Range);
    assert_eq!(CoreDataType::from_name("unknown"), CoreDataType::Invalid);
}

#[test]
fn test_range() {
    let r = Range::new(1, 10);
    assert_eq!(r.lo, 1);
    assert_eq!(r.hi, 10);
}

#[test]
fn test_user_variable_int() {
    let mut v = UserVariable::new();
    v.set_int(42);
    assert_eq!(v.get_type(), CoreDataType::Int);
    assert_eq!(v.as_i32(), 42);
}

#[test]
fn test_user_variable_float() {
    let mut v = UserVariable::new();
    v.set_float(std::f32::consts::PI);
    assert_eq!(v.get_type(), CoreDataType::Float);
    assert!((v.as_f32() - std::f32::consts::PI).abs() < 0.001);
}

#[test]
fn test_user_variable_char() {
    let mut v = UserVariable::new();
    v.set_char(65);
    assert_eq!(v.get_type(), CoreDataType::Char);
    assert_eq!(v.as_char(), 65);
}

#[test]
fn test_user_variable_range() {
    let mut v = UserVariable::new();
    v.set_range(1, 100);
    assert_eq!(v.get_type(), CoreDataType::Range);
    let r = v.as_range();
    assert_eq!(r.lo, 1);
    assert_eq!(r.hi, 100);
}

#[test]
fn test_user_variable_raise_precision() {
    let mut v = UserVariable::new();
    v.set_int(42);
    let mut src = UserVariable::new();
    src.set_float(std::f32::consts::PI);
    v.raise_precision(&src);
    assert_eq!(v.get_type(), CoreDataType::Float);
    assert!((v.as_f32() - 42.0).abs() < 0.001);
}

#[test]
fn test_user_variable_arithmetic_and_delta() {
    let mut value = UserVariable::new();
    value.set_int(10);
    let mut operand = UserVariable::new();
    operand.set_int(3);
    value.add_assign(&operand);
    value.mul_assign(&operand);
    value.sub_assign(&operand);
    assert_eq!(value.as_i32(), 36);
    value.div_assign(&operand);
    assert_eq!(value.get_type(), CoreDataType::Float);
    assert!((value.as_f32() - 12.0).abs() < f32::EPSILON);
    assert_eq!(value.get_delta(&operand).as_f32(), 9.0);
}

#[test]
fn test_ring_buffer() {
    let rb = RingBuffer::create(1024);
    assert!(rb.write_space() > 0);

    let data = b"hello";
    let written = rb.write(data, data.len());
    assert_eq!(written, data.len());

    assert!(rb.read_space() >= data.len());

    let data_len = data.len();
    let mut buf = [0u8; 10];
    let read = rb.read(&mut buf, data_len);
    assert_eq!(read, data_len);
    assert_eq!(&buf[..data_len], data);
}

#[test]
fn test_empty_ring_buffer_is_safe() {
    let rb = RingBuffer::create(0);
    let mut out = [0u8; 1];
    assert_eq!(rb.write(b"x", 1), 0);
    assert_eq!(rb.read(&mut out, 1), 0);
    assert_eq!(rb.write_space(), 0);
}

#[test]
fn test_slist() {
    let mut list = SList::new();
    let _idx1 = list.add_to_head(10);
    let idx2 = list.add_to_head(20);
    let _idx3 = list.add_to_head(30);

    // Should be 30 -> 20 -> 10
    let vals: Vec<_> = list.iter().map(|(_, c)| c.data).collect();
    assert_eq!(vals, vec![30, 20, 10]);

    assert!(list.remove(idx2));
    let vals: Vec<_> = list.iter().map(|(_, c)| c.data).collect();
    assert_eq!(vals, vec![30, 10]);
}

#[test]
fn test_rtstore() {
    let store: RTStore<i32> = RTStore::new(5);

    // All items start as Done
    let found = store.find_item_with_state(ItemState::Done, ItemState::Busy);
    assert!(found.is_some());

    let (idx, _) = found.unwrap();
    let changed = store.change_state_at_idx(idx, ItemState::Busy, ItemState::Waiting);
    assert!(changed);

    // Can't change again from the wrong state
    let changed = store.change_state_at_idx(idx, ItemState::Busy, ItemState::Done);
    assert!(!changed);
}
