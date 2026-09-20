//! Generate public synthetic inputs using normal writer APIs.
use std::{ops::ControlFlow, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .unwrap_or_else(|| "corpus".into()),
    );
    for name in ["ewf_read", "aff4_read"] {
        std::fs::create_dir_all(root.join(name))?;
    }
    let temporary = tempfile::tempdir()?;
    let input: Vec<u8> = (0..65536).map(|n| (n * 17 + n / 97) as u8).collect();
    for (n, format) in [
        ewf_image::WriteFormat::Ewf1Physical,
        ewf_image::WriteFormat::Ewf2Physical,
        ewf_image::WriteFormat::Ewf2Logical,
    ]
    .into_iter()
    .enumerate()
    {
        let path = temporary.path().join(format!("seed{n}.E01"));
        let mut writer = ewf_image::EwfWriter::create(
            &path,
            ewf_image::WriteOptions {
                format,
                ..Default::default()
            },
        )?;
        writer.write_all(&input)?;
        writer.finish()?;
        std::fs::copy(path, root.join("ewf_read").join(format!("seed{n}")))?;
    }
    for (n, profile) in [aff4_image::Profile::Physical, aff4_image::Profile::Logical]
        .into_iter()
        .enumerate()
    {
        let path = temporary.path().join(format!("seed{n}.aff4"));
        let mut writer = aff4_image::Writer::create(&path, profile, Default::default())?;
        if profile == aff4_image::Profile::Physical {
            writer.add_image(input.len() as u64, &mut input.as_slice(), |_, _| {
                ControlFlow::Continue(())
            })?;
        } else {
            writer.add_file(
                "sample.bin",
                input.len() as u64,
                &mut input.as_slice(),
                |_, _| ControlFlow::Continue(()),
            )?;
        }
        writer.finish()?;
        std::fs::copy(path, root.join("aff4_read").join(format!("seed{n}")))?;
    }
    Ok(())
}
