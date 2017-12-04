use std;
use std::slice;
use std::ptr;
use std::marker::PhantomData;
use std::mem;
use std::cmp;

use bytes::Buf;
use bytes::BufMut;
use bytes::BigEndian;
use bytes::ByteOrder;

use bincode;

use serde::Serialize;
use serde::de::DeserializeOwned;

const INITIAL_CAP: usize = 1024;

/// Circular Buffer which grows on demand and implements `bytes::{Buf,BufMut}`.
pub struct CircularBuffer {
    inner: Vec<u8>,
    start: usize,
    /// amount of data in the buffer
    remaining: usize,
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
            remaining: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.remaining
    }

    pub fn capacity(&self) -> usize {
        self.inner.capacity() - self.remaining
    }

    /// Makes sure the first 'cnt' available bytes are contiguous.
    /// Returns false if there is not enough data.
    pub fn pullup(&mut self, cnt: usize) -> bool {
        if cnt > self.remaining {
            false
        } else if cnt <= self.inner.capacity() - self.start {
            // already contiguous
            true
        } else {
            // if we got here, the two halves of the circ buffer are
            // not contiguous. Shuffle data around so start is 0.
            let cap = self.inner.capacity();
            let end = (self.start + self.remaining) % cap;
            assert!(end <= self.start);
            let start_to_cap = cap - self.start;
            let mut old = mem::replace(&mut self.inner, Vec::with_capacity(cap));
            let old_ptr = old.as_mut_ptr();
            let ptr = self.inner.as_mut_ptr();
            unsafe {
                // copy start .. cap
                ptr::copy_nonoverlapping(old_ptr.offset(self.start as isize), ptr, start_to_cap);
                // copy 0 .. end
                ptr::copy_nonoverlapping(old_ptr, ptr.offset(start_to_cap as isize), end);
            }
            self.start = 0;
            true
        }
    }

    pub fn put_frame_bincode<M: Serialize>(&mut self, msg: &M) -> Result<(), bincode::Error> {
        let size = bincode::serialized_size(msg) as usize;
        // write the length header
        self.put_u32::<BigEndian>(size as u32);
        bincode::serialize_into(&mut self.writer(), msg, bincode::Bounded(size as u64))?;
        Ok(())
    }

    pub fn drain_frames_bincode<'a, M: DeserializeOwned>(&'a mut self) -> BincodeFrameIterator<'a, M> {
        BincodeFrameIterator {
            inner: self,
            phantom: PhantomData,
        }
    }
}

pub struct BincodeFrameIterator<'a, M: DeserializeOwned> {
    inner: &'a mut CircularBuffer,
    phantom: PhantomData<M>,
}

impl<'a, M: DeserializeOwned> Iterator for BincodeFrameIterator<'a, M> {
    type Item = Result<M, bincode::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        // do we have a header?
        if self.inner.pullup(4) {
            let size = {
                let hdr = &self.inner.bytes()[0..4];
                BigEndian::read_u32(hdr) as usize
            };
            // do we have the data?
            if self.inner.pullup(4 + size) {
                let res = {
                    let msg = &self.inner.bytes()[4..4+size];
                    bincode::deserialize(msg)
                };
                self.inner.advance(4+size);
                return Some(res);
            }
        }
        None
    }
}

impl<'a, M: DeserializeOwned> Drop for BincodeFrameIterator<'a, M> {
    fn drop(&mut self) {
        while let Some(_) = self.next() {}
    }
}

impl BufMut for CircularBuffer {
    fn remaining_mut(&self) -> usize {
        std::usize::MAX
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        assert!(self.inner.capacity() - self.remaining >= cnt);
        let end = (self.start + self.remaining) % self.inner.capacity();
        // it doesn't make sense to advance past whatever was
        // returned by a bytes_mut, so we assert to catch bugs
        if end >= self.start {
            assert!(end + cnt <= self.inner.capacity(),
                    "CircularBuffer advance_mut past what bytes_mut would return");
        } else {
            assert!(end + cnt <= self.start,
                    "CircularBuffer advance_mut past what bytes_mut would return");
        }
        self.remaining += cnt;
    }

