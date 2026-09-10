use anyhow::Context;
use rcgen::generate_simple_self_signed;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let output_directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: generate_dev_certificate <output-directory>")?;
    std::fs::create_dir_all(&output_directory).with_context(|| {
        format!(
            "create certificate output directory {}",
            output_directory.display()
        )
    })?;
    let generated = generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let certificate_path = output_directory.join("relay-cert.pem");
    let private_key_path = output_directory.join("relay-key.pem");
    std::fs::write(&certificate_path, generated.cert.pem())?;
    std::fs::write(&private_key_path, generated.signing_key.serialize_pem())?;
    println!("development certificate: {}", certificate_path.display());
    println!("development private key: {}", private_key_path.display());
    println!("development-only: never commit or reuse this private key in production");
    Ok(())
}
