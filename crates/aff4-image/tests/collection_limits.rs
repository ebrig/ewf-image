//! Collection limits stop discovery/payload work and poison partial writers.
use aff4_image::{
    CollectionLimits, CollectionOptions, Error, Limits, Profile, WriteOptions, Writer,
};
use std::{fs, ops::ControlFlow};

#[test]
fn discovery_bounds_include_exclusions_and_cannot_be_relaxed_by_partial_policy() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    fs::create_dir_all(root.join("one/two")).unwrap();
    for index in 0..10 {
        fs::write(root.join(format!("file-{index}")), b"content").unwrap();
    }
    for (entries, depth, resource) in [(3, 127, "collection_entries"), (100, 1, "collection_depth")]
    {
        let path = dir.path().join("case.aff4");
        let mut writer = Writer::create(&path, Profile::Logical, WriteOptions::default()).unwrap();
        let limits = CollectionLimits {
            entries,
            depth,
            ..Default::default()
        };
        let error = writer
            .add_directory_tree_with_limits(
                &root,
                &CollectionOptions {
                    allow_partial: true,
                    exclude: vec!["file-0".into()],
                },
                &limits,
                |_, _, _| ControlFlow::Continue(()),
            )
            .unwrap_err();
        assert!(
            matches!(error, Error::ResourceLimit { resource: name, .. } if name == resource),
            "{error}"
        );
        assert!(writer.finish().is_err());
        assert!(!path.exists());
    }
}

#[test]
fn metadata_and_triples_stop_before_collecting_the_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    fs::create_dir(&root).unwrap();
    for index in 0..100 {
        fs::write(root.join(format!("file-{index:03}")), b"x").unwrap();
    }
    for resource in ["metadata_bytes", "triples"] {
        let mut limits = CollectionLimits::default();
        if resource == "metadata_bytes" {
            limits.reader.metadata_bytes = 8192;
        } else {
            limits.reader.triples = 100;
        }
        let path = dir.path().join("case.aff4");
        let mut writer = Writer::create(&path, Profile::Logical, WriteOptions::default()).unwrap();
        let mut completed = 0;
        let error = writer
            .add_directory_tree_with_limits(
                &root,
                &CollectionOptions::default(),
                &limits,
                |_, done, total| {
                    if total > 0 && done == total {
                        completed += 1;
                    }
                    ControlFlow::Continue(())
                },
            )
            .unwrap_err();
        assert!(
            matches!(error, Error::ResourceLimit { resource: name, required, limit } if name == resource && required > limit),
            "{error}"
        );
        assert!(completed > 0 && completed < 100, "completed {completed}");
        assert!(writer.finish().is_err());
        assert!(!path.exists());
    }
}

#[test]
fn chunked_member_budget_is_reserved_before_reading_payload() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    fs::create_dir(&root).unwrap();
    fs::File::create(root.join("large"))
        .unwrap()
        .set_len(2 * 1024 * 1024)
        .unwrap();
    let path = dir.path().join("case.aff4");
    let mut writer = Writer::create(&path, Profile::Logical, WriteOptions::default()).unwrap();
    let limits = CollectionLimits {
        reader: Limits {
            archive_entries: 7,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut payload_started = false;
    let error = writer
        .add_directory_tree_with_limits(
            &root,
            &CollectionOptions::default(),
            &limits,
            |_, _, total| {
                payload_started |= total > 0;
                ControlFlow::Continue(())
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        Error::ResourceLimit {
            resource: "archive_entries",
            required: 8,
            limit: 7
        }
    ));
    assert!(!payload_started);
    assert!(writer.finish().is_err());
    assert!(!path.exists());
}

#[test]
fn omitted_entries_consume_metadata_budget_even_with_partial_collection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    fs::create_dir(&root).unwrap();
    let mut options = CollectionOptions {
        allow_partial: true,
        ..Default::default()
    };
    for index in 0..100 {
        let name = format!("omitted-{index:03}");
        fs::write(root.join(&name), []).unwrap();
        options.exclude.push(name.into());
    }
    let path = dir.path().join("case.aff4");
    let mut writer = Writer::create(&path, Profile::Logical, WriteOptions::default()).unwrap();
    let limits = CollectionLimits {
        reader: Limits {
            triples: 40,
            ..Default::default()
        },
        ..Default::default()
    };
    let error = writer
        .add_directory_tree_with_limits(&root, &options, &limits, |_, _, _| {
            ControlFlow::Continue(())
        })
        .unwrap_err();
    assert!(matches!(
        error,
        Error::ResourceLimit {
            resource: "triples",
            required: 41,
            limit: 40
        }
    ));
    assert!(writer.finish().is_err());
    assert!(!path.exists());
}
