//! Initialize flash boilerplate of RMK, including USB or BLE
//!

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use rmk_config::resolved::Hardware;
use rmk_config::resolved::hardware::ChipSeries;

#[cfg(any(feature = "dfu_rp", feature = "dfu_nrf"))]
use rmk_config::resolved::hardware::DfuConfig;

pub(crate) fn expand_flash_init(hardware: &Hardware) -> TokenStream2 {
    #[cfg(feature = "shared_flash")]
    if let Some(error) = shared_flash_configuration_error(
        hardware.chip.series.clone(),
        hardware.storage.is_some(),
        hardware.communication.ble_enabled(),
    ) {
        return quote! { compile_error!(#error); };
    }

    if hardware.storage.is_none() {
        // This config actually does nothing if storage is disabled
        return quote! {
            // let storage_config = ::rmk::config::StorageConfig::default();
            // let flash = ::rmk::DummyFlash::new();
        };
    }
    let storage = hardware.storage.as_ref().unwrap();
    let num_sectors = storage.num_sectors;
    let _start_addr = storage.start_addr;
    let clear_storage = storage.clear_storage;
    let clear_layout = storage.clear_layout;

    // With dfu, the flash is already a partition that starts at the
    // storage offset, so the relative offset must be 0.
    #[cfg(any(feature = "dfu_rp", feature = "dfu_nrf"))]
    let storage_start_addr = 0usize;
    #[cfg(not(any(feature = "dfu_rp", feature = "dfu_nrf")))]
    let storage_start_addr = _start_addr;

    let mut flash_init = quote! {
        let storage_config = ::rmk::config::StorageConfig {
            num_sectors: #num_sectors,
            start_addr: #storage_start_addr,
            clear_storage: #clear_storage,
            clear_layout: #clear_layout
        };
    };
    flash_init.extend(
    match hardware.chip.series {
            ChipSeries::Stm32 => {
                quote! {
                    let flash = ::rmk::storage::async_flash_wrapper(::embassy_stm32::flash::Flash::new_blocking(p.FLASH));
                }
            }
            ChipSeries::Nrf52 => {
                #[cfg(feature = "dfu_nrf")]
                let flash_code = {
                    let dfu = hardware.dfu.as_ref().expect(
                        "[dfu] section is required in keyboard.toml (or chip default) when dfu_nrf is enabled"
                    );
                    let storage_num_sectors = hardware.storage.as_ref().map(|s| s.num_sectors).unwrap_or(32) as u32;
                    let erase_size = dfu.page_size;
                    let storage_offset = dfu.dfu_offset + dfu.dfu_size;
                    let storage_size = storage_num_sectors * erase_size;
                    let state_offset = dfu.state_offset;
                    let state_size = dfu.state_size;
                    let dfu_offset = dfu.dfu_offset;
                    let dfu_size = dfu.dfu_size;
                    let dfu_unlock_keys = expand_dfu_unlock_keys(dfu);
                    quote! {
                        #dfu_unlock_keys
                        let flash = ::rmk::storage::async_flash_wrapper(
                            ::rmk::dfu::init_flash(
                                p.NVMC,
                                #storage_offset,
                                #storage_size,
                                #state_offset,
                                #state_size,
                                #dfu_offset,
                                #dfu_size,
                            )
                        );
                    }
                };
                #[cfg(all(not(feature = "dfu_nrf"), feature = "shared_flash"))]
                let flash_code = expand_shared_flash_init();
                #[cfg(all(not(feature = "dfu_nrf"), not(feature = "shared_flash")))]
                let flash_code = quote! {
                    let flash = ::nrf_mpsl::Flash::take(mpsl, p.NVMC);
                };
                flash_code
            }
        ChipSeries::Rp2040 => {
            #[cfg(not(feature = "dfu_rp"))]
            {
                quote! {
                    const FLASH_SIZE: usize = 2 * 1024 * 1024;
                    let flash = ::embassy_rp::flash::Flash::<_, ::embassy_rp::flash::Async, FLASH_SIZE>::new(
                        p.FLASH, p.DMA_CH1, Irqs,
                    );
                }
            }
            #[cfg(feature = "dfu_rp")]
            {
                let dfu = hardware.dfu.as_ref().expect(
                    "[dfu] section is required in keyboard.toml (or chip default) when dfu_rp is enabled"
                );
                let storage_num_sectors = hardware.storage.as_ref().map(|s| s.num_sectors).unwrap_or(32) as u32;
                let erase_size = dfu.page_size;
                let storage_offset = dfu.dfu_offset + dfu.dfu_size;
                let storage_size = storage_num_sectors * erase_size;
                let state_offset = dfu.state_offset;
                let state_size = dfu.state_size;
                let dfu_offset = dfu.dfu_offset;
                let dfu_size = dfu.dfu_size;
                let dfu_unlock_keys = expand_dfu_unlock_keys(dfu);
                quote! {
                    #dfu_unlock_keys
                    let flash = ::rmk::storage::async_flash_wrapper(
                        ::rmk::dfu::init_flash(
                            p.FLASH,
                            #storage_offset,
                            #storage_size,
                            #state_offset,
                            #state_size,
                            #dfu_offset,
                            #dfu_size,
                        )
                    );
                }
            }
            }
            ChipSeries::Esp32 => {
                // ESP32 and ESP32-S3 are dual-core. Flash writes must auto-park it to avoid
                // `FlashStorageError::OtherCoreRunning`.
                let chip_name = hardware.chip.chip.to_lowercase();
                if chip_name == "esp32s3"{
                    quote! {
                        let flash = ::rmk::storage::async_flash_wrapper(
                            ::esp_storage::FlashStorage::new(p.FLASH).multicore_auto_park()
                        );
                    }
                } else {
                    quote! {
                        let flash = ::rmk::storage::async_flash_wrapper(::esp_storage::FlashStorage::new(p.FLASH));
                    }
                }
            },
        }
    );

    flash_init
}

#[cfg(feature = "shared_flash")]
fn shared_flash_configuration_error(
    series: ChipSeries,
    storage_enabled: bool,
    ble_enabled: bool,
) -> Option<&'static str> {
    if !storage_enabled {
        return Some("the `shared_flash` feature requires `[storage].enabled = true`");
    }
    if series != ChipSeries::Nrf52 {
        return Some(
            "the generated `shared_flash` service is supported only on nRF52 BLE keyboards",
        );
    }
    if !ble_enabled {
        return Some("the generated `shared_flash` service requires BLE to be enabled");
    }
    #[cfg(feature = "dfu_nrf")]
    return Some("the `shared_flash` and `dfu_nrf` features cannot be enabled together");
    #[cfg(not(feature = "dfu_nrf"))]
    None
}

#[cfg(all(feature = "shared_flash", not(feature = "dfu_nrf")))]
fn expand_shared_flash_init() -> TokenStream2 {
    quote! {
        let flash = {
            let flash_driver = ::nrf_mpsl::Flash::take(mpsl, p.NVMC);
            let flash_capacity = ::rmk::shared_flash::flash_capacity(&flash_driver);
            static SHARED_FLASH: ::static_cell::StaticCell<
                ::rmk::shared_flash::FlashMutex<::nrf_mpsl::Flash<'static>>,
            > = ::static_cell::StaticCell::new();
            let shared = SHARED_FLASH.init(::rmk::shared_flash::FlashMutex::new(flash_driver));
            #[::embassy_executor::task]
            async fn shared_flash_service(
                shared: &'static ::rmk::shared_flash::FlashMutex<::nrf_mpsl::Flash<'static>>,
            ) {
                ::rmk::shared_flash::service(shared).await
            }
            spawner.spawn(shared_flash_service(shared).unwrap());
            ::rmk::shared_flash::StorageFlash::new(shared, flash_capacity)
        };
    }
}

/// Generate the `DFU_UNLOCK_KEYS` constant from the resolved DFU config.
#[cfg(any(feature = "dfu_rp", feature = "dfu_nrf"))]
fn expand_dfu_unlock_keys(dfu: &DfuConfig) -> TokenStream2 {
    if dfu.unlock_keys.is_empty() {
        return quote! {};
    }
    let keys_expr = dfu
        .unlock_keys
        .iter()
        .map(|key| {
            let row = key[0];
            let col = key[1];
            quote! { (#row, #col) }
        })
        .collect::<Vec<_>>();
    quote! {
        const DFU_UNLOCK_KEYS: &[(u8, u8)] = &[#(#keys_expr), *];
    }
}

#[cfg(all(test, feature = "shared_flash"))]
mod tests {
    use super::*;

    #[test]
    fn rejects_storage_disabled_and_non_nrf_generation() {
        assert!(shared_flash_configuration_error(ChipSeries::Nrf52, false, true).is_some());
        assert!(shared_flash_configuration_error(ChipSeries::Nrf52, true, false).is_some());
        assert!(shared_flash_configuration_error(ChipSeries::Stm32, true, true).is_some());
        assert!(shared_flash_configuration_error(ChipSeries::Rp2040, true, true).is_some());
        assert!(shared_flash_configuration_error(ChipSeries::Esp32, true, true).is_some());
    }

    #[cfg(not(feature = "dfu_nrf"))]
    #[test]
    fn accepts_nrf_storage_and_generates_service() {
        assert_eq!(
            shared_flash_configuration_error(ChipSeries::Nrf52, true, true),
            None
        );
        let generated = expand_shared_flash_init().to_string();
        assert!(generated.contains("shared_flash_service"));
        assert!(generated.contains("StorageFlash"));
        assert!(generated.contains("flash_capacity"));
    }

    #[cfg(feature = "dfu_nrf")]
    #[test]
    fn rejects_dfu_nrf_conflict() {
        assert_eq!(
            shared_flash_configuration_error(ChipSeries::Nrf52, true, true),
            Some("the `shared_flash` and `dfu_nrf` features cannot be enabled together")
        );
    }
}
