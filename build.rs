//! Generate bindings and link the pinned libghostty-vt build.

use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=TERMTRACE_GHOSTTY_PREFIX");
    println!("cargo:rerun-if-changed=vendor/ghostty/.git");

    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let prefix = env::var_os("TERMTRACE_GHOSTTY_PREFIX")
        .map_or_else(|| root.join("vendor/ghostty/zig-out"), PathBuf::from);
    let include = prefix.join("include");
    let header = include.join("ghostty/vt.h");
    let library = prefix.join("lib/libghostty-vt.a");
    assert!(
        header.is_file() && library.is_file(),
        "pinned libghostty-vt is not built; run scripts/bootstrap-ghostty-vt.sh"
    );

    let bindings = bindgen::Builder::default()
        .header(header.display().to_string())
        .clang_arg(format!("-I{}", include.display()))
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .derive_debug(true)
        .derive_default(true)
        .generate_comments(true)
        .layout_tests(false)
        .generate()
        .expect("generate pinned libghostty-vt bindings");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"));
    bindings
        .write_to_file(output.join("ghostty_vt_bindings.rs"))
        .expect("write libghostty-vt bindings");

    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=pthread");
}
