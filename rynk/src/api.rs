//! Typed endpoint methods — the version-specific API surface over the driver
//! core in `driver.rs`.

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use rmk_types::action::{EncoderAction, KeyAction};
use rmk_types::battery::BatteryStatus;
use rmk_types::ble::BleStatus;
use rmk_types::combo::Combo;
use rmk_types::connection::{ConnectionStatus, ConnectionType};
use rmk_types::fork::Fork;
use rmk_types::led_indicator::LedIndicator;
use rmk_types::morse::Morse;
use rmk_types::protocol::rynk::{
    AbortLightingOverlayReplaceRequest, AbortLightingSceneReplaceRequest, BeginLightingOverlayReplaceRequest,
    BeginLightingSceneReplaceRequest, BehaviorConfig, BuildInfo, ClearLightingOverlayRequest, Cmd,
    CommitLightingOverlayReplaceRequest, CommitLightingSceneReplaceRequest, DeviceCapabilities, DeviceInfo,
    GetComboBulkRequest, GetComboBulkResponse, GetEncoderRequest, GetKeymapBulkRequest, GetKeymapBulkResponse,
    GetMacroRequest, GetMorseBulkRequest, GetMorseBulkResponse, KeyPosition, LayerState, LightingCapabilities,
    LightingCompiledSceneStatus, LightingCompiledScenesPage, LightingKeysPage, LightingLedsPage, LightingOutputsPage,
    LightingOverlayPage, LightingOverlayPageRequest, LightingOverlayTransaction, LightingPageRequest,
    LightingPhysicalKeysPage, LightingResult, LightingRoutesPage, LightingScenePageRequest, LightingSceneStatus,
    LightingSceneTransaction, LightingScenesPage, LightingState, LightingZoneMembershipsPage, LightingZonesPage,
    LockStatus, MacroData, MatrixState, PeripheralStatus, ProtocolVersion, PutLightingOverlayChunkRequest,
    PutLightingSceneChunkRequest, SetComboBulkRequest, SetComboRequest, SetEncoderRequest, SetForkRequest,
    SetKeyRequest, SetKeymapBulkRequest, SetLightingLayerPolicyRequest, SetLightingOverlayRequest,
    SetLightingSceneCellRequest, SetLightingStateRequest, SetMacroRequest, SetMorseBulkRequest, SetMorseRequest,
    StorageResetMode, UnsetLightingOverlayRequest, UnsetLightingSceneCellRequest, command,
};

use crate::driver::{Client, RynkHostError};
#[cfg(feature = "alloc")]
use crate::layout::LayoutInfo;

impl Client {
    /// Reject a bulk command locally when capabilities lack bulk transfer.
    fn require_bulk_transfer(&self, cmd: Cmd) -> Result<(), RynkHostError> {
        if self.capabilities.bulk_transfer_supported {
            Ok(())
        } else {
            Err(RynkHostError::Unsupported(cmd, "bulk transfer not supported"))
        }
    }

    /// Reject a BLE-only command locally when capabilities lack BLE.
    fn require_ble(&self, cmd: Cmd) -> Result<(), RynkHostError> {
        if self.capabilities.ble_enabled {
            Ok(())
        } else {
            Err(RynkHostError::Unsupported(cmd, "BLE not enabled"))
        }
    }

    /// Reject a lighting command locally when the handshake says the firmware
    /// has no lighting service.
    fn require_lighting(&self, cmd: Cmd) -> Result<(), RynkHostError> {
        if self.capabilities.lighting_enabled {
            Ok(())
        } else {
            Err(RynkHostError::Unsupported(cmd, "lighting not enabled"))
        }
    }

    fn flatten_lighting<T>(result: LightingResult<T>) -> Result<T, RynkHostError> {
        result.map_err(RynkHostError::LightingRejected)
    }

    /// Read the firmware's protocol version.
    pub async fn get_version(&self) -> Result<ProtocolVersion, RynkHostError> {
        self.request::<command::GetVersion>(&()).await
    }

    /// The firmware's capability set — the connect-handshake snapshot, not a
    /// wire fetch: capabilities are firmware constants, fixed for the session.
    pub async fn get_capabilities(&self) -> Result<DeviceCapabilities, RynkHostError> {
        Ok(self.capabilities)
    }

    /// Read the firmware and device identity.
    pub async fn get_device_info(&self) -> Result<DeviceInfo, RynkHostError> {
        self.request::<command::GetDeviceInfo>(&()).await
    }

    /// Read the application-defined diagnostic build label.
    pub async fn get_build_info(&self) -> Result<BuildInfo, RynkHostError> {
        self.request::<command::GetBuildInfo>(&()).await
    }

    /// Reboot the device — fire-and-forget: the firmware resets before its
    /// session loop can reply, so `Ok(())` only means the request frame was
    /// queued; keep the driver running until it drains.
    pub async fn reboot(&self) -> Result<(), RynkHostError> {
        self.send_no_reply::<command::Reboot>(&()).await
    }

    /// Jump to the bootloader (DFU mode) — fire-and-forget, same contract as
    /// [`reboot`](Self::reboot).
    pub async fn bootloader_jump(&self) -> Result<(), RynkHostError> {
        self.send_no_reply::<command::BootloaderJump>(&()).await
    }

