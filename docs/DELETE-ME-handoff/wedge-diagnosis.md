## Deliverable: peripheral lighting-context wedge analysis

Paths below are relative to `/home/imalison/Projects/glove80-config`. Two prefixes recur:

* **G/** = `dependencies/glove80-rmk/crates/glove80-rmk/src/`
* **R/** = `dependencies/glove80-rmk/dependencies/rmk/rmk/src/`

### Ground fact that constrains every hypothesis

`PeripheralState::set(...)` — the *only* writer of `PERIPHERAL_CONTEXT`, and therefore of the `layers.active_bits` that gate the gauge — is reached from exactly two places:

* **G/lighting.rs:636**, after `SnapshotStage::apply` returns a *committed* snapshot (i.e. the whole 64-packet transaction landed and `Commit` validated), and
* **G/lighting.rs:619**, in the `ContextUpdate` fast path, and only `if self.last_applied_revision == Some(revision)` (G/lighting.rs:617).

Note that G/lighting.rs:636 runs **before** `REPLICA_SLOT.put` and **before** `ApplyReplica` (G/lighting.rs:638-645). So *any* hypothesis in which a complete snapshot reaches the peripheral but the engine rejects it **cannot** produce the reported symptom — the context would still be refreshed. The wedge must be upstream: **no complete snapshot and no accepted `ContextUpdate` ever arrives.** That single observation eliminates the whole `ApplyReplica`-error family as a primary cause (enumerated in §6 anyway, as requested) and points at the queueing layer.

Also relevant: the peripheral *does* learn the live layer through a separate, higher-priority path — `SplitMessage::Layer` → `publish_event(LayerChangeEvent)` (R/split/peripheral.rs:190), which is arm 2 of the central's `select_biased` (R/split/driver.rs:221) and therefore keeps flowing even when the application queue is jammed. The lighting context deliberately ignores it (`PeripheralState::snapshot`, G/lighting.rs:511-523, reads only the replicated cell). This is why typing and the split link "keep working" while the gauge is frozen — and it is a very useful discriminator on hardware.

---

## W1 — `try_queue_snapshot` partial-fill + 50 ms `continue` retry = permanent SPLIT_APP_TX livelock

**CONFIRMED-BY-CODE** (structure); the throughput threshold is arithmetic I derive below and mark separately.

### The code

`try_queue_snapshot` (G/split_lighting.rs:905-1033) is **not atomic and does not pre-check free space**. It `try_send`s packets one at a time through the closure at G/split_lighting.rs:935-939 and bails out on the first failure (G/split_lighting.rs:964, 978, 991, 1001, 1013, 1024). Everything already pushed stays in `SPLIT_APP_TX` as orphaned residue. **At the moment of any such abort, `SPLIT_APP_TX` is by construction exactly full (112/112).**

The caller's failure path is the fatal part (G/central_lighting.rs:234-251):

```rust
if link_up && awaiting_ack.is_none() && (full_dirty || context_dirty) {
    let pending = if full_dirty { self.try_send_snapshot().await }
                  else { self.try_send_context_update(last_acked_revision).await };
    match pending {
        Some(pending) => { awaiting_ack = Some(pending); full_dirty = false; context_dirty = false; }
        None => { embassy_time::Timer::after_millis(50).await; continue; }   // <-- G/central_lighting.rs:246-249
    }
}
```

`continue` re-enters the top of `loop`, re-evaluates the same `if` (still true — neither flag was cleared, `awaiting_ack` was never set), and retries. **The `select4` at G/central_lighting.rs:260-268 is never reached.**

### Exact event sequence entering the wedge

Snapshot size for this config is **64 packets**, which I counted from the live rule set rather than assuming: `config/glove80.toml` yields 44 scene cells and 14 conditional cells with `led ≥ 40`, none carrying a connection/effects predicate, so the burst is `Begin + Context + Extension + ExtensionOverlay (4) + 0 overlay + 44 SceneCell + ConditionalSceneBegin (1) + 14 ConditionalSceneCell + 0 Ext + Commit (1) = 64`. (Persisted Rynk tables can push this toward the 76 you quoted; the argument is unchanged.)

Let `F` = free slots at an attempt, `N` ≈ 64, `D` = slots drained by `PeripheralManager` during one retry cycle `T` ≈ 50 ms + `ExportReplica` round-trip.

* If `F ≥ N` → success.
* If `F < N` → queue `F` packets, abort, **queue is full**, `F ← 0`.
* After `T`, `F = D`. Retry. If `D < N`, abort again, `F ← 0`. **Fixed point.**

Two realistic entries, both confirmed by code:

**(E1) Effect-hit backlog while the link is down.** `try_queue_effect_hit` (G/split_lighting.rs:896-901) is a bare `SPLIT_APP_TX.try_send` with **no link-state gate**, called for every left-half key press from `ReactiveKeyHits::central()` (`mirror_left_hits: true`, G/lighting.rs:435, call at G/lighting.rs:473). Nothing drains `SPLIT_APP_TX` while no peripheral session exists — its sole consumer is R/split/driver.rs:237, inside `PeripheralManager::run`, and nothing anywhere flushes it (verified: the only references to `SPLIT_APP_TX` in the entire tree are G/central_lighting.rs:195,325, G/split_lighting.rs:898,936, R/split/driver.rs:237). ~112 left-half keystrokes with the right half disconnected fills it. On reconnect, `full_dirty = true` (G/central_lighting.rs:275) → first attempt sees `F ≈ 0` → livelock.

**(E2) Sleep, or any interval where round-trip ack latency exceeds 500 ms.** Awake, the split link is 7.5 ms interval with `max_latency` 0 (powered) / 1 (battery) — R/split/ble/central.rs:378-392 and `keyboard.toml:29-30`. Asleep it becomes 20 ms / latency 200 (4 s effective) or 200 ms / latency 25 (5 s) — R/split/ble/central.rs:401-419. In that regime the central still runs its 500 ms ack timeout, so it enqueues a **fresh 64-packet snapshot every 500 ms** (`Either4::Fourth(Either::Second(())) => { awaiting_ack = None; full_dirty = link_up; }`, G/central_lighting.rs:305-308) while the previous one has moved a handful of packets. The queue saturates within ~1 s and the livelock is established. It then **survives waking**, because nothing ever drains the residue faster than the 50 ms refill.

Generalised entry criterion: **any sustained period where the central→peripheral application throughput falls below `N / 500 ms` ≈ 128 packets/s while an ack is outstanding causes `SPLIT_APP_TX` to grow monotonically to full.**

### Why it never self-heals — four independent reasons

1. **Escape requires `D ≥ 64` in a single ~50 ms window**, i.e. a sustained ≥1280 packets/s. On a 7.5 ms interval with 2M PHY and 33-byte ATT payloads, and with `PeripheralManager::run` (R/split/driver.rs:246-270) dispatching **one** application message per loop iteration behind an `await`ed GATT write, realistic throughput is a few hundred packets/s. *(This throughput figure is the PLAUSIBLE part; the structural fixed point is CONFIRMED.)*
2. **The self-healing loop is bypassed entirely.** `continue` never reaches `select4`, so `link.changed()` is never polled. A full disconnect/reconnect cycle does **not** reset `full_dirty`, `awaiting_ack`, or anything else — the central keeps calling `try_queue_snapshot` into a dead queue.
3. **`SPLIT_APP_TX` is never flushed on a link edge** (contrast the peripheral's `SPLIT_APP_RX` flush at G/lighting.rs:677). Residue survives arbitrarily many reconnects.
4. **The peripheral only ever sees prefixes.** Because the abort is always at the tail, the FIFO carries `Begin, Context, Extension, ExtensionOverlay, SceneCell×k` … then the next cycle's `Begin` (which cleanly resets `SnapshotStage`, G/split_lighting.rs:1074-1101). `Commit` is never delivered → `SnapshotStage::apply` never returns `Some` → **G/lighting.rs:636 never executes** → `PERIPHERAL_CONTEXT` is frozen at whatever it held when the wedge began. Both reported polarities follow directly: frozen with bit 2 set = gauge permanently lit; frozen without = gauge never appears.

Only a central reset clears it. That matches "minutes/hours".

### Hardware signature (decisive)

* On the central, `debug!("Sending message to peripheral {}: {:?}", ...)` (R/split/driver.rs:156) shows a **continuous, never-ending stream of `Application` payloads whose tag byte sequence is `1,2,7,10,6,6,6,…` and never `4` (`TAG_COMMIT`)**, repeating a `1` (`TAG_BEGIN`) roughly every 50 ms.
* **Total defmt silence on the peripheral**: no `"lighting: peripheral rejected replica"`, no `"lighting: peripheral replica slot busy"`, no ack-queue warnings (G/lighting.rs:639,654,657) — because the commit path is never reached. Silence *plus* a busy link is the fingerprint.
* **Left-half key hits stop mirroring to the right half's reactive effect**, because `try_queue_effect_hit` (G/split_lighting.rs:896-901) fails on a permanently full queue. This is visible without a probe and is the single best field test.
* Elevated current draw / degraded key latency on both halves from the constant ~128-600 packets/s of junk.
* If you can read RAM: `SPLIT_APP_TX` len pinned at 112; `CentralReplication.generation` advancing ~20/s (one per 50 ms retry, G/central_lighting.rs:165), wrapping every ~12.8 s.

---

## W2 — Same silent retry loop entered through the overlay guard

**CONFIRMED-BY-CODE** (mechanism); reachability depends on host behaviour.

G/split_lighting.rs:910-918:

```rust
let cell_count = snapshot.overlay.as_slice().iter()
    .filter(|cell| cell.slot.index() >= LEDS_PER_HALF).count();
if cell_count > LEDS_PER_HALF { return false; }
```

`LEDS_PER_HALF = 40` (G/central.rs:4) but `OVERLAY_CAPACITY = 64` (G/lighting.rs:46), and the overlay is host-writable through Rynk (`SetOverlay` / `ReplaceOverlay`, R/lighting/standard/command.rs:58-76). **41+ live right-half overlay cells makes `try_queue_snapshot` return `false` unconditionally, on every call, forever.** The central then spins the G/central_lighting.rs:246-249 loop indefinitely — same non-healing properties as W1 (never reaches `select4`, ignores link edges), but *completely silent and with zero radio traffic*, because it returns before queueing anything.

* **Sequence:** a host (glove80-control / layout editor) writes >40 right-half overlay cells with no TTL → next `LightingChangedEvent` or reconnect sets `full_dirty` → permanent spin.
* **Never heals** because overlay cells without a TTL never expire (`replica_state`, R/lighting/standard/engine.rs:317-327, only drops cells whose `expires_ms` has passed).
* **Signature:** zero `Application` traffic to the peripheral at all (contrast W1's flood), context frozen, `CentralReplication.generation` still advancing ~20/s. Read the overlay length over Rynk: `StandardState.overlay_len` with >40 slots ≥ 40.

The sibling guards `scene_count > SCENE_CAPACITY` and `conditional_scene_count > SCENE_CAPACITY` (G/split_lighting.rs:924, 932) are **unreachable**: `SCENE_CAPACITY = 100` bounds the whole table, so the right-half subset can never exceed it.

---

## W3 — The 500 ms ack timeout is a per-iteration timer, not a deadline

**CONFIRMED-BY-CODE** (defect); **PLAUSIBLE** as an hours-long wedge on its own.

G/central_lighting.rs:253-268 constructs the timeout **fresh inside the loop body**:

```rust
let timeout = async { if awaiting_ack.is_some() { Timer::after_millis(500).await } else { pending().await } };
match select4(link.changed(), lighting.next_event(), select(select(layers…, indicators…), select(battery…, peripheral_battery…)), select(SPLIT_APP_RX.receive(), timeout)).await
```

Every completion on arms 1-3, and every message on `SPLIT_APP_RX`, restarts the 500 ms from zero. While `awaiting_ack.is_some()`, the send block at G/central_lighting.rs:234 is skipped entirely, so `context_dirty` accumulates but nothing is transmitted.

* **Sequence:** press Magic → `LayerChangeEvent` → `context_dirty` → `ContextUpdate{G,R}` sent → `awaiting_ack = Some`. The ack is lost (three confirmed loss vectors: `SPLIT_APP_PERIPH_TX` depth 2 with drop-warn at G/lighting.rs:626-628/653-655; the trouble client notification queue evicting oldest with `Lagged` silently discarded, per your transport report §BLE loss vectors; `SPLIT_APP_RX` overflow at R/split_app.rs:98-102). Release Magic → another `LayerChangeEvent` → sets `context_dirty` **and restarts the timer**. As long as layer keys keep firing at <500 ms intervals — `update_tri_layer` publishes **unconditionally** on every activate/deactivate (R/keymap.rs:315,327,339,479,494), so a held `LT(1, …)` or `MO(2)` produces two events per press — nothing is ever retransmitted and the gauge stays frozen in whatever polarity the last accepted context had.
* **Why it doesn't reach hours alone:** ordinary typing without layer keys produces no `LayerChangeEvent`, so the timeout fires during any ≥500 ms lull. Rank it as a **staleness amplifier and a W1 feeder** (each timeout it *does* eventually take re-queues a 64-packet snapshot) rather than the terminal wedge.
* **Signature:** `awaiting_ack` pinned `Some` across many seconds; the gauge un-freezes the moment you stop touching layer keys for half a second. If it heals on a pause, it's W3; if it doesn't, it's W1/W2.

---

## W4 — The peripheral's link-edge RX flush destroys the first snapshot after every reconnect

**CONFIRMED-BY-CODE**; self-healing alone, but it is a systematic queue-pressure source feeding W1.

G/lighting.rs:667-681:

```rust
match select(link.changed(), SPLIT_APP_RX.receive()).await {
    First(_) => { self.stage.reset(); self.last_applied_revision = None;
                  while SPLIT_APP_RX.try_receive().is_ok() {} }
    Second(message) => self.process(message).await,
}
```

`embassy_futures::select` polls the first future first, so when both are ready the flush wins. The peripheral marks its link up only on the **first inbound message** (R/split/peripheral.rs:133), and the central starts a session by marking up (R/split/driver.rs:192-193) and then immediately writing `ConnectionStatus` (R/split/driver.rs:197-205) — while `CentralReplication` is concurrently pushing the reconnect snapshot. The peripheral's app task therefore wakes on the link edge with snapshot packets already in `SPLIT_APP_RX` and **drains them into the bin**. `last_applied_revision = None` also guarantees the next `ContextUpdate` is rejected at G/lighting.rs:617 with no ack.

Cost: **≥1 wasted 64-packet snapshot plus a 500 ms timeout on every reconnect.** Harmless on a healthy link; on a flapping link it is exactly the input W1 needs.

**Signature:** on every reconnect, a `Begin`…`Commit` burst followed by ~500 ms of silence and then a second identical burst that is the one actually acked.

---

## W5 — `SPLIT_APP_RX` depth 8 dropping burst packets while `ApplyReplica` is awaited

**PLAUSIBLE**, transient, not a standalone wedge.

`deliver_received` is drop-on-full with only a `warn!` (R/split_app.rs:98-102). The peripheral blocks inline on `CORE_MAILBOX.request(ApplyReplica)` (G/lighting.rs:642-645), which cannot complete until `LightingProcessor::drive_until_wait` returns — including a WS2812 SPI write of `40 × 24 + 48 = 1008` bytes at 4 MHz ≈ 2 ms (G/lighting.rs:167,186-207).

Working the timing as you asked: **the `ApplyReplica` window sits at the *tail* of a burst, after `Commit`**, and the central will not send another snapshot until it gets the ack or times out (G/central_lighting.rs:234). So the blocked window does not overlap the burst it belongs to. At a few hundred packets/s, a ~2-10 ms stall admits ~1-5 packets against an 8-deep inbox. **No, every retry cannot lose packets indefinitely under realistic timing** — this is a per-attempt probabilistic loss the 500 ms resend covers, not a livelock. Its real contribution is raising the reconnect/timeout rate that feeds W1.

**Signature:** sporadic `"split app message dropped (inbox full)"` on the peripheral, paired with an extra snapshot round.

---

## §6 — `ApplyReplica` error conditions, enumerated against this config

Requested explicitly. `StandardCommand::ApplyReplica` is R/lighting/standard/engine.rs:1034-1087. Every `?` in it:

| # | Error | Line | Verdict for this build |
|---|---|---|---|
| 1 | `slot.take()?` → `ReplicaSlotError::Empty` | 1035 | **Unreachable.** `REPLICA_SLOT.put` immediately precedes (G/lighting.rs:638); `LightingMailbox::request` serialises callers behind a `caller` mutex (R/lighting/processor.rs:61) and the peripheral has exactly one requester. |
| 2 | `replica_overlay` → `DeadlineOverflow` | 1036 / 371 | **Unreachable.** Requires `sample_time_ms + ttl` to overflow `u64`. |
| 3 | `replica_overlay` → `overlay.replace(...)` error | 1036 / 373 | **Unreachable.** ≤40 right-half cells vs `OVERLAY_CAP = 64`; `SnapshotStage` already rejects duplicate slots (G/split_lighting.rs:1160-1166) and over-count (1159). |
| 4 | `check_extension_param` (main) | 1043 | **Unreachable.** Both halves compile the identical `PaletteFxSource<…, REACTIVE_HITS=16>` (G/lighting.rs:77-84, 388-401) — same descriptor, same ranges. |
| 5 | `check_extension_param` (overlay) | 1048 | Same. |
| 6 | `ExtensionUnsupported` on `(replica.extension, self.extension.extension_state())` | 1051-1055 | **Unreachable.** Both sides are `PaletteFxSource`, so both yield `Some`, and `apply_extension_state` validates against the identical effect/palette lists. |
| 7 | `ExtensionUnsupported` on `extension_layers` | 1056-1060 | **Unreachable *for matched firmware*.** Note the shape is asymmetric: `SnapshotStage` sets `extension_layers = Some(...)` unconditionally on the `ExtensionOverlay` packet (G/split_lighting.rs:1146), so a peripheral whose source returned `None` here would fail *forever*. With identical builds both return `Some`. **This is the one that would bite on mismatched halves.** |
| 8 | `apply_extension_param_checked` (main / overlay) | 1067-1083 | **Unreachable**, same reasoning as 4/5. |

**Answer to "can any fail deterministically forever": no, not for this configuration and matched firmware.** And critically — per the ground fact at the top — even if one did, it could **not** produce your symptom, because `PeripheralState::set` at G/lighting.rs:636 already ran. What such a failure *would* produce is a distinct, loud signature: `"lighting: peripheral rejected replica"` (G/lighting.rs:657) repeating at exactly 2 Hz forever, with a fresh 64-packet snapshot every 500 ms and a **working** gauge.

---

## §7 — Candidates I examined and ruled out

* **Generation wraparound (`u8`, G/central_lighting.rs:165,186).** The central burns one generation per 50 ms retry, wrapping every ~12.8 s — but corruption needs residue *256 generations old* to collide with the live stage, and a 112-deep queue holds at most ~4-7 generations' worth. Additionally the FIFO delivers strictly in order, so each `Begin` cleanly resets `SnapshotStage` (G/split_lighting.rs:1074-1101) and residue from an older generation only ever *fails to match*, never corrupts. **Not a mechanism here.**
* **`ContextUpdate`-vs-`FullSnapshot` ack-kind handling** (G/central_lighting.rs:291-303). Correct: `last_acked_revision` is updated only on a `FullSnapshot` ack (300-302), which is exactly the invariant the peripheral's `last_applied_revision` guard (G/lighting.rs:617) needs. Divergence in either direction resolves via the timeout → `full_dirty` → full resend. **No hole.**
* **Layer-release event lost so `context_dirty` is never set.** `LayerChangeEvent` is a bounded pub/sub (`channel_size = 4`, `keyboard.toml:169-172`) whose subscriber skips `Lagged` and delivers the *newest* message; and `update_tri_layer` publishes unconditionally on every activate/deactivate (R/keymap.rs:303,315,327,339). The final state is always delivered. **Ruled out.**
* **`select4` starvation of the timeout/ack arm.** `select4` polls in order and arm 3 consumes exactly one event per iteration with no intervening `await`, so it can only stay ready while events are genuinely backlogged — bounded by arrival rate, not a fixed point. (The *timer restart* in W3 is the real defect here, not priority starvation.)
* **`REPLICA_SLOT` contention.** All runnables are composed with `embassy_futures::join::join`, never `select` (`dependencies/glove80-rmk/dependencies/rmk/rmk-macro/src/codegen/entry.rs:308-327`, applied at `codegen/split/peripheral.rs:534`), so no `request(ApplyReplica)` can be cancelled between `put` and `take`. `Busy` is unreachable. The `"peripheral replica slot busy"` warn (G/lighting.rs:639) should never appear; if it ever does, that is a separate permanent wedge and worth logging loudly.
* **Peripheral app task permanently stuck on `CORE_MAILBOX.request`.** `drive_until_wait` always terminates in a `Wait` — output failures return a retry deadline via `HalfOutput::retry_after` = 50 ms (G/lighting.rs:308-314) and engine errors return `now + 10` (R/lighting/processor.rs:190-197). The mailbox is always eventually serviced.
* **`set_mutable_state` output_mode clobber** — already ruled out by you; confirmed restored on the next line (R/lighting/standard/engine.rs:293-296, `self.output_mode = replica.output_mode` at 1085→ via `apply_replica` line 383).

---

## §8 — Recommended discrimination procedure on hardware

One observation separates all three live candidates, no probe required:

1. **Do left-half keypresses still show up in the right half's reactive effect while wedged?**
   * **No** → W1 (`SPLIT_APP_TX` full; `try_queue_effect_hit` failing).
   * **Yes** → not W1. Go to 2.
2. **Does the gauge un-freeze within ~1 s of not touching any layer key?**
   * **Yes** → W3.
   * **No** → W2 (check `overlay_len` and the right-half slot distribution over Rynk).

With defmt attached to the central, W1 vs W2 is immediate: W1 floods `Sending message to peripheral 0: Application(..)` and never emits tag `4`; W2 emits nothing at all.

---

# Secondary: why you have never seen the charging colour

**`ChargeState::Charging` is unreachable in this build — twice over, independently.**

### 1. No charger-detect input is configured

`ChargeState::Charging` is produced in exactly one place: `BatteryProcessor::on_charging_state_event` (R/input_device/battery.rs:195-217), which fires only on a `ChargingStateEvent`. The only producer of `ChargingStateEvent` is `ChargingStateReader` (R/input_device/battery.rs:29-92), which needs a GPIO. That GPIO comes from `[ble].charge_state` (`rmk-config/src/lib.rs:948`, `pub charge_state: Option<PinConfig>`), consumed by the macro at `rmk-macro/src/codegen/chip/ble.rs:31-46`.

The Glove80 config's entire `[ble]` section is:

```toml
[ble]
enabled = true
```

(`dependencies/glove80-rmk/crates/glove80-rmk/keyboard.toml:214-215`) — no `charge_state`, no `charge_led`. So the macro takes the `else` branch and emits `let is_charging_pin: Option<Input<'_>> = None;` (`chip/ble.rs:46`). Both halves declare only `battery_adc_pin = "vddh"` (keyboard.toml:239, 254).

### 2. `ChargingStateReader` is never instantiated *anywhere*, pin or no pin

A tree-wide grep over `rmk/src`, `rmk-macro/src` and `crates/` finds `ChargingStateReader` **only at its own definition site** (R/input_device/battery.rs:33-92). `BleBatteryConfig::charge_state_pin` (R/config/ble_battery.rs:8) is constructed and stored and **never read by anything**. So `ChargingStateEvent` has **zero publishers in this firmware**, and adding `charge_state = { pin = "…" }` to `keyboard.toml` would not fix it — the reader would still never be spawned.

### Consequent steady state on both halves

`BatteryProcessor::on_battery_adc_event` (R/input_device/battery.rs:163-192) transitions `Unavailable → Available { charge_state: ChargeState::Unknown, level }` on the first ADC reading (lines 183-190), and on every subsequent reading **preserves the existing `charge_state`** (lines 175-182). With no `ChargingStateEvent` ever arriving, both halves are pinned at `ChargeState::Unknown` for the life of the boot.

Propagation confirms it end-to-end:

* **Left/central:** `publish_event(BatteryStatusEvent::from(status))` (R/input_device/battery.rs:122) → `CentralReplication` arm / `BatteryLightingState` → `set_left_battery` (G/central_lighting.rs:115, 281).
* **Right/peripheral:** `battery_sub.next_event()` → `SplitMessage::BatteryStatus` (R/split/peripheral.rs:121) → `set_peripheral_battery` → `PeripheralBatteryEvent` (R/split/driver.rs:85-92, 298) → `set_right_battery` (G/central_lighting.rs:121, 285). The peripheral's *other* battery arm (R/split/peripheral.rs:113-119), which is the one that would send `charge_state: e.charging.into()`, is driven by `charging_state_sub` — the same event with no publisher, so it never fires.
* **Wire:** `put_battery`/`get_battery` (G/split_lighting.rs:1324-1366) encode state byte `3` = `Unknown`; `1` = `Charging` is never emitted.

### Effect on the rules

`config/glove80.toml` carries **10 rules with `charge = "charging"`** (5 levels × 2 nodes, lines 819-873). Matching is exact-equality (R/lighting/source.rs:363-371):

```rust
(ChargeCondition::Any, _) | (Charging, ChargeState::Charging) | (Discharging, Discharging) | (Unknown, Unknown)
```

`ChargeCondition::Charging` requires `ChargeState::Charging`, which never occurs → **those 10 rules can never match.** The level rules you *do* see omit `charge` entirely, so they resolve to `ChargeCondition::Any`, which matches `Unknown` — which is precisely why the ordinary gauge works while the charging colour has never once appeared.

I did not check whether the MoErgo hardware even routes a charger-status line to a GPIO; that is a schematic question, and it is moot for this build since the reader is never constructed regardless.

**Diagnosis only, as requested — no fixes proposed and no files modified.**
