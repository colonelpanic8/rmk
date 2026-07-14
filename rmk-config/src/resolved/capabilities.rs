//! Capability resolution — the single authority for which RMK capabilities are
//! active in a build.
//!
//! Inputs are `keyboard.toml` (authoritative when it describes a keyboard) and
//! the active Cargo features (the activation channel when no toml is used:
//! pure-Rust users, tests, docs.rs). `rmk/build.rs` and `rmk-types/build.rs`
//! emit the result as `rmk_*` rustc cfgs; `rmk-macro` consumes the toml-only
//! form so generated code and library gates can never disagree.

use std::collections::HashSet;

use crate::chip::{ChipModel, ChipSeries};
use crate::{BootloaderType, DisplayDriver, KeyboardTomlConfig};

/// Cargo features active on the crate being built, lowercased.
pub struct ActiveFeatures(HashSet<String>);

impl ActiveFeatures {
    /// Collect from the `CARGO_FEATURE_*` env vars cargo sets for build scripts.
    pub fn from_cargo_env() -> Self {
        Self(
            std::env::vars()
                .filter_map(|(key, _)| key.strip_prefix("CARGO_FEATURE_").map(|f| f.to_lowercase()))
                .collect(),
        )
    }

    pub fn from_names<S: AsRef<str>>(names: &[S]) -> Self {
        Self(names.iter().map(|s| s.as_ref().to_lowercase()).collect())
    }

    pub fn empty() -> Self {
        Self(HashSet::new())
    }

    pub fn contains(&self, feature: &str) -> bool {
        self.0.contains(feature)
    }
}

/// Resolved on/off state of every RMK capability.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capabilities {
    pub usb: bool,
    pub usb_high_speed: bool,
    pub ble: bool,
    pub storage: bool,
    pub vial: bool,
    pub rynk: bool,
    /// Derived: `vial || rynk`.
    pub host: bool,
    pub bulk: bool,
    /// Derived: `rynk || dfu_lock || (vial && [host].unlock_keys set)`.
    /// Vial alone doesn't imply the lock — lock-free Vial builds save flash.
    pub host_lock: bool,
    pub dfu_lock: bool,
    pub split: bool,
    pub display: bool,
    pub dfu: bool,
    pub dfu_rp: bool,
    pub dfu_nrf: bool,
    pub steno: bool,
    pub passkey_entry: bool,
    pub async_matrix: bool,
    pub watchdog: bool,
    pub adafruit_bl: bool,
    pub zsa_voyager_bl: bool,
}

/// Toml values whose explicit/absent distinction matters when merging features.
struct TomlFacts {
    chip: ChipModel,
    steno: Option<bool>,
    passkey_entry: Option<bool>,
    bootloader: Option<BootloaderType>,
    display_drivers: Vec<DisplayDriver>,
}

impl Capabilities {
    /// Resolve from toml + features — the `rmk/build.rs` entry point.
    ///
    /// With a keyboard toml the toml is authoritative and features are
    /// validated against it; without one (or with a constants-only toml that
    /// has no `[keyboard]` section) features activate capabilities directly.
    pub fn resolve(toml: Option<&KeyboardTomlConfig>, features: &ActiveFeatures) -> Result<Self, Vec<String>> {
        Self::resolve_inner(toml, features, true)
    }

    /// Like [`Self::resolve`], but without feature validation — for crates
    /// like `rmk-types` that only receive the subset of features `rmk`
    /// forwards, where "missing feature" checks would misfire. `rmk/build.rs`
    /// validates the full set against the same toml.
    pub fn resolve_forwarded(
        toml: Option<&KeyboardTomlConfig>,
        features: &ActiveFeatures,
    ) -> Result<Self, Vec<String>> {
        Self::resolve_inner(toml, features, false)
    }

    fn resolve_inner(
        toml: Option<&KeyboardTomlConfig>,
        features: &ActiveFeatures,
        validate_features: bool,
    ) -> Result<Self, Vec<String>> {
        match toml {
            Some(t) if t.keyboard.is_some() => Self::resolve_with_toml(t, features, validate_features),
            _ => {
                let caps = Self::from_features(features);
                let mut errs = Vec::new();
                caps.check_invariants(&mut errs);
                if validate_features {
                    caps.check_usb_log(features, &mut errs);
                }
                if errs.is_empty() { Ok(caps) } else { Err(errs) }
            }
        }
    }

