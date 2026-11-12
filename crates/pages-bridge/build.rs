//! Embeds the Windows application manifest, and the console program into the windowed one when
//! building the downloadable setup.

use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));

    // The setup is `pages-bridgew.exe` built with this variable naming a built
    // `pages-bridge.exe`, which it then installs next to itself. Other builds embed nothing and
    // install the console program from their own folder.
    println!("cargo:rerun-if-env-changed=PAGES_BRIDGE_EMBED_CLI");
    let embedded = out_dir.join("embedded-cli.exe");
    match std::env::var_os("PAGES_BRIDGE_EMBED_CLI") {
        Some(cli) => {
            println!("cargo:rerun-if-changed={}", PathBuf::from(&cli).display());
            std::fs::copy(&cli, &embedded).expect("PAGES_BRIDGE_EMBED_CLI names a readable file");
        }
        None => std::fs::write(&embedded, []).expect("OUT_DIR is writable"),
    }

    // The manifest selects Common Controls 6, which has the task dialog the setup shows, asks
    // for no elevation (Windows elevates programs named "setup" without one) and makes the
    // windows sharp on high-DPI screens. Linking it into tests as well keeps them loadable,
    // since the library imports the task dialog function.
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc {
        let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it")).join("windows.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // The manifest has its own execution level; the linker's default one would collide.
        println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
    }
}
