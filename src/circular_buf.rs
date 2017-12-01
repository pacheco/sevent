use std;
use std::slice;
use std::ptr;

use bytes::Buf;
use bytes::BufMut;
use bytes::BigEndian;

use bincode;

use serde::Serialize;

const INITIAL_CAP: usize = 1024;

/// Circular Buffer which grows on demand and implements `bytes::{Buf,BufMut}`.
pub struct CircularBuffer {
    inner: Vec<u8>,
    start: usize,
    end: usize,
}

impl CircularBuffer {
    pub fn new() -> Self {
        CircularBuffer::with_capacity(0)
    }

    pub fn with_capacity(cap: usize) -> Self {
        let inner = Vec::with_capacity(cap);
        CircularBuffer {
            inner,
            start: 0,
            end: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.remaining()
    }

    pub fn capacity(&self) -> usize {
        self.inner.capacity() - self.remaining()
    }

    pub fn put_frame_bincode<M: Serialize>(&mut self, msg: &M) -> Result<(), bincode::Error> {
        let size = bincode::serialized_size(msg) as usize;
        // write the length header
        self.put_u32::<BigEndian>(size as u32);
        bincode::serialize_into(&mut self.writer(), msg, bincode::Bounded(size as u64))?;
        Ok(())
    }
}

impl BufMut for CircularBuffer {
    fn remaining_mut(&self) -> usize {
        std::usize::MAX
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        // it doesn't make sense to advance past whatever was
        // returned by a bytes_mut, so we panic without
        // considering remaining_mut to catch bugs
        if self.end >= self.start {
            assert!(self.end + cnt <= self.inner.capacity() + self.start,
                    "CircularBuffer advance_mut past what was returned by bytes_mut");
            self.end = self.end + cnt;
        } else {
            assert!(self.end + cnt <= self.start,
                    "CircularBuffer advance_mut past what was returned by bytes_mut");
            self.end += cnt;
        }
    }

    unsafe fn bytes_mut(&mut self) -> &mut [u8] {
        if self.end >= self.start {
            if self.end == self.inner.capacity() {
                if self.start == 0 {
                    // simply extend the vec
                    let cap = self.inner.capacity();
                    if cap == 0 {
                        self.inner.reserve(INITIAL_CAP);
                    } else {
                        self.inner.reserve(cap*2);
                    }
                } else {
                    // return space at the start of the buffer
                    self.end = 0;
                    let ptr = self.inner.as_mut_ptr();
                    let slice = slice::from_raw_parts_mut(ptr, self.inner.capacity());
                    return &mut slice[.. self.start]
                }
            } else if self.end == self.start && self.start > 0 {
                // end and start meet in the middle of the vec. Extend and shuffle data so start is again at 0
                let cap = self.inner.capacity();
                self.inner.reserve(cap*2);
                let ptr = self.inner.as_mut_ptr();
                ptr::copy(ptr, ptr.offset(cap as isize), self.start);
                ptr::copy(ptr.offset(self.start as isize), ptr, cap);
                self.start = 0;
                self.end = cap;
            }
            let ptr = self.inner.as_mut_ptr();
            let slice = slice::from_raw_parts_mut(ptr, self.inner.capacity());
            &mut slice[self.end ..]
        } else {
            let ptr = self.inner.as_mut_ptr();
            let slice = slice::from_raw_parts_mut(ptr, self.inner.capacity());
            &mut slice[self.end .. self.start]
        }
    }
}

impl Buf for CircularBuffer {
    fn remaining(&self) -> usize {
        if self.end >= self.start {
            self.end - self.start
        } else {
            self.end + self.inner.capacity() - self.start
        }
    }

    fn bytes(&self) -> &[u8] {
        if self.end >= self.start {
            let ptr = self.inner.as_ptr();
            let slice = unsafe { slice::from_raw_parts(ptr, self.inner.capacity()) };
            &slice[self.start .. self.end]
        } else {
            let ptr = self.inner.as_ptr();
            let slice = unsafe { slice::from_raw_parts(ptr, self.inner.capacity()) };
            &slice[self.start ..]
        }
    }

    fn advance(&mut self, cnt: usize) {
        assert!(cnt <= self.remaining());
        if self.end >= self.start {
            self.start += cnt;
        } else {
            self.start = (self.start + cnt) % self.inner.capacity();
        }
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BigEndian;

    #[test]
    fn circular_buffer_new() {
        let mut cb = CircularBuffer::new();
        assert_eq!(cb.remaining(), 0);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), INITIAL_CAP);
        }

        let mut cb = CircularBuffer::with_capacity(100);
        assert_eq!(cb.remaining(), 0);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), 100);
        }
    }

    #[test]
    fn circular_buffer_put() {
        let mut cb = CircularBuffer::new();
        cb.put_i64::<BigEndian>(42);
        cb.put_i64::<BigEndian>(123);
        cb.put_i64::<BigEndian>(-21444);
        cb.put_i64::<BigEndian>(88);
        assert_eq!(cb.remaining(), 8*4);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), INITIAL_CAP - 8*4);
        }
        assert_eq!(cb.bytes().len(), 8*4);
        assert_eq!(cb.get_i64::<BigEndian>(), 42);
        assert_eq!(cb.get_i64::<BigEndian>(), 123);
        assert_eq!(cb.bytes().len(), 8*2);
        assert_eq!(cb.get_i64::<BigEndian>(), -21444);
        assert_eq!(cb.get_i64::<BigEndian>(), 88);
        assert_eq!(cb.bytes().len(), 0);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), INITIAL_CAP);
        }
    }

    #[test]
    fn circular_buffer_doubles_capacity() {
        let mut cb = CircularBuffer::with_capacity(10);
        cb.put_i64::<BigEndian>(42);
        cb.put_i64::<BigEndian>(32);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), 20 - 8*2);
        }
        cb.put_i64::<BigEndian>(12);
        assert_eq!(cb.remaining(), 8*3);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), 40 - 8*3);
        }
        assert_eq!(cb.get_i64::<BigEndian>(), 42);
        assert_eq!(cb.get_i64::<BigEndian>(), 32);
        assert_eq!(cb.get_i64::<BigEndian>(), 12);
    }

    #[test]
    fn circular_buffer_start_after_end() {
        let mut cb = CircularBuffer::with_capacity(10);
        cb.put_i32::<BigEndian>(42); // advance end by 4;
        unsafe {
            assert_eq!(cb.bytes_mut().len(), 6);
        }
        assert_eq!(cb.get_i32::<BigEndian>(), 42); // start and end meet, both reset to 0
        unsafe {
            assert_eq!(cb.bytes_mut().len(), 10);
        }
        cb.put_i32::<BigEndian>(42); // advance end by 4;
        // advance start by 3. There is still space for an i64, but it is not contiguous
        cb.get_i16::<BigEndian>();
        cb.get_i8();
        unsafe {
            assert_eq!(cb.bytes_mut().len(), 6);
        }
        cb.put_i64::<BigEndian>(33);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), 1);
        }
        assert_eq!(cb.remaining(), 9);
        cb.get_i8();
        assert_eq!(cb.get_i64::<BigEndian>(), 33);
        unsafe {
            // buffer should be reset since start/end have met
            assert_eq!(cb.bytes_mut().len(), 10);
        }
    }
}