    /// Resolve from keyboard.toml alone — the proc-macro entry point.
    ///
    /// Feature validation is not repeated here: the macro expands in the user
    /// crate, which builds after `rmk` — whose build script has already run
    /// [`Capabilities::resolve`] against the same toml.
    pub fn from_toml(toml: &KeyboardTomlConfig) -> Result<Self, String> {
        let (caps, _, mut errs) = Self::toml_caps(toml).map_err(|e| e.join("\n"))?;
        caps.check_invariants(&mut errs);
        if errs.is_empty() {
            Ok(caps)
        } else {
            Err(errs.join("\n"))
        }
    }

    /// `(cfg name, enabled)` for every capability. Build scripts declare all
    /// names (so `check-cfg` never warns) and enable the active ones.
    pub fn cfgs(&self) -> Vec<(String, bool)> {
        self.flags()
            .iter()
            .map(|(name, on)| (format!("rmk_{name}"), *on))
            .collect()
    }

    /// Names of active capabilities, matched against `subscriber_default.toml`.
    pub fn active_names(&self) -> Vec<&'static str> {
        self.flags()
            .iter()
            .filter(|(_, on)| *on)
            .map(|(name, _)| *name)
            .collect()
    }

    fn flags(&self) -> [(&'static str, bool); 21] {
        [
            ("usb", self.usb),
            ("usb_high_speed", self.usb_high_speed),
            ("ble", self.ble),
            ("storage", self.storage),
            ("vial", self.vial),
            ("rynk", self.rynk),
            ("host", self.host),
            ("bulk", self.bulk),
            ("host_lock", self.host_lock),
            ("dfu_lock", self.dfu_lock),
            ("split", self.split),
            ("display", self.display),
            ("dfu", self.dfu),
            ("dfu_rp", self.dfu_rp),
            ("dfu_nrf", self.dfu_nrf),
            ("steno", self.steno),
            ("passkey_entry", self.passkey_entry),
            ("async_matrix", self.async_matrix),
            ("watchdog", self.watchdog),
            ("adafruit_bl", self.adafruit_bl),
            ("zsa_voyager_bl", self.zsa_voyager_bl),
        ]
    }

    fn from_features(features: &ActiveFeatures) -> Self {
        Capabilities {
            usb: !features.contains("_no_usb"),
            usb_high_speed: features.contains("_usb_high_speed"),
            ble: features.contains("_ble"),
            storage: features.contains("storage"),
            vial: features.contains("vial"),
            rynk: features.contains("rynk"),
            host: features.contains("host"),
            bulk: features.contains("bulk"),
            host_lock: features.contains("host_lock"),
            dfu_lock: features.contains("dfu_lock"),
            split: features.contains("split"),
            display: features.contains("display"),
            dfu: features.contains("dfu"),
            dfu_rp: features.contains("dfu_rp"),
            dfu_nrf: features.contains("dfu_nrf"),
            steno: features.contains("steno"),
            passkey_entry: features.contains("passkey_entry"),
            async_matrix: features.contains("async_matrix"),
            watchdog: features.contains("watchdog"),
            adafruit_bl: features.contains("adafruit_bl"),
            zsa_voyager_bl: features.contains("zsa_voyager_bl"),
        }
    }

    /// Toml-side capabilities plus the facts needed for feature merging.
    /// The outer `Err` is an unresolvable chip (nothing else can be checked);
    /// other violations are collected in the returned `Vec`.
    fn toml_caps(toml: &KeyboardTomlConfig) -> Result<(Self, TomlFacts, Vec<String>), Vec<String>> {
        let chip = match toml.get_chip_model() {
            Ok(chip) => chip,
            Err(e) => return Err(vec![format!("keyboard.toml: {e}")]),
        };
        let mut errs = Vec::new();

        let (usb, ble) = match toml.get_communication_config() {
            Ok(comm) => (comm.usb_enabled(), comm.ble_enabled()),
            Err(e) => {
                errs.push(e);
                (false, false)
            }
        };
        if ble && chip.series == ChipSeries::Stm32 {
            errs.push(format!("[ble] is enabled but RMK has no BLE support for {}", chip.chip));
        }

        let host = toml.get_host_config();
        let (vial, rynk) = (host.vial_enabled, host.rynk_enabled);
        let host_unlock_keys = host.unlock_keys.as_ref().is_some_and(|keys| !keys.is_empty());

        let dfu_cfg = toml.get_dfu_config();
        let dfu = dfu_cfg.as_ref().is_some_and(|d| d.enabled);
        let (dfu_rp, dfu_nrf) = if dfu {
            match chip.series {
                ChipSeries::Rp2040 => (true, false),
                // The dfu_nrf path drives NVMC; nRF54's RRAM has no support yet.
                ChipSeries::Nrf52 if chip.chip.starts_with("nrf54") => {
                    errs.push("[dfu] is enabled but DFU is not supported on nRF54 chips".to_string());
                    (false, false)
                }
                ChipSeries::Nrf52 => (false, true),
                ChipSeries::Stm32 => (false, false),
                ChipSeries::Esp32 => {
                    errs.push("[dfu] is enabled but DFU is not supported on ESP32".to_string());
                    (false, false)
                }
            }
        } else {
            (false, false)
        };
        let dfu_lock = dfu
            && dfu_cfg
                .as_ref()
                .is_some_and(|d| d.unlock_keys.as_ref().is_some_and(|keys| !keys.is_empty()));

        let mut display_drivers: Vec<DisplayDriver> = toml.get_display_config().map(|d| d.driver).into_iter().collect();
        if let Some(split) = &toml.split {
            display_drivers.extend(split.central.display.as_ref().map(|d| d.driver.clone()));
            display_drivers.extend(
                split
                    .peripheral
                    .iter()
                    .filter_map(|p| p.display.as_ref())
                    .map(|d| d.driver.clone()),
            );
        }

        // Featureland relies on cargo's `_ble = ["storage"]` edge for this.
        if ble && !toml.get_storage_config().enabled {
            errs.push(
                "BLE requires storage for bond persistence — remove `enabled = false` from [storage]".to_string(),
            );
        }

        let keyboard = toml.keyboard.as_ref().expect("checked by get_chip_model");
        let caps = Capabilities {
            usb,
            usb_high_speed: crate::chip::usb_high_speed(&chip.chip),
            ble,
            storage: toml.get_storage_config().enabled,
            vial,
            rynk,
            host: vial || rynk,
            bulk: false,
            host_lock: rynk || dfu_lock || (vial && host_unlock_keys),
            dfu_lock,
            split: toml.split.is_some(),
            display: !display_drivers.is_empty(),
            dfu,
            dfu_rp,
            dfu_nrf,
            steno: keyboard.steno.unwrap_or(false),
            passkey_entry: toml.ble.as_ref().and_then(|b| b.passkey_entry).unwrap_or(false),
            async_matrix: keyboard.async_matrix.unwrap_or(false),
            watchdog: keyboard.watchdog.unwrap_or(true),
            adafruit_bl: keyboard.bootloader == Some(BootloaderType::Adafruit),
            zsa_voyager_bl: keyboard.bootloader == Some(BootloaderType::ZsaVoyager),
        };
        let facts = TomlFacts {
            chip,
            steno: keyboard.steno,
            passkey_entry: toml.ble.as_ref().and_then(|b| b.passkey_entry),
            bootloader: keyboard.bootloader,
            display_drivers,
        };
        Ok((caps, facts, errs))
    }

    fn resolve_with_toml(
        toml: &KeyboardTomlConfig,
        features: &ActiveFeatures,
        validate_features: bool,
    ) -> Result<Self, Vec<String>> {
        let (mut caps, facts, mut errs) = Self::toml_caps(toml)?;

        // Purely additive inputs the toml has no field for (or left unset).
        caps.bulk = features.contains("bulk");
        caps.usb_high_speed |= features.contains("_usb_high_speed");
        caps.dfu_lock |= features.contains("dfu_lock");
        if facts.steno.is_none() {
            caps.steno = features.contains("steno");
        }
        if facts.passkey_entry.is_none() {
            caps.passkey_entry = features.contains("passkey_entry");
        }
        if facts.bootloader.is_none() {
            caps.adafruit_bl = features.contains("adafruit_bl");
            caps.zsa_voyager_bl = features.contains("zsa_voyager_bl");
        }
        caps.host_lock = caps.host_lock || caps.dfu_lock || features.contains("host_lock");

        if !validate_features {
            caps.check_invariants(&mut errs);
            return if errs.is_empty() { Ok(caps) } else { Err(errs) };
        }

        // Features asserting a capability the toml resolves to disabled.
        // `watchdog` is exempt: it is a default feature, and the toml must be
        // able to disable it without `default-features = false`.
        let contradictions = [
            ("storage", caps.storage, "set `enabled = true` in [storage]"),
            ("vial", caps.vial, "set `vial_enabled = true` in [host]"),
            ("rynk", caps.rynk, "set `rynk_enabled = true` in [host]"),
            ("split", caps.split, "add a [split] section"),
            (
                "async_matrix",
                caps.async_matrix,
                "set `async_matrix = true` in [keyboard]",
            ),
            ("_ble", caps.ble, "set `enabled = true` in [ble]"),
            ("dfu", caps.dfu, "set `enabled = true` in [dfu]"),
            ("dfu_rp", caps.dfu_rp, "enable [dfu] on an rp2040 chip"),
            ("dfu_nrf", caps.dfu_nrf, "enable [dfu] on an nRF chip"),
            ("steno", caps.steno, "set `steno = true` in [keyboard]"),
            (
                "passkey_entry",
                caps.passkey_entry,
                "set `passkey_entry = true` in [ble]",
            ),
            (
                "adafruit_bl",
                caps.adafruit_bl,
                "set `bootloader = \"adafruit\"` in [keyboard]",
            ),
            (
                "zsa_voyager_bl",
                caps.zsa_voyager_bl,
                "set `bootloader = \"zsa_voyager\"` in [keyboard]",
            ),
        ];
        for (feature, resolved_on, fix) in contradictions {
            if features.contains(feature) && !resolved_on {
                errs.push(format!(
                    "feature `{feature}` is enabled in Cargo.toml but keyboard.toml resolves it to disabled — {fix}, or drop the feature"
                ));
            }
        }
        if features.contains("_no_usb") && caps.usb {
            errs.push(
                "feature `_no_usb` (usually via a chip BLE alias) conflicts with `usb_enable = true` in [keyboard] — the chip alias may not match [keyboard].chip"
                    .to_string(),
            );
        }

        // Capabilities that need a dependency-gating feature the build lacks.
        if caps.ble && !features.contains("_ble") {
            errs.push(format!(
                "[ble] is enabled in keyboard.toml but no BLE feature is active — add `{}` to rmk's features in Cargo.toml",
                ble_feature_hint(&facts.chip)
            ));
        }
        if caps.dfu {
            let flavor = match facts.chip.series {
                ChipSeries::Rp2040 => Some("dfu_rp"),
                ChipSeries::Nrf52 if facts.chip.chip.starts_with("nrf54") => None,
                ChipSeries::Nrf52 => Some("dfu_nrf"),
                ChipSeries::Stm32 => Some("dfu"),
                ChipSeries::Esp32 => None,
            };
            if let Some(feature) = flavor
                && !features.contains(feature)
            {
                errs.push(format!(
                    "[dfu] is enabled in keyboard.toml but the `{feature}` feature is missing — add it to rmk's features in Cargo.toml"
                ));
            }
        }
        if caps.adafruit_bl && !features.contains("adafruit_bl") {
            errs.push(
                "`bootloader = \"adafruit\"` requires the `adafruit_bl` feature — add it to rmk's features in Cargo.toml"
                    .to_string(),
            );
        }
        let missing_display: HashSet<&'static str> = facts
            .display_drivers
            .iter()
            .map(display_family_feature)
            .filter(|f| !features.contains(f))
            .collect();
        for feature in missing_display {
            errs.push(format!(
                "[display] needs the `{feature}` feature — add it to rmk's features in Cargo.toml"
            ));
        }

        caps.check_invariants(&mut errs);
        caps.check_usb_log(features, &mut errs);
        if errs.is_empty() { Ok(caps) } else { Err(errs) }
    }

    fn check_invariants(&self, errs: &mut Vec<String>) {
        if self.vial && self.rynk {
            errs.push(
                "Vial and Rynk are mutually exclusive — enable only one of `vial_enabled`/`rynk_enabled` in [host] (or the `vial`/`rynk` features)"
                    .to_string(),
            );
        }
        if self.host && !(self.vial || self.rynk) {
            errs.push("`host` requires `vial` or `rynk`".to_string());
        }
        if self.bulk && !self.rynk {
            errs.push("`bulk` requires `rynk`".to_string());
        }
        if self.passkey_entry && !self.ble {
            errs.push("passkey entry requires BLE — enable [ble] (or a chip BLE feature)".to_string());
        }
    }

    /// `usb_log` stays a plain feature; these are its only cross-checks.
    fn check_usb_log(&self, features: &ActiveFeatures, errs: &mut Vec<String>) {
        if features.contains("usb_log") {
            if !self.usb {
                errs.push("`usb_log` requires USB — it cannot be used on a USB-less build".to_string());
            }
            if self.usb_high_speed {
                errs.push(
                    "`usb_log` is incompatible with high-speed USB (`embassy-usb-logger` caps packets at 64 bytes)"
                        .to_string(),
                );
            }
        }
    }
}

