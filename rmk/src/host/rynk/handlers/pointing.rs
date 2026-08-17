//! Pointing-configuration handlers.

use rmk_types::protocol::rynk::command::{GetPointingCapabilities, GetPointingConfig, SetPointingConfig};
use rmk_types::protocol::rynk::{
    POINTING_MODE_KEYPAD, PointingCapabilities, PointingConfig, RynkError, SetPointingConfigRequest,
};

use super::super::RynkService;
use super::Handle;
use crate::input_device::pointing_config;

impl Handle<GetPointingConfig> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<PointingConfig, RynkError> {
        Ok(pointing_config::get().await)
    }
}

impl Handle<SetPointingConfig> for RynkService<'_> {
    async fn handle(&self, r: SetPointingConfigRequest) -> Result<PointingConfig, RynkError> {
        pointing_config::replace(r.config).await
    }
}

impl Handle<GetPointingCapabilities> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<PointingCapabilities, RynkError> {
        Ok(PointingCapabilities {
            mode_flags: POINTING_MODE_KEYPAD,
        })
    }
}
