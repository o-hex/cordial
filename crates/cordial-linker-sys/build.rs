use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let native = root.join("native");

    if !root.join("third_party/mcpelauncher-linker/bionic/linker/linker.cpp").exists() {
        panic!(
            "third_party/mcpelauncher-linker is not checked out.\n\
             Run: git submodule update --init --recursive"
        );
    }

    // AOSP bionic does not build with GCC; see docs/base-evaluation.md §2.1.
    let dst = cmake::Config::new(&native)
        .define("CMAKE_C_COMPILER", "clang")
        .define("CMAKE_CXX_COMPILER", "clang++")
        .define("CMAKE_BUILD_TYPE", "Release")
        .build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=cordial_linker_shim");
    println!("cargo:rustc-link-lib=static=cordial_jni_shim");
    println!("cargo:rustc-link-lib=static=cordial_liblog");
    println!("cargo:rustc-link-lib=static=jnivm");
    // After jnivm: it is jnivm that references `Log::debug`, and a static
    // archive only satisfies symbols from archives listed after it.
    println!("cargo:rustc-link-lib=static=logger");
    println!("cargo:rustc-link-lib=static=linker");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=dylib=z");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=pthread");

    // Watch the whole native tree, not a hand-maintained list. A file missing
    // from that list is not a build error — Cargo simply does not re-run this
    // script, and the stale object from the previous build gets linked. That
    // failure looks exactly like code that compiled but had no effect.
    for entry in std::fs::read_dir(&native).expect("native/ is readable") {
        let path = entry.expect("readable dir entry").path();
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