impl KeyboardTomlConfig {
    /// Minimal rmk feature list this keyboard.toml requires — dependency-gating
    /// features only, since capabilities activate from the toml itself. Used by
    /// project generators and the example-consistency test.
    pub fn firmware_features(&self) -> Result<Vec<String>, String> {
        let (caps, facts, errs) = Capabilities::toml_caps(self).map_err(|e| e.join("\n"))?;
        if !errs.is_empty() {
            return Err(errs.join("\n"));
        }
        let mut features = Vec::new();
        if caps.ble {
            match facts.chip.series {
                ChipSeries::Nrf52 | ChipSeries::Esp32 => features.push(format!("{}_ble", facts.chip.chip)),
                ChipSeries::Rp2040 => features.push("pico_w_ble".to_string()),
                ChipSeries::Stm32 => unreachable!("rejected by toml_caps"),
            }
        }
        if facts.chip.series == ChipSeries::Rp2040 {
            features.push("rp2040".to_string());
        }
        if caps.dfu_rp {
            features.push("dfu_rp".to_string());
        } else if caps.dfu_nrf {
            features.push("dfu_nrf".to_string());
        } else if caps.dfu {
            features.push("dfu".to_string());
        }
        if caps.adafruit_bl {
            features.push("adafruit_bl".to_string());
        }
        features.extend(
            facts
                .display_drivers
                .iter()
                .map(|d| display_family_feature(d).to_string()),
        );
        features.sort();
        features.dedup();
        Ok(features)
    }
}

