//! Manage BLE profiles and bonding information

#[cfg(feature = "_ble")]
use bt_hci::{cmd::le::LeSetPhy, controller::ControllerCmdAsync};
use embassy_sync::channel::Channel;
use trouble_host::prelude::*;
use trouble_host::{BondInformation, LongTermKey};

use super::ble_server::CCCD_TABLE_SIZE;
use crate::NUM_BLE_PROFILE;
use crate::channel::BLE_PROFILE_CHANNEL;
#[cfg(feature = "storage")]
use crate::channel::FLASH_CHANNEL;
use crate::state::current_profile;
#[cfg(feature = "storage")]
use crate::state::set_ble_profile;

pub(crate) static UPDATED_PROFILE: Channel<crate::RawMutex, ProfileInfoUpdate, NUM_BLE_PROFILE> = Channel::new();
pub(crate) static UPDATED_CCCD_TABLE: Channel<crate::RawMutex, ProfileCccdTable, NUM_BLE_PROFILE> = Channel::new();

/// BLE profile info
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ProfileInfo {
    pub(crate) slot_num: u8,
    pub(crate) removed: bool,
    pub(crate) info: BondInformation,
    /// Raw bytes of the trouble-host `ClientAttTable` for this peer.
    /// Reconstructed via `ClientAttTableView::try_from_raw` when applied to the stack.
    pub(crate) cccd_table: heapless::Vec<u8, CCCD_TABLE_SIZE>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) struct ProfileInfoUpdate {
    pub(crate) generation: u32,
    pub(crate) profile_info: ProfileInfo,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) struct ProfileCccdTable {
    pub(crate) slot_num: u8,
    pub(crate) generation: u32,
    pub(crate) table: heapless::Vec<u8, CCCD_TABLE_SIZE>,
}

/// Returns the maximum number of bytes required to encode T.
pub const fn varint_max<T: Sized>() -> usize {
    const BITS_PER_BYTE: usize = 8;
    const BITS_PER_VARINT_BYTE: usize = 7;

    // How many data bits do we need for this type?
    let bits = core::mem::size_of::<T>() * BITS_PER_BYTE;

    // We add (BITS_PER_VARINT_BYTE - 1), to ensure any integer divisions
    // with a remainder will always add exactly one full byte, but
    // an evenly divided number of bits will be the same
    let roundup_bits = bits + (BITS_PER_VARINT_BYTE - 1);

    // Apply division, using normal "round down" integer division
    roundup_bits / BITS_PER_VARINT_BYTE
}

// Manual MaxSize implementation
impl postcard::experimental::max_size::MaxSize for ProfileInfo {
    const POSTCARD_MAX_SIZE: usize = varint_max::<Self>();
}

impl Default for ProfileInfo {
    fn default() -> Self {
        Self {
            slot_num: 0,
            removed: false,
            info: BondInformation::new(
                Identity {
                    addr: Address::default(),
                    irk: None,
                },
                LongTermKey(0),
                SecurityLevel::NoEncryption,
                false,
            ),
            cccd_table: heapless::Vec::new(),
        }
    }
}

/// BLE profile switch action
pub(crate) enum BleProfileAction {
    Switch(u8),
    Previous,
    Next,
    ClearAndSwitch(u8),
    ClearBond,
}

/// Changes requested by a processed profile action.
///
/// Clearing is deliberately deferred to the connection loop so it can tear
/// down the matching connection before removing the bond from the stack.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) struct ProfileActionEffect {
    pub(crate) select_slot: Option<u8>,
    pub(crate) profile_changed: bool,
    pub(crate) clear_slot: Option<u8>,
    pub(crate) reject_slot: Option<u8>,
}

fn profile_action_effect(action: BleProfileAction, current: u8) -> ProfileActionEffect {
    let (select_slot, clear_slot) = match action {
        BleProfileAction::Switch(profile) => (Some(profile), None),
        BleProfileAction::Previous => (
            Some(if current == 0 {
                NUM_BLE_PROFILE as u8 - 1
            } else {
                current - 1
            }),
            None,
        ),
        BleProfileAction::Next => (Some((current + 1) % NUM_BLE_PROFILE as u8), None),
        BleProfileAction::ClearAndSwitch(profile) => (Some(profile), Some(profile)),
        BleProfileAction::ClearBond => (None, Some(current)),
    };

    ProfileActionEffect {
        select_slot,
        profile_changed: select_slot.is_some_and(|profile| profile != current),
        clear_slot,
        reject_slot: None,
    }
}

