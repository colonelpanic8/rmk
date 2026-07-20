//! Lighting control events.

use rmk_macro::event;
use rmk_types::action::LightAction;

/// A command for the active lighting controller.
///
/// Key actions are the first producer. Future RMK versions can add commands for
/// host protocol adapters without making those adapters part of rendering or
/// hardware implementations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum LightingCommand {
    /// A resolved RMK light action and its key edge.
    Action { action: LightAction, pressed: bool },
}

/// A serialized mutation request for the active lighting controller.
#[event(
    channel_size = crate::LIGHTING_COMMAND_EVENT_CHANNEL_SIZE,
    pubs = crate::LIGHTING_COMMAND_EVENT_PUB_SIZE,
    subs = crate::LIGHTING_COMMAND_EVENT_SUB_SIZE
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LightingCommandEvent(pub LightingCommand);

impl LightingCommandEvent {
    /// Create a command from a resolved keymap light action.
    pub fn from_action(action: LightAction, pressed: bool) -> Self {
        Self(LightingCommand::Action { action, pressed })
    }
}

impl_payload_wrapper!(LightingCommandEvent, LightingCommand);
