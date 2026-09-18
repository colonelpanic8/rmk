//! Every advertisement RMK sends, in one place.

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Instant, with_timeout};
use rmk_types::constants::{
    BLE_ADVERTISING_FAST_INTERVAL_MS, BLE_ADVERTISING_FAST_TIMEOUT_SECS, BLE_ADVERTISING_SLOW_INTERVAL_MS,
};
use trouble_host::prelude::appearance::human_interface_device::KEYBOARD;
use trouble_host::prelude::service::{BATTERY, HUMAN_INTERFACE_DEVICE};
use trouble_host::prelude::*;

/// Company identifier marking an advertisement as RMK's own.
const RMK_ADV_COMPANY_ID: u16 = 0x5253;

// First payload byte of an RMK advertisement, naming the kind. This is wire
// format between two RMK devices: append kinds, never renumber.
const SPLIT_PERIPHERAL: u8 = 0;
const DONGLE_SEEKING: u8 = 1;

/// An advertisement, named for the peer meant to answer it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub(crate) enum Adv<'a> {
    /// A peer that already knows us: ADV_DIRECT_IND carries no data, only this address.
    Directed(Address),
    /// Any BLE host, which finds us as a standard HID keyboard.
    Host { name: &'a str },
    /// The split central that owns peripheral `id`.
    SplitPeripheral { id: u8 },
    /// An RMK dongle whose pairing window is open.
    DongleSeeking,
}

impl Adv<'_> {
    /// Encode into `buf`, which the returned advertisement borrows.
    fn build<'b>(&self, buf: &'b mut [u8; 31]) -> Result<Advertisement<'b>, Error> {
        let adv_data: &[AdStructure] = match *self {
            Self::Directed(peer) => return Ok(Advertisement::ConnectableNonscannableDirected { peer }),
            Self::Host { name } => &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::CompleteServiceUuids16(&[BATTERY.to_le_bytes(), HUMAN_INTERFACE_DEVICE.to_le_bytes()]),
                AdStructure::CompleteLocalName(name.as_bytes()),
                AdStructure::Unknown {
                    ty: 0x19, // Appearance, which trouble-host has no variant for
                    data: &KEYBOARD.to_le_bytes(),
                },
            ],
            // The two kinds below name themselves to another RMK device and nothing
            // else, undiscoverable: only RMK should act on these, and no host should
            // list them.
            Self::SplitPeripheral { id } => &[
                AdStructure::Flags(BR_EDR_NOT_SUPPORTED),
                AdStructure::ManufacturerSpecificData {
                    company_identifier: RMK_ADV_COMPANY_ID,
                    payload: &[SPLIT_PERIPHERAL, id],
                },
            ],
            Self::DongleSeeking => &[
                AdStructure::Flags(BR_EDR_NOT_SUPPORTED),
                AdStructure::ManufacturerSpecificData {
                    company_identifier: RMK_ADV_COMPANY_ID,
                    payload: &[DONGLE_SEEKING],
                },
            ],
        };
        AdStructure::encode_slice(adv_data, &mut buf[..])?;
        Ok(Advertisement::ConnectableScannableUndirected {
            adv_data: &buf[..],
            scan_data: &[],
        })
    }

    /// Read an RMK advertisement out of a scan report, or `None` if the report
    /// is not one of ours.
    pub(crate) fn decode(adv_data: &[u8]) -> Option<Adv<'static>> {
        let mut rest = adv_data;
        loop {
            // Every AD structure is a length byte covering the type byte and the data.
            let (&len, tail) = rest.split_first()?;
            let (structure, tail) = tail.split_at_checked(len as usize)?;
            rest = tail;
            // 0xFF is manufacturer-specific data: company id (little-endian), then us.
            let [0xFF, lo, hi, payload @ ..] = structure else {
                continue;
            };
            if u16::from_le_bytes([*lo, *hi]) != RMK_ADV_COMPANY_ID {
                continue;
            }
            return match payload {
                [SPLIT_PERIPHERAL, id] => Some(Adv::SplitPeripheral { id: *id }),
                [DONGLE_SEEKING] => Some(Adv::DongleSeeking),
                // A kind only a newer firmware knows.
                _ => None,
            };
        }
    }

    fn phase(
        &self,
        elapsed: Duration,
        timeout: Duration,
        fast_timeout: Duration,
    ) -> Option<(AdvertisementParameters, Duration)> {
        if elapsed >= timeout {
            return None;
        }
        let fast = matches!(self, Self::Host { .. })
            && BLE_ADVERTISING_FAST_INTERVAL_MS != BLE_ADVERTISING_SLOW_INTERVAL_MS
            && elapsed < fast_timeout;
        let phase_end = if fast { fast_timeout.min(timeout) } else { timeout };
        let (phy, interval) = match self {
            Self::Host { .. } => (
                PhyKind::Le2M,
                Duration::from_millis(u64::from(if fast {
                    BLE_ADVERTISING_FAST_INTERVAL_MS
                } else {
                    BLE_ADVERTISING_SLOW_INTERVAL_MS
                })),
            ),
            _ => (PhyKind::Le1M, Duration::from_millis(50)),
        };
        Some((
            AdvertisementParameters {
                primary_phy: phy,
                secondary_phy: phy,
                tx_power: TxPower::Plus8dBm,
                interval_min: interval,
                interval_max: interval,
                ..Default::default()
            },
            phase_end - elapsed,
        ))
    }
}

