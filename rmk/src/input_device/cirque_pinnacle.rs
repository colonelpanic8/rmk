//! Cirque Pinnacle (1CA027) touchpad driver.
//!
//! The Pinnacle is the touch ASIC in Cirque's TM040040/TM035035/TM023023
//! "GlidePoint" circle trackpads, used as a pair of SPI modules on MoErgo's
//! Go60. This driver runs the chip in relative mode: the pad streams
//! PS/2-style delta packets and (unless taps are disabled) synthesizes
//! button presses from tap gestures, which are published through
//! [`PointingEvent::buttons`].
//!
//! The register sequence mirrors the ZMK community driver
//! (petejohanson/cirque-input-module) that MoErgo ships for the Go60, so a
//! pad configured identically behaves identically:
//!
//! * ADC attenuation ("sensitivity") and per-axis minimum Z are tuned
//!   through the extended register access (ERA) window at 0x1B..0x1E.
//! * Feed config 2 carries tap/rotation options; feed config 1 enables the
//!   feed and per-axis inversion, applied by the chip before packets are
//!   produced.
//! * Packets are read from 0x12 after the data-ready (HW_DR) line rises and
//!   acknowledged by clearing STATUS1.
//!
//! Like that driver, the wheel byte of the 4-byte Intellimouse packet is
//! not consumed; only the 3-byte delta/button packet is read.
//!
//! Touch presence is published on the Z axis (absolute, 1 while a finger
//! is on the pad). Relative mode has no explicit liftoff flag, but the pad
//! is configured to send [`Z_IDLE_COUNT`] all-zero packets after a finger
//! lifts and stays silent while a resting finger merely stops moving, so
//! an all-zero packet with no buttons is itself the liftoff signal.

use embassy_time::Timer;
use embedded_hal::digital::OutputPin;
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::SpiBus;
use rmk_macro::input_device;

use crate::event::{Axis, AxisEvent, AxisValType, PointingEvent};

const WRITE: u8 = 0x80;
const READ: u8 = 0xA0;
const AUTOINC: u8 = 0xFC;
const FILLER: u8 = 0xFB;

const REG_FW_ID: u8 = 0x00;
const REG_STATUS1: u8 = 0x02;
const STATUS1_SW_DR: u8 = 1 << 2;
const REG_SYS_CFG: u8 = 0x03;
const SYS_CFG_EN_SLEEP: u8 = 1 << 2;
const SYS_CFG_RESET: u8 = 1 << 0;
const REG_FEED_CFG1: u8 = 0x04;
const FEED_CFG1_EN_FEED: u8 = 1 << 0;
const FEED_CFG1_INV_X: u8 = 1 << 6;
const FEED_CFG1_INV_Y: u8 = 1 << 7;
const REG_FEED_CFG2: u8 = 0x05;
const FEED_CFG2_EN_IM: u8 = 1 << 0;
const FEED_CFG2_DIS_TAP: u8 = 1 << 1;
const FEED_CFG2_DIS_SEC: u8 = 1 << 2;
const FEED_CFG2_EN_BTN_SCRL: u8 = 1 << 6;
const FEED_CFG2_ROTATE_90: u8 = 1 << 7;
const REG_CAL_CFG: u8 = 0x07;
const CAL_CFG_CALIBRATE: u8 = 1 << 0;
const REG_Z_IDLE: u8 = 0x0A;
const REG_SLEEP_INTERVAL: u8 = 0x0C;
const REG_PACKET0: u8 = 0x12;

const REG_ERA_VALUE: u8 = 0x1B;
const REG_ERA_HIGH_BYTE: u8 = 0x1C;
const REG_ERA_LOW_BYTE: u8 = 0x1D;
const REG_ERA_CONTROL: u8 = 0x1E;
const ERA_CONTROL_READ: u8 = 0x01;
const ERA_CONTROL_WRITE: u8 = 0x02;

