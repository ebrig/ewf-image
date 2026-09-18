//! Acquires one regular file as a physical image or logical evidence file.
use aff4_image::{Profile, WriteOptions, Writer};
use std::ops::ControlFlow;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let source = args
        .next()
        .ok_or("usage: acquire SOURCE OUTPUT [logical]")?;
    let output = args.next().ok_or("missing output")?;
    let profile = if args.next().as_deref() == Some("logical") {
        Profile::Logical
    } else {
        Profile::Physical
    };
    let mut file = std::fs::File::open(&source)?;
    let size = file.metadata()?.len();
    let mut writer = Writer::create(output, profile, WriteOptions::default())?;
    match profile {
        Profile::Physical => {
            writer.add_image(size, &mut file, |_, _| ControlFlow::Continue(()))?;
        }
        Profile::Logical => {
            writer.add_file("evidence.bin", size, &mut file, |_, _| {
                ControlFlow::Continue(())
            })?;
        }
    }
    let result = writer.finish()?;
    println!("{}", result.path.display());
    for stream in result.streams {
        println!("{} {} {}", stream.id, stream.size, stream.sha256);
    }
    Ok(())
}