    /// Ask the application to route a bootloader jump to one split peripheral.
    /// Unlike the local jump, the central remains online and acknowledges
    /// whether the board-specific route accepted the request.
    pub async fn peripheral_bootloader_jump(&self, slot: u8) -> Result<(), RynkHostError> {
        if !self.capabilities.is_split {
            return Err(RynkHostError::Unsupported(
                Cmd::PeripheralBootloaderJump,
                "not a split keyboard",
            ));
        }
        self.request::<command::PeripheralBootloaderJump>(&slot).await
    }

    /// Reset persistent storage. Rejected locally when storage is disabled
    /// ([`DeviceCapabilities::storage_enabled`]), where the wipe would be a silent
    /// no-op.
    pub async fn storage_reset(&self, mode: StorageResetMode) -> Result<(), RynkHostError> {
        if !self.capabilities.storage_enabled {
            return Err(RynkHostError::Unsupported(Cmd::StorageReset, "storage not enabled"));
        }
        self.request::<command::StorageReset>(&mode).await
    }

    /// Read the current lock state without side effects.
    ///
    /// [`LockStatus::key_positions`] is the challenge to hold; empty while
    /// [`locked`](LockStatus::locked) means the device is permanently locked
    /// (no `unlock_keys` configured in keyboard.toml).
    pub async fn get_lock_status(&self) -> Result<LockStatus, RynkHostError> {
        self.request::<command::GetLockStatus>(&()).await
    }

    /// Arm/refresh a physical-presence unlock attempt and sample the held
    /// challenge keys.
    ///
    /// Poll every ~150 ms while the user holds the challenge keys:
    /// [`remaining_keys`](LockStatus::remaining_keys) counts down, and the
    /// attempt succeeds ([`locked`](LockStatus::locked) `== false`) once all
    /// are held simultaneously. The firmware window lapses ~500 ms after polls
    /// stop, so a cancel is just "stop polling".
    pub async fn unlock_poll(&self) -> Result<LockStatus, RynkHostError> {
        self.request::<command::UnlockPoll>(&()).await
    }

    /// Relock immediately. A no-op on an `insecure` device.
    pub async fn lock(&self) -> Result<(), RynkHostError> {
        self.request::<command::Lock>(&()).await
    }

    /// Read one key's action.
    pub async fn get_key(&self, layer: u8, row: u8, col: u8) -> Result<KeyAction, RynkHostError> {
        self.request::<command::GetKeyAction>(&KeyPosition { layer, row, col })
            .await
    }

    /// Write one key's action.
    pub async fn set_key(&self, layer: u8, row: u8, col: u8, action: KeyAction) -> Result<(), RynkHostError> {
        let req = SetKeyRequest {
            position: KeyPosition { layer, row, col },
            action,
        };
        self.request::<command::SetKeyAction>(&req).await
    }

    /// Read the currently selected default layer index.
    pub async fn get_default_layer(&self) -> Result<u8, RynkHostError> {
        self.request::<command::GetDefaultLayer>(&()).await
    }

    /// Set the default layer.
    pub async fn set_default_layer(&self, layer: u8) -> Result<(), RynkHostError> {
        self.request::<command::SetDefaultLayer>(&layer).await
    }

    /// Read both rotation actions for one encoder on one layer.
    pub async fn get_encoder(&self, encoder_id: u8, layer: u8) -> Result<EncoderAction, RynkHostError> {
        self.request::<command::GetEncoderAction>(&GetEncoderRequest { encoder_id, layer })
            .await
    }

    /// Set both rotation actions for one encoder on one layer.
    pub async fn set_encoder(&self, encoder_id: u8, layer: u8, action: EncoderAction) -> Result<(), RynkHostError> {
        let req = SetEncoderRequest {
            encoder_id,
            layer,
            action,
        };
        self.request::<command::SetEncoderAction>(&req).await
    }

    /// Read one page of key actions starting at `(layer, start_row, start_col)`,
    /// walking the row-major, layer-major keymap: up to `max_bulk_keys`, fewer at
    /// the end. An out-of-geometry position is rejected with `RynkError::Invalid`.
    /// Bulk firmware only ([`DeviceCapabilities::bulk_transfer_supported`]);
    /// rejected locally otherwise.
    pub async fn get_keymap_bulk(
        &self,
        layer: u8,
        start_row: u8,
        start_col: u8,
    ) -> Result<GetKeymapBulkResponse, RynkHostError> {
        self.require_bulk_transfer(Cmd::GetKeymapBulk)?;
        self.request::<command::GetKeymapBulk>(&GetKeymapBulkRequest {
            layer,
            start_row,
            start_col,
        })
        .await
    }

    /// Write `request.actions` into the keymap starting at
    /// `(request.layer, request.start_row, request.start_col)`. Bulk firmware only
    /// ([`DeviceCapabilities::bulk_transfer_supported`]); rejected locally otherwise.
    pub async fn set_keymap_bulk(&self, request: SetKeymapBulkRequest) -> Result<(), RynkHostError> {
        self.require_bulk_transfer(Cmd::SetKeymapBulk)?;
        self.request::<command::SetKeymapBulk>(&request).await
    }

