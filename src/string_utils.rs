//! String and bounded-buffer helpers ported from `fweelin_string_utils.h`.

#[derive(Debug, PartialEq, Eq)]
pub struct TokenSpan<'a> {
    pub begin: &'a str,
    pub len: usize,
    pub next: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PathExpandResult {
    Ok,
    /// The buffer was too small; it holds a partial path.
    Truncated,
    /// `home_dir` is empty, so `~` cannot be expanded.
    MissingHome,
    /// No destination buffer (or an empty one) was supplied: an invalid
    /// argument rather than a buffer that was too small.
    InvalidBuffer,
}

fn c_string_bytes(src: &[u8]) -> &[u8] {
    &src[..src.iter().position(|byte| *byte == 0).unwrap_or(src.len())]
}

/// Split `src` at the first `delim` byte, like C's `strchr`.
///
/// `delim` is a single byte, so only an ASCII delimiter can split a token: a
/// continuation byte of a multi-byte character may occur inside it, and a
/// length that lands there is not a character boundary. A non-ASCII delimiter
/// therefore yields the whole string (`None` is documented as
/// "no more tokens").
pub fn split_token(src: &str, delim: u8) -> TokenSpan<'_> {
    let bytes = c_string_bytes(src.as_bytes());
    let delim = if delim.is_ascii() { delim } else { 0 };
    let len = if delim == 0 {
        bytes.len()
    } else {
        bytes
            .iter()
            .position(|&b| b == delim)
            .unwrap_or(bytes.len())
    };
    TokenSpan {
        begin: src,
        len,
        // `len` is a byte offset (possibly the position of a non-ASCII byte),
        // so the remainder is taken through `get` rather than slicing: a
        // delimiter that is not a character boundary yields no remainder
        // instead of panicking.
        next: (delim != 0 && len < bytes.len())
            .then(|| src.get(len + 1..))
            .flatten(),
    }
}

/// Copy a token's text.
///
/// `TokenSpan::len` is produced by [`split_token`], and only ASCII delimiters
/// are honoured, so the length is always a character boundary (it is either the
/// delimiter's byte offset or the string length); an invalid boundary yields an
/// empty string instead of panicking.
pub fn dup_token(span: &TokenSpan<'_>) -> String {
    span.begin.get(..span.len).unwrap_or("").to_owned()
}

pub fn copy_truncate_bytes(dst: Option<&mut [u8]>, src: &[u8]) -> usize {
    let Some(dst) = dst else { return 0 };
    if dst.is_empty() {
        return 0;
    }
    let bytes = c_string_bytes(src);
    let n = bytes.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&bytes[..n]);
    dst[n] = 0;
    n
}

pub fn copy_truncate(dst: Option<&mut [u8]>, src: &str) -> usize {
    copy_truncate_bytes(dst, src.as_bytes())
}

/// Append `src` after the existing C string in `dst`.
///
/// Precondition: `dst` must already contain a NUL terminator. A buffer without
/// one is left untouched (the caller's data is not overwritten) and `0` is
/// returned; the same happens for an empty or missing buffer.
pub fn append_truncate_bytes(dst: Option<&mut [u8]>, src: &[u8]) -> usize {
    let Some(dst) = dst else { return 0 };
    if dst.is_empty() {
        return 0;
    }
    // A buffer without a terminator has no append position: returning `0`
    // keeps the caller's bytes intact instead of clobbering the last one.
    let Some(mut pos) = dst.iter().position(|&b| b == 0) else {
        return 0;
    };
    let bytes = c_string_bytes(src);
    let n = bytes.len().min(dst.len() - 1 - pos);
    dst[pos..pos + n].copy_from_slice(&bytes[..n]);
    pos += n;
    dst[pos] = 0;
    pos
}

pub fn append_truncate(dst: Option<&mut [u8]>, src: &str) -> usize {
    append_truncate_bytes(dst, src.as_bytes())
}

pub fn copy_filename_truncate(dst: Option<&mut [u8]>, src: &str) -> bool {
    let copied = copy_truncate(dst, src);
    copied < c_string_bytes(src.as_bytes()).len()
}

/// Expand a leading `~` against `home_dir`.
///
/// A missing or empty destination is reported as
/// [`PathExpandResult::InvalidBuffer`]: it is an invalid argument, not a
/// buffer that was too small.
pub fn expand_home_path(
    dst: Option<&mut [u8]>,
    src: &str,
    home_dir: &str,
) -> PathExpandResult {
    let Some(dst) = dst else {
        return PathExpandResult::InvalidBuffer;
    };
    if dst.is_empty() {
        return PathExpandResult::InvalidBuffer;
    }
    let src_bytes = c_string_bytes(src.as_bytes());
    if src_bytes.first() != Some(&b'~') {
        return if copy_filename_truncate(Some(&mut *dst), src) {
            PathExpandResult::Truncated
        } else {
            PathExpandResult::Ok
        };
    }
    if home_dir.is_empty() {
        dst[0] = 0;
        return PathExpandResult::MissingHome;
    }
    let home = home_dir;
    let copied = copy_truncate_bytes(Some(&mut *dst), home.as_bytes());
    let expanded = append_truncate_bytes(Some(&mut *dst), &src_bytes[1..]);
    if copied < c_string_bytes(home.as_bytes()).len()
        || expanded.saturating_sub(copied) < src_bytes[1..].len()
    {
        PathExpandResult::Truncated
    } else {
        PathExpandResult::Ok
    }
}

pub fn alloc_saveable_stub(
    basename: &str,
    hashtext: &str,
    objname: &str,
    ext: &str,
) -> String {
    let mut out = format!("{basename}-{hashtext}");
    if !objname.is_empty() {
        out.push('-');
        out.push_str(objname);
    }
    out.push_str(ext);
    out
}

/// Join a library path and a saveable stub.
///
/// C++ `sprintf("%s/%s", ...)` inserts the separator unconditionally, so a
/// `library_path` that already ends with `/` produces a doubled separator; the
/// behaviour is kept (and pinned by a test) rather than trimmed, because saved
/// paths are compared against names written by the original implementation.
pub fn alloc_saveable_path(
    library_path: &str,
    basename: &str,
    hashtext: &str,
    objname: &str,
    ext: &str,
) -> String {
    format!(
        "{}/{}",
        library_path,
        alloc_saveable_stub(basename, hashtext, objname, ext)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_copy_append_and_home_expansion_stop_at_c_nul() {
        let mut buffer = [0xaa; 6];
        assert_eq!(copy_truncate_bytes(Some(&mut buffer), b"abc\0def"), 3);
        assert_eq!(&buffer[..4], b"abc\0");
        assert_eq!(append_truncate_bytes(Some(&mut buffer), b"XY\0z"), 5);
        assert_eq!(&buffer, b"abcXY\0");

        let mut path = [0; 12];
        assert_eq!(
            expand_home_path(Some(&mut path), "~/x", "/home/a"),
            PathExpandResult::Ok
        );
        assert_eq!(&path[..10], b"/home/a/x\0");
        assert_eq!(
            expand_home_path(Some(&mut path), "~/long", "/home/abcdef"),
            PathExpandResult::Truncated
        );
    }

    #[test]
    fn saveable_names_match_cpp_separator_rules() {
        assert_eq!(
            alloc_saveable_stub("loop", "hash", "name", ".wav"),
            "loop-hash-name.wav"
        );
        assert_eq!(
            alloc_saveable_path("", "loop", "hash", "", ".wav"),
            "/loop-hash.wav"
        );
    }
}
