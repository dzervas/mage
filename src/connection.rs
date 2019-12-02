use std::io;
use std::io::{Read, Write, Error, ErrorKind};
use stream::Stream;
use channel::Channel;
use crossbeam_channel::{Sender, Receiver, bounded as ch};
use std::collections::HashMap;
use std::borrow::BorrowMut;
use std::ops::DerefMut;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub struct Connection<'conn> {
    pub id: u32,
    stream: Stream,
    reader: &'conn mut (dyn Read + Send),
    writer: &'conn mut (dyn Write + Send),
    channels: HashMap<u8, Vec<(Sender<Vec<u8>>, Receiver<Vec<u8>>)>>
}

impl<'conn> Connection<'conn> {
    pub fn new(id: u32, reader: &'conn mut (dyn Read + Send), writer: &'conn mut (dyn Write + Send), server: bool, seed: &[u8], remote_key: &[u8]) -> Result<Self> {
        match Stream::new(server, seed, remote_key) {
            Ok(stream) => Ok(Connection {
                id,
                stream,
                reader,
                writer,
                channels: HashMap::new()
            }),
            Err(e) => Err(e)
        }
    }

    pub fn new_box(id: u32, mut box_reader: Box<dyn Read + Send>, mut box_writer: Box<dyn Write + Send>, server: bool, seed: &[u8], remote_key: &[u8]) -> Result<Self> {
        let reader = *box_reader;
        let writer = *box_writer;

        Connection::new(id, &mut *reader, &mut *writer, server, seed, remote_key)
    }

    pub fn read_all_channels(&mut self) -> Result<HashMap<u8, Vec<u8>>> {
        let mut result: HashMap<u8, Vec<u8>> = HashMap::new();
        let mut original = [0u8; 256];
        // TODO: Handle too small buffer
        let size = self.reader.read(&mut original)?;

        let packets = self.stream.dechunk(&original[..size])?;

        for p in packets {
            result.entry(p.get_channel()).or_insert(Vec::new()).extend(p.data);
        }

        Ok(result)
    }

    pub fn write_channel(&mut self, channel: u8, data: &[u8]) -> Result<usize> {
        let packets = self.stream.chunk(0, channel, data)?;
        let mut result: usize = 0;

        for p in packets {
            result += self.writer.write(p.as_slice())?;
            // Is that needed?
            self.writer.flush()?;
        }

        Ok(result)
    }

    #[allow(dead_code)]
    fn get_channel(&mut self, channel: u8) -> Channel {
        let (from_ch, to_conn) = ch(0);
        let (from_conn, to_ch) = ch(0);
        self.channels.entry(channel).or_insert(Vec::new()).push((from_conn, to_conn));
        println!("{:?}", self.channels);

        Channel {
            sender: from_ch,
            receiver: to_ch,
        }
    }

    #[allow(dead_code)]
    fn channel_loop(&mut self) -> Result<()> {
        for (k, v) in self.read_all_channels().unwrap().iter() {
            for c in self.channels.get(k).unwrap() {
                c.0.send(v.clone())?;
            }
        }

        // Maybe do this a better way?
        // Can't call write_channel inside iter cause it's already borrowed
        let mut buf: HashMap<u8, Vec<u8>> = HashMap::new();

        for (k, v) in self.channels.iter() {
            for (_, r) in v {
                match r.try_recv() {
                    Ok(d) => {
                        buf.entry(*k).or_insert(Vec::new()).append(d.to_vec().borrow_mut());
                    },
                    _ => {}
                };
            }
        }

        for (k, v) in buf.iter() {
            self.write_channel(*k, v.as_slice())?;
            self.flush()?;
        }

        Ok(())
    }
}

#[deprecated(since="0.1.0", note="Please use `read_all_channels` or `channel_loop` with `get_channel`")]
impl Read for Connection<'_> {
    fn read(&mut self, mut buf: &mut [u8]) -> io::Result<usize> {
        let dechunked = match self.read_all_channels() {
            Ok(d) => d,
            Err(e) => return Err(io::Error::new(ErrorKind::Other, e.to_string()))
        };

        let bytes = match dechunked.get(&0u8) {
            Some(d) => d,
            None => return Ok(0usize)
        };

        if bytes.len() > buf.len() {
            return Err(Error::new(ErrorKind::WouldBlock, "Buffer is too small"))
        }

        buf.write(bytes.as_slice())
    }
}

#[deprecated(since="0.1.0", note="Please use `write_channel` or `channel_loop` with `get_channel`")]
impl Write for Connection<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.write_channel(0, buf) {
            Ok(d) => Ok(d),
            Err(e) => Err(io::Error::new(ErrorKind::Other, e.to_string()))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::{BufReader, BufWriter};
    use std::borrow::BorrowMut;
    use std::thread::{sleep, spawn};
    use std::time::Duration;

    // Known keys: vec![1; 32] -> public vec![171, 47, 202, 50, 137, 131, 34, 194, 8, 251, 45, 171, 80, 72, 189, 67, 195, 85, 198, 67, 15, 88, 136, 151, 203, 87, 73, 97, 207, 169, 128, 111]
    // Known keys: vec![2; 32] -> public vec![252, 59, 51, 147, 103, 165, 34, 93, 83, 169, 45, 56, 3, 35, 175, 208, 53, 215, 129, 123, 109, 27, 228, 125, 148, 111, 107, 9, 169, 203, 220, 6]