    /// Read the physical layout. The firmware serves it as an opaque,
    /// compressed blob paged over `GetLayout`; this reassembles every page (by
    /// byte offset), inflates the blob, and decodes it into [`LayoutInfo`]. An
    /// empty blob (firmware built without a `[layout].map`) yields an empty
    /// [`LayoutInfo`], not an error.
    #[cfg(feature = "alloc")]
    pub async fn get_layout(&self) -> Result<LayoutInfo, RynkHostError> {
        const MAX_LAYOUT_BLOB_LEN: usize = 64 * 1024;
        let first = self.request::<command::GetLayout>(&0u32).await?;
        let total_len = first.total_len as usize;
        if total_len > MAX_LAYOUT_BLOB_LEN {
            return Err(RynkHostError::Layout(alloc::format!(
                "advertised layout blob length {total_len} exceeds maximum {MAX_LAYOUT_BLOB_LEN}"
            )));
        }
        let mut collected: Vec<u8> = first.bytes.to_vec();
        // The advertised length bounds repeated or over-long pages; an empty page ends a stalled transfer.
        while !collected.is_empty() && collected.len() < total_len {
            let chunk = self.request::<command::GetLayout>(&(collected.len() as u32)).await?;
            if chunk.bytes.is_empty() {
                break;
            }
            collected.extend_from_slice(&chunk.bytes);
        }
        collected.truncate(total_len);
        LayoutInfo::from_compressed_blob(&collected).map_err(RynkHostError::Layout)
    }

    /// Read one combo entry by index.
    pub async fn get_combo(&self, index: u8) -> Result<Combo, RynkHostError> {
        self.request::<command::GetCombo>(&index).await
    }

    /// Write one combo entry by index.
    pub async fn set_combo(&self, index: u8, config: Combo) -> Result<(), RynkHostError> {
        self.request::<command::SetCombo>(&SetComboRequest { index, config })
            .await
    }

    /// Read one page of combos starting at slot `start_index`: up to
    /// `max_bulk_configs`, fewer at the end, empty once `start_index` reaches the
    /// slot count. Bulk firmware only
    /// ([`DeviceCapabilities::bulk_transfer_supported`]); rejected locally otherwise.
    pub async fn get_combo_bulk(&self, start_index: u8) -> Result<GetComboBulkResponse, RynkHostError> {
        self.require_bulk_transfer(Cmd::GetComboBulk)?;
        self.request::<command::GetComboBulk>(&GetComboBulkRequest { start_index })
            .await
    }

    /// Write `request.configs` into the combo table at slot `request.start_index`.
    /// Bulk firmware only ([`DeviceCapabilities::bulk_transfer_supported`]);
    /// rejected locally otherwise.
    pub async fn set_combo_bulk(&self, request: SetComboBulkRequest) -> Result<(), RynkHostError> {
        self.require_bulk_transfer(Cmd::SetComboBulk)?;
        self.request::<command::SetComboBulk>(&request).await
    }

    /// Read one fork entry by index.
    pub async fn get_fork(&self, index: u8) -> Result<Fork, RynkHostError> {
        self.request::<command::GetFork>(&index).await
    }

    /// Write one fork entry by index.
    pub async fn set_fork(&self, index: u8, config: Fork) -> Result<(), RynkHostError> {
        self.request::<command::SetFork>(&SetForkRequest { index, config })
            .await
    }

    /// Read one morse entry by index.
    pub async fn get_morse(&self, index: u8) -> Result<Morse, RynkHostError> {
        self.request::<command::GetMorse>(&index).await
    }

    /// Write one morse entry by index.
    pub async fn set_morse(&self, index: u8, config: Morse) -> Result<(), RynkHostError> {
        self.request::<command::SetMorse>(&SetMorseRequest { index, config })
            .await
    }

    /// Read one page of morses starting at slot `start_index`: up to
    /// `max_bulk_configs`, fewer at the end, empty once `start_index` reaches the
    /// slot count. Bulk firmware only
    /// ([`DeviceCapabilities::bulk_transfer_supported`]); rejected locally otherwise.
    pub async fn get_morse_bulk(&self, start_index: u8) -> Result<GetMorseBulkResponse, RynkHostError> {
        self.require_bulk_transfer(Cmd::GetMorseBulk)?;
        self.request::<command::GetMorseBulk>(&GetMorseBulkRequest { start_index })
            .await
    }

    /// Write `request.configs` into the morse table at slot `request.start_index`.
    /// Bulk firmware only ([`DeviceCapabilities::bulk_transfer_supported`]);
    /// rejected locally otherwise.
    pub async fn set_morse_bulk(&self, request: SetMorseBulkRequest) -> Result<(), RynkHostError> {
        self.require_bulk_transfer(Cmd::SetMorseBulk)?;
        self.request::<command::SetMorseBulk>(&request).await
    }

    /// Read one chunk of macro data at byte `offset`. The reply is always a
    /// full build-time chunk, zero-filled past the end of macro space —
    /// termination comes from parsing the macro encoding, not chunk length.
    pub async fn get_macro(&self, offset: u16) -> Result<MacroData, RynkHostError> {
        self.request::<command::GetMacro>(&GetMacroRequest { offset }).await
    }

