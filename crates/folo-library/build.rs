//! Embeds `packs/` (the built-in packs and the alias table, ADR-0038 D1, D19)
//! in the crate, the way the node embeds the UI's files.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let packs = manifest.join("../../packs");
    println!("cargo:rerun-if-changed={}", packs.display());
    let mut files = Vec::new();
    walk(&packs, &packs, &mut files);
    files.sort();
    let mut code = String::from("pub const FILES: &[(&str, &[u8])] = &[\n");
    for (relative, absolute) in files {
        let absolute = absolute.canonicalize().expect("a packs file");
        // `{:?}` escapes the backslashes of Windows paths
        let _ = writeln!(
            code,
            "    ({relative:?}, include_bytes!({:?})),",
            absolute.display().to_string()
        );
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    std::fs::write(out.join("builtin_files.rs"), code).expect("the output folder is writable");
}

/// Every file below `folder`, as its path from `root` with `/` and its full path.
fn walk(root: &Path, folder: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Ok(relative) = path.strip_prefix(root) {
            let parts: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push((parts.join("/"), path));
        }
    }
}
