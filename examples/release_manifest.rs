//! Maintainer-only release signing tool. The secret is read from the environment,
//! never from an argument, a file in the repository, or a generated artifact.
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};
#[path = "../version_format.rs"]
mod version_format;

fn key() -> Result<SigningKey, String> {
    let value =
        std::env::var("GPUI_UPDATE_SIGNING_KEY").map_err(|_| "Missing GPUI_UPDATE_SIGNING_KEY")?;
    let bytes: [u8; 32] = hex::decode(value.trim())
        .map_err(|_| "Signing key must be 64 hexadecimal characters")?
        .try_into()
        .map_err(|_| "Signing key must be a 32-byte Ed25519 seed")?;
    Ok(SigningKey::from_bytes(&bytes))
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let signing = key()?;
    match args.as_slice() {
        [mode] if mode == "public-key" => {
            println!("{}", hex::encode(signing.verifying_key().as_bytes()))
        }
        [mode, version, exe, output] if mode == "sign" => {
            let version = version_format::normalize(version)?;
            let mut file = fs::File::open(exe).map_err(|e| e.to_string())?;
            let size = file.metadata().map_err(|e| e.to_string())?.len();
            if size == 0 || size > 96 * 1024 * 1024 {
                return Err("Release executable size is out of range".into());
            }
            let mut hash = Sha256::new();
            let mut buf = [0u8; 65536];
            loop {
                let n = file.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
            }
            let manifest = serde_json::to_string(&serde_json::json!({
                "version": version, "size": size, "sha256": hex::encode(hash.finalize()),
                "target": "x86_64-pc-windows-msvc", "notes": "详情请查看该版本的发布说明。", "notes_en": "See the release notes for details."
            })).map_err(|e| e.to_string())?;
            let signature = hex::encode(signing.sign(manifest.as_bytes()).to_bytes());
            let envelope = serde_json::json!({"manifest":manifest,"signature":signature});
            // Never silently replace a previously signed manifest.
            use std::io::Write;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(Path::new(output))
                .map_err(|e| e.to_string())?;
            output
                .write_all(
                    serde_json::to_string_pretty(&envelope)
                        .map_err(|e| e.to_string())?
                        .as_bytes(),
                )
                .map_err(|e| e.to_string())?;
            output.sync_all().map_err(|e| e.to_string())?;
        }
        _ => return Err("Usage: release_manifest public-key | sign VERSION EXE OUTPUT".into()),
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
