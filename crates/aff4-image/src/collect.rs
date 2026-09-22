//! Local logical acquisition with explicit coverage and source-change checks.
use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

/// Local collection policy. Collection is not an atomic filesystem snapshot.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CollectionOptions {
    /// Paths relative to the collection root to exclude, including descendants.
    pub exclude: Vec<PathBuf>,
    /// Publish a partial collection after discovery errors, recording every issue.
    /// Read errors and source mutation always poison the writer.
    pub allow_partial: bool,
}

/// One excluded, unsupported, or inaccessible source entry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CollectionIssue {
    /// Original path; used only for reporting, never as an output destination.
    pub path: PathBuf,
    /// Why this entry was not collected.
    pub reason: String,
}

/// Coverage of a local directory acquisition.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CollectionReport {
    /// Successfully collected regular files.
    pub files: u64,
    /// Recorded folders, including empty ones.
    pub folders: u64,
    /// Original file bytes accepted.
    pub bytes: u64,
    /// Exclusions and skipped entries; an empty list is not snapshot certification.
    pub issues: Vec<CollectionIssue>,
}

pub(super) fn issue_metadata(issue: &CollectionIssue) -> String {
    format!(
        "; a:collectionIssue {}",
        oxrdf::Literal::new_simple_literal(format!("{}: {}", issue.path.display(), issue.reason))
    )
}

pub(super) fn task_metadata(report: &CollectionReport) -> String {
    let task = identifier();
    let notes = oxrdf::Literal::new_simple_literal(
        "Portable local collection; links not followed; cross-file snapshot consistency, automatic ADS/xattr/ACL capture not provided",
    );
    let mut text = format!(
        "<{task}> a a:LogicalAcquisitionTask; a:notes {notes}; a:filesCollected {}; a:bytesCollected {}",
        report.files, report.bytes
    );
    for issue in &report.issues {
        text.push_str(&issue_metadata(issue));
    }
    text.push_str(" .\n");
    text
}

fn record_issue(
    report: &mut CollectionReport,
    budget: &mut collection_budget::Budget<'_>,
    issue: CollectionIssue,
) -> Result<()> {
    budget.issue(&issue)?;
    report.issues.push(issue);
    Ok(())
}

fn timestamp(value: std::io::Result<SystemTime>) -> Option<i128> {
    value
        .ok()
        .map(|time| match time.duration_since(UNIX_EPOCH) {
            Ok(n) => n.as_nanos() as i128,
            Err(n) => -(n.duration().as_nanos() as i128),
        })
}

fn source_metadata(
    path: &Path,
    meta: &fs::Metadata,
    parent: Option<String>,
) -> Result<LogicalMetadata> {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = path
        .to_str()
        .ok_or_else(|| {
            Error::Unsupported("source path is not Unicode; no lossy conversion is allowed".into())
        })?
        .as_bytes()
        .to_vec();
    let mut result = LogicalMetadata {
        path: bytes,
        parent,
        created: timestamp(meta.created()),
        modified: timestamp(meta.modified()),
        accessed: timestamp(meta.accessed()),
        ..LogicalMetadata::default()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        result.mode = Some(meta.mode());
        result.changed =
            Some(i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec()));
    }
    // Windows does not expose a metadata-change timestamp through std::fs.
    #[cfg(not(unix))]
    {
        result.mode = None;
    }
    Ok(result)
}

fn same_source(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.ino() != after.ino()
            || before.dev() != after.dev()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return false;
        }
    }
    true
}

fn open_regular(path: &Path) -> Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: open the link itself, not its target.
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::Unsupported("source is not a regular file".into()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(Error::Unsupported("reparse points are not followed".into()));
        }
    }
    Ok(file)
}

impl Writer {
    /// Collects a local directory tree using ordinary source permissions.
    /// Symlinks/reparse points and special files are never followed. Exclusions
    /// are explicit; skipped objects require `allow_partial`. Metadata records
    /// the collection policy and omissions. Read errors, cancellation, or a
    /// detected source change prevent publication. Use a stable source snapshot
    /// when consistency across files is required; this is best-effort live reading.
    /// ADS/xattrs/ACLs are not automatically captured by this portable collector.
    pub fn add_directory_tree(
        &mut self,
        root: impl AsRef<Path>,
        options: &CollectionOptions,
        progress: impl FnMut(&Path, u64, u64) -> ControlFlow<()>,
    ) -> Result<CollectionReport> {
        self.add_directory_tree_with_limits(root, options, &CollectionLimits::default(), progress)
    }

