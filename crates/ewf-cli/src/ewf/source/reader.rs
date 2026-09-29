//! One outstanding source operation, with owned storage and no output access.
//!
//! A stopped worker is never reused. If the kernel ignores cancellation, its
//! handle and buffer stay owned by that worker until the operation returns.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{DeviceBuffer, READ_BLOCK_BYTES};

const POLL: Duration = Duration::from_millis(20);

#[cfg(test)]
mod tests;

pub(super) struct Reader {
    requests: Option<SyncSender<(u64, usize, Box<DeviceBuffer>)>>,
    replies: Receiver<(Box<DeviceBuffer>, io::Result<usize>)>,
    buffer: Option<Box<DeviceBuffer>>,
    thread: JoinHandle<()>,
    halt: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    timeout: Option<Duration>,
    failure: Option<io::ErrorKind>,
}

impl Reader {
    pub fn new<R: Read + Seek + Send + 'static>(
        mut source: R,
        stop: Arc<AtomicBool>,
        timeout: Option<Duration>,
    ) -> io::Result<Self> {
        let (requests, pending) = mpsc::sync_channel::<(u64, usize, Box<DeviceBuffer>)>(1);
        let (completed, replies) = mpsc::sync_channel(1);
        let halt = Arc::new(AtomicBool::new(false));
        let halted = Arc::clone(&halt);
        let thread = thread::Builder::new()
            .name("ewf-source-read".into())
            .spawn(move || {
                while let Ok((offset, length, mut buffer)) = pending.recv() {
                    if halted.load(Ordering::Acquire) {
                        break;
                    }
                    let result = (|| {
                        let position = source.seek(SeekFrom::Start(offset)).map_err(|error| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                format!("source seek failed: {error}"),
                            )
                        })?;
                        if position != offset {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "source seek returned a different offset",
                            ));
                        }
                        if halted.load(Ordering::Acquire) {
                            return Err(io::ErrorKind::Interrupted.into());
                        }
                        let count = source.read(&mut buffer.0[..length])?;
                        if count > length {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "source returned an invalid read length",
                            ));
                        }
                        Ok(count)
                    })();
                    if halted.load(Ordering::Acquire) || completed.send((buffer, result)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests: Some(requests),
            replies,
            buffer: Some(Box::new(DeviceBuffer([0; READ_BLOCK_BYTES]))),
            thread,
            halt,
            stop,
            timeout,
            failure: None,
        })
    }

    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(kind) = self.failure {
            return Err(kind.into());
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let started = Instant::now();
        self.check_stop(started)?;
        let length = buffer.len().min(READ_BLOCK_BYTES);
        let storage = self.buffer.take().expect("healthy reader owns its buffer");
        if self
            .requests
            .as_ref()
            .expect("healthy reader has sender")
            .send((offset, length, storage))
            .is_err()
        {
            self.failure = Some(io::ErrorKind::BrokenPipe);
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        loop {
            self.check_stop(started)?;
            let wait = self.timeout.map_or(POLL, |limit| {
                POLL.min(limit.saturating_sub(started.elapsed()))
            });
            match self.replies.recv_timeout(wait) {
                Ok((storage, result)) => {
                    // Stop/deadline wins over a completion observed after it.
                    self.check_stop(started)?;
                    self.buffer = Some(storage);
                    let count = result?;
                    buffer[..count].copy_from_slice(&self.buffer.as_ref().unwrap().0[..count]);
                    return Ok(count);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.failure = Some(io::ErrorKind::BrokenPipe);
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
            }
        }
    }

    fn check_stop(&mut self, started: Instant) -> io::Result<()> {
        let reason = if self.stop.load(Ordering::Relaxed) {
            Some((io::ErrorKind::Interrupted, "source read cancelled"))
        } else if self.timeout.is_some_and(|limit| started.elapsed() >= limit) {
            Some((io::ErrorKind::TimedOut, "source read deadline expired"))
        } else {
            None
        };
        if let Some((kind, message)) = reason {
            self.failure = Some(kind);
            self.shutdown();
            return Err(io::Error::new(kind, message));
        }
        Ok(())
    }

    pub fn failure(&self) -> Option<io::ErrorKind> {
        self.failure
    }

    fn shutdown(&mut self) {
        self.halt.store(true, Ordering::Release);
        self.requests.take();
        // Never wait for a driver here. The worker owns all memory used by I/O.
        if !self.thread.is_finished() {
            #[cfg(windows)]
            super::windows::cancel_read(&self.thread);
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.shutdown();
    }
}