fn ble_feature_hint(chip: &ChipModel) -> String {
    match chip.series {
        ChipSeries::Nrf52 | ChipSeries::Esp32 => format!("{}_ble", chip.chip),
        ChipSeries::Rp2040 => "pico_w_ble".to_string(),
        ChipSeries::Stm32 => "a BLE-capable chip feature".to_string(),
    }
}

fn display_family_feature(driver: &DisplayDriver) -> &'static str {
    match driver {
        DisplayDriver::Ssd1306 => "ssd1306",
        DisplayDriver::Sh1106 | DisplayDriver::Sh1107 | DisplayDriver::Sh1108 | DisplayDriver::Ssd1309 => "oled_async",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(s: &str) -> KeyboardTomlConfig {
        toml::from_str(s).unwrap()
    }

    fn feats(names: &[&str]) -> ActiveFeatures {
        ActiveFeatures::from_names(names)
    }

    fn err_text(result: Result<Capabilities, Vec<String>>) -> String {
        result.expect_err("expected resolution errors").join("\n")
    }

    const RP2040_USB: &str = r#"
[keyboard]
name = "t"
vendor_id = 0x4c4b
product_id = 0x4643
chip = "rp2040"
usb_enable = true
"#;

    const NRF52840_BLE: &str = r#"
[keyboard]
name = "t"
vendor_id = 0x4c4b
product_id = 0x4643
chip = "nrf52840"
usb_enable = true

[ble]
enabled = true
"#;

    // Cargo's closure of `nrf52840_ble` + `storage`, as CARGO_FEATURE_* exposes it.
    const NRF_BLE_CLOSURE: &[&str] = &["nrf52840_ble", "_nrf_ble", "_ble", "storage", "defmt"];

    #[test]
    fn featureland_matches_feature_closure() {
        let caps = Capabilities::resolve(None, &feats(NRF_BLE_CLOSURE)).unwrap();
        assert!(caps.ble && caps.storage && caps.usb);
        assert!(!caps.split && !caps.vial && !caps.watchdog);
    }

    #[test]
    fn featureland_no_usb_inverts_polarity() {
        let caps = Capabilities::resolve(None, &feats(&["_ble", "storage", "_no_usb"])).unwrap();
        assert!(!caps.usb && caps.ble);
    }

    #[test]
    fn toml_defaults_activate_core_capabilities() {
        let caps = Capabilities::resolve(Some(&cfg(RP2040_USB)), &feats(&["defmt"])).unwrap();
        assert!(caps.usb && caps.storage && caps.vial && caps.host && caps.watchdog);
        // Vial without [host].unlock_keys stays lock-free (saves flash).
        assert!(!caps.host_lock);
        assert!(!caps.ble && !caps.split && !caps.rynk && !caps.async_matrix && !caps.steno);
    }

    #[test]
    fn host_unlock_keys_enable_the_lock() {
        let toml = cfg(&format!("{RP2040_USB}\n[host]\nunlock_keys = [[0, 0]]"));
        let caps = Capabilities::resolve(Some(&toml), &feats(&[])).unwrap();
        assert!(caps.vial && caps.host_lock);

        let toml = cfg(&format!(
            "{RP2040_USB}\n[host]\nvial_enabled = false\nrynk_enabled = true"
        ));
        let caps = Capabilities::resolve(Some(&toml), &feats(&[])).unwrap();
        assert!(caps.rynk && caps.host_lock);
    }

    #[test]
    fn constants_only_toml_falls_back_to_features() {
        let caps = Capabilities::resolve(Some(&cfg("[rmk]\ncombo_max_num = 2")), &feats(&["split"])).unwrap();
        assert!(caps.split && caps.usb && !caps.watchdog);
    }

    #[test]
    fn toml_watchdog_off_wins_over_default_feature() {
        let toml = cfg(&format!("{RP2040_USB}watchdog = false"));
        let caps = Capabilities::resolve(Some(&toml), &feats(&["defmt", "watchdog", "storage", "vial"])).unwrap();
        assert!(!caps.watchdog);
    }

    #[test]
    fn vial_rynk_exclusive_in_toml() {
        let toml = cfg(&format!(
            "{RP2040_USB}\n[host]\nvial_enabled = true\nrynk_enabled = true"
        ));
        assert!(err_text(Capabilities::resolve(Some(&toml), &feats(&[]))).contains("mutually exclusive"));
    }

    #[test]
    fn vial_rynk_exclusive_across_sources() {
        let toml = cfg(&format!(
            "{RP2040_USB}\n[host]\nvial_enabled = false\nrynk_enabled = true"
        ));
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(&["vial"])));
        assert!(err.contains("feature `vial`"), "{err}");
    }

    #[test]
    fn host_requires_vial_or_rynk_in_featureland() {
        let err = err_text(Capabilities::resolve(None, &feats(&["host"])));
        assert!(err.contains("`host` requires"), "{err}");
    }

    #[test]
    fn storage_contradiction() {
        let toml = cfg(&format!("{RP2040_USB}\n[storage]\nenabled = false"));
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(&["storage"])));
        assert!(err.contains("feature `storage`"), "{err}");
    }

    #[test]
    fn ble_requires_chip_feature_with_hint() {
        let err = err_text(Capabilities::resolve(Some(&cfg(NRF52840_BLE)), &feats(&["defmt"])));
        assert!(err.contains("nrf52840_ble"), "{err}");
    }

    #[test]
    fn ble_closure_resolves() {
        let caps = Capabilities::resolve(Some(&cfg(NRF52840_BLE)), &feats(NRF_BLE_CLOSURE)).unwrap();
        assert!(caps.ble && caps.usb && caps.storage);
    }

    #[test]
    fn ble_requires_storage_in_toml() {
        let toml = cfg(&format!("{NRF52840_BLE}\n[storage]\nenabled = false"));
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(NRF_BLE_CLOSURE)));
        assert!(err.contains("bond persistence"), "{err}");
    }

    #[test]
    fn no_usb_feature_conflicts_with_usb_toml() {
        let err = err_text(Capabilities::resolve(
            Some(&cfg(NRF52840_BLE)),
            &feats(&["_ble", "storage", "_no_usb"]),
        ));
        assert!(err.contains("_no_usb"), "{err}");
    }

    #[test]
    fn usb_log_requires_usb() {
        let toml = cfg(&format!(
            "{}",
            NRF52840_BLE.replace("usb_enable = true", "usb_enable = false")
        ));
        let err = err_text(Capabilities::resolve(
            Some(&toml),
            &feats(&["_ble", "storage", "usb_log"]),
        ));
        assert!(err.contains("`usb_log` requires USB"), "{err}");
    }

    #[test]
    fn usb_log_conflicts_with_high_speed() {
        let err = err_text(Capabilities::resolve(None, &feats(&["usb_log", "_usb_high_speed"])));
        assert!(err.contains("high-speed"), "{err}");
    }

    #[test]
    fn dfu_requires_chip_flavor_feature() {
        let toml = cfg(&format!("{RP2040_USB}\n[dfu]\nenabled = true"));
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(&[])));
        assert!(err.contains("`dfu_rp`"), "{err}");
    }

    #[test]
    fn dfu_flavor_derives_from_chip() {
        let toml = cfg(&format!("{RP2040_USB}\n[dfu]\nenabled = true"));
        let caps = Capabilities::resolve(Some(&toml), &feats(&["dfu", "dfu_rp"])).unwrap();
        assert!(caps.dfu && caps.dfu_rp && !caps.dfu_nrf);
    }

    #[test]
    fn dfu_flavor_mismatch_is_reported() {
        let toml = cfg(&format!("{NRF52840_BLE}\n[dfu]\nenabled = true"));
        let mut features = NRF_BLE_CLOSURE.to_vec();
        features.extend(["dfu", "dfu_rp"]);
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(&features)));
        assert!(err.contains("feature `dfu_rp`") && err.contains("`dfu_nrf`"), "{err}");
    }

    #[test]
    fn dfu_unlock_keys_enable_dfu_lock() {
        let toml = cfg(&format!(
            "{RP2040_USB}\n[dfu]\nenabled = true\nunlock_keys = [[0, 0], [0, 1]]"
        ));
        let caps = Capabilities::resolve(Some(&toml), &feats(&["dfu", "dfu_rp"])).unwrap();
        assert!(caps.dfu_lock && caps.host_lock);
    }

    #[test]
    fn bulk_requires_rynk() {
        let err = err_text(Capabilities::resolve(None, &feats(&["bulk"])));
        assert!(err.contains("`bulk` requires `rynk`"), "{err}");
    }

    #[test]
    fn passkey_requires_ble() {
        let err = err_text(Capabilities::resolve(None, &feats(&["passkey_entry"])));
        assert!(err.contains("passkey entry requires BLE"), "{err}");
    }

    #[test]
    fn async_matrix_contradiction() {
        let err = err_text(Capabilities::resolve(Some(&cfg(RP2040_USB)), &feats(&["async_matrix"])));
        assert!(err.contains("feature `async_matrix`"), "{err}");
    }

    #[test]
    fn async_matrix_from_toml() {
        let toml = cfg(&format!("{RP2040_USB}async_matrix = true"));
        let caps = Capabilities::resolve(Some(&toml), &feats(&[])).unwrap();
        assert!(caps.async_matrix);
    }

    #[test]
    fn split_feature_without_section_is_rejected() {
        let err = err_text(Capabilities::resolve(Some(&cfg(RP2040_USB)), &feats(&["split"])));
        assert!(err.contains("feature `split`"), "{err}");
    }

    #[test]
    fn steno_explicit_off_contradicts_feature() {
        let toml = cfg(&format!("{RP2040_USB}steno = false"));
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(&["steno"])));
        assert!(err.contains("feature `steno`"), "{err}");
    }

    #[test]
    fn steno_unset_unions_with_feature() {
        let caps = Capabilities::resolve(Some(&cfg(RP2040_USB)), &feats(&["steno"])).unwrap();
        assert!(caps.steno);
    }

    #[test]
    fn adafruit_bootloader_requires_feature() {
        let toml = cfg(&NRF52840_BLE.replace("usb_enable = true", "usb_enable = true\nbootloader = \"adafruit\""));
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(NRF_BLE_CLOSURE)));
        assert!(err.contains("`adafruit_bl`"), "{err}");
    }

    #[test]
    fn zsa_bootloader_activates_from_toml_alone() {
        let toml = cfg(&format!("{RP2040_USB}bootloader = \"zsa_voyager\""));
        let caps = Capabilities::resolve(Some(&toml), &feats(&[])).unwrap();
        assert!(caps.zsa_voyager_bl && !caps.adafruit_bl);
    }

    const DISPLAY_SECTION: &str = r#"