    /// Write one chunk of macro data starting at byte `offset`. Writes past
    /// the end of the device's macro space are truncated by the firmware.
    pub async fn set_macro(&self, offset: u16, data: MacroData) -> Result<(), RynkHostError> {
        self.request::<command::SetMacro>(&SetMacroRequest { offset, data })
            .await
    }

    /// Read the global behavior config.
    pub async fn get_behavior(&self) -> Result<BehaviorConfig, RynkHostError> {
        self.request::<command::GetBehaviorConfig>(&()).await
    }

    /// Write the global behavior config.
    pub async fn set_behavior(&self, config: BehaviorConfig) -> Result<(), RynkHostError> {
        self.request::<command::SetBehaviorConfig>(&config).await
    }

    /// Read the currently active layer.
    pub async fn get_current_layer(&self) -> Result<u8, RynkHostError> {
        self.request::<command::GetCurrentLayer>(&()).await
    }

    /// Read the default layer and complete active-layer bitmap.
    pub async fn get_layer_state(&self) -> Result<LayerState, RynkHostError> {
        self.request::<command::GetLayerState>(&()).await
    }

    /// Read the matrix scan bitmap.
    pub async fn get_matrix_state(&self) -> Result<MatrixState, RynkHostError> {
        self.request::<command::GetMatrixState>(&()).await
    }

    /// Read battery status. BLE firmware only ([`DeviceCapabilities::ble_enabled`]);
    /// rejected locally otherwise.
    pub async fn get_battery_status(&self) -> Result<BatteryStatus, RynkHostError> {
        self.require_ble(Cmd::GetBatteryStatus)?;
        self.request::<command::GetBatteryStatus>(&()).await
    }

    /// Read one split peripheral's status by slot. Split keyboards only
    /// ([`DeviceCapabilities::is_split`]); rejected locally otherwise.
    pub async fn get_peripheral_status(&self, slot: u8) -> Result<PeripheralStatus, RynkHostError> {
        if !self.capabilities.is_split {
            return Err(RynkHostError::Unsupported(
                Cmd::GetPeripheralStatus,
                "not a split keyboard",
            ));
        }
        self.request::<command::GetPeripheralStatus>(&slot).await
    }

    /// Read the current words-per-minute estimate.
    pub async fn get_wpm(&self) -> Result<u16, RynkHostError> {
        self.request::<command::GetWpm>(&()).await
    }

    /// Read the firmware's sleep state.
    pub async fn get_sleep_state(&self) -> Result<bool, RynkHostError> {
        self.request::<command::GetSleepState>(&()).await
    }

    /// Read the host LED indicator state (caps/num/scroll lock, etc.).
    pub async fn get_led_indicator(&self) -> Result<LedIndicator, RynkHostError> {
        self.request::<command::GetLedIndicator>(&()).await
    }

    /// Read lighting limits, supported effects, and topology identity.
    pub async fn get_lighting_capabilities(&self) -> Result<LightingCapabilities, RynkHostError> {
        self.require_lighting(Cmd::GetLightingCapabilities)?;
        Self::flatten_lighting(self.request::<command::GetLightingCapabilities>(&()).await?)
    }

