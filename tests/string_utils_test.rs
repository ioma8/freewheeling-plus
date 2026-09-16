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
    // A multi-byte character before the delimiter must not truncate the token:
    // the reported length has to stay on a character boundary.
    assert_eq!(dup_token(&split_token("héllo:rest", b':')), "héllo");
    // Only a single-byte delimiter can split: a continuation byte (0xC3 in
    // "héllo") is not a delimiter, so the whole string is one token.
    assert_eq!(split_token("héllo", 0xc3).next, None);
    assert_eq!(dup_token(&split_token("héllo", 0xc3)), "héllo");
}

#[test]
fn saveable_paths_keep_the_cpp_separator_rules() {
    // `sprintf("%s/%s")` inserts the separator unconditionally: a library path
    // that already ends with `/` yields a doubled one. The behaviour is pinned
    // so saved names stay byte-compatible with the original implementation.
    assert_eq!(
        alloc_saveable_path("lib/", "loop", "hash", "", ".wav"),
        "lib//loop-hash.wav"
    );
    assert_eq!(
        alloc_saveable_path("lib", "loop", "hash", "name", ".wav"),
        "lib/loop-hash-name.wav"
    );
}

#[test]
fn bounded_operations_report_exact_truncation() {
    let mut b = [0; 4];
    assert_eq!(copy_truncate(Some(&mut b), "abcd"), 3);
    assert_eq!(&b, b"abc\0");
    assert!(copy_filename_truncate(Some(&mut b), "abcd"));
    assert_eq!(append_truncate(Some(&mut b), "z"), 3);
    // A buffer without a NUL terminator has no append position: it is left
    // untouched instead of losing the caller's last byte.
    let mut full = *b"wxyz";
    assert_eq!(append_truncate(Some(&mut full), "q"), 0);
    assert_eq!(&full, b"wxyz");
    // A missing or empty destination is an invalid argument, not a truncation.
    assert_eq!(
        expand_home_path(None, "~/x", "/home"),
        PathExpandResult::InvalidBuffer
    );
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
