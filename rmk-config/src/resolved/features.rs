//! Resolved `rmk` cargo features — the single source of truth for which features
//! a generated firmware project must enable, derived from `keyboard.toml`.
//!
//! `rmkit` calls [`KeyboardTomlConfig::firmware_features`] at project-generation
//! time to write the `rmk` dependency line, and the proc-macro reuses
//! [`RMK_DEFAULT_FEATURES`], so the feature list is never hand-copied outside
//! `rmk/Cargo.toml`.

use std::collections::BTreeSet;

use crate::KeyboardTomlConfig;
use crate::chip::ChipSeries;

/// rmk's default feature set. Mirrors `default = [...]` in `rmk/Cargo.toml`; a
/// test in this module asserts the two stay equal, so bumping rmk's defaults
/// without updating this fails CI.
pub const RMK_DEFAULT_FEATURES: &[&str] = &["defmt", "storage", "vial", "host_lock", "watchdog"];

/// The `rmk` cargo features a firmware project needs, resolved from config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareFeatures {
    /// Canonical chip id (e.g. `"rp2040"`, `"nrf52840"`, `"esp32c3"`).
    pub chip: String,
    /// Chip family — picks the HAL template.
    pub series: ChipSeries,
    /// Board id when a supported board is used — drives template overlays.
    pub board: Option<String>,
    /// Whether this is a split keyboard.
    pub is_split: bool,
    /// When true, keep `default-features = true` and `rmk_features` lists only
    /// the extra non-default features. When false, `rmk_features` is the
    /// complete explicit set and the caller emits `default-features = false`.
    pub use_rmk_default_features: bool,
    /// Features for the `rmk` dependency line, sorted and deduped.
    pub rmk_features: Vec<String>,
}

