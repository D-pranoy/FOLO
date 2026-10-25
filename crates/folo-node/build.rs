//! Embeds the UI's build, `ui/dist`, in the node, so each node serves the
//! UI it was built with (ADR-0035 D7). Without it the node is built with no
//! UI files.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let ui = manifest.join("../../ui");
    let dist = ui.join("dist");
    let mut files = Vec::new();
    if dist.is_dir() {
        println!("cargo:rerun-if-changed={}", dist.display());
        collect(&dist, &dist, &mut files);
    } else {
        // Watching the UI's folder notices its build appearing; a path that
        // doesn't exist would run this script on every build
        println!("cargo:rerun-if-changed={}", ui.display());
    }
    files.sort();
    let mut code = String::from(
        "/// The UI's files, by path, sorted.\npub static FILES: &[(&str, &[u8])] = &[\n",
    );
    for (name, path) in &files {
        let path = path.to_str().expect("UI file paths are UTF-8");
        writeln!(code, "    ({name:?}, include_bytes!({path:?})),").expect("a String takes it");
    }
    code.push_str("];\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets it")).join("ui_files.rs");
    fs::write(out, code).expect("OUT_DIR is writable");
}

/// Every file under `dir`, as (its path below `root` with `/`, its path).
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let entries = fs::read_dir(dir).expect("the UI's build can be read");
    for entry in entries {
        let path = entry.expect("the UI's build can be read").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let name = path
                .strip_prefix(root)
                .expect("below the root")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.push((name, path));
        }
    }
}
