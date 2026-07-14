//! Resolved configuration types — the public API of `rmk-config`.
//!
//! These types represent the final, validated, defaults-applied output of the
//! 3-layer TOML merge (event defaults → chip defaults → user config).
//!
//! There are eight resolved entry points, each consumed at a different stage:
//!
//! - [`Capabilities`] — which capabilities are compiled in; the single
//!   authority behind the `rmk_*` cfgs and the proc-macro's codegen decisions
//! - [`BuildConstants`] — compile-time constants emitted by `rmk-types/build.rs`
//! - [`Identity`] — keyboard identity for USB descriptors and BLE advertising
//! - [`Hardware`] — complete hardware config for proc-macro code generation
//! - [`Host`] — host-tool configuration such as Vial support
//! - [`Behavior`] — behavioral config (combos, macros, morse, forks, etc.)
//! - [`Keymap`] — keymap and encoder data for keymap generation
//! - [`Layout`] — the physical layout blob streamed over `GetLayout`
//!
//! Consumers call resolution methods on [`KeyboardTomlConfig`](crate::KeyboardTomlConfig)
//! or the [`Capabilities`] constructors:
//! - `Capabilities::resolve(toml, features)` → `Result<Capabilities, Vec<String>>` (build scripts)
//! - `Capabilities::from_toml(&toml)` → `Result<Capabilities, String>` (proc-macro)
//! - `.build_constants(&capabilities)` → `Result<BuildConstants, String>`
//! - `.firmware_features()` → `Result<Vec<String>, String>` (project generators)
//! - `.identity()` → `Result<Identity, String>`
//! - `.hardware()` → `Result<Hardware, String>`
//! - `.host()` → `Host`
//! - `.behavior()` → `Result<Behavior, String>`
//! - `.keymap()` → `Result<Keymap, String>`
//! - `.layout()` → `Result<Layout, String>`
//!
//! Supporting types stay namespaced under their module to avoid flattening the
//! public API with overly generic names.

pub mod behavior;
pub mod build_constants;
pub mod capabilities;
pub mod hardware;
pub mod host;
pub mod identity;
pub mod keymap;
pub mod layout;

pub use behavior::Behavior;
pub use build_constants::BuildConstants;
pub use capabilities::{ActiveFeatures, Capabilities};
pub use hardware::Hardware;
pub use host::Host;
pub use identity::Identity;
pub use keymap::Keymap;
pub use layout::Layout;

// Re-export constants used by codegen
pub use crate::keycode_alias::KEYCODE_ALIAS;