const ERA_REG_X_AXIS_WIDE_Z_MIN: u16 = 0x0149;
const ERA_REG_Y_AXIS_WIDE_Z_MIN: u16 = 0x0168;
const ERA_REG_TRACKING_ADC_CONFIG: u16 = 0x0187;

const PACKET0_BTN_MASK: u8 = 0x07;
const PACKET0_X_SIGN: u8 = 1 << 4;
const PACKET0_Y_SIGN: u8 = 1 << 5;

/// Number of Z=0 packets the pad sends after a finger lifts; the ZMK driver
/// uses 5 and tap release detection depends on at least one being sent.
const Z_IDLE_COUNT: u8 = 5;

/// ADC attenuation. `X1` is the most sensitive, `X4` the least.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum PinnacleSensitivity {
    X1,
    X2,
    X3,
    X4,
}

impl PinnacleSensitivity {
    fn adc_config_bits(self) -> u8 {
        match self {
            PinnacleSensitivity::X1 => 0x00,
            PinnacleSensitivity::X2 => 0x40,
            PinnacleSensitivity::X3 => 0x80,
            PinnacleSensitivity::X4 => 0xC0,
        }
    }
}

/// Pad configuration, applied once during initialization.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PinnacleConfig {
    pub sensitivity: PinnacleSensitivity,
    /// Swap the X and Y axes in hardware (pad mounted rotated 90°).
    pub rotate_90: bool,
    /// Invert the X axis in hardware.
    pub x_invert: bool,
    /// Invert the Y axis in hardware.
    pub y_invert: bool,
    /// Disable all tap gestures (no device-originated buttons).
    pub no_taps: bool,
    /// Disable only the secondary (corner/two-finger) tap.
    pub no_secondary_tap: bool,
    /// Let the pad drop into its low-power state after 5 s without a finger.
    pub sleep: bool,
    /// Minimum Z ("wide Z min") for X-axis touch detection at the pad edge.
    pub x_axis_z_min: u8,
    /// Minimum Z ("wide Z min") for Y-axis touch detection at the pad edge.
    pub y_axis_z_min: u8,
}

