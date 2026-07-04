use std::collections::HashMap;

use serde::Deserialize;

use crate::duration::DurationMillis;

/// The three decision-mode flags are documented as mutually exclusive; a config
/// enabling two would silently pick whichever the firmware checks first.
fn check_morse_mode_exclusive(
    permissive_hold: Option<bool>,
    hold_on_other_press: Option<bool>,
    normal_mode: Option<bool>,
    ctx: &str,
) -> Result<(), String> {
    let enabled = [permissive_hold, hold_on_other_press, normal_mode]
        .into_iter()
        .flatten()
        .filter(|enabled| *enabled)
        .count();
    if enabled > 1 {
        return Err(format!(
            "keyboard.toml: {ctx} enables more than one of `permissive_hold`, `hold_on_other_press` and `normal_mode` — they are mutually exclusive decision modes, set at most one"
        ));
    }
    Ok(())
}

impl crate::KeyboardTomlConfig {
    /// Behavior action strings (combos, forks, morse, encoders live in keymap.rs)
    /// get the same alias/layer-name/range resolution as `[[keymap.layer]].keys`;
    /// without this, `MO(9)` in a morse hold or `MO(nav)` in a fork would slip
    /// through to codegen and die at runtime or fail deep in the macro.
    fn resolve_behavior_actions(&self, behavior: &mut BehaviorConfig, num_layers: u8) -> Result<(), String> {
        let aliases = self.aliases.clone().unwrap_or_default();
        let layers = self.keymap.as_ref().map(|k| k.layer.as_slice()).unwrap_or(&[]);
        let layer_names = Self::collect_layer_names(layers)?;
        let resolve = |action: &mut String, ctx: String| -> Result<(), String> {
            *action = Self::resolve_single_action(action, &aliases, &layer_names, num_layers)
                .map_err(|e| format!("keyboard.toml: {ctx}: {e}"))?;
            Ok(())
        };

        if let Some(combo) = &mut behavior.combo {
            for (i, c) in combo.combos.iter_mut().enumerate() {
                for action in &mut c.actions {
                    resolve(action, format!("combo #{i} actions"))?;
                }
                resolve(&mut c.output, format!("combo #{i} output"))?;
            }
        }
        if let Some(fork) = &mut behavior.fork {
            for (i, f) in fork.forks.iter_mut().enumerate() {
                resolve(&mut f.trigger, format!("fork #{i} trigger"))?;
                resolve(&mut f.negative_output, format!("fork #{i} negative_output"))?;
                resolve(&mut f.positive_output, format!("fork #{i} positive_output"))?;
            }
        }
        if let Some(morse) = &mut behavior.morse
            && let Some(morses) = &mut morse.morses
        {
            for (i, m) in morses.iter_mut().enumerate() {
                for action in [&mut m.tap, &mut m.hold, &mut m.hold_after_tap, &mut m.double_tap]
                    .into_iter()
                    .flatten()
                {
                    resolve(action, format!("morse #{i}"))?;
                }
                for actions in [&mut m.tap_actions, &mut m.hold_actions].into_iter().flatten() {
                    for action in actions {
                        resolve(action, format!("morse #{i}"))?;
                    }
                }
                if let Some(pairs) = &mut m.morse_actions {
                    for pair in pairs {
                        resolve(&mut pair.action, format!("morse #{i}"))?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn get_behavior_config(&self) -> Result<BehaviorConfig, String> {
        let default = self.behavior.clone().unwrap_or_default();
        // `layers` defaults to the number of `[[keymap.layer]]` blocks (see get_keymap_config).
        let num_layers = self
            .keymap
            .as_ref()
            .map(|k| k.layers.unwrap_or(k.layer.len() as u8))
            .unwrap_or_default();
        match self.behavior.clone() {
            Some(mut behavior) => {
                behavior.tri_layer = match behavior.tri_layer {
                    Some(tri_layer) => {
                        if tri_layer.upper >= num_layers {
                            return Err("keyboard.toml: Tri layer upper is larger than [keymap].layers".to_string());
                        } else if tri_layer.lower >= num_layers {
                            return Err("keyboard.toml: Tri layer lower is larger than [keymap].layers".to_string());
                        } else if tri_layer.adjust >= num_layers {
                            return Err("keyboard.toml: Tri layer adjust is larger than [keymap].layers".to_string());
                        }
                        Some(tri_layer)
                    }
                    None => default.tri_layer,
                };
                behavior.one_shot = behavior.one_shot.or(default.one_shot);
                behavior.one_shot_modifiers = behavior.one_shot_modifiers.or(default.one_shot_modifiers);
                behavior.combo = behavior.combo.or(default.combo);
                if let Some(combo) = &behavior.combo {
                    if combo.combos.len() > self.rmk.combo_max_num {
                        return Err("keyboard.toml: number of combos is greater than combo_max_num configured under [rmk] section".to_string());
                    }
                    for (i, c) in combo.combos.iter().enumerate() {
                        if c.actions.len() > self.rmk.combo_max_length {
                            return Err(format!(
                                "keyboard.toml: number of keys in combo #{} is greater than combo_max_length configured under [rmk] section",
                                i
                            ));
                        }
                        if let Some(layer) = c.layer
                            && layer >= num_layers
                        {
                            return Err(format!(
                                "keyboard.toml: layer in combo #{} is greater than [keymap].layers",
                                i
                            ));
                        }
                    }
                }
                behavior.macros = behavior.macros.or(default.macros);
                if let Some(macros) = &behavior.macros {
                    let macros_size = macros
                        .macros
                        .iter()
                        .map(|m| {
                            m.operations
                                .iter()
                                .map(|op| match op {
                                    MacroOperation::Tap { .. }
                                    | MacroOperation::Down { .. }
                                    | MacroOperation::Up { .. } => 3,
                                    MacroOperation::Delay { .. } => 4,
                                    MacroOperation::Text { text } => text.len(),
                                })
                                .sum::<usize>()
                        })
                        .sum::<usize>();

                    if macros_size > self.rmk.macro_space_size {
                        return Err(format!(
                            "keyboard.toml: total size of macros ({}) is greater than macro_space_size configured under [rmk] section",
                            macros_size
                        ));
                    }
                }
                behavior.fork = behavior.fork.or(default.fork);
                if let Some(fork) = &behavior.fork
                    && fork.forks.len() > self.rmk.fork_max_num
                {
                    return Err(
                        "keyboard.toml: number of forks is greater than fork_max_num configured under [rmk] section"
                            .to_string(),
                    );
                }
                behavior.morse = behavior.morse.or(default.morse);
                if let Some(morse) = &behavior.morse
                    && let Some(morses) = &morse.morses
                    && morses.len() > self.rmk.morse_max_num
                {
                    return Err(
                        "keyboard.toml: number of morses is greater than morse_max_num configured under [rmk] section"
                            .to_string(),
                    );
                }
                if let Some(morse) = &behavior.morse {
                    check_morse_mode_exclusive(
                        morse.permissive_hold,
                        morse.hold_on_other_press,
                        morse.normal_mode,
                        "[behavior.morse]",
                    )?;
                    if let Some(profiles) = &morse.profiles {
                        for (name, profile) in profiles {
                            check_morse_mode_exclusive(
                                profile.permissive_hold,
                                profile.hold_on_other_press,
                                profile.normal_mode,
                                &format!("profile '{name}' in [behavior.morse.profiles]"),
                            )?;
                        }
                    }
                }
                self.resolve_behavior_actions(&mut behavior, num_layers)?;
                Ok(behavior)
            }
            None => Ok(default),
        }
    }
}

/// Configurations for actions behavior
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BehaviorConfig {
    pub tri_layer: Option<TriLayerConfig>,
    pub one_shot: Option<OneShotConfig>,
    pub one_shot_modifiers: Option<OneShotModifiersConfig>,
    pub combo: Option<CombosConfig>,
    #[serde(alias = "macro")]
    pub macros: Option<MacrosConfig>,
    pub fork: Option<ForksConfig>,
    pub morse: Option<MorsesConfig>,
}

/// Per Key configurations profiles for morse, tap-hold, etc.
/// overrides the defaults given in TapHoldConfig
#[derive(Clone, Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct MorseProfile {
    pub enable_flow_tap: Option<bool>,

    /// if true, tap-hold key will always send tap action when tapped with the same hand only
    pub unilateral_tap: Option<bool>,

    /// The decision mode of the morse/tap-hold key (only one of permissive_hold, hold_on_other_press and normal_mode can be true)
    /// /// if none of them is given, normal mode will be the default
    pub permissive_hold: Option<bool>,
    pub hold_on_other_press: Option<bool>,
    pub normal_mode: Option<bool>,

    /// If the key is pressed longer than this, it is accepted as `hold` (in milliseconds)
    pub hold_timeout: Option<DurationMillis>,

    /// The time elapsed from the last release of a key is longer than this, it will break the morse pattern (in milliseconds)
    pub gap_timeout: Option<DurationMillis>,
}

/// Configurations for tri layer
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TriLayerConfig {
    pub upper: u8,
    pub lower: u8,
    pub adjust: u8,
}

/// Configurations for oneshot modifiers/layers
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OneShotConfig {
    pub timeout: Option<DurationMillis>,
}

/// Configurations for oneshot modifiers
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneShotModifiersConfig {
    pub activate_on_keypress: Option<bool>,
    pub quick_release: Option<bool>,
}

/// Configurations for combos
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CombosConfig {
    #[serde(default)]
    pub combos: Vec<ComboConfig>,
    pub timeout: Option<DurationMillis>,
    pub prior_idle_time: Option<DurationMillis>,
}

/// Configurations for combo
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComboConfig {
    pub actions: Vec<String>,
    pub output: String,
    pub layer: Option<u8>,
}

/// Configurations for macros
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MacrosConfig {
    pub macros: Vec<MacroConfig>,
}

/// Configurations for macro
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MacroConfig {
    pub operations: Vec<MacroOperation>,
}

/// Macro operations (TOML deserialization type — resolved equivalent is in `resolved::behavior`)
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "lowercase")]
pub(crate) enum MacroOperation {
    Tap { keycode: String },
    Down { keycode: String },
    Up { keycode: String },
    Delay { duration: DurationMillis },
    Text { text: String },
}

/// Configurations for forks
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForksConfig {
    pub forks: Vec<ForkConfig>,
}

/// Configurations for fork
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkConfig {
    pub trigger: String,
    pub negative_output: String,
    pub positive_output: String,
    pub match_any: Option<String>,
    pub match_none: Option<String>,
    pub kept_modifiers: Option<String>,
    pub bindable: Option<bool>,
}

/// Configurations for morse keys
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MorsesConfig {
    pub enable_flow_tap: Option<bool>, //default: false
    /// used in permissive_hold mode
    pub prior_idle_time: Option<DurationMillis>,

    /// if true, tap-hold key will always send tap action when tapped with the same hand only
    pub unilateral_tap: Option<bool>,

    /// The decision mode of the morse/tap-hold key (only one of permissive_hold, hold_on_other_press and normal_mode can be true)
    /// if none of them is given, normal mode will be the default
    pub permissive_hold: Option<bool>,
    pub hold_on_other_press: Option<bool>,
    pub normal_mode: Option<bool>,

    /// If the key is pressed longer than this, it is accepted as `hold` (in milliseconds)
    pub hold_timeout: Option<DurationMillis>,

    /// The time elapsed from the last release of a key is longer than this, it will break the morse pattern (in milliseconds)
    pub gap_timeout: Option<DurationMillis>,

    /// these can be used to overrides the defaults given above
    pub profiles: Option<HashMap<String, MorseProfile>>,

    /// the definition of morse / tap dance keys
    pub morses: Option<Vec<MorseConfig>>,
}

/// Configurations for morse
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MorseConfig {
    // name of morse profile (to address BehaviorConfig::morse.profiles[self.profile])
    pub profile: Option<String>,

    pub tap: Option<String>,
    pub hold: Option<String>,
    pub hold_after_tap: Option<String>,
    pub double_tap: Option<String>,
    /// Array of tap actions for each tap count (0-indexed)
    pub tap_actions: Option<Vec<String>>,
    /// Array of hold actions for each tap count (0-indexed)
    pub hold_actions: Option<Vec<String>>,
    /// Array of morse patter->action pairs  count (0-indexed)
    pub morse_actions: Option<Vec<MorseActionPair>>,
}

/// Configurations for morse action pairs
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MorseActionPair {
    pub pattern: String, // for example morse code of "B": "-..." or "_..." or "1000"
    pub action: String,  // "B"
}
