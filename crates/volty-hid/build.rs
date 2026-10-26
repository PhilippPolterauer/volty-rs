use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn collect_profiles(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    println!("cargo:rerun-if-changed={}", directory.display());

    let mut entries = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();

    for path in entries {
        if path.is_dir() {
            collect_profiles(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path);
        }
    }

    Ok(())
}

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
    );
    let protocols_dir = manifest_dir.join("protocols");
    let mut files = Vec::new();
    collect_profiles(&protocols_dir, &mut files).unwrap_or_else(|error| {
        panic!(
            "failed to read built-in protocol profiles from {}: {error}",
            protocols_dir.display()
        )
    });

    let mut generated = String::from("pub static BUILTIN_PROFILES: &[(&str, &str)] = &[\n");
    for path in files {
        let relative = path
            .strip_prefix(&protocols_dir)
            .expect("profile path must be under protocols");
        let name = relative
            .with_extension("")
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .expect("built-in profile names must be valid UTF-8")
            })
            .collect::<Vec<_>>()
            .join("/");
        let source = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "failed to read built-in profile {}: {error}",
                path.display()
            )
        });
        writeln!(&mut generated, "    ({name:?}, {source:?}),")
            .expect("writing generated profile registry to a string cannot fail");
    }
    generated.push_str("];\n");

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    fs::write(output.join("builtin_profiles.rs"), generated)
        .expect("failed to write built-in profile registry");
}
