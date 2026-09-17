use super::*;
use std::io::Cursor;
use std::ops::ControlFlow;

struct Stalled {
    data: Cursor<Vec<u8>>,
    entered: mpsc::Sender<()>,
    release: Receiver<()>,
    seek: bool,
}

impl Stalled {
    fn wait(&self) {
        self.entered.send(()).unwrap();
        let _ = self.release.recv();
    }
}

impl Read for Stalled {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.seek && self.data.position() >= 1024 {
            self.wait();
        }
        self.data.read(buffer)
    }
}

impl Seek for Stalled {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        if self.seek && offset == SeekFrom::Start(1024) {
            self.wait();
        }
        self.data.seek(offset)
    }
}

fn wait_finished(reader: &Reader) {
    let start = Instant::now();
    while !reader.thread.is_finished() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "read worker did not retire"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn stalled_seek_and_read_stop_without_late_data_and_resume() {
    use super::super::Source;
    use ewf_image::{
        AcquisitionOptions, AcquisitionReadOptions, AcquisitionStatus, AcquisitionWriter, Image,
        UnreadableSectorPolicy,
    };
    for seek in [false, true] {
        for cancel in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let output = directory.path().join("case.E01");
            let raw = directory.path().join("source.raw");
            let bytes: Vec<u8> = (0..4096).map(|n| (n % 251) as u8).collect();
            std::fs::write(&raw, &bytes).unwrap();
            let mut source = Source::open(&raw, Some(512), &output).unwrap();
            let identity = source.identity.fingerprint().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let (entered, observed) = mpsc::channel();
            let (release, held) = mpsc::channel();
            source.reader = Some(
                Reader::new(
                    Stalled {
                        data: Cursor::new(bytes.clone()),
                        entered,
                        release: held,
                        seek,
                    },
                    Arc::clone(&stop),
                    (!cancel).then_some(Duration::from_millis(500)),
                )
                .unwrap(),
            );
            let signal = Arc::clone(&stop);
            let controller = thread::spawn(move || {
                observed.recv_timeout(Duration::from_secs(5)).unwrap();
                if cancel {
                    signal.store(true, Ordering::Relaxed);
                }
            });
            let options = AcquisitionOptions {
                sectors_per_chunk: 1,
                chunks_per_segment: 2,
                ..AcquisitionOptions::new(4096)
            };
            let mut writer = AcquisitionWriter::create(&output, &options, identity).unwrap();
            let read_options = AcquisitionReadOptions {
                retries: 100,
                unreadable_sector_policy: UnreadableSectorPolicy::ZeroFill,
                ..AcquisitionReadOptions::default()
            };
            let result = writer.acquire_with_progress(&mut source, &read_options, |_| {
                if stop.load(Ordering::Relaxed) {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            });
            controller.join().unwrap();
            if cancel {
                assert_eq!(result.unwrap().status, AcquisitionStatus::Cancelled);
            } else {
                assert!(
                    matches!(result, Err(ewf_image::EwfError::Io(ref e)) if e.kind() == io::ErrorKind::TimedOut),
                    "seek={seek}: {result:?}"
                );
            }
            assert_eq!(writer.position(), 1024);
            assert_eq!(writer.checkpoint_offset(), 1024);
            assert!(writer.acquisition_errors().is_empty());
            let mut untouched = [0xCC; 512];
            assert!(source.read(&mut untouched).is_err());
            assert_eq!(untouched, [0xCC; 512]);
            // Even after the worker returns successful late data, this source
            // stays retired and cannot feed a subsequent attempt or output.
            release.send(()).unwrap();
            wait_finished(source.reader.as_ref().unwrap());
            assert!(source.read(&mut untouched).is_err());
            assert_eq!(untouched, [0xCC; 512]);
            drop(source);
            drop(writer);
            let checkpoint =
                AcquisitionWriter::validate_checkpoint(&output, &options, identity, |_| {
                    ControlFlow::Continue(())
                })
                .unwrap();
            assert_eq!(checkpoint.checkpoint_bytes, 1024);
            let mut writer = AcquisitionWriter::resume(&output, &options, identity).unwrap();
            let mut source = Source::open(&raw, Some(512), &output).unwrap();
            source
                .configure_reads(Arc::new(AtomicBool::new(false)), None)
                .unwrap();
            writer.acquire_from(&mut source, &read_options).unwrap();
            writer.finish().unwrap();
            let image = Image::open(&output).unwrap();
            let mut actual = Vec::new();
            image.cursor().read_to_end(&mut actual).unwrap();
            assert_eq!(actual, bytes);
            assert!(image.verify().unwrap().sha256_match.unwrap());
        }
    }
}

#[test]
fn cancellation_wins_over_successful_completion() {
    struct CancelOnRead {
        stop: Arc<AtomicBool>,
        data: Cursor<Vec<u8>>,
    }
    impl Read for CancelOnRead {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let result = self.data.read(buffer);
            self.stop.store(true, Ordering::Relaxed);
            result
        }
    }
    impl Seek for CancelOnRead {
        fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
            self.data.seek(offset)
        }
    }
    for _ in 0..20 {
        let stop = Arc::new(AtomicBool::new(false));
        let mut reader = Reader::new(
            CancelOnRead {
                stop: Arc::clone(&stop),
                data: Cursor::new(vec![42; 512]),
            },
            stop,
            None,
        )
        .unwrap();
        let mut buffer = [0xCC; 512];
        assert_eq!(
            reader.read_at(0, &mut buffer).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(buffer, [0xCC; 512]);
        wait_finished(&reader);
    }
}

#[test]
fn real_pending_pipe_read_can_be_abandoned_safely() {
    struct Pipe(std::io::PipeReader, mpsc::Sender<()>);
    impl Read for Pipe {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.1.send(()).unwrap();
            self.0.read(buffer)
        }
    }
    impl Seek for Pipe {
        fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
            match offset {
                SeekFrom::Start(offset) => Ok(offset),
                _ => unreachable!(),
            }
        }
    }
    for cancel in [false, true] {
        let (input, output) = std::io::pipe().unwrap();
        let (entered, observed) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let controller = thread::spawn(move || {
            observed.recv_timeout(Duration::from_secs(5)).unwrap();
            thread::sleep(Duration::from_millis(50));
            if cancel {
                signal.store(true, Ordering::Relaxed);
            }
        });
        let mut reader = Reader::new(
            Pipe(input, entered),
            stop,
            (!cancel).then_some(Duration::from_millis(500)),
        )
        .unwrap();
        let mut buffer = [0xCC; 512];
        assert_eq!(
            reader.read_at(0, &mut buffer).unwrap_err().kind(),
            if cancel {
                io::ErrorKind::Interrupted
            } else {
                io::ErrorKind::TimedOut
            }
        );
        assert_eq!(buffer, [0xCC; 512]);
        controller.join().unwrap();
        // With the write end still open, Windows must cancel the actual OS read.
        #[cfg(windows)]
        wait_finished(&reader);
        drop(output);
        wait_finished(&reader);
    }
}
