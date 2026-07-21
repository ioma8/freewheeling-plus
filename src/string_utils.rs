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
    Truncated,
    MissingHome,
}

/// Byte-accurate C token span. Unlike the UTF-8 convenience wrapper below,
/// this supports every possible `char` delimiter and terminates at the first
/// NUL byte exactly like `fweelin_split_token`.
#[derive(Debug, PartialEq, Eq)]
pub struct ByteTokenSpan<'a> {
    pub begin: &'a [u8],
    pub len: usize,
    pub next: Option<&'a [u8]>,
}

fn c_string_bytes(src: &[u8]) -> &[u8] {
    &src[..src.iter().position(|byte| *byte == 0).unwrap_or(src.len())]
}

pub fn split_token_bytes(src: Option<&[u8]>, delim: u8) -> ByteTokenSpan<'_> {
    let Some(src) = src else {
        return ByteTokenSpan {
            begin: b"",
            len: 0,
            next: None,
        };
    };
    let src = c_string_bytes(src);
    let len = if delim == 0 {
        src.len()
    } else {
        src.iter()
            .position(|byte| *byte == delim)
            .unwrap_or(src.len())
    };
    ByteTokenSpan {
        begin: src,
        len,
        next: (delim != 0 && len < src.len()).then(|| &src[len + 1..]),
    }
}

pub fn dup_token_bytes(span: &ByteTokenSpan<'_>) -> Vec<u8> {
    span.begin[..span.len]
        .iter()
        .copied()
        .chain(std::iter::once(0))
        .collect()
}

pub fn split_token(src: &str, delim: u8) -> TokenSpan<'_> {
    let bytes = c_string_bytes(src.as_bytes());
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
        // Config text uses ASCII delimiters. Preserve this UTF-8 convenience
        // API without allowing an arbitrary byte delimiter to panic; callers
        // needing exact C byte semantics use `split_token_bytes`.
        next: (delim != 0 && len < bytes.len())
            .then(|| src.get(len + 1..))
            .flatten(),
    }
}

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

pub fn append_truncate_bytes(dst: Option<&mut [u8]>, src: &[u8]) -> usize {
    let Some(dst) = dst else { return 0 };
    if dst.is_empty() {
        return 0;
    }
    let mut pos = dst.iter().position(|&b| b == 0).unwrap_or(dst.len());
    if pos == dst.len() {
        dst[pos - 1] = 0;
        return pos - 1;
    }
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

pub fn expand_home_path(
    dst: Option<&mut [u8]>,
    src: &str,
    home_dir: &str,
) -> PathExpandResult {
    let Some(dst) = dst else {
        return PathExpandResult::Truncated;
    };
    if dst.is_empty() {
        return PathExpandResult::Truncated;
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
    if copied < c_string_bytes(home.as_bytes()).len() || expanded - copied < src_bytes[1..].len() {
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
    fn byte_token_helpers_match_c_delimiter_and_nul_rules() {
        let span = split_token_bytes(Some(b"one,two\0ignored"), b',');
        assert_eq!(&span.begin[..span.len], b"one");
        assert_eq!(span.next, Some(&b"two"[..]));
        assert_eq!(dup_token_bytes(&span), b"one\0");

        let non_utf8 = split_token_bytes(Some(&[0xff, b'/', 0xfe]), b'/');
        assert_eq!(dup_token_bytes(&non_utf8), vec![0xff, 0]);
    }

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
