//! Binary entry point of the fixture application.

fn main() -> anyhow::Result<()> {
    let payload = demo_app::build_payload(b"fixture payload")?;
    println!("encoded {payload}");
    Ok(())
}