impl Default for PinnacleConfig {
    fn default() -> Self {
        Self {
            sensitivity: PinnacleSensitivity::X1,
            rotate_90: false,
            x_invert: false,
            y_invert: false,
            no_taps: false,
            no_secondary_tap: false,
            sleep: false,
            x_axis_z_min: 5,
            y_axis_z_min: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum PinnacleError {
    /// SPI bus or CS pin failure
    Spi,
    /// A register write was not acknowledged with the filler byte
    BadWriteEcho,
    /// The ERA state machine did not finish an extended register access
    EraTimeout,
    /// Forced recalibration did not complete
    CalibrationTimeout,
}

/// Decode a 3-byte relative-mode packet into (dx, dy, buttons).
///
/// Deltas are 9-bit two's complement: the magnitude byte plus a sign bit in
/// packet 0. Buttons are the low three bits of packet 0, already in HID
/// order (bit 0 primary), asserted by tap gestures in relative mode.
fn decode_relative_packet(packet: [u8; 3]) -> (i16, i16, u8) {
    let mut dx = packet[1] as i16;
    if packet[0] & PACKET0_X_SIGN != 0 {
        dx -= 256;
    }
    let mut dy = packet[2] as i16;
    if packet[0] & PACKET0_Y_SIGN != 0 {
        dy -= 256;
    }
    (dx, dy, packet[0] & PACKET0_BTN_MASK)
}

/// Cirque Pinnacle touchpad as an RMK input device.
///
/// Generic over the SPI bus (mode 1, ≤10 MHz per the datasheet; the Go60
/// modules run at 1 MHz), a dedicated active-low chip select, and the
/// pad's data-ready line (HW_DR, active high).
#[input_device(publish = PointingEvent)]
pub struct CirquePinnacle<SPI: SpiBus, CS: OutputPin, DR: Wait> {
    id: u8,
    spi: SPI,
    cs: CS,
    dr: DR,
    config: PinnacleConfig,
    initialized: bool,
    buttons: u8,
    touching: bool,
}

impl<SPI: SpiBus, CS: OutputPin, DR: Wait> CirquePinnacle<SPI, CS, DR> {
    pub fn new(id: u8, spi: SPI, mut cs: CS, dr: DR, config: PinnacleConfig) -> Self {
        cs.set_high().ok();
        Self {
            id,
            spi,
            cs,
            dr,
            config,
            initialized: false,
            buttons: 0,
            touching: false,
        }
    }

    async fn transfer(&mut self, rx: &mut [u8], tx: &[u8]) -> Result<(), PinnacleError> {
        self.cs.set_low().map_err(|_| PinnacleError::Spi)?;
        let result = self.spi.transfer(rx, tx).await;
        self.cs.set_high().map_err(|_| PinnacleError::Spi)?;
        result.map_err(|_| PinnacleError::Spi)
    }

    async fn write_reg(&mut self, addr: u8, val: u8) -> Result<(), PinnacleError> {
        let tx = [WRITE | addr, val];
        let mut rx = [0u8; 2];
        self.transfer(&mut rx, &tx).await?;
        if rx[1] != FILLER {
            return Err(PinnacleError::BadWriteEcho);
        }
        // The pad needs a short quiet period between register writes.
        Timer::after_micros(50).await;
        Ok(())
    }

    /// Read `buf.len()` registers starting at `addr` (auto-increment read).
    async fn seq_read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), PinnacleError> {
        let mut tx = [AUTOINC; 8];
        let mut rx = [0u8; 8];
        let n = buf.len() + 3;
        debug_assert!(n <= tx.len());
        tx[0] = READ | addr;
        self.transfer(&mut rx[..n], &tx[..n]).await?;
        buf.copy_from_slice(&rx[3..n]);
        Ok(())
    }

    async fn clear_status(&mut self) -> Result<(), PinnacleError> {
        self.write_reg(REG_STATUS1, 0).await
    }

    async fn wait_era_idle(&mut self) -> Result<(), PinnacleError> {
        for _ in 0..100 {
            let mut control = [0u8; 1];
            self.seq_read(REG_ERA_CONTROL, &mut control).await?;
            if control[0] == 0 {
                return Ok(());
            }
            Timer::after_micros(50).await;
        }
        Err(PinnacleError::EraTimeout)
    }

    async fn era_read(&mut self, addr: u16) -> Result<u8, PinnacleError> {
        self.write_reg(REG_ERA_HIGH_BYTE, (addr >> 8) as u8).await?;
        self.write_reg(REG_ERA_LOW_BYTE, addr as u8).await?;
        self.write_reg(REG_ERA_CONTROL, ERA_CONTROL_READ).await?;
        self.wait_era_idle().await?;
        let mut val = [0u8; 1];
        self.seq_read(REG_ERA_VALUE, &mut val).await?;
        self.clear_status().await?;
        Ok(val[0])
    }

    async fn era_write(&mut self, addr: u16, val: u8) -> Result<(), PinnacleError> {
        self.write_reg(REG_ERA_VALUE, val).await?;
        self.write_reg(REG_ERA_HIGH_BYTE, (addr >> 8) as u8).await?;
        self.write_reg(REG_ERA_LOW_BYTE, addr as u8).await?;
        self.write_reg(REG_ERA_CONTROL, ERA_CONTROL_WRITE).await?;
        self.wait_era_idle().await?;
        self.clear_status().await
    }

    async fn force_recalibrate(&mut self) -> Result<(), PinnacleError> {
        let mut cal = [0u8; 1];
        self.seq_read(REG_CAL_CFG, &mut cal).await?;
        self.write_reg(REG_CAL_CFG, cal[0] | CAL_CFG_CALIBRATE).await?;
        // Full calibration takes up to ~100 ms.
        for _ in 0..200 {
            self.seq_read(REG_CAL_CFG, &mut cal).await?;
            if cal[0] & CAL_CFG_CALIBRATE == 0 {
                return Ok(());
            }
            Timer::after_millis(1).await;
        }
        Err(PinnacleError::CalibrationTimeout)
    }

    async fn init(&mut self) -> Result<(), PinnacleError> {
        let mut fw_id = [0u8; 2];
        self.seq_read(REG_FW_ID, &mut fw_id).await?;
        info!(
            "pinnacle {}: found ASIC id 0x{:02x}, firmware version 0x{:02x}",
            self.id, fw_id[0], fw_id[1]
        );

        Timer::after_millis(10).await;
        self.clear_status().await?;
        self.write_reg(REG_SYS_CFG, SYS_CFG_RESET).await?;
        Timer::after_millis(20).await;

        self.write_reg(REG_Z_IDLE, Z_IDLE_COUNT).await?;

        let adc = self.era_read(ERA_REG_TRACKING_ADC_CONFIG).await?;
        self.era_write(
            ERA_REG_TRACKING_ADC_CONFIG,
            (adc & 0x3F) | self.config.sensitivity.adc_config_bits(),
        )
        .await?;

        self.era_write(ERA_REG_X_AXIS_WIDE_Z_MIN, self.config.x_axis_z_min)
            .await?;
        self.era_write(ERA_REG_Y_AXIS_WIDE_Z_MIN, self.config.y_axis_z_min)
            .await?;

        self.force_recalibrate().await?;

        if self.config.sleep {
            let mut sys_cfg = [0u8; 1];
            self.seq_read(REG_SYS_CFG, &mut sys_cfg).await?;
            self.write_reg(REG_SYS_CFG, sys_cfg[0] | SYS_CFG_EN_SLEEP).await?;
        }
        self.write_reg(REG_SLEEP_INTERVAL, 255).await?;

        let mut feed_cfg2 = FEED_CFG2_EN_IM | FEED_CFG2_EN_BTN_SCRL;
        if self.config.no_taps {
            feed_cfg2 |= FEED_CFG2_DIS_TAP;
        }
        if self.config.no_secondary_tap {
            feed_cfg2 |= FEED_CFG2_DIS_SEC;
        }
        if self.config.rotate_90 {
            feed_cfg2 |= FEED_CFG2_ROTATE_90;
        }
        self.write_reg(REG_FEED_CFG2, feed_cfg2).await?;

        let mut feed_cfg1 = FEED_CFG1_EN_FEED;
        if self.config.x_invert {
            feed_cfg1 |= FEED_CFG1_INV_X;
        }
        if self.config.y_invert {
            feed_cfg1 |= FEED_CFG1_INV_Y;
        }
        self.write_reg(REG_FEED_CFG1, feed_cfg1).await?;

        self.clear_status().await?;
        Ok(())
    }

    /// Wait for a data-ready edge, then read and acknowledge one packet.
    ///
    /// Returns `None` for packets that carry no change we can report
    /// (communication glitches, stale DR, or a Z-idle packet past the one
    /// that already reported the liftoff).
    async fn read_packet(&mut self) -> Result<Option<PointingEvent>, PinnacleError> {
        self.dr.wait_for_high().await.map_err(|_| PinnacleError::Spi)?;

        let mut status = [0u8; 1];
        self.seq_read(REG_STATUS1, &mut status).await?;
        // 0xFF means the pad didn't drive MISO; without SW_DR the packet
        // registers hold stale data.
        if status[0] == 0xFF || status[0] & STATUS1_SW_DR == 0 {
            self.clear_status().await?;
            return Ok(None);
        }

        let mut packet = [0u8; 3];
        self.seq_read(REG_PACKET0, &mut packet).await?;
        self.clear_status().await?;

        let (dx, dy, buttons) = decode_relative_packet(packet);
        let mut lifted = false;
        if dx != 0 || dy != 0 {
            self.touching = true;
        } else if buttons == 0 {
            // A resting finger is silent; only the post-liftoff Z-idle
            // burst produces all-zero packets.
            lifted = self.touching;
            self.touching = false;
        }
        if dx == 0 && dy == 0 && buttons == self.buttons && !lifted {
            return Ok(None);
        }
        self.buttons = buttons;

        Ok(Some(PointingEvent {
            device_id: self.id,
            buttons,
            axes: [
                AxisEvent {
                    typ: AxisValType::Rel,
                    axis: Axis::X,
                    value: dx,
                },
                AxisEvent {
                    typ: AxisValType::Rel,
                    axis: Axis::Y,
                    value: dy,
                },
                AxisEvent {
                    typ: AxisValType::Abs,
                    axis: Axis::Z,
                    value: self.touching as i16,
                },
            ],
        }))
    }

    async fn read_pointing_event(&mut self) -> PointingEvent {
        loop {
            if !self.initialized {
                match self.init().await {
                    Ok(()) => self.initialized = true,
                    Err(e) => {
                        error!("pinnacle {}: initialization failed: {:?}; retrying in 1 s", self.id, e);
                        Timer::after_secs(1).await;
                        continue;
                    }
                }
            }
            match self.read_packet().await {
                Ok(Some(event)) => return event,
                Ok(None) => {}
                Err(e) => {
                    error!("pinnacle {}: read failure: {:?}", self.id, e);
                    Timer::after_millis(5).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::vec::Vec;

    use embedded_hal::digital::{ErrorType as PinErrorType, OutputPin};
    use embedded_hal_async::digital::Wait;
    use embedded_hal_async::spi::{ErrorType as SpiErrorType, SpiBus};

    use super::*;
    use crate::test_support::test_block_on as block_on;

    /// Register-level model of a Pinnacle: serves auto-increment reads,
    /// acknowledges writes with the filler byte, completes ERA transactions
    /// and calibration instantly.
    #[derive(Default)]
    struct FakePinnacle {
        regs: [u8; 0x20],
        era_mem: HashMap<u16, u8>,
        write_log: Vec<(u8, u8)>,
    }

    impl FakePinnacle {
        fn era_addr(&self) -> u16 {
            (self.regs[REG_ERA_HIGH_BYTE as usize] as u16) << 8 | self.regs[REG_ERA_LOW_BYTE as usize] as u16
        }

        fn read_reg(&self, addr: u8) -> u8 {
            match addr {
                REG_FW_ID => 0x07, // ASIC id of every Pinnacle revision
                0x01 => 0x3A,
                _ => self.regs[addr as usize],
            }
        }

        fn write_reg(&mut self, addr: u8, val: u8) {
            self.write_log.push((addr, val));
            self.regs[addr as usize] = val;
            match addr {
                REG_ERA_CONTROL => {
                    let era_addr = self.era_addr();
                    if val == ERA_CONTROL_WRITE {
                        self.era_mem.insert(era_addr, self.regs[REG_ERA_VALUE as usize]);
                    } else if val == ERA_CONTROL_READ {
                        self.regs[REG_ERA_VALUE as usize] = self.era_mem.get(&era_addr).copied().unwrap_or(0);
                    }
                    self.regs[REG_ERA_CONTROL as usize] = 0;
                }
                REG_CAL_CFG => {
                    self.regs[REG_CAL_CFG as usize] = val & !CAL_CFG_CALIBRATE;
                }
                _ => {}
            }
        }
    }

    #[derive(Debug)]
    struct NoError;
    impl embedded_hal_async::spi::Error for NoError {
        fn kind(&self) -> embedded_hal_async::spi::ErrorKind {
            embedded_hal_async::spi::ErrorKind::Other
        }
    }
    impl SpiErrorType for FakePinnacle {
        type Error = NoError;
    }

    impl SpiBus for FakePinnacle {
        async fn read(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            words.fill(FILLER);
            Ok(())
        }

        async fn write(&mut self, words: &[u8]) -> Result<(), Self::Error> {
            let mut rx = [0u8; 8];
            self.transfer(&mut rx[..words.len()], words).await
        }

        async fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), Self::Error> {
            read.fill(FILLER);
            let op = write[0];
            if op & !0x1F == READ {
                let base = op & 0x1F;
                for (i, slot) in read.iter_mut().enumerate().skip(3) {
                    *slot = self.read_reg(base + (i as u8 - 3));
                }
            } else if op & !0x1F == WRITE {
                self.write_reg(op & 0x1F, write[1]);
            }
            Ok(())
        }

        async fn transfer_in_place(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            let tx: Vec<u8> = words.to_vec();
            let mut rx = vec![0u8; words.len()];
            self.transfer(&mut rx, &tx).await?;
            words.copy_from_slice(&rx);
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct DummyCs;
    impl PinErrorType for DummyCs {
        type Error = Infallible;
    }
    impl OutputPin for DummyCs {
        fn set_low(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct DummyDr;
    impl PinErrorType for DummyDr {
        type Error = Infallible;
    }
    impl Wait for DummyDr {
        async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn decode_sign_extends_nine_bit_deltas_and_extracts_buttons() {
        assert_eq!(decode_relative_packet([0x00, 0x7F, 0x02]), (127, 2, 0));
        assert_eq!(decode_relative_packet([PACKET0_X_SIGN, 0xFF, 0x00]), (-1, 0, 0));
        assert_eq!(decode_relative_packet([PACKET0_Y_SIGN, 0x00, 0x80]), (0, -128, 0));
        assert_eq!(
            decode_relative_packet([PACKET0_X_SIGN | PACKET0_Y_SIGN, 0x01, 0x01]),
            (-255, -255, 0)
        );
        assert_eq!(decode_relative_packet([0x03, 0x00, 0x00]), (0, 0, 0x03));
    }

    #[test]
    fn init_configures_the_pad_like_the_go60_zmk_definition() {
        // The Go60 board definition: sensitivity 1x, rotate-90, y-invert,
        // no-secondary-tap.
        let config = PinnacleConfig {
            rotate_90: true,
            y_invert: true,
            no_secondary_tap: true,
            ..Default::default()
        };
        let mut device = CirquePinnacle::new(0, FakePinnacle::default(), DummyCs, DummyDr, config);
        block_on(async { device.init().await }).unwrap();

        let pad = &device.spi;
        assert_eq!(pad.regs[REG_Z_IDLE as usize], Z_IDLE_COUNT);
        assert_eq!(pad.regs[REG_SLEEP_INTERVAL as usize], 255);
        assert_eq!(pad.era_mem[&ERA_REG_TRACKING_ADC_CONFIG], 0x00, "1x attenuation");
        assert_eq!(pad.era_mem[&ERA_REG_X_AXIS_WIDE_Z_MIN], 5);
        assert_eq!(pad.era_mem[&ERA_REG_Y_AXIS_WIDE_Z_MIN], 4);
        assert_eq!(
            pad.regs[REG_FEED_CFG2 as usize],
            FEED_CFG2_EN_IM | FEED_CFG2_EN_BTN_SCRL | FEED_CFG2_DIS_SEC | FEED_CFG2_ROTATE_90
        );
        assert_eq!(pad.regs[REG_FEED_CFG1 as usize], FEED_CFG1_EN_FEED | FEED_CFG1_INV_Y);
        assert!(
            pad.write_log.contains(&(REG_SYS_CFG, SYS_CFG_RESET)),
            "the pad must be reset before configuration"
        );
        // Sleep was not requested, so EN_SLEEP must never have been set.
        assert!(
            !pad.write_log
                .iter()
                .any(|&(addr, val)| addr == REG_SYS_CFG && val & SYS_CFG_EN_SLEEP != 0)
        );
    }

    #[test]
    fn packets_become_pointing_events_and_idle_packets_are_suppressed() {
        let mut device = CirquePinnacle::new(7, FakePinnacle::default(), DummyCs, DummyDr, PinnacleConfig::default());
        block_on(async {
            device.init().await.unwrap();

            // A movement packet with the primary button tapped.
            device.spi.regs[REG_STATUS1 as usize] = STATUS1_SW_DR;
            device.spi.regs[REG_PACKET0 as usize] = 0x01 | PACKET0_X_SIGN;
            device.spi.regs[(REG_PACKET0 + 1) as usize] = 0xFE;
            device.spi.regs[(REG_PACKET0 + 2) as usize] = 0x03;
            let event = device.read_packet().await.unwrap().expect("event");
            assert_eq!(event.device_id, 7);
            assert_eq!(event.buttons, 0x01);
            assert_eq!(event.axes[0].value, -2);
            assert_eq!(event.axes[1].value, 3);
            assert_eq!(event.axes[2].value, 1, "motion means a finger is present");
            // Reading the packet acknowledged it.
            assert_eq!(device.spi.regs[REG_STATUS1 as usize], 0);

            // Button released with no motion: still an event (button edge).
            device.spi.regs[REG_STATUS1 as usize] = STATUS1_SW_DR;
            device.spi.regs[REG_PACKET0 as usize] = 0;
            device.spi.regs[(REG_PACKET0 + 1) as usize] = 0;
            device.spi.regs[(REG_PACKET0 + 2) as usize] = 0;
            let event = device.read_packet().await.unwrap().expect("release event");
            assert_eq!(event.buttons, 0);
            assert_eq!(event.axes[0].value, 0);
            assert_eq!(event.axes[2].value, 0, "an all-zero packet is the liftoff");

            // The next idle packet carries no change and is suppressed.
            device.spi.regs[REG_STATUS1 as usize] = STATUS1_SW_DR;
            assert!(device.read_packet().await.unwrap().is_none());

            // Stale DR without SW_DR is ignored.
            device.spi.regs[REG_STATUS1 as usize] = 0;
            assert!(device.read_packet().await.unwrap().is_none());
        });
    }

    #[test]
    fn liftoff_without_a_button_edge_is_reported_exactly_once() {
        let mut device = CirquePinnacle::new(7, FakePinnacle::default(), DummyCs, DummyDr, PinnacleConfig::default());
        block_on(async {
            device.init().await.unwrap();

            // Motion with no buttons: presence rides the Z axis.
            device.spi.regs[REG_STATUS1 as usize] = STATUS1_SW_DR;
            device.spi.regs[REG_PACKET0 as usize] = 0;
            device.spi.regs[(REG_PACKET0 + 1) as usize] = 0x01;
            device.spi.regs[(REG_PACKET0 + 2) as usize] = 0;
            let event = device.read_packet().await.unwrap().expect("motion event");
            assert_eq!(event.buttons, 0);
            assert_eq!(event.axes[2].value, 1);

            // The first Z-idle packet is the liftoff: no motion, no button
            // edge, but it must still produce an event with Z back at 0.
            device.spi.regs[REG_STATUS1 as usize] = STATUS1_SW_DR;
            device.spi.regs[(REG_PACKET0 + 1) as usize] = 0;
            let event = device.read_packet().await.unwrap().expect("liftoff event");
            assert_eq!(event.axes[0].value, 0);
            assert_eq!(event.axes[1].value, 0);
            assert_eq!(event.axes[2].value, 0);

            // The rest of the Z-idle burst is suppressed.
            device.spi.regs[REG_STATUS1 as usize] = STATUS1_SW_DR;
            assert!(device.read_packet().await.unwrap().is_none());
        });
    }
}
