use core::cell::{Cell, RefCell};
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec;
use std::vec::Vec;

use embassy_futures::{select::select, yield_now};
use embassy_time::{Duration, MockDriver};
use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use embedded_storage_async::nor_flash::{NorFlash, NorFlashErrorKind, ReadNorFlash};
use rmk_types::action::{Action, KeyAction};
use rmk_types::keycode::{HidKeyCode, KeyCode};
use rmk_types::protocol::rynk::Deframer;
use rmk_types::protocol::rynk::{Cmd, RYNK_HEADER_SIZE, RynkError, RynkHeader, encode_frame};

use super::*;
use crate::config::{BehaviorConfig, PositionalConfig, RmkConfig};
use crate::core_traits::Runnable;
use crate::host::rynk::RynkService;
use crate::keymap::{KeyMap, KeymapData};

#[derive(Default)]
struct Metrics {
    reads: Cell<usize>,
    max_reads: Cell<usize>,
    polls: Cell<usize>,
    erases: Cell<usize>,
}

#[derive(Clone)]
struct Flash {
    bytes: Rc<RefCell<Vec<u8>>>,
    metrics: Rc<Metrics>,
}

impl embedded_storage_async::nor_flash::ErrorType for Flash {
    type Error = NorFlashErrorKind;
}

impl ReadNorFlash for Flash {
    const READ_SIZE: usize = 1;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.metrics.reads.set(self.metrics.reads.get() + 1);
        bytes.copy_from_slice(&self.bytes.borrow()[offset as usize..offset as usize + bytes.len()]);
        Ok(())
    }

    fn capacity(&self) -> usize {
        self.bytes.borrow().len()
    }
}

impl NorFlash for Flash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = 4096;

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        yield_now().await;
        assert_eq!(offset as usize % Self::WRITE_SIZE, 0);
        assert_eq!(bytes.len() % Self::WRITE_SIZE, 0);
        for (to, from) in self.bytes.borrow_mut()[offset as usize..offset as usize + bytes.len()]
            .iter_mut()
            .zip(bytes)
        {
            assert_eq!(*to & from, *from, "NOR writes cannot set cleared bits");
            *to &= from;
        }
        Ok(())
    }

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        yield_now().await;
        assert_eq!(from as usize % Self::ERASE_SIZE, 0);
        assert_eq!(to as usize % Self::ERASE_SIZE, 0);
        self.bytes.borrow_mut()[from as usize..to as usize].fill(0xff);
        self.metrics.erases.set(self.metrics.erases.get() + 1);
        Ok(())
    }
}

fn drive<F: Future>(future: F, metrics: &Metrics) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..1_000_000 {
        metrics.reads.set(0);
        let result = future.as_mut().poll(&mut cx);
        metrics.max_reads.set(metrics.max_reads.get().max(metrics.reads.get()));
        metrics.polls.set(metrics.polls.get() + 1);
        if let Poll::Ready(value) = result {
            return value;
        }
        MockDriver::get().advance(Duration::from_micros(100));
    }
    panic!("runtime apply did not finish");
}

struct Input(VecDeque<Vec<u8>>);

impl ErrorType for Input {
    type Error = ErrorKind;
}

impl Read for Input {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        let Some(frame) = self.0.front_mut() else {
            return Ok(0);
        };
        // Exercise fragmented COBS requests.
        let count = frame.len().min(bytes.len()).min(23);
        bytes[..count].copy_from_slice(&frame[..count]);
        frame.drain(..count);
        if frame.is_empty() {
            self.0.pop_front();
        }
        Ok(count)
    }
}

#[derive(Default)]
struct Output(Vec<u8>);

impl ErrorType for Output {
    type Error = ErrorKind;
}

impl Write for Output {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn bulk_apply_and_garbage_collection_yield_and_preserve_actions() {
    MockDriver::get().reset();
    FLASH_CHANNEL.clear();
    let metrics = Rc::new(Metrics::default());
    let flash = Flash {
        bytes: Rc::new(RefCell::new(vec![0xff; 32768])),
        metrics: metrics.clone(),
    };
    let config = StorageConfig {
        num_sectors: 8,
        ..StorageConfig::default()
    };
    let keys = [[[KeyAction::No; 14]; 6]; 16];
    let mut behavior = BehaviorConfig::default();
    let mut storage = drive(
        Storage::<_, 6, 14, 16>::new(flash.clone(), &keys, &None, &config, &behavior),
        &metrics,
    );
    let erases_before = metrics.erases.get();
    metrics.max_reads.set(0);

    let positional = PositionalConfig::<6, 14>::default();
    let mut data = KeymapData::<6, 14, 16, 0>::new(keys);
    let keymap = drive(KeyMap::new(&mut data, &mut behavior, &positional), &metrics);
    let mut rmk_config = RmkConfig::default();
    rmk_config.lock_config.insecure = true;
    let service = RynkService::new(&keymap, &rmk_config);
    let mut last = Vec::new();

