use super::*;

thread_local! {
    pub(super) static FAIL_DIRECTORY_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CRASH_AT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

pub(super) fn boundary(name: &str) {
    if CRASH_AT.with_borrow(|value| value == name) {
        std::process::exit(73);
    }
}

fn writer(path: &Path) -> Writer {
    let mut writer = Writer::create(path, Profile::Physical, WriteOptions::default()).unwrap();
    writer
        .add_image(65_537, &mut std::io::repeat(31), |_, _| {
            std::ops::ControlFlow::Continue(())
        })
        .unwrap();
    writer
}

#[test]
fn published_sync_error_keeps_path_and_verifiable_result() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.aff4");
    let writer = writer(&path);
    FAIL_DIRECTORY_SYNC.set(true);
    let outcome =
        writer.finish_verified(
            crate::Limits::default(),
            |_, _, _| ControlFlow::Continue(()),
        );
    FAIL_DIRECTORY_SYNC.set(false);
    let Err(Error::PublishedButUnsynced { result, .. }) = outcome else {
        panic!("expected published outcome");
    };
    assert_eq!(result.path, path.canonicalize().unwrap());
    let mut container = crate::Container::open(&path).unwrap();
    assert!(
        container
            .verify_all(Some(&result.metadata_sha256), |_, _, _| {
                std::ops::ControlFlow::Continue(())
            })
            .unwrap()
            .all_match()
    );
    assert!(Writer::create(&path, Profile::Physical, WriteOptions::default()).is_err());
}

#[test]
fn verified_finish_cancellation_and_late_collision_preserve_destination() {
    for cancel_at in ["staged verification", "before publication"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.aff4");
        let outcome = Writer::create(&path, Profile::Logical, WriteOptions::default())
            .unwrap()
            .finish_verified(crate::Limits::default(), |phase, _, _| {
                if phase == cancel_at {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            });
        assert!(matches!(outcome, Err(Error::Aborted)));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.aff4");
    let outcome = writer(&path).finish_verified(crate::Limits::default(), |phase, _, _| {
        if phase == "before publication" {
            fs::write(&path, b"other owner").unwrap();
        }
        ControlFlow::Continue(())
    });
    assert!(matches!(outcome, Err(Error::Io(_))));
    assert_eq!(fs::read(&path).unwrap(), b"other owner");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn crash_worker() {
    let Some(path) = std::env::var_os("AFF4_TEST_CRASH_PATH") else {
        return;
    };
    CRASH_AT.set(std::env::var("AFF4_TEST_CRASH_AT").unwrap());
    writer(Path::new(&path)).finish().unwrap();
    panic!("boundary did not terminate process");
}

#[test]
fn process_interruption_before_and_after_publication() {
    for boundary in [
        "metadata",
        "zip_closed",
        "file_synced",
        "published",
        "directory_synced",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.aff4");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "writer::failure_tests::crash_worker"])
            .env("AFF4_TEST_CRASH_PATH", &path)
            .env("AFF4_TEST_CRASH_AT", boundary)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        if matches!(boundary, "published" | "directory_synced") {
            let mut container = crate::Container::open(&path).unwrap();
            assert!(
                container
                    .verify_all(None, |_, _, _| std::ops::ControlFlow::Continue(()))
                    .unwrap()
                    .all_match()
            );
        } else {
            assert!(!path.exists());
            // A new transaction can publish despite inert crash leftovers.
            writer(&path).finish().unwrap();
        }
    }
}