/// Broadcast `adv` and hand back the connection a central makes on it, or
/// [`Error::Timeout`] if none does within `timeout`.
pub(crate) async fn advertise<'a, 'b, C: Controller, const ATT: usize, const CONN: usize>(
    peripheral: &mut Peripheral<'a, C, DefaultPacketPool>,
    server: &'b AttributeServer<'_, NoopRawMutex, DefaultPacketPool, ATT, CONN>,
    adv: Adv<'_>,
    timeout: Duration,
) -> Result<GattConnection<'a, 'b, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut buf = [0; 31];
    let started = Instant::now();
    let fast_timeout = Duration::from_secs(u64::from(BLE_ADVERTISING_FAST_TIMEOUT_SECS));
    while let Some((params, remaining)) = adv.phase(started.elapsed(), timeout, fast_timeout) {
        // Include controller setup in the deadline and drop the advertiser before restarting.
        let attempt = async {
            let advertiser = peripheral.advertise(&params, adv.build(&mut buf)?).await?;
            Ok::<_, BleHostError<C::Error>>(advertiser.accept().await?)
        };
        match with_timeout(remaining, attempt).await {
            Ok(result) => return Ok(result?.with_attribute_server(server)?),
            Err(_) => continue,
        }
    }
    Err(Error::Timeout.into())
}

#[cfg(test)]
mod tests {
    use embassy_time::Duration;
    use rmk_types::constants::{BLE_ADVERTISING_FAST_INTERVAL_MS, BLE_ADVERTISING_SLOW_INTERVAL_MS};
    use trouble_host::prelude::Address;

    use super::Adv;

    #[test]
    fn host_backs_off_without_extending_the_deadline() {
        let host = Adv::Host { name: "RMK" };
        let total = Duration::from_secs(300);
        let fast = Duration::from_secs(5);
        let (params, remaining) = host.phase(Duration::from_secs(2), total, fast).unwrap();
        assert_eq!(
            params.interval_min,
            Duration::from_millis(BLE_ADVERTISING_FAST_INTERVAL_MS.into())
        );
        assert_eq!(params.interval_max, params.interval_min);
        assert_eq!(remaining, Duration::from_secs(3));
        let (params, remaining) = host.phase(fast, total, fast).unwrap();
        assert_eq!(
            params.interval_min,
            Duration::from_millis(BLE_ADVERTISING_SLOW_INTERVAL_MS.into())
        );
        assert_eq!(remaining, Duration::from_secs(295));
        assert!(host.phase(total, total, fast).is_none());
        assert!(host.phase(total + fast, total, fast).is_none());
    }

