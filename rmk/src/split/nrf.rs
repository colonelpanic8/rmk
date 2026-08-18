use embassy_nrf::buffered_uarte::{BufferedUarteRx, BufferedUarteTx, Error};
use embassy_nrf::gpio::{Input, Output};
use embassy_time::{Duration, Timer};
use embedded_io_async::{ErrorType, Read, Write};

/// nRF buffered UARTE behind a half-duplex transceiver with an explicit
/// direction pin.
///
/// Reception runs continuously into the UARTE ring buffer, so bytes that
/// arrive while no `read()` is pending — or after a `read()` future is
/// dropped — are buffered, not lost. `write()` turns the bus around for
/// exactly one frame: it drives the bus, waits until the last byte has left
/// the shift register, and releases the bus again.
pub struct HalfDuplexUarte<'d> {
    tx: BufferedUarteTx<'d>,
    rx: BufferedUarteRx<'d>,
    direction: Output<'d>,
    turnaround: Duration,
}

impl<'d> HalfDuplexUarte<'d> {
    pub fn new(
        tx: BufferedUarteTx<'d>,
        rx: BufferedUarteRx<'d>,
        mut direction: Output<'d>,
        turnaround: Duration,
    ) -> Self {
        direction.set_low();
        Self {
            tx,
            rx,
            direction,
            turnaround,
        }
    }
}

impl ErrorType for HalfDuplexUarte<'_> {
    type Error = Error;
}

impl Read for HalfDuplexUarte<'_> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        // A dropped `write()` future can leave the bus held; release it
        // before listening.
        self.direction.set_low();
        self.rx.read(buf).await
    }
}

impl Write for HalfDuplexUarte<'_> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.direction.set_high();
        let mut written = 0;
        while written < buf.len() {
            written += self.tx.write(&buf[written..]).await?;
        }
        // Resolves once the TX ring is drained, i.e. after the final ENDTX:
        // the last byte is on the wire.
        self.tx.flush().await?;
        Timer::after(self.turnaround).await;
        self.direction.set_low();
        Ok(buf.len())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Monitor an active-high or active-low split-cable detect input.
///
/// Edge-driven: the task sleeps on a GPIO event and wakes only when the
/// line moves, then debounces by acting on the level once it has settled.
pub async fn run_wired_detect(mut input: Input<'_>, active_low: bool) {
    let mut current = input.is_high() != active_low;
    crate::split::selector::update(current);

    loop {
        input.wait_for_any_edge().await;
        Timer::after_millis(10).await;
        let settled = input.is_high() != active_low;
        if settled != current {
            current = settled;
            crate::split::selector::update(current);
        }
    }
}
