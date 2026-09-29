//! Bounded read-ahead for one-pass physical transfers.
//!
//! The producer owns the device reader. Two reusable buffers bound read-ahead
//! while the consumer hashes and writes earlier data. Dropping the consumer
//! disconnects the channels without waiting for an uncooperative device read.

use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

const BUFFER_BYTES: usize = 256 * 1024;
const BUFFERS: usize = 2;

pub(crate) struct Reader {
    available: SyncSender<Vec<u8>>,
    completed: Receiver<(Vec<u8>, io::Result<usize>)>,
    current: Option<Vec<u8>>,
    position: usize,
    valid: usize,
    finished: bool,
}

impl Reader {
    pub(crate) fn new<R: Read + Send + 'static>(mut source: R) -> io::Result<Self> {
        let (available, free) = mpsc::sync_channel(BUFFERS);
        let (filled, completed) = mpsc::sync_channel(BUFFERS);
        for _ in 0..BUFFERS {
            available
                .send(vec![0; BUFFER_BYTES])
                .expect("initial buffer channel is open");
        }
        thread::Builder::new()
            .name("ewf-source-prefetch".into())
            .spawn(move || {
                while let Ok(mut buffer) = free.recv() {
                    let result = source.read(&mut buffer);
                    let terminal = !matches!(result, Ok(count) if count > 0);
                    if filled.send((buffer, result)).is_err() || terminal {
                        break;
                    }
                }
            })?;
        Ok(Self {
            available,
            completed,
            current: None,
            position: 0,
            valid: 0,
            finished: false,
        })
    }
}

impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.finished {
            return Ok(0);
        }
        if self.position == self.valid {
            if let Some(buffer) = self.current.take() {
                // The producer may already have filled the other buffer, but
                // this bounded channel never allocates a third one.
                // The producer may have already sent EOF or an error and
                // closed its receive end. The pending completion decides the
                // result; returning this buffer is optional at that point.
                let _ = self.available.send(buffer);
            }
            let (buffer, result) = self
                .completed
                .recv()
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
            match result {
                Ok(0) => {
                    self.finished = true;
                    return Ok(0);
                }
                Ok(count) if count <= buffer.len() => {
                    self.current = Some(buffer);
                    self.position = 0;
                    self.valid = count;
                }
                Ok(_) => return Err(io::ErrorKind::InvalidData.into()),
                Err(error) => {
                    self.finished = true;
                    return Err(error);
                }
            }
        }
        let count = output.len().min(self.valid - self.position);
        output[..count].copy_from_slice(
            &self.current.as_ref().expect("active buffer")[self.position..self.position + count],
        );
        self.position += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn second_read_overlaps_consumption_of_first_buffer() {
        struct Stall {
            data: Cursor<Vec<u8>>,
            reads: usize,
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Read for Stall {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                self.reads += 1;
                if self.reads == 2 {
                    self.entered.send(()).unwrap();
                    self.release.recv().unwrap();
                }
                self.data.read(output)
            }
        }
        let bytes: Vec<u8> = (0..BUFFER_BYTES * 2).map(|n| (n % 251) as u8).collect();
        let (entered, observed) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let mut reader = Reader::new(Stall {
            data: Cursor::new(bytes.clone()),
            reads: 0,
            entered,
            release: blocked,
        })
        .unwrap();
        let mut first = vec![0; 32768];
        assert_eq!(reader.read(&mut first).unwrap(), first.len());
        assert_eq!(first, bytes[..first.len()]);
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        release.send(()).unwrap();
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, bytes[first.len()..]);
    }

    #[test]
    fn source_error_does_not_expose_failed_buffer_bytes() {
        struct FailAfterOne(bool);
        impl Read for FailAfterOne {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                if self.0 {
                    return Err(io::ErrorKind::NotConnected.into());
                }
                self.0 = true;
                output.fill(0x5a);
                Ok(output.len())
            }
        }
        let mut reader = Reader::new(FailAfterOne(false)).unwrap();
        let mut output = Vec::new();
        let error = reader.read_to_end(&mut output).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotConnected);
        assert_eq!(output, vec![0x5a; BUFFER_BYTES]);
    }
}