/// Manage BLE profiles and bonding information
///
/// ProfileManager is responsible for:
/// 1. Managing multiple BLE profiles, allowing users to switch between multiple devices
/// 2. Storing and loading bonding information for each profile
/// 3. Updating the bonding information of the active profile to the BLE stack
/// 4. Handling profile switch, clear, and save operations
#[cfg(feature = "_ble")]
pub(crate) struct ProfileManager<'b, 's, C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool>
where
    's: 'b,
{
    /// List of bonded devices
    bonded_devices: heapless::Vec<ProfileInfo, NUM_BLE_PROFILE>,
    /// BLE stack
    stack: &'b Stack<'s, C, P>,
}

#[cfg(feature = "_ble")]
impl<'b, 's, C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool> ProfileManager<'b, 's, C, P>
where
    's: 'b,
{
    /// Create a new profile manager
    pub(crate) fn new(stack: &'b Stack<'s, C, P>) -> Self {
        Self {
            bonded_devices: heapless::Vec::new(),
            stack,
        }
    }

    /// Load stored bonding information
    #[cfg(feature = "storage")]
    pub(crate) async fn load_bonded_devices(&mut self) {
        use crate::storage::{read_active_ble_profile, read_bond_info};

        self.bonded_devices.clear();
        for slot_num in 0..NUM_BLE_PROFILE {
            if let Some(info) = read_bond_info(slot_num as u8).await
                && !info.removed
                && let Err(e) = self.bonded_devices.push(info)
            {
                error!("Failed to add bond info: {:?}", e);
            }
        }
        debug!("Loaded {} bond info", self.bonded_devices.len());

        let profile = match read_active_ble_profile().await {
            Some(profile) if (profile as usize) < NUM_BLE_PROFILE => {
                debug!("Loaded active profile: {}", profile);
                profile
            }
            Some(profile) => {
                warn!("Stored BLE profile {} is invalid, using profile 0", profile);
                0
            }
            None => {
                debug!("Loaded default active profile");
                0
            }
        };
        set_ble_profile(profile);
    }

    /// Cached bond info for a profile, cloned to free the caller from borrow
    /// conflicts with concurrent `update_profile()`.
    pub(crate) fn bond_info(&self, slot_num: u8) -> Option<ProfileInfo> {
        self.bonded_devices
            .iter()
            .find(|bond_info| !bond_info.removed && bond_info.slot_num == slot_num)
            .cloned()
    }

    pub(crate) fn bonded_profiles(&self) -> impl Iterator<Item = &ProfileInfo> {
        self.bonded_devices.iter().filter(|info| !info.removed)
    }

    /// Restore every persisted BLE host bond into the freshly created stack.
    pub(crate) fn update_stack_bonds(&self) {
        for info in self.bonded_devices.iter().filter(|info| !info.removed) {
            debug!("Add bond info of profile {}: {:?}", info.slot_num, info);
            if let Err(e) = self.stack.add_bond_information(info.info.clone()) {
                debug!("Add bond info error: {:?}", e);
            }
        }
    }

    /// Add/update bonding information
    pub(crate) async fn add_profile_info(&mut self, mut profile_info: ProfileInfo) -> bool {
        profile_info.removed = false;

        if let Some(index) = self
            .bonded_devices
            .iter()
            .position(|info| info.slot_num == profile_info.slot_num)
        {
            if !self.bonded_devices[index].removed
                && self.bonded_devices[index].info == profile_info.info
                && self.bonded_devices[index].cccd_table == profile_info.cccd_table
            {
                info!("Skip saving same bonding info");
                return true;
            }
        }

        if let Err(e) = self.stack.add_bond_information(profile_info.info.clone()) {
            error!("Failed to add bond info of profile {}: {:?}", profile_info.slot_num, e);
            return false;
        }

        // Update profile information in memory only after the stack accepted
        // the bond, so persisted state cannot claim an unusable profile.
        if let Some(index) = self
            .bonded_devices
            .iter()
            .position(|info| info.slot_num == profile_info.slot_num)
        {
            let old_identity = self.bonded_devices[index].info.identity;
            if !old_identity.match_identity(&profile_info.info.identity)
                && let Err(e) = self.stack.remove_bond_information(old_identity)
            {
                debug!("Remove old bond info of profile {}: {:?}", profile_info.slot_num, e);
            }
            self.bonded_devices[index] = profile_info.clone();
        } else {
            // If there is no bonding information with the same slot number, add it
            if let Err(e) = self.bonded_devices.push(profile_info.clone()) {
                error!("Failed to add bond info: {:?}", e);
                if let Err(remove_error) = self.stack.remove_bond_information(profile_info.info.identity) {
                    debug!("Rollback rejected bond info error: {:?}", remove_error);
                }
                return false;
            }
        }

        #[cfg(feature = "storage")]
        // Send bonding information to the flash task for saving
        FLASH_CHANNEL
            .send(crate::storage::FlashOperationMessage::ProfileInfo(profile_info))
            .await;

        true
    }

    /// Update CCCD table in the stack
    pub(crate) async fn update_profile_cccd_table(&mut self, update: ProfileCccdTable) {
        // Update profile information in memory
        if let Some(index) = self
            .bonded_devices
            .iter()
            .position(|info| info.slot_num == update.slot_num)
        {
            if self.bonded_devices[index].cccd_table == update.table {
                debug!("Skip updating same CCCD table");
                return;
            }

            debug!("Updating profile {} CCCD table: {:?}", update.slot_num, update.table);
            self.bonded_devices[index].cccd_table = update.table;

            #[cfg(feature = "storage")]
            FLASH_CHANNEL
                .send(crate::storage::FlashOperationMessage::ProfileInfo(
                    self.bonded_devices[index].clone(),
                ))
                .await;
        } else {
            error!("Failed to update profile CCCD table: profile not found");
        }
    }

    /// Clear bonding information of the specified slot
    pub(crate) async fn clear_bond(&mut self, slot_num: u8) {
        info!("Clearing bonding information on profile: {}", slot_num);

        // Update bonding information in memory
        if let Some(bond_info) = self
            .bonded_devices
            .iter_mut()
            .find(|bond_info| bond_info.slot_num == slot_num && !bond_info.removed)
        {
            bond_info.removed = true;
            if let Err(e) = self.stack.remove_bond_information(bond_info.info.identity) {
                debug!("Remove bond info of profile {}: {:?}", slot_num, e);
            }
        }

        #[cfg(feature = "storage")]
        FLASH_CHANNEL
            .send(crate::storage::FlashOperationMessage::ClearSlot(slot_num))
            .await;
    }

    async fn apply_profile_action(&mut self, action: BleProfileAction) -> ProfileActionEffect {
        let effect = profile_action_effect(action, current_profile());
        if effect
            .select_slot
            .is_some_and(|profile| profile as usize >= NUM_BLE_PROFILE)
        {
            warn!("Ignoring invalid BLE profile selection");
            return ProfileActionEffect::default();
        }
        if let Some(profile) = effect.select_slot
            && effect.profile_changed
        {
            #[cfg(feature = "storage")]
            FLASH_CHANNEL
                .send(crate::storage::FlashOperationMessage::ActiveBleProfile(profile))
                .await;
            info!("Selected BLE profile: {}", profile);
        }

        info!("Update profile done");
        effect
    }

    /// Process one pending profile update without parking the caller.
    pub(crate) async fn poll_profile_update(
        &mut self,
        slot_generations: &[u32; NUM_BLE_PROFILE],
    ) -> ProfileActionEffect {
        if let Ok(action) = BLE_PROFILE_CHANNEL.try_receive() {
            return self.apply_profile_action(action).await;
        }

        if let Ok(update) = UPDATED_PROFILE.try_receive() {
            let slot_num = update.profile_info.slot_num;
            if slot_generations.get(slot_num as usize).copied() == Some(update.generation) {
                if !self.add_profile_info(update.profile_info).await {
                    return ProfileActionEffect {
                        reject_slot: Some(slot_num),
                        ..Default::default()
                    };
                }
            } else {
                warn!(
                    "Ignoring stale BLE profile update for slot {}, generation {}",
                    slot_num, update.generation
                );
            }
            return ProfileActionEffect::default();
        }

        if let Ok(table) = UPDATED_CCCD_TABLE.try_receive() {
            if slot_generations.get(table.slot_num as usize).copied() == Some(table.generation) {
                self.update_profile_cccd_table(table).await;
            } else {
                warn!(
                    "Ignoring stale BLE CCCD update for slot {}, generation {}",
                    table.slot_num, table.generation
                );
            }
        }

        ProfileActionEffect::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{BleProfileAction, profile_action_effect};

    #[test]
    fn selecting_current_profile_does_not_report_a_change() {
        let effect = profile_action_effect(BleProfileAction::Switch(2), 2);

        assert_eq!(effect.select_slot, Some(2));
        assert!(!effect.profile_changed);
        assert_eq!(effect.clear_slot, None);
    }

    #[test]
    fn clearing_current_profile_still_requests_a_clear() {
        let effect = profile_action_effect(BleProfileAction::ClearAndSwitch(2), 2);

        assert_eq!(effect.select_slot, Some(2));
        assert!(!effect.profile_changed);
        assert_eq!(effect.clear_slot, Some(2));
    }
}
