//! What one asset costs to read out of the APK, through the entry points the
//! engine actually calls.
//!
//! Ignored by default and driven by the environment, because it needs a real
//! Roblox APK and Cordial ships none. Run it with:
//!
//! ```text
//! CORDIAL_BENCH_APK=/path/to/base.apk \
//!   cargo test -p cordial-runtime --test apk_read_cost -- --ignored --nocapture
//! ```
//!
//! **Why this exists rather than a bespoke script.** AGENTS.md's rule is that
//! an ad-hoc harness usually ends up scoring something that turns out to be
//! constant across every run. This scores the one thing the zip change touches
//! and nothing else: `AAssetManager_open` -> `AAsset_getBuffer` ->
//! `AAsset_close`, the same three calls Roblox makes, over the same names, on
//! the same file. It is a control for that change and is not a frame rate, a
//! present count, or any other proxy for one.
//!
//! It reads the APK's own `content/textures/ui` tree -- the menu chrome -- one
//! name at a time, each name once, so every read is a cache miss and therefore
//! a real trip through the zip. That is deliberately the worst case and
//! deliberately the case a menu opening for the first time hits.

use std::path::PathBuf;
use std::time::Instant;

use cordial_runtime::android::asset;

fn apk() -> Option<PathBuf> {
    std::env::var_os("CORDIAL_BENCH_APK").map(PathBuf::from)
}

#[test]
#[ignore = "needs a real Roblox APK in CORDIAL_BENCH_APK"]
fn reading_the_menu_chrome_out_of_the_apk() {
    let Some(apk) = apk() else {
        panic!("set CORDIAL_BENCH_APK to a Roblox APK");
    };
    asset::set_apk(&apk).expect("set_apk");

    let names: Vec<String> = asset::apk_asset_names()
        .expect("central directory")
        .into_iter()
        .filter(|n| n.starts_with("content/textures/ui/"))
        .collect();
    assert!(!names.is_empty(), "no UI textures in this APK");

    // The first read pays for whatever one-off indexing the implementation
    // does, and quoting it separately is the point: before the change there
    // was no one-off, because every read re-indexed.
    let started = Instant::now();
    let mut bytes = 0usize;
    let mut first = None;
    for name in &names {
        let at = Instant::now();
        match asset::probe(name) {
            Ok(n) => bytes += n,
            Err(e) => panic!("{name}: {e}"),
        }
        if first.is_none() {
            first = Some(at.elapsed());
        }
    }
    let total = started.elapsed();

    // Every name again, all now cached, so this is the floor the cache
    // provides and the thing that must not have regressed.
    let cached_start = Instant::now();
    for name in &names {
        asset::probe(name).expect("cached read");
    }
    let cached = cached_start.elapsed();

    println!(
        "apk-read-cost: {} names, {} bytes\n\
         apk-read-cost:   first read      {:?}\n\
         apk-read-cost:   cold total      {:?}  ({:.3} ms/asset)\n\
         apk-read-cost:   warm total      {:?}  ({:.3} ms/asset)",
        names.len(),
        bytes,
        first.unwrap(),
        total,
        total.as_secs_f64() * 1000.0 / names.len() as f64,
        cached,
        cached.as_secs_f64() * 1000.0 / names.len() as f64,
    );
}
