//! Behavior-config handlers (combo timeout, one-shot timeout, tap intervals).

use rmk_types::protocol::rynk::command::{
    GetAutoMouseLayerConfigs, GetBehaviorConfig, GetBehaviorOptions, SetAutoMouseLayerConfigs, SetBehaviorConfig,
    SetBehaviorOptions,
};
use rmk_types::protocol::rynk::{
    AutoMouseLayerConfigState, BehaviorConfig, BehaviorOptions, RynkError, SetAutoMouseLayerConfigsRequest,
};

use super::super::RynkService;
use super::Handle;

impl Handle<GetBehaviorConfig> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<BehaviorConfig, RynkError> {
        Ok(BehaviorConfig {
            combo_timeout_ms: self.ctx.combo_timeout().as_millis() as u16,
            oneshot_timeout_ms: self.ctx.one_shot_timeout().as_millis() as u16,
            tap_interval_ms: self.ctx.tap_interval(),
            tap_capslock_interval_ms: self.ctx.tap_capslock_interval(),
        })
    }
}

impl Handle<SetBehaviorConfig> for RynkService<'_> {
    async fn handle(&self, cfg: BehaviorConfig) -> Result<(), RynkError> {
        self.ctx.set_combo_timeout(cfg.combo_timeout_ms).await;
        self.ctx.set_one_shot_timeout(cfg.oneshot_timeout_ms).await;
        self.ctx.set_tap_interval(cfg.tap_interval_ms).await;
        self.ctx.set_tap_capslock_interval(cfg.tap_capslock_interval_ms).await;
        Ok(())
    }
}

impl Handle<GetBehaviorOptions> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<BehaviorOptions, RynkError> {
        Ok(self.ctx.behavior_options())
    }
}

impl Handle<SetBehaviorOptions> for RynkService<'_> {
    async fn handle(&self, options: BehaviorOptions) -> Result<(), RynkError> {
        if self.ctx.set_behavior_options(options).await {
            Ok(())
        } else {
            Err(RynkError::Invalid)
        }
    }
}

impl Handle<GetAutoMouseLayerConfigs> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<AutoMouseLayerConfigState, RynkError> {
        Ok(AutoMouseLayerConfigState {
            capacity: crate::AUTO_MOUSE_LAYER_MAX_NUM as u8,
            configs: self.ctx.auto_mouse_layer_configs(),
        })
    }
}

impl Handle<SetAutoMouseLayerConfigs> for RynkService<'_> {
    async fn handle(&self, request: SetAutoMouseLayerConfigsRequest) -> Result<(), RynkError> {
        if self.ctx.set_auto_mouse_layer_configs(request.configs).await {
            Ok(())
        } else {
            Err(RynkError::Invalid)
        }
    }
}
