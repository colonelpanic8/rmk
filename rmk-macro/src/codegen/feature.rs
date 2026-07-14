//! RMK feature state forwarded to this proc-macro crate by Cargo.

include!(concat!(env!("OUT_DIR"), "/rmk_features.rs"));

/// RMK features enabled after Cargo resolves defaults and transitive dependencies.
pub(crate) fn get_rmk_features() -> Vec<&'static str> {
    ENABLED_RMK_FEATURES.to_vec()
}

/// Check whether the given RMK feature is enabled.
pub(crate) fn is_feature_enabled(feature_list: &[&str], feature: &str) -> bool {
    feature_list.contains(&feature)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    #[test]
    fn macro_features_are_forwarded_by_rmk() {
        let macro_manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let rmk_manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rmk/Cargo.toml");

        // The sibling crate is present in the repository but not in the
        // standalone package published to crates.io.
        if !rmk_manifest_path.exists() {
            return;
        }

        let macro_manifest =
            cargo_toml::Manifest::from_slice(&fs::read(macro_manifest_path).unwrap()).unwrap();
        let rmk_manifest =
            cargo_toml::Manifest::from_slice(&fs::read(rmk_manifest_path).unwrap()).unwrap();

        for feature in macro_manifest.features.keys() {
            if feature == "default" {
                continue;
            }
            assert!(
                !feature.contains('-'),
                "rmk-macro feature `{feature}` must use underscores so its Cargo environment name is reversible"
            );
            let forwarding = format!("rmk-macro/{feature}");
            assert!(
                rmk_manifest
                    .features
                    .get(feature)
                    .is_some_and(|members| members.contains(&forwarding)),
                "rmk feature `{feature}` must forward `{forwarding}`"
            );
        }
    }
}