    /// Read authoritative standard lighting state and its concurrency revision.
    pub async fn get_lighting_state(&self) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::GetLightingState)?;
        Self::flatten_lighting(self.request::<command::GetLightingState>(&()).await?)
    }

    /// Atomically replace standard mutable state when the revision still matches.
    pub async fn set_lighting_state(&self, request: SetLightingStateRequest) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::SetLightingState)?;
        Self::flatten_lighting(self.request::<command::SetLightingState>(&request).await?)
    }

    pub async fn get_lighting_physical_keys(
        &self,
        request: LightingPageRequest,
    ) -> Result<LightingPhysicalKeysPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingPhysicalKeys)?;
        Self::flatten_lighting(self.request::<command::GetLightingPhysicalKeys>(&request).await?)
    }

    /// Read real logical matrix keys, including keys with no measured geometry.
    pub async fn get_lighting_keys(&self, request: LightingPageRequest) -> Result<LightingKeysPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingKeys)?;
        Self::flatten_lighting(self.request::<command::GetLightingKeys>(&request).await?)
    }

    pub async fn get_lighting_leds(&self, request: LightingPageRequest) -> Result<LightingLedsPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingLeds)?;
        Self::flatten_lighting(self.request::<command::GetLightingLeds>(&request).await?)
    }

    pub async fn get_lighting_zones(&self, request: LightingPageRequest) -> Result<LightingZonesPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingZones)?;
        Self::flatten_lighting(self.request::<command::GetLightingZones>(&request).await?)
    }

    pub async fn get_lighting_zone_memberships(
        &self,
        request: LightingPageRequest,
    ) -> Result<LightingZoneMembershipsPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingZoneMemberships)?;
        Self::flatten_lighting(self.request::<command::GetLightingZoneMemberships>(&request).await?)
    }

    pub async fn get_lighting_outputs(
        &self,
        request: LightingPageRequest,
    ) -> Result<LightingOutputsPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingOutputs)?;
        Self::flatten_lighting(self.request::<command::GetLightingOutputs>(&request).await?)
    }

    pub async fn get_lighting_routes(&self, request: LightingPageRequest) -> Result<LightingRoutesPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingRoutes)?;
        Self::flatten_lighting(self.request::<command::GetLightingRoutes>(&request).await?)
    }

    /// Set one transient overlay cell when the state revision matches.
    pub async fn set_lighting_overlay(
        &self,
        request: SetLightingOverlayRequest,
    ) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::SetLightingOverlay)?;
        Self::flatten_lighting(self.request::<command::SetLightingOverlay>(&request).await?)
    }

    /// Remove one transient overlay cell when the state revision matches.
    pub async fn unset_lighting_overlay(
        &self,
        request: UnsetLightingOverlayRequest,
    ) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::UnsetLightingOverlay)?;
        Self::flatten_lighting(self.request::<command::UnsetLightingOverlay>(&request).await?)
    }

    /// Clear the transient overlay when the state revision matches.
    pub async fn clear_lighting_overlay(
        &self,
        request: ClearLightingOverlayRequest,
    ) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::ClearLightingOverlay)?;
        Self::flatten_lighting(self.request::<command::ClearLightingOverlay>(&request).await?)
    }

    /// Reserve a bounded staging transaction for atomic overlay replacement.
    pub async fn begin_lighting_overlay_replace(
        &self,
        request: BeginLightingOverlayReplaceRequest,
    ) -> Result<LightingOverlayTransaction, RynkHostError> {
        self.require_lighting(Cmd::BeginLightingOverlayReplace)?;
        Self::flatten_lighting(self.request::<command::BeginLightingOverlayReplace>(&request).await?)
    }

    /// Stage one ordered chunk. It does not mutate the live overlay.
    pub async fn put_lighting_overlay_chunk(
        &self,
        request: PutLightingOverlayChunkRequest,
    ) -> Result<(), RynkHostError> {
        self.require_lighting(Cmd::PutLightingOverlayChunk)?;
        Self::flatten_lighting(self.request::<command::PutLightingOverlayChunk>(&request).await?)
    }

    /// Atomically publish a complete staged overlay replacement.
    pub async fn commit_lighting_overlay_replace(
        &self,
        request: CommitLightingOverlayReplaceRequest,
    ) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::CommitLightingOverlayReplace)?;
        Self::flatten_lighting(self.request::<command::CommitLightingOverlayReplace>(&request).await?)
    }

    /// Discard a staged overlay replacement without changing live state.
    pub async fn abort_lighting_overlay_replace(
        &self,
        request: AbortLightingOverlayReplaceRequest,
    ) -> Result<(), RynkHostError> {
        self.require_lighting(Cmd::AbortLightingOverlayReplace)?;
        Self::flatten_lighting(self.request::<command::AbortLightingOverlayReplace>(&request).await?)
    }

    /// Read one page of transient overlay cells, pinned to a state revision.
    pub async fn get_lighting_overlay(
        &self,
        request: LightingOverlayPageRequest,
    ) -> Result<LightingOverlayPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingOverlay)?;
        Self::flatten_lighting(self.request::<command::GetLightingOverlay>(&request).await?)
    }

    /// Read scene limits and occupancy. Scene support is discovered through
    /// [`LightingCapabilities::features`] (`LAYER_SCENES`) plus this endpoint;
    /// firmware without a scene table rejects it with `Unsupported`.
    pub async fn get_lighting_scene_status(&self) -> Result<LightingSceneStatus, RynkHostError> {
        self.require_lighting(Cmd::GetLightingSceneStatus)?;
        Self::flatten_lighting(self.request::<command::GetLightingSceneStatus>(&()).await?)
    }

    /// Read one page of stored scene cells, pinned to a state revision.
    pub async fn get_lighting_scenes(
        &self,
        request: LightingScenePageRequest,
    ) -> Result<LightingScenesPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingScenes)?;
        Self::flatten_lighting(self.request::<command::GetLightingScenes>(&request).await?)
    }

    /// Discover the immutable board-compiled scene source, including empty sources.
    pub async fn get_lighting_compiled_scene_status(&self) -> Result<LightingCompiledSceneStatus, RynkHostError> {
        self.require_lighting(Cmd::GetLightingCompiledSceneStatus)?;
        Self::flatten_lighting(self.request::<command::GetLightingCompiledSceneStatus>(&()).await?)
    }

    /// Read one topology-revision-pinned page of board-compiled scene cells.
    pub async fn get_lighting_compiled_scenes(
        &self,
        request: LightingPageRequest,
    ) -> Result<LightingCompiledScenesPage, RynkHostError> {
        self.require_lighting(Cmd::GetLightingCompiledScenes)?;
        Self::flatten_lighting(self.request::<command::GetLightingCompiledScenes>(&request).await?)
    }

    /// Insert or update one durable scene cell when the revision matches.
    pub async fn set_lighting_scene_cell(
        &self,
        request: SetLightingSceneCellRequest,
    ) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::SetLightingSceneCell)?;
        Self::flatten_lighting(self.request::<command::SetLightingSceneCell>(&request).await?)
    }

    /// Remove one durable scene cell when the revision matches.
    pub async fn unset_lighting_scene_cell(
        &self,
        request: UnsetLightingSceneCellRequest,
    ) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::UnsetLightingSceneCell)?;
        Self::flatten_lighting(self.request::<command::UnsetLightingSceneCell>(&request).await?)
    }

    /// Set the scene layer-composition policy when the revision matches.
    pub async fn set_lighting_layer_policy(
        &self,
        request: SetLightingLayerPolicyRequest,
    ) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::SetLightingLayerPolicy)?;
        Self::flatten_lighting(self.request::<command::SetLightingLayerPolicy>(&request).await?)
    }

    /// Reserve the bounded staging transaction for atomic scene replacement.
    pub async fn begin_lighting_scene_replace(
        &self,
        request: BeginLightingSceneReplaceRequest,
    ) -> Result<LightingSceneTransaction, RynkHostError> {
        self.require_lighting(Cmd::BeginLightingSceneReplace)?;
        Self::flatten_lighting(self.request::<command::BeginLightingSceneReplace>(&request).await?)
    }

    /// Stage one ordered scene chunk. It does not mutate the live table.
    pub async fn put_lighting_scene_chunk(&self, request: PutLightingSceneChunkRequest) -> Result<(), RynkHostError> {
        self.require_lighting(Cmd::PutLightingSceneChunk)?;
        Self::flatten_lighting(self.request::<command::PutLightingSceneChunk>(&request).await?)
    }

    /// Atomically publish a complete staged scene replacement.
    pub async fn commit_lighting_scene_replace(
        &self,
        request: CommitLightingSceneReplaceRequest,
    ) -> Result<LightingState, RynkHostError> {
        self.require_lighting(Cmd::CommitLightingSceneReplace)?;
        Self::flatten_lighting(self.request::<command::CommitLightingSceneReplace>(&request).await?)
    }

    /// Discard a staged scene replacement without changing live state.
    pub async fn abort_lighting_scene_replace(
        &self,
        request: AbortLightingSceneReplaceRequest,
    ) -> Result<(), RynkHostError> {
        self.require_lighting(Cmd::AbortLightingSceneReplace)?;
        Self::flatten_lighting(self.request::<command::AbortLightingSceneReplace>(&request).await?)
    }

    /// Read the active connection type (USB / BLE).
    pub async fn get_connection_type(&self) -> Result<ConnectionType, RynkHostError> {
        self.request::<command::GetConnectionType>(&()).await
    }

    /// Read the full connection status — the same payload the `ConnectionChange`
    /// topic pushes, for recovering a missed push.
    pub async fn get_connection_status(&self) -> Result<ConnectionStatus, RynkHostError> {
        self.request::<command::GetConnectionStatus>(&()).await
    }

    /// Read BLE status (active profile, connection state). BLE firmware only
    /// ([`DeviceCapabilities::ble_enabled`]); rejected locally otherwise.
    pub async fn get_ble_status(&self) -> Result<BleStatus, RynkHostError> {
        self.require_ble(Cmd::GetBleStatus)?;
        self.request::<command::GetBleStatus>(&()).await
    }

    /// Switch to a BLE profile by slot. BLE firmware only; rejected locally
    /// otherwise.
    pub async fn switch_ble_profile(&self, slot: u8) -> Result<(), RynkHostError> {
        self.require_ble(Cmd::SwitchBleProfile)?;
        self.request::<command::SwitchBleProfile>(&slot).await
    }

    /// Clear (unbond) a BLE profile by slot. Tears down the active link if it
    /// targets the connected profile. BLE firmware only; rejected locally
    /// otherwise.
    pub async fn clear_ble_profile(&self, slot: u8) -> Result<(), RynkHostError> {
        self.require_ble(Cmd::ClearBleProfile)?;
        self.request::<command::ClearBleProfile>(&slot).await
    }
}

