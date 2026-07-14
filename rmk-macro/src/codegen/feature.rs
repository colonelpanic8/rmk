//! RMK feature state forwarded to this proc-macro crate by Cargo.

macro_rules! define_rmk_features {
    ($($feature:literal),+ $(,)?) => {
        const TRACKED_RMK_FEATURES: &[&str] = &[$($feature),+];

        /// RMK features enabled after Cargo resolves defaults and transitive dependencies.
        pub(crate) fn get_rmk_features() -> Vec<&'static str> {
            [$(($feature, cfg!(feature = $feature))),+]
                .into_iter()
                .filter_map(|(feature, enabled)| enabled.then_some(feature))
                .collect()
        }
    };
}

define_rmk_features!(
    "async_matrix",
    "dfu_lock",
    "dfu_nrf",
    "dfu_rp",
    "rynk",
    "split",
    "storage",
    "vial",
);

/// Check whether the given RMK feature is enabled.
pub(crate) fn is_feature_enabled(feature_list: &[&str], feature: &str) -> bool {
    assert!(
        TRACKED_RMK_FEATURES.contains(&feature),
        "rmk-macro queried untracked feature `{feature}`"
    );
    feature_list.contains(&feature)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::TRACKED_RMK_FEATURES;

    #[test]
    fn tracked_features_are_forwarded_by_rmk() {
        let macro_manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let rmk_manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rmk/Cargo.toml");

        // The sibling crate is present in the repository but not in the
        // standalone package published to crates.io.
        if !rmk_manifest_path.exists() {
            return;
        }

        let macro_manifest = cargo_toml::Manifest::from_path(macro_manifest_path).unwrap();
        let rmk_manifest = cargo_toml::Manifest::from_path(rmk_manifest_path).unwrap();

        for feature in TRACKED_RMK_FEATURES {
            assert!(
                macro_manifest.features.contains_key(*feature),
                "rmk-macro is missing the `{feature}` marker feature"
            );

            let forwarding = format!("rmk-macro/{feature}");
            assert!(
                rmk_manifest
                    .features
                    .get(*feature)
                    .is_some_and(|members| members.contains(&forwarding)),
                "rmk feature `{feature}` must forward `{forwarding}`"
            );
        }
    }
}
