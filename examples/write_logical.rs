//! Acquires two caller-supplied streams into a logical evidence container.
use ewf_image::{LogicalEntryMetadata, LogicalWriter, WriteFormat, WriteOptions};
use std::io::Cursor;

fn main() -> ewf_image::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "files.L01".into());
    let mut writer = LogicalWriter::create(
        path,
        WriteOptions {
            format: WriteFormat::Ewf1Logical,
            ..WriteOptions::default()
        },
    )?;
    let directory = writer.add_directory(
        1,
        LogicalEntryMetadata {
            name: "Documents".into(),
            ..LogicalEntryMetadata::default()
        },
    )?;
    writer.add_file(
        directory,
        LogicalEntryMetadata {
            name: "note.txt".into(),
            ..LogicalEntryMetadata::default()
        },
        5,
        &mut Cursor::new(b"hello"),
    )?;
    let result = writer.finish()?;
    println!("wrote {} segment(s)", result.segment_paths.len());
    Ok(())
}
