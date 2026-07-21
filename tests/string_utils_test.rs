use freewheeling_plus::string_utils::*;

#[test]
fn token_splitting_matches_corners() {
    assert_eq!(
        split_token("a:b", b':'),
        TokenSpan {
            begin: "a:b",
            len: 1,
            next: Some("b")
        }
    );
    assert_eq!(split_token(":b", b':').next, Some("b"));
    assert_eq!(split_token("a", 0).next, None);
    assert_eq!(
        split_token("", b':'),
        TokenSpan {
            begin: "",
            len: 0,
            next: None
        }
    );
    assert_eq!(dup_token(&split_token("a:b", b':')), "a");
}

#[test]
fn bounded_operations_report_exact_truncation() {
    let mut b = [0; 4];
    assert_eq!(copy_truncate(Some(&mut b), "abcd"), 3);
    assert_eq!(&b, b"abc\0");
    assert!(copy_filename_truncate(Some(&mut b), "abcd"));
    assert_eq!(append_truncate(Some(&mut b), "z"), 3);
    let mut full = *b"wxyz";
    assert_eq!(append_truncate(Some(&mut full), "q"), 3);
    assert_eq!(&full, b"wxy\0");
}

#[test]
fn expansion_and_names_preserve_null_inputs() {
    let mut b = [0; 8];
    assert_eq!(
        expand_home_path(Some(&mut b), "~/x", "/home"),
        PathExpandResult::Ok
    );
    assert_eq!(&b[..8], b"/home/x\0");
    assert_eq!(
        expand_home_path(Some(&mut b), "~/x", ""),
        PathExpandResult::MissingHome
    );
    assert_eq!(
        alloc_saveable_stub("", "h", "", ".wav"),
        "-h.wav"
    );
    assert_eq!(
        alloc_saveable_path("", "b", "h", "o", ""),
        "/b-h-o"
    );
}
