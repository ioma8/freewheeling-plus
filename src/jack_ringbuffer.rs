//! Safe, single-owner equivalent of the bundled `jack/ringbuffer.h` API.
//!
//! As in JACK, one byte is kept unused so equal read and write positions mean
//! empty rather than full.  The vector methods expose the two contiguous parts
//! of the buffer; callers must use the matching advance method afterwards.

#[derive(Debug, PartialEq, Eq)]
pub struct RingBufferData<'a> {
    pub buf: &'a [u8],
    pub len: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RingBufferDataMut<'a> {
    pub buf: &'a mut [u8],
    pub len: usize,
}

pub struct RingBuffer {
    buf: Vec<u8>,
    read_pos: usize,
    write_pos: usize,
}

impl RingBuffer {
    pub fn create(size: usize) -> Self {
        Self {
            buf: vec![0; size],
            read_pos: 0,
            write_pos: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn read_space(&self) -> usize {
        let size = self.buf.len();
        if size == 0 {
            return 0;
        }
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            size - self.read_pos + self.write_pos
        }
    }

    pub fn write_space(&self) -> usize {
        self.buf.len().saturating_sub(self.read_space() + 1)
    }

    pub fn reset(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
    }

    pub fn write(&mut self, src: &[u8], count: usize) -> usize {
        let count = count.min(src.len()).min(self.write_space());
        if count == 0 {
            return 0;
        }
        let vectors = self.write_vector();
        let first = vectors[0].len.min(count);
        vectors[0].buf[..first].copy_from_slice(&src[..first]);
        if count > first {
            vectors[1].buf[..count - first].copy_from_slice(&src[first..count]);
        }
        self.advance_write(count);
        count
    }

    pub fn read(&mut self, dst: &mut [u8], count: usize) -> usize {
        let count = count.min(dst.len()).min(self.read_space());
        if count == 0 {
            return 0;
        }
        let vectors = self.read_vector();
        let first = vectors[0].len.min(count);
        dst[..first].copy_from_slice(&vectors[0].buf[..first]);
        if count > first {
            dst[first..count].copy_from_slice(&vectors[1].buf[..count - first]);
        }
        self.advance_read(count);
        count
    }

    pub fn read_vector(&self) -> [RingBufferData<'_>; 2] {
        let (first, second) = self.read_lengths();
        [
            RingBufferData {
                buf: &self.buf[self.read_pos..self.read_pos + first],
                len: first,
            },
            RingBufferData {
                buf: &self.buf[..second],
                len: second,
            },
        ]
    }

    pub fn write_vector(&mut self) -> [RingBufferDataMut<'_>; 2] {
        let (first, second) = self.write_lengths();
        let (left, right) = self.buf.split_at_mut(self.write_pos);
        let second = second.min(left.len());
        [
            RingBufferDataMut {
                buf: &mut right[..first],
                len: first,
            },
            RingBufferDataMut {
                buf: &mut left[..second],
                len: second,
            },
        ]
    }

    pub fn advance_read(&mut self, count: usize) {
        self.read_pos = self.advance(self.read_pos, count.min(self.read_space()));
    }
    pub fn advance_write(&mut self, count: usize) {
        self.write_pos = self.advance(self.write_pos, count.min(self.write_space()));
    }

    pub fn mlock(&self) -> bool {
        if self.buf.is_empty() {
            return true;
        }
        unsafe { libc::mlock(self.buf.as_ptr().cast(), self.buf.len()) == 0 }
    }

    fn advance(&self, pos: usize, count: usize) -> usize {
        if self.buf.is_empty() {
            0
        } else {
            (pos + count) % self.buf.len()
        }
    }
    fn read_lengths(&self) -> (usize, usize) {
        let n = self.read_space();
        let first = n.min(self.buf.len() - self.read_pos);
        (first, n - first)
    }
    fn write_lengths(&self) -> (usize, usize) {
        let n = self.write_space();
        let first = n.min(self.buf.len() - self.write_pos);
        (first, n - first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_and_wraparound() {
        let mut rb = RingBuffer::create(5);
        assert_eq!(rb.write(b"abcdx", 5), 4);
        let mut out = [0; 2];
        assert_eq!(rb.read(&mut out, 2), 2);
        assert_eq!(&out, b"ab");
        assert_eq!(rb.write(b"XY", 2), 2);
        let mut all = [0; 4];
        assert_eq!(rb.read(&mut all, 4), 4);
        assert_eq!(&all, b"cdXY");
    }

    #[test]
    fn vectors_advance_reset_and_empty_sizes() {
        let mut rb = RingBuffer::create(4);
        assert_eq!(rb.write_vector()[0].len, 3);
        rb.advance_write(2);
        assert_eq!(rb.read_vector()[0].buf, b"\0\0");
        rb.reset();
        assert_eq!(rb.read_space(), 0);
        for size in [0, 1] {
            let mut x = RingBuffer::create(size);
            assert_eq!(x.write(b"x", 1), 0);
            assert!(x.mlock());
        }
    }
}
