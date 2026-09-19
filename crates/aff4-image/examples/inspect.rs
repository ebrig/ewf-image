//! Lists evidence streams and verifies one explicitly selected identifier.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut container =
        aff4_image::Container::open(args.next().ok_or("usage: inspect IMAGE [STREAM]")?)?;
    if let Some(id) = args.next() {
        if id == "--verify-all" {
            let report =
                container.verify_all(None, |_, _, _| std::ops::ControlFlow::Continue(()))?;
            println!("{report:#?}");
            return Ok(());
        }
        println!(
            "{:#?}",
            container.verify(&id, |_, _| std::ops::ControlFlow::Continue(()))?
        );
    } else {
        for stream in container.streams()? {
            println!("{} {:?}", stream.id, stream.size);
        }
    }
    Ok(())
}
