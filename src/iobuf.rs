use std;
use std::slice;
use std::ptr;
use std::marker::PhantomData;
use std::cmp;
use std::io::Cursor;
use std::collections::VecDeque;

use bytes::Buf;
use bytes::BufMut;
use bytes::BigEndian;
use bytes::ByteOrder;

use bincode;

use mio;

use serde::Serialize;
use serde::de::DeserializeOwned;

use iovec::IoVec;

const DEFAULT_CAP: usize = 1024;
const MIN_CAP: usize = 32;
const IOVEC_MAX_LEN: usize = 128;

/// IO Buffer implementing `bytes::{Buf,BufMut}`.
pub struct IoBuffer {
    inner: VecDeque<Cursor<Vec<u8>>>,
    /// amount of data in the buffer
    remaining: usize,
}

impl IoBuffer {
    pub fn new() -> Self {
        IoBuffer {
            inner: VecDeque::new(),
            remaining: 0,
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        let mut cb = IoBuffer::new();
        cb.reserve(cap);
        cb
    }

    pub fn reserve(&mut self, cnt: usize) {
        // mutually exclusive
        let mut alloc = false;
        let mut resize = false;
        if let Some(cur) = self.inner.back_mut() {
            if cur.get_mut().capacity() - cur.get_mut().len() < cnt {
                if cur.get_mut().len() == 0 {
                    resize = true;
                } else {
                    alloc = true;
                }
            }
        } else {
            alloc = true;
        }

        if alloc {
            // add a new vec
            let mut size = MIN_CAP;
            while size < cnt {
                size <<= 1;
            }
            let new = Cursor::new(Vec::with_capacity(size));
            self.inner.push_back(new);
        } else if resize {
            // resize last vec
            let mut size = MIN_CAP;
            while size < cnt {
                size <<= 1;
            }
            self.inner.back_mut().unwrap().get_mut().reserve(size);
        }
    }

    pub fn len(&self) -> usize {
        self.remaining
    }

    pub fn writev_to(&self, stream: &mio::net::TcpStream) -> std::io::Result<usize> {
        let mut iovecs: [&IoVec; IOVEC_MAX_LEN] = unsafe { std::mem::uninitialized() };
        let cnt = cmp::min(IOVEC_MAX_LEN, self.inner.len());
        for i in 0 .. cnt {
            iovecs[i] = self.inner.get(i).unwrap().bytes().into();
        }
        stream.write_bufs(&iovecs[.. cnt])
    }

    /// Peeks the next cnt bytes if available. It doesn't modify the
    /// buffer, but may need to create a temporary vec if the
    /// data is not contiguous.
    pub fn with_peek<F, T>(&mut self, cnt: usize, mut f: F)
                           -> T
        where F: FnMut(Option<&[u8]>) -> T,
    {
        if self.remaining < cnt {
            // not enough available data
            f(None)
        } else {
            // available contiguous data
            if let Some(cur) = self.inner.front() {
                if cur.remaining() >= cnt {
                    let pos = cur.position() as usize;
                    return f(Some(&cur.get_ref()[pos..pos+cnt]));
                }
            }
            // data available but not contiguous
            if cnt <= 32 {
                // avoid allocation if peek is small
                let mut tmp = [0; 32];
                let tmp_ptr = tmp.as_mut_ptr();
                let mut off: isize = 0;
                let mut curidx: usize = 0;
                while (off as usize) < cnt {
                    let cur = self.inner.get_mut(curidx).unwrap();
                    let pos = cur.position() as isize;
                    let ptr = cur.get_mut().as_mut_ptr();
                    unsafe {
                        let to_copy = cmp::min(cnt - off as usize, cur.remaining());
                        ptr::copy_nonoverlapping(ptr.offset(pos),
                                                 tmp_ptr.offset(off),
                                                 to_copy);
                        off += to_copy as isize;
                        curidx += 1;
                    }
                }
                f(Some(&tmp[..cnt]))
            } else {
                let mut tmp = Vec::with_capacity(cnt);
                let tmp_ptr: *mut u8 = tmp.as_mut_ptr();
                let mut off: isize = 0;
                let mut curidx: usize = 0;
                while (off as usize) < cnt {
                    let cur = self.inner.get_mut(curidx).unwrap();
                    let pos = cur.position() as isize;
                    let ptr: *const u8  = cur.get_mut().as_ptr();
                    unsafe {
                        let to_copy = cmp::min(cnt - off as usize, cur.remaining());
                        ptr::copy_nonoverlapping(ptr.offset(pos),
                                                 tmp_ptr.offset(off),
                                                 to_copy);
                        off += to_copy as isize;
                        curidx += 1;
                    }
                }
                f(Some(&tmp[..cnt]))
            }
        }
    }

    pub fn put_frame_bincode<M: Serialize>(&mut self, msg: &M) -> Result<(), bincode::Error> {
        let size = bincode::serialized_size(msg)? as usize;
        // write the length header
        self.put_u32::<BigEndian>(size as u32);
        bincode::serialize_into(&mut self.writer(), msg)?;
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
    inner: &'a mut IoBuffer,
    phantom: PhantomData<M>,
}

impl<'a, M: DeserializeOwned> Iterator for BincodeFrameIterator<'a, M> {
    type Item = Result<M, bincode::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        // do we have a header?
        let hdr = self.inner.with_peek(4, |hdr| {
            hdr.map(|bytes| {
                BigEndian::read_u32(bytes) as usize
            })
        });

        if let Some(size) = hdr {
            if self.inner.remaining() >= 4 + size {
                self.inner.advance(4);
                return Some(bincode::deserialize_from(&mut self.inner.reader()));
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

impl BufMut for IoBuffer {
    fn remaining_mut(&self) -> usize {
        std::usize::MAX
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        // advancing more than what can be written in the last vec does not make sense
        if let Some(cur) = self.inner.back_mut() {
            let curvec = cur.get_mut();
            let len = curvec.len();
            let remaining_mut = curvec.capacity() - len;
            assert!(remaining_mut >= cnt);
            curvec.set_len(len + cnt);
            self.remaining += cnt;
        } else if cnt > 0 {
            panic!("not enough remaining_mut");
        }
    }

    unsafe fn bytes_mut(&mut self) -> &mut [u8] {
        let mut alloc = false;
        if let Some(cur) = self.inner.back_mut() {
            if cur.get_mut().capacity() == cur.get_mut().len() {
                alloc = true;
            }
        } else {
            alloc = true;
        }

        if alloc {
            let new = Cursor::new(Vec::with_capacity(DEFAULT_CAP));
            self.inner.push_back(new);
        }
        let backvec = self.inner.back_mut().unwrap().get_mut();
        let len = backvec.len();
        let cap = backvec.capacity();
        let ptr = backvec.as_mut_ptr();
        let slice = slice::from_raw_parts_mut(ptr, cap);
        &mut slice[len .. cap]
    }
}

impl Buf for IoBuffer {
    fn remaining(&self) -> usize {
        self.remaining
    }

    fn bytes(&self) -> &[u8] {
        if let Some(cur) = self.inner.front() {
            cur.bytes()
        } else {
            std::default::Default::default() // empty slice
        }
    }

    fn advance(&mut self, cnt: usize) {
        assert!(cnt <= self.remaining);
        self.remaining -= cnt;
        let mut adv = 0;
        while adv < cnt {
            let mut cur = self.inner.pop_front().unwrap();
            let to_adv = cmp::min(cur.remaining(), cnt - adv);
            cur.advance(to_adv);
            adv += to_adv;
            if cur.remaining() > 0 {
                assert!(adv == cnt);
                self.inner.push_front(cur);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BigEndian;

    #[test]
    fn buffer_new() {
        let mut cb = IoBuffer::new();
        assert_eq!(cb.remaining(), 0);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), DEFAULT_CAP);
        }

        let mut cb = IoBuffer::with_capacity(100);
        assert_eq!(cb.remaining(), 0);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), 128);
        }
    }

    #[test]
    fn buffer_put() {
        let mut cb = IoBuffer::new();
        cb.put_i64::<BigEndian>(42);
        cb.put_i64::<BigEndian>(123);
        cb.put_i64::<BigEndian>(-21444);
        cb.put_i64::<BigEndian>(88);
        assert_eq!(cb.remaining(), 8*4);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), DEFAULT_CAP - 8*4);
        }
        assert_eq!(cb.bytes().len(), 8*4);
        assert_eq!(cb.get_i64::<BigEndian>(), 42);
        assert_eq!(cb.get_i64::<BigEndian>(), 123);
        assert_eq!(cb.bytes().len(), 8*2);
        assert_eq!(cb.get_i64::<BigEndian>(), -21444);
        assert_eq!(cb.get_i64::<BigEndian>(), 88);
        assert_eq!(cb.bytes().len(), 0);
        unsafe {
            assert_eq!(cb.bytes_mut().len(), DEFAULT_CAP);
        }
    }

    #[test]
    fn buffer_peek() {
        let mut cb = IoBuffer::with_capacity(10);
        cb.with_peek(0, |buf| {
            assert!(buf.is_some());
        });
        cb.with_peek(1, |buf| {
            assert!(buf.is_none());
        });

        cb.put_u64::<BigEndian>(0x1122334455667788);
        cb.with_peek(8, |buf| {
            let bytes = buf.unwrap();
            assert_eq!(BigEndian::read_u64(bytes), 0x1122334455667788);
        });
        cb.with_peek(9, |buf| {
            assert!(buf.is_none());
        });

        assert_eq!(cb.get_u32::<BigEndian>(), 0x11223344); // read 4
        assert_eq!(cb.get_u16::<BigEndian>(), 0x5566); // read 2
        cb.with_peek(2, |buf| {
            let bytes = buf.unwrap();
            assert_eq!(BigEndian::read_u16(bytes), 0x7788);
        });
        cb.with_peek(3, |buf| {
            assert!(buf.is_none());
        });
        cb.put_u64::<BigEndian>(0x1122334455667788); // put 8 => buffer full and starting at 6
        cb.with_peek(10, |buf| {
            let bytes = buf.unwrap();
            assert_eq!(BigEndian::read_u16(&bytes[..2]), 0x7788);
            assert_eq!(BigEndian::read_u64(&bytes[2..10]), 0x1122334455667788);
        });
    }

    #[test]
    fn buffer_reserve() {
        let mut cb = IoBuffer::new();
        unsafe {
            cb.reserve(1);
            assert_eq!(cb.bytes_mut().len(), 32);
            cb.reserve(32);
            assert_eq!(cb.bytes_mut().len(), 32);
            cb.reserve(33);
            assert_eq!(cb.bytes_mut().len(), 64);
        }
    }

    #[test]
    fn buffer_bincode() {
        let mut cb = IoBuffer::with_capacity(3);
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
