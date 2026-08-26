//! The whole fetch, end to end, against the live service.
//!
//! This exists because every claim in `provider::mirror`'s header is a claim
//! about a service nobody here controls, and the only way to keep those claims
//! honest is to re-run them and paste what came back. It is an example rather
//! than a test because it moves 150 MB and takes minutes.
//!
//! ```bash
//! cargo run --release -p cordial-update --example fetch_probe
//! cargo run --release -p cordial-update --example fetch_probe -- --download
//! ```
//!
//! Without `--download` it asks every source what it has and stops, which is
//! the cheap half and the one worth running often.

use cordial_update::provider::{self, Cancel, Progress, Want};

fn main() {
    let download = std::env::args().any(|a| a == "--download");
    let only = std::env::args().skip(1).find(|a| !a.starts_with("--"));
    let mut say = |p: Progress| match p {
        Progress::Asking { provider } => eprintln!("  asking {provider}"),
        Progress::Fetching { file, done, total } => match total {
            Some(t) => eprintln!("  {file}: {done}/{t}"),
            None => eprintln!("  {file}: {done}"),
        },
        Progress::Verifying { file } => eprintln!("  verifying {file}"),
    };

    for p in provider::all() {
        println!("== {} (network: {}) ==", p.name(), p.needs_network());
        let available = match p.newest(&mut say) {
            Ok(v) => {
                println!("   newest: {} (code {})", v.name, v.code);
                v
            }
            Err(e) => {
                println!("   unavailable: {e}");
                continue;
            }
        };

        if !download && p.needs_network() {
            println!("   (pass --download to fetch)");
            continue;
        }

        let into = std::env::temp_dir().join(format!("cordial-fetch-probe-{}", p.name()));
        let _ = std::fs::remove_dir_all(&into);
        std::fs::create_dir_all(&into).expect("scratch");

        match p.fetch(&available, &Cancel::new(), &into, &mut say) {
            Ok(archives) => {
                println!("   base:  {}", archives.base.display());
                println!("   split: {}", archives.split.display());
                let trusted = cordial_update::apk_signature::pinned();
                for file in archives.distinct() {
                    match cordial_update::apk_signature::verify_signed_by(file, &trusted) {
                        Ok(s) => println!(
                            "   VERIFIED {} -> {}",
                            file.file_name().unwrap_or_default().to_string_lossy(),
                            s.certificate_sha256
                        ),
                        Err(e) => println!("   REFUSED  {}: {e}", file.display()),
                    }
                }
            }
            Err(e) => println!("   fetch failed: {e}"),
        }
    }

    // The entry point everything else should use: pick a source, fetch, and
    // verify, with no way to get bytes back that nobody has checked.
    println!("== obtain(Newest, {}) ==", only.as_deref().unwrap_or("any source"));
    let into = std::env::temp_dir().join("cordial-obtain-probe");
    let _ = std::fs::remove_dir_all(&into);
    std::fs::create_dir_all(&into).expect("scratch");
    match provider::obtain(only.as_deref(), Want::Newest, &Cancel::new(), &into, &mut say) {
        Ok(got) => {
            println!("   {} from {}", got.version.name, got.provider);
            println!("   signed by {}", got.certificate_sha256);
            println!("   base:  {}", got.archives.base.display());
            println!("   split: {}", got.archives.split.display());
        }
        Err(e) => println!("   {e}"),
    }
    let _ = std::fs::remove_dir_all(&into);
}