    /// Collects with explicit discovery and incremental metadata/entry budgets.
    /// Limits include excluded/skipped entries and deferred hierarchy/issue records.
    /// Budget failures poison the writer even when partial collection is allowed.
    /// Use the same reader limits with `finish_verified` for the full ZIP checks.
    pub fn add_directory_tree_with_limits(
        &mut self,
        root: impl AsRef<Path>,
        options: &CollectionOptions,
        limits: &CollectionLimits,
        mut progress: impl FnMut(&Path, u64, u64) -> ControlFlow<()>,
    ) -> Result<CollectionReport> {
        self.logical_parent(None)?;
        if fs::symlink_metadata(root.as_ref())?
            .file_type()
            .is_symlink()
        {
            return Err(Error::Unsupported("root symlink is not followed".into()));
        }
        self.set_path_separator(std::path::MAIN_SEPARATOR)?;
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(malformed("collection source must be a directory"));
        }
        if self.path.starts_with(&root) {
            return Err(malformed("output must be outside the collection tree"));
        }
        for excluded in &options.exclude {
            if excluded.as_os_str().is_empty()
                || excluded.components().any(|c| {
                    !matches!(
                        c,
                        std::path::Component::Normal(_) | std::path::Component::CurDir
                    )
                })
            {
                return Err(malformed(
                    "exclusions must be nonempty relative paths without parent traversal",
                ));
            }
        }
        let operation = (|| {
            let mut budget = collection_budget::Budget::new(self, limits)?;
            budget.discover(0)?;
            let mut report = CollectionReport::default();
            let mut pending = vec![(root.clone(), None, 0usize)];
            while let Some((path, parent, depth)) = pending.pop() {
                if progress(&path, 0, 0).is_break() {
                    return Err(Error::Aborted);
                }
                let relative = path
                    .strip_prefix(&root)
                    .map_err(|_| malformed("source escaped root"))?;
                if options.exclude.iter().any(|p| relative.starts_with(p)) {
                    record_issue(
                        &mut report,
                        &mut budget,
                        CollectionIssue {
                            path,
                            reason: "excluded by collection policy".into(),
                        },
                    )?;
                    continue;
                }
                let discovered = match fs::symlink_metadata(&path) {
                    Ok(meta) => meta,
                    Err(e) if options.allow_partial => {
                        record_issue(
                            &mut report,
                            &mut budget,
                            CollectionIssue {
                                path,
                                reason: e.to_string(),
                            },
                        )?;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                };
                let mut link = discovered.file_type().is_symlink();
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    link |= discovered.file_attributes() & 0x400 != 0;
                }
                #[cfg(not(windows))]
                {
                    link |= false;
                }
                if link || !discovered.is_file() && !discovered.is_dir() {
                    let reason = "link/reparse point or special file not collected";
                    if !options.allow_partial {
                        return Err(Error::Unsupported(format!("{}: {reason}", path.display())));
                    }
                    record_issue(
                        &mut report,
                        &mut budget,
                        CollectionIssue {
                            path,
                            reason: reason.into(),
                        },
                    )?;
                    continue;
                }
                if discovered.is_dir() {
                    let id =
                        self.add_folder(&source_metadata(&path, &discovered, parent.clone())?)?;
                    budget.added(self, &id, true, parent.as_deref())?;
                    report.folders += 1;
                    let children = (|| -> Result<Vec<PathBuf>> {
                        let mut children = Vec::new();
                        for entry in fs::read_dir(&path)? {
                            if progress(&path, 0, 0).is_break() {
                                return Err(Error::Aborted);
                            }
                            let entry = entry?;
                            budget.discover(depth + 1)?;
                            children.push(entry.path());
                        }
                        Ok(children)
                    })();
                    match children {
                        Ok(mut children) => {
                            children.sort();
                            pending.extend(
                                children
                                    .into_iter()
                                    .rev()
                                    .map(|p| (p, Some(id.clone()), depth + 1)),
                            );
                        }
                        Err(Error::Io(e)) if options.allow_partial => record_issue(
                            &mut report,
                            &mut budget,
                            CollectionIssue {
                                path,
                                reason: e.to_string(),
                            },
                        )?,
                        Err(e) => return Err(e),
                    }
                } else {
                    let mut file = open_regular(&path)?;
                    let before = file.metadata()?;
                    if !same_source(&discovered, &before) {
                        return Err(malformed("source changed before opening"));
                    }
                    budget.file(self, Some(before.len()))?;
                    let id = self.add_file_with_metadata(
                        &source_metadata(&path, &before, parent.clone())?,
                        before.len(),
                        &mut file,
                        |done, size| progress(&path, done, size),
                    )?;
                    budget.added(self, &id, false, parent.as_deref())?;
                    let after = file.metadata()?;
                    if !same_source(&before, &after) {
                        return Err(malformed(format!(
                            "source changed during collection: {}",
                            path.display()
                        )));
                    }
                    report.files += 1;
                    report.bytes = report
                        .bytes
                        .checked_add(before.len())
                        .ok_or_else(|| malformed("collection byte count overflow"))?;
                }
            }
            self.metadata.push_str(&task_metadata(&report));
            Ok(report)
        })();
        if operation.is_err() {
            self.poisoned = true;
        }
        operation
    }
}