    unsafe fn bytes_mut(&mut self) -> &mut [u8] {
        // check if we need to alloc more space
        if self.remaining == self.inner.capacity() {
            let cap = self.inner.capacity();
            if self.start == 0 {
                // easy, just make more space at the end
                if cap == 0 {
                    self.inner.reserve(INITIAL_CAP);
                } else {
                    self.inner.reserve(cap*2);
                }
            } else {
                let end = (self.start + self.remaining) % self.inner.capacity();
                // double the buffer and make data start at 0 again
                let mut old = mem::replace(&mut self.inner, Vec::with_capacity(cap*2));
                let old_ptr = old.as_mut_ptr();
                let ptr = self.inner.as_mut_ptr();
                // copy start .. cap
                ptr::copy_nonoverlapping(old_ptr.offset(self.start as isize),
                                         ptr,
                                         cap - self.start);
                // copy 0 .. end
                ptr::copy_nonoverlapping(old_ptr,
                                         ptr.offset((cap - self.start) as isize),
                                         end);
                self.start = 0;
            }
        }

        let end = (self.start + self.remaining) % self.inner.capacity();
        let ptr = self.inner.as_mut_ptr();
        let slice = slice::from_raw_parts_mut(ptr, self.inner.capacity());

        let up_to = if self.start <= end {
            self.inner.capacity()
        } else {
            self.start
        };

        &mut slice[end .. up_to]
    }
}

impl Buf for CircularBuffer {
    fn remaining(&self) -> usize {
        self.remaining
    }

    fn bytes(&self) -> &[u8] {
        let ptr = self.inner.as_ptr();
        let slice = unsafe { slice::from_raw_parts(ptr, self.inner.capacity()) };
        let end = cmp::min(self.start + self.remaining, self.inner.capacity());
        &slice[self.start .. end]
    }

    fn advance(&mut self, cnt: usize) {
        assert!(cnt <= self.remaining);
        self.start = (self.start + cnt) % self.inner.capacity();
        self.remaining -= cnt;
        if self.remaining == 0 {
            self.start = 0;
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

    #[test]
    fn circular_buffer_pullup() {
        let mut cb = CircularBuffer::with_capacity(10);
        assert!(cb.pullup(0));
        assert!(!cb.pullup(1));

        cb.put_u64::<BigEndian>(0x1122334455667788);
        assert!(cb.pullup(8));
        assert!(!cb.pullup(9));

        assert_eq!(cb.get_u32::<BigEndian>(), 0x11223344); // read 4
        assert_eq!(cb.get_u16::<BigEndian>(), 0x5566); // read 2
        assert!(cb.pullup(2));
        assert!(!cb.pullup(3));
        cb.put_u64::<BigEndian>(0x1122334455667788); // put 8 => buffer full and starting at 6
        // this will be broken up in two parts
        assert_eq!(cb.remaining(), 10);
        assert_eq!(cb.bytes().len(), 4);
        // make it contiguous
        assert!(cb.pullup(5));
        assert_eq!(cb.remaining(), 10);
        assert_eq!(cb.bytes().len(), 10);
        assert_eq!(cb.get_u16::<BigEndian>(), 0x7788); // read 2
        assert_eq!(cb.remaining(), 8);
        assert_eq!(cb.bytes().len(), 8);
        assert_eq!(cb.get_u64::<BigEndian>(), 0x1122334455667788);
    }

    #[test]
    fn circular_buffer_bincode() {
        let mut cb = CircularBuffer::with_capacity(3);
        let msg0 = (1u64, Some(true), "foobar".to_owned());
        let msg1 = (1234u64, Some(false), "bladsf lakds jfkjsa kfdjds".to_owned());
        cb.put_frame_bincode(&msg0).unwrap();
        cb.put_frame_bincode(&msg1).unwrap();
        let msgs: Vec<Result<(u64, Option<bool>, String), bincode::Error>> = cb.drain_frames_bincode().collect();
        assert_eq!(msgs[0].as_ref().expect("deserialization failure"),
                   &msg0);
        assert_eq!(msgs[1].as_ref().expect("deserialization failure"),
                   &msg1);
    }
}