impl KeyboardTomlConfig {
    /// Derive the complete `rmk` cargo feature set from `keyboard.toml`.
    ///
    /// This is the single source of truth for config→feature mapping: `rmkit`
    /// writes the returned set onto the generated project's `rmk` dependency.
    pub fn firmware_features(&self) -> Result<FirmwareFeatures, String> {
        let chip = self.get_chip_model()?;
        let host = self.get_host_config();
        if host.rynk_enabled && host.vial_enabled {
            return Err(
                "[host]: `rynk_enabled = true` requires `vial_enabled = false` — Vial and Rynk use mutually-exclusive rmk features"
                    .to_string(),
            );
        }
        let is_split = self.split.is_some();

        // 1. Chip baseline. ESP curates its own set (`default-features = false`);
        //    every other family builds on rmk's defaults.
        let default_on = chip.series != ChipSeries::Esp32;
        let mut features: BTreeSet<String> = BTreeSet::new();
        if default_on {
            features.extend(RMK_DEFAULT_FEATURES.iter().map(|s| s.to_string()));
        }
        let is_pico_w = chip
            .board
            .as_deref()
            .is_some_and(|b| matches!(b, "Pi Pico W" | "Pico W" | "pi_pico_w" | "pico_w"));
        match &chip.series {
            ChipSeries::Rp2040 => {
                if is_pico_w {
                    features.insert("pico_w_ble".to_string());
                }
                features.insert("rp2040".to_string());
            }
            ChipSeries::Nrf52 => {
                features.insert("async_matrix".to_string());
                features.insert(format!("{}_ble", chip.chip));
                if chip.chip == "nrf52840" {
                    features.insert("adafruit_bl".to_string());
                }
            }
            ChipSeries::Esp32 => {
                features.insert(format!("{}_ble", chip.chip));
                // host_lock is a dep-free marker; include it so esp Vial keyboards
                // get the same unlock gate as every other chip (the stock esp
                // template predates host_lock becoming a universal default).
                features.insert("host_lock".to_string());
                features.insert("log".to_string());
                features.insert("storage".to_string());
                features.insert("vial".to_string());
            }
            // STM32's HAL is embassy-stm32, selected by its own chip feature; rmk
            // has no stm32 chip feature.
            ChipSeries::Stm32 => {}
        }

        // 2. Toggles from keyboard.toml, layered on the baseline.
        if is_split {
            features.insert("split".to_string());
        }
        // passkey_entry pulls in `_ble`; only honor it on a chip that actually has BLE.
        if self.ble.as_ref().and_then(|b| b.passkey_entry).unwrap_or(false)
            && features.iter().any(|f| f.ends_with("_ble"))
        {
            features.insert("passkey_entry".to_string());
        }
        if !host.vial_enabled {
            features.remove("vial");
            if !host.rynk_enabled {
                // Vial off and no Rynk: nothing consumes the lock gate, drop it.
                features.remove("host_lock");
            }
        }
        if host.rynk_enabled {
            features.insert("rynk".to_string());
            features.insert("host_lock".to_string());
        }
        if !self.get_storage_config().enabled {
            // `_ble` ⇒ storage, and the proc-macro hard-panics if keyboard.toml's
            // `storage.enabled` disagrees with the `storage` cargo feature. So a
            // storage-off BLE config cannot produce a buildable project — reject it
            // here with a clear message instead of emitting one that won't compile.
            if features.iter().any(|f| f.ends_with("_ble")) {
                return Err(
                    "[storage].enabled = false is not supported on a BLE keyboard — BLE requires storage; remove the [storage] override."
                        .to_string(),
                );
            }
            features.remove("storage");
        }
        if !self.get_dependency_config().defmt_log {
            features.remove("defmt");
        }

        // 3. Emit. Keep `default-features` on when nothing default was dropped,
        //    so the generated Cargo.toml stays minimal; otherwise list the full
        //    set explicitly with defaults off.
        let (use_rmk_default_features, rmk_features) = if default_on {
            let defaults: BTreeSet<&str> = RMK_DEFAULT_FEATURES.iter().copied().collect();
            let dropped_a_default = defaults.iter().any(|d| !features.contains(*d));
            if dropped_a_default {
                (false, features.into_iter().collect())
            } else {
                (
                    true,
                    features.into_iter().filter(|f| !defaults.contains(f.as_str())).collect(),
                )
            }
        } else {
            (false, features.into_iter().collect())
        };

        Ok(FirmwareFeatures {
            chip: chip.chip,
            series: chip.series,
            board: chip.board,
            is_split,
            use_rmk_default_features,
            rmk_features,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    /// Resolve through the real layered pipeline (chip defaults applied), exactly
    /// as rmkit does. `new_from_toml_path` takes a path, so write to a temp file;
    /// chip defaults are what supply `[storage].enabled = true`, etc.
    fn features(body: &str) -> FirmwareFeatures {
        try_features(body).expect("derive features")
    }

    fn try_features(body: &str) -> Result<FirmwareFeatures, String> {
        // Unique per call (pid + atomic counter) so parallel tests using the same
        // config body don't collide on the temp path and race on write/remove.
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("rmk_ff_{}_{}.toml", std::process::id(), seq));
        std::fs::write(&path, body).expect("write temp keyboard.toml");
        let cfg = KeyboardTomlConfig::new_from_toml_path(&path);
        let _ = std::fs::remove_file(&path);
        cfg.firmware_features()
    }

    /// Minimal keyboard.toml body with a `chip = ...`.
    fn chip_toml(chip: &str) -> String {
        format!("[keyboard]\nname = \"t\"\nvendor_id = 0x4c4b\nproduct_id = 0x4643\nchip = \"{chip}\"\n")
    }

    // --- Oracle: stock per-chip configs reproduce the frozen template feature lists ---

    #[test]
    fn rp2040_matches_template() {
        let f = features(&chip_toml("rp2040"));
        assert!(f.use_rmk_default_features);
        assert_eq!(f.rmk_features, vec!["rp2040"]);
    }

    #[test]
    fn pico_w_board_matches_template() {
        let f = features("[keyboard]\nname = \"t\"\nvendor_id = 1\nproduct_id = 1\nboard = \"pico_w\"\n");
        assert!(f.use_rmk_default_features);
        assert_eq!(f.rmk_features, vec!["pico_w_ble", "rp2040"]);
    }

    #[test]
    fn nrf52840_matches_template() {
        let f = features(&chip_toml("nrf52840"));
        assert!(f.use_rmk_default_features);
        assert_eq!(f.rmk_features, vec!["adafruit_bl", "async_matrix", "nrf52840_ble"]);
    }

    #[test]
    fn nrf52832_has_no_adafruit_bl() {
        let f = features(&chip_toml("nrf52832"));
        assert_eq!(f.rmk_features, vec!["async_matrix", "nrf52832_ble"]);
    }

    #[test]
    fn esp32c3_matches_template() {
        let f = features(&chip_toml("esp32c3"));
        assert!(!f.use_rmk_default_features, "esp uses default-features = false");
        assert_eq!(f.rmk_features, vec!["esp32c3_ble", "host_lock", "log", "storage", "vial"]);
    }

    #[test]
    fn esp32s3_matches_template() {
        let f = features(&chip_toml("esp32s3"));
        assert!(!f.use_rmk_default_features);
        assert_eq!(f.rmk_features, vec!["esp32s3_ble", "host_lock", "log", "storage", "vial"]);
    }

    #[test]
    fn stm32_has_no_rmk_chip_feature() {
        let f = features(&chip_toml("stm32f103"));
        assert!(f.use_rmk_default_features);
        assert!(f.rmk_features.is_empty());
    }

    #[test]
    fn split_adds_split_feature() {
        let matrix = "matrix = { matrix_type = \"normal\", row_pins = [\"PIN_0\"], col_pins = [\"PIN_1\"] }";
        let toml = format!(
            "{base}\n[split]\nconnection = \"serial\"\n\
             [split.central]\nrows = 1\ncols = 1\nrow_offset = 0\ncol_offset = 0\n{matrix}\n\
             [[split.peripheral]]\nrows = 1\ncols = 1\nrow_offset = 0\ncol_offset = 0\n{matrix}\n",
            base = chip_toml("rp2040"),
        );
        let f = features(&toml);
        assert!(f.is_split);
        assert!(f.rmk_features.contains(&"split".to_string()));
    }

    // --- Regressions for the two feature-drift bugs ---

    #[test]
    fn light_pins_do_not_add_controller() {
        // Bug (a): `controller` is not a real rmk feature; setting [light] pins
        // must never emit it.
        let toml = format!("{}\n[light]\ncapslock = {{ pin = \"PIN_0\", low_active = true }}\n", chip_toml("rp2040"));
        let f = features(&toml);
        assert!(!f.rmk_features.iter().any(|x| x == "controller"));
    }

    #[test]
    fn vial_disabled_drops_vial_and_host_lock() {
        // Bug (b): with Vial off (and no Rynk) both `vial` and `host_lock` go.
        let toml = format!("{}\n[host]\nvial_enabled = false\n", chip_toml("rp2040"));
        let f = features(&toml);
        assert!(!f.use_rmk_default_features);
        assert!(!f.rmk_features.iter().any(|x| x == "vial"));
        assert!(!f.rmk_features.iter().any(|x| x == "host_lock"));
        assert_eq!(f.rmk_features, vec!["defmt", "rp2040", "storage", "watchdog"]);
    }

    #[test]
    fn rynk_enables_rynk_and_host_lock_without_vial() {
        let toml = format!("{}\n[host]\nvial_enabled = false\nrynk_enabled = true\n", chip_toml("rp2040"));
        let f = features(&toml);
        assert!(f.rmk_features.contains(&"rynk".to_string()));
        assert!(f.rmk_features.contains(&"host_lock".to_string()));
        assert!(!f.rmk_features.iter().any(|x| x == "vial"));
    }

    #[test]
    fn vial_and_rynk_both_enabled_is_an_error() {
        let toml = format!("{}\n[host]\nvial_enabled = true\nrynk_enabled = true\n", chip_toml("rp2040"));
        assert!(try_features(&toml).is_err());
    }

    #[test]
    fn storage_off_usb_chip_drops_storage() {
        let toml = format!("{}\n[storage]\nenabled = false\n", chip_toml("rp2040"));
        let f = features(&toml);
        assert!(!f.rmk_features.iter().any(|x| x == "storage"));
    }

    #[test]
    fn storage_off_ble_chip_is_rejected() {
        // BLE requires storage and the macro panics on a mismatch, so this must be a
        // clear generation-time error, not a project that fails to compile.
        for chip in ["nrf52840", "esp32c3"] {
            let toml = format!("{}\n[storage]\nenabled = false\n", chip_toml(chip));
            assert!(try_features(&toml).is_err(), "storage-off on {chip} should be rejected");
        }
    }

    #[test]
    fn passkey_ignored_without_ble() {
        // passkey_entry pulls in `_ble`; on a USB-only chip it must not be emitted.
        let toml = format!("{}\n[ble]\nenabled = true\npasskey_entry = true\n", chip_toml("rp2040"));
        let f = features(&toml);
        assert!(!f.rmk_features.iter().any(|x| x == "passkey_entry"));
    }

    #[test]
    fn nrf52840_board_matches_template() {
        let f = features("[keyboard]\nname = \"t\"\nvendor_id = 1\nproduct_id = 1\nboard = \"nice!nano_v2\"\n");
        assert_eq!(f.chip, "nrf52840");
        assert_eq!(f.rmk_features, vec!["adafruit_bl", "async_matrix", "nrf52840_ble"]);
    }

    #[test]
    fn split_on_nrf_and_stm32() {
        let matrix = "matrix = { matrix_type = \"normal\", row_pins = [\"P0_02\"], col_pins = [\"P0_03\"] }";
        for chip in ["nrf52840", "stm32f411ce"] {
            let toml = format!(
                "{base}\n[split]\nconnection = \"serial\"\n\
                 [split.central]\nrows = 1\ncols = 1\nrow_offset = 0\ncol_offset = 0\n{matrix}\n\
                 [[split.peripheral]]\nrows = 1\ncols = 1\nrow_offset = 0\ncol_offset = 0\n{matrix}\n",
                base = chip_toml(chip),
            );
            let f = features(&toml);
            assert!(f.is_split && f.rmk_features.contains(&"split".to_string()), "split missing for {chip}");
        }
    }

    #[test]
    fn defmt_log_off_drops_defmt() {
        let toml = format!("{}\n[dependency]\ndefmt_log = false\n", chip_toml("rp2040"));
        let f = features(&toml);
        assert!(!f.rmk_features.iter().any(|x| x == "defmt"));
    }

    // --- The const stays aligned with rmk/Cargo.toml ---

    #[test]
    fn rmk_default_features_mirror_cargo_toml() {
        let manifest = std::fs::read_to_string("../rmk/Cargo.toml").expect("read ../rmk/Cargo.toml");
        let value: toml::Value = toml::from_str(&manifest).expect("parse rmk/Cargo.toml");
        let default = value["features"]["default"].as_array().expect("[features].default array");
        let actual: BTreeSet<String> = default.iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let expected: BTreeSet<String> = RMK_DEFAULT_FEATURES.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            actual, expected,
            "RMK_DEFAULT_FEATURES is out of sync with rmk/Cargo.toml [features].default"
        );
    }

    /// Every feature name `firmware_features()` can emit must be a real key in
    /// `rmk/Cargo.toml` `[features]`. This runs the derivation over a chip ×
    /// toggle matrix and checks each emitted name against rmk's actual feature
    /// table — the guard that binds rmk-config's feature vocabulary to rmk, and
    /// that would have caught both the `controller` (nonexistent) and the
    /// `vial_lock`→`host_lock` rename bugs the moment they were introduced.
    #[test]
    fn every_derived_feature_exists_in_rmk() {
        let manifest = std::fs::read_to_string("../rmk/Cargo.toml").expect("read ../rmk/Cargo.toml");
        let value: toml::Value = toml::from_str(&manifest).expect("parse rmk/Cargo.toml");
        let keys: BTreeSet<String> = value["features"]
            .as_table()
            .expect("[features] table")
            .keys()
            .cloned()
            .collect();

        let split = format!(
            "{base}\n[split]\nconnection = \"serial\"\n\
             [split.central]\nrows = 1\ncols = 1\nrow_offset = 0\ncol_offset = 0\nmatrix = {{ matrix_type = \"normal\", row_pins = [\"PIN_0\"], col_pins = [\"PIN_1\"] }}\n\
             [[split.peripheral]]\nrows = 1\ncols = 1\nrow_offset = 0\ncol_offset = 0\nmatrix = {{ matrix_type = \"normal\", row_pins = [\"PIN_2\"], col_pins = [\"PIN_3\"] }}\n",
            base = chip_toml("rp2040"),
        );

        let configs: Vec<String> = vec![
            // One config per chip → exercises every chip feature + the defaults.
            chip_toml("rp2040"),
            chip_toml("nrf52840"),
            chip_toml("nrf52832"),
            chip_toml("esp32c3"),
            chip_toml("esp32c6"),
            chip_toml("esp32s3"),
            chip_toml("esp32h2"),
            chip_toml("stm32f411ce"),
            "[keyboard]\nname = \"t\"\nvendor_id = 1\nproduct_id = 1\nboard = \"pico_w\"\n".to_string(),
            // Toggles → exercises every derived-toggle feature.
            format!("{}\n[storage]\nenabled = false\n", chip_toml("rp2040")),
            format!("{}\n[dependency]\ndefmt_log = false\n", chip_toml("rp2040")),
            format!("{}\n[host]\nvial_enabled = false\n", chip_toml("rp2040")),
            format!("{}\n[host]\nvial_enabled = false\nrynk_enabled = true\n", chip_toml("rp2040")),
            format!("{}\n[ble]\nenabled = true\npasskey_entry = true\n", chip_toml("nrf52840")),
            format!("{}\n[light]\ncapslock = {{ pin = \"PIN_0\", low_active = true }}\n", chip_toml("rp2040")),
            split,
        ];

        // Implicit defaults count too (they must exist in rmk when default-features stays on).
        let mut emitted: BTreeSet<String> = RMK_DEFAULT_FEATURES.iter().map(|s| s.to_string()).collect();
        for cfg in &configs {
            let f = try_features(cfg).unwrap_or_else(|e| panic!("derive failed for config:\n{cfg}\nerror: {e}"));
            emitted.extend(f.rmk_features);
        }

        let missing: Vec<&String> = emitted.iter().filter(|n| !keys.contains(*n)).collect();
        assert!(
            missing.is_empty(),
            "firmware_features() emits features absent from rmk/Cargo.toml [features]: {missing:?}"
        );
    }
}
