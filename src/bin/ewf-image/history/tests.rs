use super::*;
use ewf_image::{AcquisitionReadOptions, AcquisitionWriter, UnreadableSectorPolicy};
use std::io::{Cursor, Seek, SeekFrom};
use std::ops::ControlFlow;

fn fixture() -> (tempfile::TempDir, Session, ReadArgs) {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("source.raw");
    fs::write(&raw, vec![0x5A; 1536]).unwrap();
    let output = dir.path().join("case.E01");
    let source = super::super::source::Source::open(&raw, Some(512), &output).unwrap();
    let session = Session {
        schema_version: 1,
        output,
        source: source.identity,
        sectors_per_chunk: 1,
        chunks_per_segment: 1,
        compression: "raw".into(),
        case_number: None,
        evidence_number: None,
        examiner: None,
        software_version: env!("CARGO_PKG_VERSION").into(),
    };
    session.save().unwrap();
    let args = ReadArgs {
        read_timeout_ms: None,
        retries: 1,
        zero_fill: true,
        checkpoint_interval: None,
        stop_after: None,
    };
    (dir, session, args)
}

#[test]
fn logging_failure_stops_at_a_recoverable_checkpoint() {
    let (_dir, session, args) = fixture();
    let mut history = History::open(
        &session,
        super::super::session::lock(&session.output).unwrap(),
        true,
    )
    .unwrap();
    history.start("acquire", &args).unwrap();
    let options = session.options().unwrap();
    let identity = session.source.fingerprint().unwrap();
    let mut writer = AcquisitionWriter::create(&session.output, &options, identity).unwrap();
    let mut source = Cursor::new(vec![0x5A; 1536]);
    let result = writer
        .acquire_with_progress(&mut source, &AcquisitionReadOptions::default(), |p| {
            if p.checkpoint_bytes == 512 && history.failure.is_none() {
                // Inject a publication error after data has been sealed. An
                // unexpected path must never be overwritten by the next record.
                fs::create_dir(
                    history
                        .directory
                        .join(format!("{:020}.json", history.sequence + 1)),
                )
                .unwrap();
            }
            if history.progress(p).is_err() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap();
    assert_eq!(result.status, ewf_image::AcquisitionStatus::Cancelled);
    assert!(history.failure.is_some());
    assert_eq!(writer.checkpoint_offset(), 512);
    assert_eq!(writer.position(), 512);
    assert!(history.event("run_end", json!({})).is_err());
    drop(writer);
    let inspected =
        AcquisitionWriter::validate_checkpoint(&session.output, &options, identity, |_| {
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(inspected.checkpoint_bytes, 512);
}

#[test]
fn read_attempts_and_substitution_ranges_are_recorded_once() {
    struct Faulty(Cursor<Vec<u8>>);
    impl Read for Faulty {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.0.position() == 512 {
                Err(io::Error::other("bad sector"))
            } else {
                self.0.read(buffer)
            }
        }
    }
    impl Seek for Faulty {
        fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
            self.0.seek(offset)
        }
    }
    let (_dir, session, args) = fixture();
    let mut history = History::open(
        &session,
        super::super::session::lock(&session.output).unwrap(),
        true,
    )
    .unwrap();
    history.start("acquire", &args).unwrap();
    let mut writer = AcquisitionWriter::create(
        &session.output,
        &session.options().unwrap(),
        session.source.fingerprint().unwrap(),
    )
    .unwrap();
    let options = AcquisitionReadOptions {
        retries: 1,
        unreadable_sector_policy: UnreadableSectorPolicy::ZeroFill,
        ..AcquisitionReadOptions::default()
    };
    writer
        .acquire_with_progress(&mut Faulty(Cursor::new(vec![0x5A; 1536])), &options, |p| {
            history.progress(p).unwrap();
            ControlFlow::Continue(())
        })
        .unwrap();
    let records: Vec<Record> = (1..=history.sequence)
        .map(|sequence| {
            serde_json::from_slice(
                &fs::read(history.directory.join(format!("{sequence:020}.json"))).unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(
        records.iter().filter(|r| r.event == "read_error").count(),
        2
    );
    let substitutions: Vec<_> = records
        .iter()
        .filter(|r| r.event == "substitution")
        .collect();
    assert_eq!(substitutions.len(), 1);
    assert_eq!(substitutions[0].data["first_sector"], 1);
    assert_eq!(substitutions[0].data["sector_count"], 1);
    let report = report(&session, false).unwrap();
    assert_eq!(report["retry_attempts"], 1);
    assert_eq!(report["read_attempts"], 4);
    assert_eq!(report["counters_complete"], false);
}

#[test]
fn publication_is_exclusive_and_bad_history_names_never_panic() {
    let (_dir, session, args) = fixture();
    let mut history = History::open(
        &session,
        super::super::session::lock(&session.output).unwrap(),
        true,
    )
    .unwrap();
    history.start("acquire", &args).unwrap();
    let first = history.directory.join("00000000000000000001.json");
    let original = fs::read(&first).unwrap();
    assert!(publish(&first, b"replacement", false).is_err());
    assert_eq!(fs::read(&first).unwrap(), original);
    fs::write(history.directory.join("aaaaaaaaaaaaaaaaaaaé.json"), b"bad").unwrap();
    assert!(report(&session, false).is_err());
}