#[cfg(feature = "alloc")]
impl Client {
    /// Read the whole keymap (every layer, row-major) by paging `GetKeymapBulk`.
    pub async fn read_all_keymap(&self) -> Result<Vec<KeyAction>, RynkHostError> {
        let caps = self.capabilities;
        let (rows, cols) = (caps.num_rows as u16, caps.num_cols as u16);
        let total = caps.num_layers as usize * rows as usize * cols as usize;
        self.read_all(total, async |c, start| {
            let (layer, row, col) = keymap_pos(start, rows, cols);
            c.get_keymap_bulk(layer, row, col).await.map(|r| r.actions)
        })
        .await
    }

    /// Read every combo slot by paging `GetComboBulk`.
    pub async fn read_all_combos(&self) -> Result<Vec<Combo>, RynkHostError> {
        let total = self.capabilities.max_combos as usize;
        self.read_all(total, async |c, start| {
            c.get_combo_bulk(start as u8).await.map(|r| r.configs)
        })
        .await
    }

    /// Read every morse slot by paging `GetMorseBulk`.
    pub async fn read_all_morses(&self) -> Result<Vec<Morse>, RynkHostError> {
        let total = self.capabilities.max_morse as usize;
        self.read_all(total, async |c, start| {
            c.get_morse_bulk(start as u8).await.map(|r| r.configs)
        })
        .await
    }