    for pass in 0..4 {
        let mut input = Input(VecDeque::new());
        last = (0..84)
            .map(|i| match i % 3 {
                0 => KeyAction::Single(Action::Key(KeyCode::Hid(HidKeyCode::A))),
                1 => KeyAction::TapHold(Action::Key(KeyCode::Hid(HidKeyCode::Escape)), Action::LayerOn(2), pass),
                _ => KeyAction::Morse(pass),
            })
            .collect();
        // Repeated replacements create obsolete entries and force real GC.
        for layer in 0..7u8 {
            for (page, actions) in last.chunks(28).enumerate() {
                let start = page * 28;
                let request = ([layer, (start / 14) as u8, (start % 14) as u8], actions);
                let mut bytes = [0; 512];
                let len = encode_frame(
                    &mut bytes,
                    RynkHeader {
                        cmd: Cmd::SetKeymapBulk,
                        seq: input.0.len() as u8,
                    },
                    &request,
                )
                .unwrap();
                input.0.push_back(bytes[..len].to_vec());
            }
        }
        let mut output = Output::default();
        drive(
            select(
                async {
                    service.run_session(&mut input, &mut output).await;
                    // This response is an ordering barrier after the queued writes.
                    read_connection_type().await;
                },
                storage.run(),
            ),
            &metrics,
        );
        let mut df = Deframer::new();
        df.commit(output.0.len());
        let mut replies = 0;
        while let Some(len) = df.next(&mut output.0) {
            assert_eq!(
                postcard::from_bytes::<Result<(), RynkError>>(&output.0[RYNK_HEADER_SIZE..len]).unwrap(),
                Ok(())
            );
            replies += 1;
        }
        assert_eq!(replies, 21);
    }
    assert!(metrics.erases.get() > erases_before, "must exercise garbage collection");
    // Reopen the same bytes as after reset; no cached map or live keymap survives.
    let mut reopened = drive(
        Storage::<_, 6, 14, 16>::new(flash, &keys, &None, &config, &BehaviorConfig::default()),
        &metrics,
    );
    for layer in 0..7u8 {
        for (i, expected) in last.iter().enumerate() {
            let got = drive(
                reopened.fetch_data(StorageKey::keymap(layer, (i / 14) as u8, (i % 14) as u8)),
                &metrics,
            );
            let Some(StorageData::KeyAction(got)) = got else {
                panic!("key missing after reopening")
            };
            // KeyAction::PartialEq deliberately ignores the tap-hold profile.
            let mut actual_bytes = [0; 32];
            let mut expected_bytes = [0; 32];
            assert_eq!(
                postcard::to_slice(&got, &mut actual_bytes).unwrap(),
                postcard::to_slice(expected, &mut expected_bytes).unwrap()
            );
        }
    }
    std::println!(
        "max ready reads per poll: {}; polls: {}; erases: {}",
        metrics.max_reads.get(),
        metrics.polls.get(),
        metrics.erases.get()
    );
    assert!(
        metrics.max_reads.get() <= 32,
        "flash scans must leave time for watchdog and transport tasks"
    );
}

#[test]
fn simultaneous_metadata_reads_keep_their_own_replies() {
    let mut context = Context::from_waker(Waker::noop());
    let mut first = Box::pin(read_layer_metadata(0));
    let mut second = Box::pin(read_layer_metadata(1));
    assert!(first.as_mut().poll(&mut context).is_pending());
    assert!(second.as_mut().poll(&mut context).is_pending());
    assert!(matches!(
        FLASH_CHANNEL.try_receive().unwrap(),
        FlashOperationMessage::ReadLayerMetadata(0)
    ));
    assert!(FLASH_CHANNEL.try_receive().is_err());
    LAYER_METADATA_RESPONSE.signal(None);
    assert_eq!(first.as_mut().poll(&mut context), Poll::Ready(None));
    assert!(second.as_mut().poll(&mut context).is_pending());
    assert!(matches!(
        FLASH_CHANNEL.try_receive().unwrap(),
        FlashOperationMessage::ReadLayerMetadata(1)
    ));
    LAYER_METADATA_RESPONSE.signal(Some(LayerMetadata::vacant()));
    assert_eq!(
        second.as_mut().poll(&mut context),
        Poll::Ready(Some(LayerMetadata::vacant()))
    );
}

#[test]
fn cancelled_metadata_read_cannot_supply_the_next_reply() {
    let mut context = Context::from_waker(Waker::noop());
    let mut first = Box::pin(read_layer_metadata(0));
    assert!(first.as_mut().poll(&mut context).is_pending());
    assert!(matches!(
        FLASH_CHANNEL.try_receive().unwrap(),
        FlashOperationMessage::ReadLayerMetadata(0)
    ));
    drop(first);
    let mut second = Box::pin(read_layer_metadata(1));
    assert!(second.as_mut().poll(&mut context).is_pending());
    assert!(FLASH_CHANNEL.try_receive().is_err());
    LAYER_METADATA_RESPONSE.signal(None);
    assert!(second.as_mut().poll(&mut context).is_pending());
    assert!(matches!(
        FLASH_CHANNEL.try_receive().unwrap(),
        FlashOperationMessage::ReadLayerMetadata(1)
    ));
    LAYER_METADATA_RESPONSE.signal(Some(LayerMetadata::vacant()));
    assert_eq!(
        second.as_mut().poll(&mut context),
        Poll::Ready(Some(LayerMetadata::vacant()))
    );
}
