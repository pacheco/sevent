use std;
use std::io::Cursor;
use std::marker::PhantomData;

use serde::Serialize;
use serde::de::DeserializeOwned;
use bincode;

use bytes::ByteOrder;
use bytes::BigEndian;
use bytes::BufMut;

use ::Error;

pub trait VecExt {
    fn put_frame_bincode<M: Serialize>(&mut self, msg: &M) -> Result<(), Error>;
    fn drain_frames_bincode<M: DeserializeOwned>(&mut self) -> BincodeFrameIterator<M>;
}

impl VecExt for Vec<u8> {
    fn put_frame_bincode<M: Serialize>(&mut self, msg: &M) -> Result<(), Error> {
        let size = bincode::serialized_size(msg)? as usize;
        // reserve space for the msg and header
        self.reserve(size + 4);
        // write the length header
        self.put_u32::<BigEndian>(size as u32);
        // extend the vector and serialize the message
        let new_len = self.len() + size;
        unsafe { self.set_len(new_len); }
        let mut cursor = Cursor::new(&mut self[new_len - size .. new_len]);
        bincode::serialize_into(&mut cursor, msg)?;
        Ok(())
    }

    fn drain_frames_bincode<M: DeserializeOwned>(&mut self) -> BincodeFrameIterator<M> {
        BincodeFrameIterator {
            inner: self,
            pos: 0,
            phantom: PhantomData,
        }
    }
}

pub struct BincodeFrameIterator<'a, M: DeserializeOwned> {
    inner: &'a mut Vec<u8>,
    pos: usize,
    phantom: PhantomData<M>,
}

impl<'a, M: DeserializeOwned> Iterator for BincodeFrameIterator<'a, M> {
    type Item = Result<M, bincode::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        // do we have a header?
        if self.pos + 4 > self.inner.len() { return None };
        let size = BigEndian::read_u32(&self.inner[self.pos..]) as usize;
        // do we have the data?
        if self.pos + 4 + size > self.inner.len() { return None };
        let res = bincode::deserialize(&self.inner[self.pos + 4 .. self.pos + 4 + size]);
        self.pos += 4 + size;
        Some(res)
    }
}

impl<'a, M: DeserializeOwned> Drop for BincodeFrameIterator<'a, M> {
    fn drop(&mut self) {
        self.inner.drain(..self.pos);
    }
}

pub trait DurationExt {
    fn as_nanosecs(&self) -> u64;
    fn as_usecs(&self) -> u64;
}

impl DurationExt for std::time::Duration {
    fn as_nanosecs(&self) -> u64 {
        self.as_secs()*1_000_000_000 + u64::from(self.subsec_nanos())
    }

    fn as_usecs(&self) -> u64 {
        self.as_secs()*1_000_000 + u64::from(self.subsec_nanos())/1000
    }
}