    #[test]
    fn new() {
        let file = File::create("/tmp/mage-test").unwrap();
        let mut reader = BufReader::new(file.try_clone().unwrap());
        let mut writer = BufWriter::new(file);

        assert!(Connection::new(10, &mut reader, &mut writer, true, &[1; 32], &[2; 32]).is_ok(), "Can't create dummy connection");
        assert!(Connection::new(10, &mut reader, &mut writer, true, &[1; 31], &[2; 32]).is_err(), "Key seed is too small, must be 32 bytes");
        assert!(Connection::new(10, &mut reader, &mut writer, true, &[1; 33], &[2; 32]).is_err(), "Key seed is too big, must be 32 bytes");
        assert!(Connection::new(10, &mut reader, &mut writer, true, &[1; 32], &[2; 31]).is_err(), "Remote key is too small, must be 32 bytes");
        assert!(Connection::new(10, &mut reader, &mut writer, true, &[1; 32], &[2; 33]).is_err(), "Remote key is too big, must be 32 bytes");
//        assert!(Connection::new(0x1FFFFFF, &mut reader, &mut writer, true, &[1; 32], &[2; 32]).is_err(), "ID is longer than 3 bytes");
        assert!(Connection::new(0xFFFFFF, &mut reader, &mut writer, true, &[1; 32], &[2; 32]).is_ok(), "Can't create dummy connection");
        assert!(Connection::new(0xFF, &mut reader, &mut writer, true, &[1; 32], &[2; 32]).is_ok(), "Can't create dummy connection");
        assert!(Connection::new(0, &mut reader, &mut writer, true, &[1; 32], &[2; 32]).is_ok(), "Can't create dummy connection");
    }

    #[test]
    fn read_write() {
        let file = OpenOptions::new().read(true).write(true).create(true).open("/tmp/mage-test").unwrap();
        let mut reader = BufReader::new(file.try_clone().unwrap());
        let mut writer = BufWriter::new(file);
        let mut conn = Connection::new(0xFFFF, &mut reader, &mut writer, false, &[1; 32], &[252, 59, 51, 147, 103, 165, 34, 93, 83, 169, 45, 56, 3, 35, 175, 208, 53, 215, 129, 123, 109, 27, 228, 125, 148, 111, 107, 9, 169, 203, 220, 6]).unwrap();

        let file2 = OpenOptions::new().read(true).write(true).open("/tmp/mage-test").unwrap();
        let mut reader2 = BufReader::new(file2.try_clone().unwrap());
        let mut writer2 = BufWriter::new(file2);
        let mut conn2 = Connection::new(0xFFFF, &mut reader2, &mut writer2, true, &[2; 32], &[171, 47, 202, 50, 137, 131, 34, 194, 8, 251, 45, 171, 80, 72, 189, 67, 195, 85, 198, 67, 15, 88, 136, 151, 203, 87, 73, 97, 207, 169, 128, 111]).unwrap();

        test_rw(true, conn.borrow_mut(), conn2.borrow_mut(), &[7; 100]);
        test_rw(true, conn2.borrow_mut(), conn.borrow_mut(), &[7; 100]);
        test_rw(true, conn.borrow_mut(), conn2.borrow_mut(), &[7; 1]);
        test_rw(true, conn2.borrow_mut(), conn.borrow_mut(), &[7; 1]);
        // TODO: Find a way to keep the connection open even after error
//        test_rw(false, conn.borrow_mut(), conn2.borrow_mut(), &[7; 100000]);
//        test_rw(false, conn2.borrow_mut(), conn.borrow_mut(), &[7; 100000]);

        // Channels
        println!("Channels:");

        let mut chan = conn.get_channel(4);
        let mut chan_other = conn.get_channel(0xF);

        let mut chan2 = conn2.get_channel(4);
        let mut chan2_other = conn2.get_channel(0xF);

        let thread = spawn(move || {
            test_rw(true, chan.borrow_mut(), chan2.borrow_mut(), &[7; 100]);
            test_rw(true, chan2.borrow_mut(), chan.borrow_mut(), &[7; 100]);
            test_rw(true, chan_other.borrow_mut(), chan2_other.borrow_mut(), &[7; 100]);
            test_rw(true, chan2_other.borrow_mut(), chan_other.borrow_mut(), &[7; 100]);
            // TODO: Find a way to keep the channel open even after error
//            test_rw(false, chan_other.borrow_mut(), chan2_other.borrow_mut(), &[7; 100000]);
//            test_rw(false, chan2_other.borrow_mut(), chan_other.borrow_mut(), &[7; 100000]);
            // This blocks
//            test_rw(false, chan.borrow_mut(), chan2_other.borrow_mut(), &[7; 100]);
//            test_rw(false, chan2_other.borrow_mut(), chan.borrow_mut(), &[7; 100]);
        });

        // I see no other way than sleep.
        // channel_loop is non-blocking (should be) and the test
        // has to end at some point
        for _ in 0..6 {
            sleep(Duration::from_millis(100));
            conn.channel_loop().unwrap();
            sleep(Duration::from_millis(100));
            conn2.channel_loop().unwrap();
        }

        thread.join().unwrap();
    }

    #[cfg_attr(tarpaulin, skip)]
    fn test_rw(succ: bool, a: &mut impl Write, b: &mut impl Read, data: &[u8]) {
        let mut buf = [0u8; 2048];

        // Should be always ok to write & flush
        a.write(data).unwrap();
        a.flush().unwrap();
        let r = b.read(&mut buf);

        assert_eq!(r.is_ok(), succ);
        if succ { assert_eq!(&buf[..r.unwrap()], data); }
//        else { assert_ne!(&buf[..r.unwrap()], data); }
    }
}
