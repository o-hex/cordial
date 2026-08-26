//! Verify an APK's signature and print who signed it.
//!
//! `cargo run -p cordial-update --example verify-apk -- <path>`
//!
//! Written to be pointed at a real Roblox APK, because that is the only test
//! that means anything here: a verifier that passes its own synthetic fixtures
//! and rejects the genuine article is worse than no verifier.
fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: verify-apk <path-to-apk>");
        std::process::exit(2);
    };
    let path = std::path::Path::new(&path);
    match cordial_update::apk_signature::verify(path) {
        Ok(signer) => {
            println!("verified");
            println!("  signing certificate SHA-256: {}", signer.certificate_sha256);
        }
        Err(e) => {
            println!("refused: {e}");
            std::process::exit(1);
        }
    }
}
