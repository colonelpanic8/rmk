# RMK split-app channel transport semantics (verified 2026-08-06 against rmk @ assembled 59fd18e1)

Produced by a code-audit agent; every claim carries file:line evidence. Paths relative to
/home/imalison/Projects/glove80-config/dependencies/glove80-rmk/dependencies/rmk unless noted;
consumers in /home/imalison/Projects/glove80-config/dependencies/glove80-rmk/crates/glove80-rmk/src/.
Feature set: defmt, storage, rynk, watchdog, nrf52840_ble, split, async_matrix, lighting, adafruit_bl (dfu_split and display OFF).

## Statics (rmk/src/split_app.rs)
- SPLIT_APP_TX: Channel<RawMutex, SplitAppData, 112> (line 85). Central→peripheral app traffic.
- SPLIT_APP_RX: Channel<..., 8> (line 89). Inbox on BOTH halves, filled by split read loops via deliver_received (98-102): try_send, drop-on-full with only a warn!. Never backpressures BLE.
- SPLIT_APP_PERIPH_TX: Channel<..., 2> (line 93). Peripheral→central (acks). Drops warn via defmt (lighting.rs:626,653); recovery is the central's 500 ms ack timeout.
- SPLIT_APP_LINK: Watch<RawMutex, bool, 2> (line 107). Max 2 receivers; each half's app task takes one with .expect().
- SPLIT_APP_MSG_MAX = 26; SplitMessage::Application is the last postcard variant; SPLIT_MESSAGE_MAX_SIZE = 32 (GATT characteristic ceiling). Compile-time asserts keep every lighting packet <= 26; BEGIN/CELL/CONDITIONAL_SCENE_CELL are exactly 26.

## Send/receive semantics
- try_queue_snapshot aborts the whole snapshot on any full-queue try_send (packets already enqueued stay as orphaned residue); central retries after 50 ms sleep.
- PeripheralManager::run (central) DEQUEUES from SPLIT_APP_TX then writes (split/driver.rs:236-237, 155-165): any non-Disconnected write error (serialize, packet-pool OutOfMemory) silently discards the already-dequeued message; disconnect cancels mid-send losing the in-flight message. Peripheral side same shape, error discarded with .ok() (split/peripheral.rs:143-146).
- Application arms are the LOWEST-priority arm of select_biased in both split loops — sustained key/layer traffic can starve app traffic.
- NOTHING drains or flushes SPLIT_APP_TX while the link is down; residue survives reconnects (SPLIT_APP_RX by contrast is flushed on link edge by the peripheral, lighting.rs:677).
- Peripheral app task awaits CORE_MAILBOX.request(ApplyReplica) inline (lighting.rs:642-645); while it does, incoming packets past 8 are dropped by deliver_received. A full snapshot burst is ~65-76 packets against an 8-deep inbox — the natural place snapshots die (self-healing only via central's 500 ms timeout + resend).

## BLE transport
- Central→peripheral: GATT Write Without Response; peripheral→central: GATT Notification. Both bottom out in a per-connection FIFO (l2cap-tx-queue-16) with real backpressure; the RADIO never drops or reorders (link-layer acked, ordered).
- Loss vectors (host layer): (1) notify before CCCD subscription silently discarded AND reported Ok (trouble attribute.rs:1032-1035) — why the peripheral only marks link up on first INBOUND message; (2) central's client notification queue (depth 8, gatt-client-notification-queue-size-8) EVICTS OLDEST on overflow via publish_immediate, and NotificationListener::next silently discards WaitResult::Lagged — peripheral→central loss with ZERO logging (trouble gatt.rs:1926, 1008-1016; embassy-sync pubsub publish_immediate); (3) packet-pool exhaustion (16 x 251B) surfaces as the silently-dropped write error above; (4) SPLIT_APP_RX overflow at destination; (5) connection loss cancelling mid-send.
- No transport-level retry/ack for Application messages (dfu_split's chunk ack protocol is compiled out). The only recovery primitive is the SPLIT_APP_LINK false→true edge; the glove80 lighting layer's generation/revision/Ack/500ms-timeout machinery is the end-to-end reliability.

## SPLIT_APP_LINK (embassy_sync Watch) semantics
- .changed() fires on EVERY send including repeated same-value writes (send bumps current_id unconditionally; no equality check). Repeated false→false is reachable (central connects but never writes, then drops).
- Single-slot state: a down→up cycle completing between two polls COLLAPSES — the waiter wakes once and reads true; the false is never observed. Realistic when the central app task is blocked on CORE_MAILBOX.request or its 50 ms sleep. Benign for current resync logic (both edges lead to the same resync), fatal for any future edge-counting.
- A fresh receiver gets the current value on first .changed() (after any send has happened); before the first send it blocks. Cancel-safe as a select arm.
- Central's LinkGuard marks up immediately on session start (split/driver.rs:192-193); peripheral marks up only on first inbound message (split/peripheral.rs:133).

## Ranked failure modes
1. SPLIT_APP_RX depth 8 dropping snapshot-burst packets while the peripheral awaits ApplyReplica.
2. SPLIT_APP_TX never flushed on link edges; orphaned partial-snapshot residue persists across reconnects, eating the 112 slots.
3. trouble notification queue evicting oldest (peripheral→central acks/messages) with Lagged silently discarded — zero logging.
4. Dequeue-then-write loses messages on write error or cancellation.
5. Watch collapse of down→up transitions (benign today, a trap for new logic).
6. SPLIT_APP_PERIPH_TX depth 2 — acks are the first casualty under any burst.
