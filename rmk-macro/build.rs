use std::env;
use std::fmt::Write;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Cargo exposes the resolved features of this crate to its build script.
    let mut enabled: Vec<_> = env::vars_os()
        .filter_map(|(name, _)| {
            name.into_string()
                .ok()?
                .strip_prefix("CARGO_FEATURE_")
                .map(|feature| feature.to_ascii_lowercase())
        })
        .collect();
    enabled.sort_unstable();

    let mut generated = String::from("const ENABLED_RMK_FEATURES: &[&str] = &[\n");
    for feature in enabled {
        writeln!(generated, "    {feature:?},").unwrap();
    }
    generated.push_str("];\n");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out_dir.join("rmk_features.rs"), generated).unwrap();
}