    /// Write the whole keymap by paging `SetKeymapBulk` in `max_bulk_keys` chunks.
    pub async fn write_all_keymap(&self, actions: &[KeyAction]) -> Result<(), RynkHostError> {
        let caps = self.capabilities;
        let (rows, cols) = (caps.num_rows as u16, caps.num_cols as u16);
        let page = caps.max_bulk_keys as usize;
        self.write_all(page, actions, async |c, start, actions| {
            let (layer, row, col) = keymap_pos(start, rows, cols);
            c.set_keymap_bulk(SetKeymapBulkRequest {
                layer,
                start_row: row,
                start_col: col,
                actions,
            })
            .await
        })
        .await
    }

    /// Write every combo by paging `SetComboBulk` in `max_bulk_configs` chunks.
    pub async fn write_all_combos(&self, configs: &[Combo]) -> Result<(), RynkHostError> {
        let page = self.capabilities.max_bulk_configs as usize;
        self.write_all(page, configs, async |c, start, configs| {
            c.set_combo_bulk(SetComboBulkRequest {
                start_index: start as u8,
                configs,
            })
            .await
        })
        .await
    }

    /// Read the whole transient overlay under one state revision. Expiry or a
    /// concurrent mutation restarts the snapshot, bounded to a few attempts.
    pub async fn read_all_lighting_overlay(
        &self,
    ) -> Result<(u32, Vec<rmk_types::protocol::rynk::LightingOverlayCell>), RynkHostError> {
        const ATTEMPTS: usize = 4;
        let mut last_error = None;
        for _ in 0..ATTEMPTS {
            let state = self.get_lighting_state().await?;
            let mut cells = Vec::new();
            let mut offset: u16 = 0;
            let mut first_page = true;
            let mut conflicted = false;
            while first_page || offset < state.overlay_len {
                first_page = false;
                match self
                    .get_lighting_overlay(LightingOverlayPageRequest {
                        revision: state.revision,
                        offset,
                    })
                    .await
                {
                    Ok(page) => {
                        if page.revision != state.revision || page.total_count != state.overlay_len {
                            return Err(RynkHostError::InconsistentResponse {
                                cmd: Cmd::GetLightingOverlay,
                                reason: "page revision/count disagrees with the pinned state",
                            });
                        }
                        if offset >= state.overlay_len {
                            if !page.items.is_empty() {
                                return Err(RynkHostError::InconsistentResponse {
                                    cmd: Cmd::GetLightingOverlay,
                                    reason: "empty snapshot returned unexpected cells",
                                });
                            }
                            break;
                        }
                        if page.items.is_empty() || offset as usize + page.items.len() > state.overlay_len as usize {
                            return Err(RynkHostError::InconsistentResponse {
                                cmd: Cmd::GetLightingOverlay,
                                reason: "page is empty or extends beyond the advertised count",
                            });
                        }
                        offset += page.items.len() as u16;
                        cells.extend(page.items.iter().copied());
                    }
                    Err(
                        error @ RynkHostError::LightingRejected(
                            rmk_types::protocol::rynk::LightingError::StateRevisionConflict { .. },
                        ),
                    ) => {
                        last_error = Some(error);
                        conflicted = true;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            if !conflicted {
                if cells.len() != state.overlay_len as usize {
                    return Err(RynkHostError::InconsistentResponse {
                        cmd: Cmd::GetLightingOverlay,
                        reason: "pagination ended before the advertised count",
                    });
                }
                return Ok((state.revision, cells));
            }
        }
        Err(last_error.expect("a retried read only exits with a recorded conflict"))
    }

    /// Read every immutable board-compiled scene cell under one topology revision.
    pub async fn read_all_lighting_compiled_scenes(
        &self,
    ) -> Result<
        (
            LightingCompiledSceneStatus,
            Vec<rmk_types::protocol::rynk::LightingSceneCell>,
        ),
        RynkHostError,
    > {
        let status = self.get_lighting_compiled_scene_status().await?;
        let mut cells = Vec::new();
        let mut offset: u16 = 0;
        let mut first_page = true;
        while first_page || offset < status.scene_len {
            first_page = false;
            let page = self
                .get_lighting_compiled_scenes(LightingPageRequest {
                    topology_revision: status.topology_revision,
                    offset,
                })
                .await?;
            if page.topology_revision != status.topology_revision || page.total_count != status.scene_len {
                return Err(RynkHostError::InconsistentResponse {
                    cmd: Cmd::GetLightingCompiledScenes,
                    reason: "page topology revision/count disagrees with status",
                });
            }
            if offset >= status.scene_len {
                if !page.items.is_empty() {
                    return Err(RynkHostError::InconsistentResponse {
                        cmd: Cmd::GetLightingCompiledScenes,
                        reason: "empty compiled source returned unexpected cells",
                    });
                }
                break;
            }
            if page.items.is_empty()
                || page.items.len() > status.chunk_capacity as usize
                || offset as usize + page.items.len() > status.scene_len as usize
            {
                return Err(RynkHostError::InconsistentResponse {
                    cmd: Cmd::GetLightingCompiledScenes,
                    reason: "page is empty, oversized, or extends beyond the advertised count",
                });
            }
            offset += page.items.len() as u16;
            cells.extend(page.items.iter().copied());
        }
        if cells.len() != status.scene_len as usize {
            return Err(RynkHostError::InconsistentResponse {
                cmd: Cmd::GetLightingCompiledScenes,
                reason: "pagination ended before the advertised count",
            });
        }
        Ok((status, cells))
    }

    /// Read the whole stored scene table by paging `GetLightingScenes` under
    /// one pinned revision. A concurrent lighting mutation invalidates the
    /// pin; the read restarts from a fresh status, bounded by a few attempts.
    pub async fn read_all_lighting_scenes(
        &self,
    ) -> Result<(u32, Vec<rmk_types::protocol::rynk::LightingSceneCell>), RynkHostError> {
        const ATTEMPTS: usize = 4;
        let mut last_error = None;
        for _ in 0..ATTEMPTS {
            let status = self.get_lighting_scene_status().await?;
            let mut cells = Vec::new();
            let mut offset: u16 = 0;
            let mut conflicted = false;
            while offset < status.scene_len {
                match self
                    .get_lighting_scenes(LightingScenePageRequest {
                        revision: status.revision,
                        offset,
                    })
                    .await
                {
                    Ok(page) => {
                        if page.items.is_empty() {
                            break;
                        }
                        offset += page.items.len() as u16;
                        cells.extend(page.items.iter().copied());
                    }
                    Err(
                        error @ RynkHostError::LightingRejected(
                            rmk_types::protocol::rynk::LightingError::StateRevisionConflict { .. },
                        ),
                    ) => {
                        last_error = Some(error);
                        conflicted = true;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            if !conflicted {
                return Ok((status.revision, cells));
            }
        }
        Err(last_error.expect("a retried read only exits with a recorded conflict"))
    }

    /// Atomically replace the whole stored scene table: begin, stage in
    /// chunk-sized pages, and commit. A staging failure is followed by a
    /// best-effort abort so the firmware transaction is not left dangling.
    pub async fn replace_all_lighting_scenes(
        &self,
        expected_revision: u32,
        cells: &[rmk_types::protocol::rynk::LightingSceneCell],
    ) -> Result<LightingState, RynkHostError> {
        let transaction = self
            .begin_lighting_scene_replace(BeginLightingSceneReplaceRequest {
                expected_revision,
                cell_count: cells.len() as u16,
            })
            .await?;
        let mut offset: u16 = 0;
        for chunk in cells.chunks(rmk_types::protocol::rynk::LIGHTING_SCENE_CHUNK_SIZE) {
            let mut request = PutLightingSceneChunkRequest {
                transaction_id: transaction.id,
                offset,
                cells: Default::default(),
            };
            for cell in chunk {
                request.cells.push(*cell).expect("chunks are chunk-size bounded");
            }
            if let Err(error) = self.put_lighting_scene_chunk(request).await {
                let _ = self
                    .abort_lighting_scene_replace(AbortLightingSceneReplaceRequest {
                        transaction_id: transaction.id,
                    })
                    .await;
                return Err(error);
            }
            offset += chunk.len() as u16;
        }
        self.commit_lighting_scene_replace(CommitLightingSceneReplaceRequest {
            transaction_id: transaction.id,
        })
        .await
    }

    /// Write every morse by paging `SetMorseBulk` in `max_bulk_configs` chunks.
    pub async fn write_all_morses(&self, configs: &[Morse]) -> Result<(), RynkHostError> {
        let page = self.capabilities.max_bulk_configs as usize;
        self.write_all(page, configs, async |c, start, configs| {
            c.set_morse_bulk(SetMorseBulkRequest {
                start_index: start as u8,
                configs,
            })
            .await
        })
        .await
    }

    /// Page a whole resource in: fetch from cursor 0 until `total` items are read
    /// or an empty page marks the firmware's clamped end.
    async fn read_all<Item>(
        &self,
        total: usize,
        mut fetch: impl AsyncFnMut(&Self, u16) -> Result<Vec<Item>, RynkHostError>,
    ) -> Result<Vec<Item>, RynkHostError> {
        let mut out = Vec::new();
        let mut start: u16 = 0;
        while (start as usize) < total {
            let page = fetch(self, start).await?;
            if page.is_empty() {
                break; // firmware paged out before reaching `total`
            }
            start += page.len() as u16;
            out.extend(page);
        }
        Ok(out)
    }

    /// Page a whole resource out: send `items` in `page`-sized chunks, each a
    /// bounded `Set*Bulk` at its flat cursor.
    async fn write_all<Item: Clone>(
        &self,
        page: usize,
        items: &[Item],
        mut store: impl AsyncFnMut(&Self, u16, Vec<Item>) -> Result<(), RynkHostError>,
    ) -> Result<(), RynkHostError> {
        // Preserve the capability error path instead of panicking in `chunks(0)`.
        let mut start: u16 = 0;
        for chunk in items.chunks(page.max(1)) {
            store(self, start, chunk.to_vec()).await?;
            start += chunk.len() as u16;
        }
        Ok(())
    }
}

/// Map a flat, row-major, layer-major key cursor to its `(layer, row, col)`
/// address for the device's `rows`×`cols` geometry. `u16` arithmetic since the
/// keymap can exceed 255 keys; the address components each fit in `u8`.
#[cfg(feature = "alloc")]
fn keymap_pos(cursor: u16, rows: u16, cols: u16) -> (u8, u8, u8) {
    let layer = cursor / (rows * cols);
    let row = (cursor / cols) % rows;
    let col = cursor % cols;
    (layer as u8, row as u8, col as u8)
}
