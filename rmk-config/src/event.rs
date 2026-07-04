//! `[event]` per-event channel configuration and its defaults.

use serde::Deserialize;

/// Event channel default configuration
pub(crate) const EVENT_DEFAULT_CONFIG: &str = include_str!("default_config/event_default.toml");

/// Event channel configuration for a single event type
///
/// Fields are serde-defaulted so a partial user override (`[event.keyboard]
/// channel_size = 32`) parses standalone; the real values always come from the
/// merge with event_default.toml, which defines every event completely.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct EventChannelConfig {
    /// Channel buffer size
    pub channel_size: usize,
    /// Number of publishers
    pub pubs: usize,
    /// Number of subscribers
    pub subs: usize,
}

impl Default for EventChannelConfig {
    fn default() -> Self {
        Self {
            channel_size: 1,
            pubs: 1,
            subs: 1,
        }
    }
}

/// Macro to define EventConfig and related code without repetition
macro_rules! define_event_config {
    ($($field:ident),* $(,)?) => {
        /// Event configuration for all controller events
        /// Default values are loaded from event_default.toml
        #[derive(Clone, Debug, Deserialize)]
        #[serde(deny_unknown_fields, default)]
        pub(crate) struct EventConfig {
            $(pub $field: EventChannelConfig,)*
        }

        /// Cached default EventConfig parsed from event_default.toml
        static EVENT_CONFIG_DEFAULTS: std::sync::LazyLock<EventConfig> = std::sync::LazyLock::new(|| {
            #[derive(Deserialize)]
            struct Inner { $($field: EventChannelConfig,)* }
            #[derive(Deserialize)]
            struct Wrapper { event: Inner }
            let w: Wrapper = toml::from_str(EVENT_DEFAULT_CONFIG).expect("Failed to parse event_default.toml");
            EventConfig { $($field: w.event.$field,)* }
        });

        impl Default for EventConfig {
            fn default() -> Self {
                EVENT_CONFIG_DEFAULTS.clone()
            }
        }
    };
}

define_event_config!(
    // Connection events
    connection_status_change,
    // Input events
    modifier,
    keyboard,
    // Keyboard state events
    layer_change,
    wpm_update,
    led_indicator,
    sleep_state,
    // Power events
    battery_status,
    battery_adc,
    charging_state,
    // Pointing device events
    pointing,
    // Split events
    peripheral_connected,
    central_connected,
    peripheral_battery,
    clear_peer,
    // Action events
    action,
);