[display]
driver = "sh1106"
size = "128x64"

[display.protocol.i2c]
instance = "I2C0"
sda = "PIN_0"
scl = "PIN_1"
"#;

    #[test]
    fn display_requires_family_feature() {
        let toml = cfg(&format!("{RP2040_USB}{DISPLAY_SECTION}"));
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(&[])));
        assert!(err.contains("`oled_async`"), "{err}");
    }

    #[test]
    fn display_resolves_with_family_feature() {
        let toml = cfg(&format!("{RP2040_USB}{DISPLAY_SECTION}"));
        let caps = Capabilities::resolve(Some(&toml), &feats(&["oled_async", "display", "sh1106"])).unwrap();
        assert!(caps.display);
    }

    #[test]
    fn forwarded_resolution_skips_feature_validation() {
        // rmk-types sees only forwarded features (no chip aliases, no display
        // family, no dfu flavor) — values resolve, nothing misfires.
        let toml = cfg(&format!("{NRF52840_BLE}{DISPLAY_SECTION}\n[dfu]\nenabled = true"));
        let caps = Capabilities::resolve_forwarded(Some(&toml), &feats(&["_ble", "split"])).unwrap();
        assert!(caps.ble && caps.display && caps.dfu && caps.dfu_nrf);
        // The full resolution rejects the same inputs.
        assert!(Capabilities::resolve(Some(&toml), &feats(&["_ble", "split"])).is_err());
    }

    #[test]
    fn firmware_features_covers_chip_dfu_bootloader_display() {
        let toml = cfg(&format!(
            "{}\n[dfu]\nenabled = true",
            NRF52840_BLE.replace("usb_enable = true", "usb_enable = true\nbootloader = \"adafruit\"")
        ));
        let features = toml.firmware_features().unwrap();
        assert_eq!(features, vec!["adafruit_bl", "dfu_nrf", "nrf52840_ble"]);

        let toml = cfg(&format!("{RP2040_USB}{DISPLAY_SECTION}"));
        let features = toml.firmware_features().unwrap();
        assert_eq!(features, vec!["oled_async", "rp2040"]);
    }

    /// Load through the real chip-default merge (nrf54 defaults are new).
    fn load_with_chip_defaults(name: &str, user_toml: &str) -> KeyboardTomlConfig {
        let path = std::env::temp_dir().join(format!(
            "rmk-caps-{name}-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, user_toml).unwrap();
        let config = KeyboardTomlConfig::load_for_build(&path).unwrap();
        std::fs::remove_file(&path).ok();
        config
    }

    #[test]
    fn nrf54_resolves_with_chip_defaults() {
        let base = "[keyboard]\nname = \"t\"\nvendor_id = 1\nproduct_id = 1\n";

        // nrf54lm20: high-speed USB + BLE from the chip defaults.
        let toml = load_with_chip_defaults("lm20", &format!("{base}chip = \"nrf54lm20\""));
        let closure = ["nrf54lm20_ble", "_nrf_ble", "_ble", "_usb_high_speed", "storage"];
        let caps = Capabilities::resolve(Some(&toml), &feats(&closure)).unwrap();
        assert!(caps.usb && caps.usb_high_speed && caps.ble && caps.storage);
        assert_eq!(toml.firmware_features().unwrap(), vec!["nrf54lm20_ble"]);
        // The hint names the chip feature when BLE is on without it.
        let err = err_text(Capabilities::resolve(Some(&toml), &feats(&["defmt"])));
        assert!(err.contains("nrf54lm20_ble"), "{err}");

        // nrf54l15: BLE-only, no USB.
        let toml = load_with_chip_defaults("l15", &format!("{base}chip = \"nrf54l15\""));
        let caps = Capabilities::resolve(
            Some(&toml),
            &feats(&["nrf54l15_ble", "_nrf_ble", "_ble", "_no_usb", "storage"]),
        )
        .unwrap();
        assert!(!caps.usb && caps.ble);
    }

    #[test]
    fn dfu_is_rejected_on_nrf54() {
        let toml = cfg(
            "[keyboard]\nname = \"t\"\nvendor_id = 1\nproduct_id = 1\nchip = \"nrf54lm20\"\nusb_enable = true\n\n[ble]\nenabled = true\n\n[dfu]\nenabled = true",
        );
        let err = err_text(Capabilities::resolve(
            Some(&toml),
            &feats(&["nrf54lm20_ble", "_nrf_ble", "_ble", "_usb_high_speed", "storage"]),
        ));
        assert!(err.contains("not supported on nRF54"), "{err}");
    }
}
