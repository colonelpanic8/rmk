//! Keyboard state events

use rmk_macro::event;
use rmk_types::led_indicator::LedIndicator;

/// Active layer changed event
#[event(channel_size = crate::LAYER_CHANGE_EVENT_CHANNEL_SIZE, pubs = crate::LAYER_CHANGE_EVENT_PUB_SIZE, subs = crate::LAYER_CHANGE_EVENT_SUB_SIZE)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LayerChangeEvent(pub u8);

impl LayerChangeEvent {
    pub fn new(layer: u8) -> Self {
        Self(layer)
    }
}

impl_payload_wrapper!(LayerChangeEvent, u8);

/// The runtime auto mouse layer table changed and its runner must reload it.
#[event(channel_size = 1, pubs = 1, subs = 1)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct AutoMouseLayerConfigChangeEvent;

/// WPM updated event
#[event(channel_size = crate::WPM_UPDATE_EVENT_CHANNEL_SIZE, pubs = crate::WPM_UPDATE_EVENT_PUB_SIZE, subs = crate::WPM_UPDATE_EVENT_SUB_SIZE)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct WpmUpdateEvent(pub u16);

impl WpmUpdateEvent {
    pub fn new(wpm: u16) -> Self {
        Self(wpm)
    }
}

impl_payload_wrapper!(WpmUpdateEvent, u16);

/// LED indicator state changed event
#[event(channel_size = crate::LED_INDICATOR_EVENT_CHANNEL_SIZE, pubs = crate::LED_INDICATOR_EVENT_PUB_SIZE, subs = crate::LED_INDICATOR_EVENT_SUB_SIZE)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LedIndicatorEvent(pub LedIndicator);

impl LedIndicatorEvent {
    pub fn new(indicator: LedIndicator) -> Self {
        Self(indicator)
    }
}

impl_payload_wrapper!(LedIndicatorEvent, LedIndicator);

/// Sleep state changed event
#[event(channel_size = crate::SLEEP_STATE_EVENT_CHANNEL_SIZE, pubs = crate::SLEEP_STATE_EVENT_PUB_SIZE, subs = crate::SLEEP_STATE_EVENT_SUB_SIZE)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct SleepStateEvent(pub bool);

impl SleepStateEvent {
    pub fn new(sleeping: bool) -> Self {
        Self(sleeping)
    }
}

impl_payload_wrapper!(SleepStateEvent, bool);

/// Live maintenance policy changed.
#[event(channel_size = crate::MAINTENANCE_MODE_EVENT_CHANNEL_SIZE, pubs = crate::MAINTENANCE_MODE_EVENT_PUB_SIZE, subs = crate::MAINTENANCE_MODE_EVENT_SUB_SIZE)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MaintenanceModeEvent(pub bool);

impl_payload_wrapper!(MaintenanceModeEvent, bool);

/// Authoritative lighting state or application-owned render context changed.
///
/// This is deliberately a unit invalidation: consumers read a fresh state
/// snapshot from the lighting controller instead of trusting an event mirror.
#[event(channel_size = crate::LIGHTING_CHANGED_EVENT_CHANNEL_SIZE, pubs = crate::LIGHTING_CHANGED_EVENT_PUB_SIZE, subs = crate::LIGHTING_CHANGED_EVENT_SUB_SIZE)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LightingChangedEvent;

impl LightingChangedEvent {
    pub const fn new() -> Self {
        Self
    }
}