    #[test]
    fn short_deadlines_and_disabled_fast_window() {
        let host = Adv::Host { name: "RMK" };
        let total = Duration::from_secs(2);
        let (_, remaining) = host
            .phase(Duration::from_secs(1), total, Duration::from_secs(5))
            .unwrap();
        assert_eq!(remaining, Duration::from_secs(1));
        let (params, remaining) = host
            .phase(Duration::from_secs(0), total, Duration::from_secs(0))
            .unwrap();
        assert_eq!(
            params.interval_min,
            Duration::from_millis(BLE_ADVERTISING_SLOW_INTERVAL_MS.into())
        );
        assert_eq!(remaining, total);
        assert!(
            host.phase(Duration::from_secs(0), Duration::from_secs(0), Duration::from_secs(5))
                .is_none()
        );
    }

    #[test]
    fn split_and_dongle_do_not_back_off() {
        for adv in [
            Adv::SplitPeripheral { id: 0 },
            Adv::DongleSeeking,
            Adv::Directed(Address::random([1; 6])),
        ] {
            for elapsed in [Duration::from_secs(0), Duration::from_secs(5)] {
                let total = Duration::from_secs(30);
                let (params, remaining) = adv.phase(elapsed, total, Duration::from_secs(5)).unwrap();
                assert_eq!(params.interval_min, Duration::from_millis(50));
                assert_eq!(remaining, total - elapsed);
            }
        }
    }

    /// Overrunning the 31-byte legacy advertisement only fails at runtime.
    fn fits(adv: Adv<'_>) -> bool {
        adv.build(&mut [0; 31]).is_ok()
    }

    #[test]
    fn every_advertisement_fits_the_legacy_budget() {
        assert!(fits(Adv::SplitPeripheral { id: 0xFF }));
        assert!(fits(Adv::DongleSeeking));
        // Flags, UUIDs and appearance leave 16 bytes for the name.
        assert!(fits(Adv::Host {
            name: "0123456789abcdef"
        }));
        assert!(!fits(Adv::Host {
            name: "0123456789abcdefg"
        }));
    }

    #[test]
    fn every_rmk_kind_round_trips_through_an_advertisement() {
        for adv in [Adv::SplitPeripheral { id: 2 }, Adv::DongleSeeking] {
            let mut buf = [0; 31];
            adv.build(&mut buf).unwrap();
            assert_eq!(Adv::decode(&buf), Some(adv));
        }
    }

    #[test]
    fn decode_skips_preceding_structures() {
        // An 18-byte 128-bit service UUID list sits between the flags and the MSD.
        let mut data = [0u8; 27];
        data[..5].copy_from_slice(&[0x02, 0x01, 0x06, 0x11, 0x07]);
        data[21..].copy_from_slice(&[0x05, 0xFF, 0x53, 0x52, 0x00, 0x02]);
        assert_eq!(Adv::decode(&data), Some(Adv::SplitPeripheral { id: 2 }));
    }

    #[test]
    fn decode_rejects_foreign_unknown_and_malformed_reports() {
        // Another vendor's company id.
        assert_eq!(
            Adv::decode(&[0x02, 0x01, 0x04, 0x05, 0xFF, 0x4C, 0x00, 0x00, 0x02]),
            None
        );
        // A kind a newer firmware knows and this one does not.
        assert_eq!(Adv::decode(&[0x04, 0xFF, 0x53, 0x52, 0x7F]), None);
        // A length running past the end of the report.
        assert_eq!(Adv::decode(&[0x02, 0x01, 0x04, 0x09, 0xFF, 0x53]), None);
    }
}
