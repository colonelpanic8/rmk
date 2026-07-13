# BLE Multi-Profile User Operation Plan

## Goal

Make RMK BLE multi-profile behavior understandable from the user's point of view:

- A user can pair a new keyboard without learning BLE internals.
- A user can switch between already paired hosts without accidentally clearing bonds.
- A user can keep multiple remembered hosts connected at the same time and choose which one receives keyboard input.
- A user can recover from "the host forgot the keyboard" without guessing firmware state.
- A user who does not remember the current BLE profile still has a clear recovery key.

The implementation must still keep BLE behavior defensible:

- If the keyboard is visible as a named keyboard in a host's Add Device UI, it must be pairable/connectable.
- If a slot is already bonded and is only trying to reconnect to its stored host, it should not look like a normal pairable keyboard to unrelated hosts.

## Non-Goals

This plan does not define hardware-specific indication behavior.

RMK should reuse existing status events for important BLE profile transitions and logs for transient diagnostics. Individual keyboards can decide how, or whether, to consume those signals.

## User Mental Model

RMK exposes BLE profiles as slots.

For a keyboard with `N` BLE profiles:

- `BT0..BT(N-1)` mean "use this Bluetooth slot".
- `Clear BT` means "clear whichever Bluetooth slot is active now".
- A slot can be empty or remembered.
- An empty slot can pair with a new host.
- A remembered slot can be connected, reconnecting, or asleep.
- Multiple remembered slots may be connected at the same time.
- The active slot is the current input target. Normal keyboard reports go only to the active connected slot, not to every connected host.
- Switching slots should not disconnect other connected hosts.

Users should not need to understand advertising, bonds, FAL, HDDA, RPA, IRK, or stack state.

## Required User Operations

### First Pairing

User story:

> I turned on the keyboard for the first time. I want to pair it with my computer.

Expected operation:

1. Turn on the keyboard.
2. Open the host Bluetooth Add Device UI.
3. Select the named keyboard.

Expected behavior:

- If the current BLE slot is empty, the keyboard is visible for pairing after boot.
- If the pairing window expires, the keyboard sleeps to save power.
- Pressing any key wakes the keyboard.
- After wake, if the current slot is still empty, the keyboard becomes visible for pairing again.

Important user rule:

- Timeout is not a failure. Press any key to wake the current slot again.

### Use An Already Paired Host

User story:

> I already paired this keyboard to my laptop. I want to use that laptop again.

Expected operation:

1. Short press the slot key, for example `BT0`.
2. Use the keyboard after that slot is selected.

Expected behavior:

- If that slot is already connected, input routing moves to it immediately.
- If that slot is not connected, RMK tries to reconnect to that slot's remembered host.
- Other connected slots remain connected.
- The keyboard should not appear as a new pairable device to other hosts during this reconnect.
- The user should not need to open the host Add Device UI for this case.

Important user rule:

- Short press means "use this slot"; it does not clear the slot.

### Pair A Different Host

User story:

> I want to pair this keyboard to another computer, phone, or tablet.

Expected operation if the target slot is empty:

1. Short press the target slot key, for example `BT1`.
2. Open the new host's Bluetooth Add Device UI.
3. Select the named keyboard.

Expected operation if the target slot is already remembered:

1. Long press the target slot key for 5 seconds, for example `BT1`.
2. The slot is cleared and selected.
3. Open the new host's Bluetooth Add Device UI.
4. Select the named keyboard.

Expected behavior:

- Long pressing `BTn` clears slot `n`, switches to slot `n`, and enters visible pairing.
- If slot `n` was connected, only that slot's connection is disconnected. Other connected hosts stay connected.
- The release event after a successful long press must not also trigger a short press switch action.

Important user rule:

- To reuse a slot for a new host, long press that slot for 5 seconds.

### Host Forgot The Keyboard

User story:

> I clicked "Forget device" on my computer. Now I want to pair the keyboard again.

Expected operation when the user knows the slot:

1. Long press that slot key for 5 seconds, for example `BT0`.
2. Pair the named keyboard again from the host Bluetooth UI.

Expected operation when the user does not know the current slot:

1. Long press `Clear BT` for 5 seconds.
2. Pair the named keyboard again from the host Bluetooth UI.

Expected behavior:

- Short pressing a remembered slot must not automatically make it visible for public pairing.
- The keyboard should only clear automatically if the BLE stack reports explicit bond loss for the active connection.
- Otherwise, user recovery is a long press clear.

Important user rule:

- If the host forgot the keyboard, the keyboard slot must be cleared too.

### Current Slot Unknown

User story:

> I do not remember which BLE slot is active, but I want to reset the current Bluetooth connection.

Expected operation:

1. Long press `Clear BT` for 5 seconds.
2. The current active slot is cleared.
3. The keyboard enters visible pairing for that same slot.

Expected behavior:

- `Clear BT` is required even if `BT0..BT(N-1)` long press exists.
- `Clear BT` clears the current active slot, not a fixed slot.
- `Clear BT` should be a long-press action so accidental taps do not destroy a bond.

Important user rule:

- If you do not know the current slot, long press `Clear BT`.

### Slot Mapping Unknown

User story:

> I do not remember which computer is on `BT0`, `BT1`, or `BT2`.

Expected operation:

1. Short press a slot and see whether the expected host reconnects.
2. Try another slot if needed.
3. To reuse a slot, long press that slot for 5 seconds and pair again.

Expected behavior:

- Short pressing slots is safe and non-destructive.
- Clearing only happens after a 5-second long press.

Important user rule:

- Try slots with short presses. Reuse a slot with a long press.

### Sleep And Wake

User story:

> The keyboard stopped advertising or reconnecting. I want to continue using it.

Expected operation:

1. Press any key.

Expected behavior:

- After boot pairing or reconnect windows expire, the keyboard sleeps.
- Any key wakes the keyboard.
- If the current slot is empty, wake starts visible pairing.
- If the current slot is remembered, wake starts reconnect.
- If reconnect completes quickly, RMK should preserve the wake key report when technically safe.
- If reconnect takes too long, RMK should drop the wake key report rather than sending stale input later.

Important user rule:

- Sleep is normal. Press any key to wake the current slot.

## Required Key Semantics

### `BT0..BT(N-1)`

Short press:

- Switch to that slot.
- If the slot is empty, enter visible pairing.
- If the slot is remembered and already connected, route keyboard reports to that connection immediately.
- If the slot is remembered but disconnected, try to reconnect to the remembered host.
- Do not disconnect other connected slots.
- Do not clear anything.

Long press for 5 seconds:

- Clear that specific slot.
- Disconnect that slot's connection if it is currently connected.
- Switch to that slot.
- Enter visible pairing.
- Suppress the release event so it does not trigger the short press behavior.

### `Next BT` / `Previous BT`

Short press:

- Switch to the next or previous slot.
- The selected slot follows the same rule as a `BTn` short press:
  - empty slot -> visible pairing,
  - remembered slot -> reconnect.

Long press:

- No special clear behavior in phase 1.
- Keeping clear behavior only on `BTn` and `Clear BT` makes the user model simpler.

### `Clear BT`

Short press:

- No destructive action.
- It may be ignored or reserved for a future non-destructive status action.

Long press for 5 seconds:

- Clear the current active slot.
- Keep that slot active.
- Enter visible pairing.
- Suppress the release event.

This key is mandatory because the user may not remember which profile is active.

### Split Peer Clear Key

Existing split peer clearing behavior must remain separate from BLE host slot clearing.

If a split build exposes a "clear split peer" key, its user-facing label should not be confused with `Clear BT`:

- `Clear BT`: clear current host BLE slot.
- `Clear Split`: forget the bonded split peer.

Both may use a 5-second long press, but they must be documented as different operations.

## Events And Type Reuse

Prefer existing public status types.

RMK already has:

- `BleStatus { profile, state }`
- `BleState::{Advertising, Connected, Inactive}`
- `ConnectionStatusChangeEvent(ConnectionStatus)`

Use `ConnectionStatusChangeEvent` for profile/state transitions:

- Slot selected: `ble.profile` changes.
- Active slot pairing or reconnect started: `ble.state = Advertising`.
- Active slot connected: `ble.state = Connected`.
- Active slot sleep/inactive: `ble.state = Inactive`.

Do not add public `PairingStarted`, `ReconnectStarted`, `Sleeping`, or `SlotSelected` events unless a real downstream consumer cannot use `ConnectionStatusChangeEvent`.

With simultaneous host connections, `BleStatus` remains a coarse public status for the active output slot. It does not try to describe every connected host.

The BLE task needs internal runtime state keyed by slot:

- current connection handle/task for each connected slot,
- per-slot encryption/readiness,
- per-slot CCCD/client table state,
- per-slot pending first-wake report cache if needed,
- mapping from accepted peer identity to slot.

This internal per-slot connection table is not persisted and is not a public event stream.

Do not add a new public BLE profile event type in phase 1.

The following are transient outcomes, not state that needs to be stored or replayed:

- a slot was cleared,
- `BondLost` caused a slot clear,
- an unexpected peer was rejected.

Represent durable facts in the existing storage/profile data. Represent user-visible coarse state through existing `ConnectionStatusChangeEvent`. Use logs for transient diagnostics such as rejected peers and bond-lost recovery.

Do not add separate public `reason` or `source` enums in phase 1. If a reason is useful for logs or internal branch selection, keep it private inside `rmk/src/ble/mod.rs`.

## BLE Behavior Required To Support The User Model

### Visible Pairing

Use when:

- The active slot is empty.
- A slot was just cleared by long pressing `BTn`.
- The current slot was just cleared by long pressing `Clear BT`.
- The stack reported `BondLost` and RMK cleared the accepted slot.

Behavior:

- Legacy connectable scannable undirected advertising.
- The advertising payload and scan response together include the information needed for normal host pairing:
  - Flags.
  - HID UUID.
  - Battery UUID.
  - Keyboard appearance.
  - Complete local name when it fits; otherwise move name to scan response or use a shortened name.
- Filter policy is unfiltered.
- Controller filter accept list is cleared before advertising.
- `bondable = true`.
- Any host can discover and pair.

User meaning:

- "This slot is ready to pair."

Important implementation rule:

- Visible pairing must not fail only because the product name does not fit in the 31-byte legacy advertising payload.

### Hidden Reconnect

Use when:

- One or more remembered slots are not currently connected and RMK is trying to reconnect them to their stored hosts.

Behavior:

- Legacy connectable undirected advertising.
- Advertising payload is minimal, normally flags only.
- No local name.
- No HID UUID.
- No appearance.
- Empty scan response.
- Controller filter accept list contains stored peer identity addresses for remembered slots that are not currently connected, subject to controller capacity.
- Advertising filter policy is `FilterConnAndScan`.
- `bondable = false`.
- After accept, RMK maps the peer identity to its slot and validates it in software before GATT/HID work.
- If validation fails, disconnect immediately and resume reconnect advertising.

User meaning:

- "Remembered slots are trying to reconnect to their stored hosts."

Important implementation rule:

- Do not create a named, connectable-looking device that unrelated hosts can see but cannot successfully pair with.

### HDDA Wake Burst

High-duty directed advertising is an optimization, not the user's mental model.

Allowed only when all conditions are true:

- Active slot is remembered.
- BLE is inactive because pairing/reconnect previously timed out or the keyboard slept.
- Wake reason is direct user input, such as a key press or pointing movement.
- The active slot is not already connected.
- There is no active advertising.
- The stack is idle.
- The target peer identity is available and suitable for directed advertising.

Behavior:

- Try one bounded high-duty directed advertising window, about 1.28 seconds.
- `bondable = false`.
- If it fails, fall back to hidden reconnect fast, then hidden reconnect slow.
- Do not retry HDDA repeatedly for the same wake.

Do not use HDDA for:

- Startup.
- Empty slot pairing.
- Clear-and-pair flow.
- Host-forgot-bond recovery.
- Link loss.
- Host sleep.
- Host reboot.
- Out-of-range disconnect.

User meaning:

- None directly. The user only experiences "press any key to wake and reconnect."

## End-User Flows Mapped To Firmware Modes

### Boot

If active slot is empty:

```text
Boot -> PairingVisible -> timeout -> Sleep
```

If active slot is remembered:

```text
Boot -> HiddenReconnectFast/Slow for remembered unconnected slots -> timeout -> Sleep if active slot is still disconnected
```

Required behavior:

- Boot and reboot must start the correct active BLE mode immediately.
- The keyboard must not enter sleep first and wait for user input before advertising an empty slot or reconnecting a remembered slot.
- No HDDA on boot.
- Boot windows should be long enough for users to turn on the keyboard and then operate the host UI.

Recommended timing:

- Empty slot visible pairing: about 300 seconds.
- Remembered unconnected slots fast reconnect: about 5 seconds at about 20 ms interval.
- Remembered unconnected slots slow reconnect on boot: about 180-300 seconds at about 1 second interval.

### Slot Short Press

If selected slot is empty:

```text
ShortPress(BTn) -> select slot n -> PairingVisible
```

If selected slot is remembered:

```text
ShortPress(BTn) -> select slot n -> already connected OR HiddenReconnectFast/Slow for slot n
```

Required behavior:

- No public pairing fallback for remembered slots.
- If the selected slot is already connected, switching is only a routing change.
- Other connected slots remain connected.
- The user must long press to clear before pairing a new host to that slot.

### Slot Long Press

```text
LongPress(BTn, 5s) -> clear slot n -> select slot n -> PairingVisible
```

Required behavior:

- This is the recovery path when the user knows the target slot.
- Only slot `n` is disconnected/cleared; other connected slots remain connected.
- Release after successful long press is consumed.

### Clear Current Long Press

```text
LongPress(Clear BT, 5s) -> clear current slot -> PairingVisible
```

Required behavior:

- This is the recovery path when the user does not remember which profile is active.
- Short press must not clear the bond.
- Release after successful long press is consumed.

### Unexpected Disconnect

```text
Connected -> unexpected disconnect -> HiddenReconnectFast -> HiddenReconnectSlow -> Sleep
```

Required behavior:

- Do not use HDDA for ordinary link loss.
- Do not become publicly pairable just because a remembered host disconnected.
- A disconnect from one slot must not tear down other connected slots.

### Wake From Sleep

If active slot is empty:

```text
Sleep + any key -> PairingVisible
```

If active slot is remembered:

```text
Sleep + any key -> if active slot connected, route input; otherwise optional one HDDA burst -> HiddenReconnectFast/Slow
```

Required behavior:

- Any key can wake.
- Wake input should be preserved only within a short safe reconnect window.

## First Wake Report Policy

User expectation:

- If the host reconnects immediately after a key wakes the keyboard, that key should usually work.
- If reconnect takes too long, the key should not appear several seconds later.

Implementation direction:

- Add a tiny BLE-only pending report cache independent from `BLE_REPORT_CHANNEL`.
- Enable it only for `Sleep + user input` reconnect.
- Store only reports that would otherwise target the active BLE slot while that slot is not connected yet.
- Do not duplicate reports successfully routed to USB.
- Store a bounded ring of the first few reports, for example 4 reports, with timestamp and slot/profile generation.
- Flush once after encryption completes for the active slot that captured the report and before normal BLE report forwarding for that slot.
- Drop the cache if:
  - active profile changes,
  - slot is cleared,
  - visible pairing starts,
  - identity validation fails,
  - reconnect times out,
  - USB becomes the active transport,
  - TTL expires.

Recommended TTL:

- Around the HDDA window plus a small margin, for example 2 seconds.

This must not become a general offline typeahead buffer.

## Current Local Status

The repository is currently still mostly at the pre-plan BLE profile implementation:

- `rmk/src/ble/profile.rs` models `BleProfileAction::ClearBond` without a slot argument.
- `UPDATED_CCCD_TABLE` carries only raw CCCD table bytes and updates the current active profile at save time.
- `rmk/src/keyboard.rs` sends the old `BleProfileAction::ClearBond` shape.
- `rmk/src/ble/mod.rs` has a single visible, bondable, undirected advertising path.
- Existing docs describe `User0..User(N-1)` as profile switching and `User(N+2)` as clear current profile.
- Existing Vial example titles use labels such as "Bluetooth Channel 0" and "Clear bond info for current channel".

The first implementation step should introduce the user-facing action model and the slot-aware save model, then update all call sites in one compile-preserving step.

## Code Change Plan

### 1. Reuse The Existing Profile Action Shape

Files:

- `rmk/src/ble/profile.rs`
- `rmk/src/keyboard.rs`
- `rmk/src/ble/mod.rs`

Tasks:

- Keep `BleProfileAction` close to the current design:
  - keep `Switch(u8)`,
  - keep `Previous`,
  - keep `Next`,
  - keep existing `ClearBond` as the current-slot clear action,
  - add only `ClearAndSwitch(u8)` for `BTn` long press.
- Do not introduce both `ClearBond(slot)` and `ClearCurrentAndPair` unless implementation proves the current-slot `ClearBond` shape is unsafe.
- Define the user-visible effect of `ClearBond` as: clear the current active slot, keep it active, then enter visible pairing.
- Define the user-visible effect of `ClearAndSwitch(slot)` as: clear `slot`, switch to `slot`, then enter visible pairing.
- After a slot is actually cleared, update storage/profile cache and publish the resulting `ConnectionStatusChangeEvent` if profile/state changed.
- Capture slot/profile generation when a connection is accepted.
- Tag CCCD updates and bond save events with the accepted connection's slot/generation.
- Avoid `current_profile()` inside GATT save paths after a connection has already been accepted.
- Split profile data saving from idle stack bond injection.
- Bond save and CCCD save may update cache/flash while connected, but must not switch active slot.
- Maintain an internal slot -> connection table so multiple accepted connections can be served concurrently.
- A connected slot owns its GATT/HID writer state, CCCD state, and connection task.
- HID input reports are sent only to the active slot's connected HID writer. They are not broadcast to all connected hosts.
- Host output reports such as LED state are tracked per slot; user-facing state should follow the active slot unless a keyboard explicitly chooses otherwise.

### 2. Implement Long Press User Actions

File:

- `rmk/src/keyboard.rs`

Tasks:

- For `User0..User(NUM_BLE_PROFILE - 1)`:
  - On press, start a 5-second long-press wait.
  - If release happens before timeout, send `Switch(slot)`.
  - If timeout wins, send `ClearAndSwitch(slot)`.
  - Remember that this key press was consumed by long press.
  - On release after long press, suppress the normal short-press switch.
  - If another key event arrives before timeout, push it into `unprocessed_events` and keep normal behavior.
- For `User(NUM_BLE_PROFILE + 2)` / `Clear BT`:
  - On press, start a 5-second long-press wait.
  - If release happens before timeout, perform no destructive action.
  - If timeout wins, send existing `BleProfileAction::ClearBond`.
  - `ClearBond` clears the current active slot and enters visible pairing.
  - Suppress release after a successful long press.
- Keep `Next`, `Previous`, output toggle, and split clear peer behavior compatible.
- Keep split peer clearing separate from host BLE slot clearing.

### 3. Add Multi-Connection Advertising And Accept Loop

File:

- `rmk/src/ble/mod.rs`

Tasks:

- Replace the single `advertise()` path with mode-aware advertising.
- The BLE loop must be able to keep serving existing slot connections while advertising for another empty or remembered slot, if the controller/stack supports advertising while connected.
- Do not add new public advertising mode types.
- Prefer private helper functions inside `rmk/src/ble/mod.rs`, for example:
  - visible pairing advertising,
  - hidden reconnect advertising,
  - optional one-shot HDDA wake burst.
- If a private enum makes the loop easier to write, keep it file-local and minimal; it should not become a public event/status type.
- Snapshot active slot and active bond before starting advertising.
- Choose advertising sequence from user intent:
  - empty active slot -> visible pairing,
  - remembered unconnected slots on startup -> hidden reconnect,
  - remembered selected slot on slot switch -> hidden reconnect if not already connected,
  - remembered slot on link loss -> hidden reconnect for that slot while other slots stay connected,
  - remembered active slot on user wake -> if disconnected, optional one HDDA burst, then hidden reconnect,
  - cleared slot -> visible pairing.
- Do not use HDDA for startup, clear-and-pair, host-forgot recovery, or ordinary link loss.
- Publish existing `ConnectionStatusChangeEvent` through `set_ble_profile()` / `set_ble_state()` when profile or coarse BLE state changes.
- Stop advertising when all useful targets are connected and no selected empty/cleared slot is waiting for pairing.

### 4. Add Filter Accept List Handling

File:

- `rmk/src/ble/mod.rs`

Tasks:

- Add controller command bounds required by `Peripheral::set_filter_accept_list()`:
  - `LeClearFilterAcceptList`
  - `LeAddDeviceToFilterAcceptList`
- Before visible pairing advertising:
  - clear FAL,
  - use `AdvFilterPolicy::Unfiltered`.
- Before hidden reconnect advertising:
  - set FAL to unconnected remembered slots' peer identity addresses, subject to controller capacity,
  - use `AdvFilterPolicy::FilterConnAndScan`.
- Make hidden advertising payload flags-only.
- Explicitly clear scan response data for hidden advertising.
- Do not rely on passing `scan_data: &[]` to `Peripheral::advertise()` if `trouble-host` skips zero-length scan response commands.
- Add the command bound required for explicit scan response clearing:
  - `LeSetScanResponseData`
- Make visible advertising payload construction resilient to 31-byte legacy advertising limits.

### 5. Enforce Bondable State And Peer Validation

File:

- `rmk/src/ble/mod.rs`

Tasks:

- Visible pairing:
  - `conn.raw().set_bondable(true)`.
- Hidden reconnect and HDDA:
  - `conn.raw().set_bondable(false)`.
  - Check accepted peer identity against the active bond captured for this advertising attempt.
  - If mismatch:
    - log warning,
    - disconnect immediately,
    - return to reconnect advertising.
- Do identity check before:
  - restoring CCCD table,
  - updating PHY,
  - running GATT task,
  - starting HID writer task.

### 6. Keep Profile, Advertising, And Connection State Consistent

Files:

- `rmk/src/ble/profile.rs`
- `rmk/src/ble/mod.rs`

Profile switch:

```text
receive Switch/Next/Previous
update active output slot
if selected slot is connected: route reports to it
if selected slot is empty: make it the visible pairing target
if selected slot is remembered but disconnected: include it in reconnect advertising
```

Clear target slot:

```text
receive ClearBond or ClearAndSwitch(slot)
resolve target slot
disconnect only that slot if connected
wait for that slot's connection task to finish
clear that slot's bond/cache/CCCD data
make that slot the visible pairing target
keep other slot connections running
```

Tasks:

- Profile switch actions should not disconnect other slots. They only change the active output slot and possibly start reconnect/pairing for the selected slot.
- Clear actions disconnect only the target slot if it is connected, then clear that slot's storage/cache and enter visible pairing for that slot.
- `ProfileManager::add_profile_info()` must not rewrite global stack bond state in a way that invalidates other active slot connections.
- Stack bond/security state must support all currently connected slots plus reconnect candidates, or be updated only in ways that are safe while those connections remain active.
- All save and clear decisions caused by a connection must use the captured slot/generation, not current profile at event handling time.

### 7. Handle BondLost

File:

- `rmk/src/ble/mod.rs`

Tasks:

- Change `gatt_events_task()` to return a connection-end reason instead of only `Result<(), Error>`.
- On `GattConnectionEvent::BondLost`:
  - return `ConnectionEnd::BondLost { slot, generation }` using the slot/generation captured at accept time.
- BLE connection loop handles `BondLost` by:
  - disconnecting that slot's connection if still connected,
  - clearing the accepted connection's slot only if generation still matches,
  - logging the recovery,
  - entering visible pairing only for that slot.

This is the only automatic host-forgot recovery path because it is based on explicit stack evidence.

### 8. Validate Pairing Identity

File:

- `rmk/src/ble/mod.rs`

Tasks:

- In `PairingComplete`, inspect the returned bond.
- Save the bond only if it has identity data that can support reliable reconnect:
  - public identity address,
  - random static identity address,
  - resolvable private address with distributed peer IRK.
- Do not treat every random address as stable.
- Check random address type bits carefully with the `bt-hci`/`BdAddr` byte order in mind:
  - static random has the two most significant random-address bits set,
  - RPA has the `01` pattern,
  - NRPA has the `00` pattern.
- RPA/NRPA without a peer IRK must not be saved as a normal hidden reconnect slot.
- If pairing lacks usable identity:
  - reject the profile save,
  - disconnect,
  - return to visible pairing,
  - log a reason that can be surfaced by integrators.

If `trouble-host` later exposes pairing key-distribution policy, move this from post-pairing validation into SMP negotiation.

### 9. Preserve First Wake Report

Files:

- `rmk/src/ble/mod.rs`
- `rmk/src/channel.rs`
- possibly keyboard/report channel code

Tasks:

- Verify whether the wake key/pointing event still reaches report generation while BLE reconnects.
- Verify whether report routing drops the first report because BLE is not active until encrypted/connected.
- If the first report can be lost:
  - add the pending report cache near the report-routing boundary,
  - enable it only for user-input wake from sleep,
  - cache only when BLE is the intended transport and USB did not accept the report,
  - store a bounded ring of reports with slot generation and timestamp,
  - flush once after encryption completes,
  - clear on timeout, profile switch, clear slot, identity mismatch, visible pairing, or USB becoming active.

### 10. Address Identity For Simultaneous Multi-Host

Simultaneous multi-host support is a phase-1 design requirement.

Phase 1 should use one consistent local BLE identity address for the keyboard and support multiple central connections under that identity.

Why:

- A bonded BLE HID device's advertised address, pairing identity, encryption/security manager state, resolving-list state, and stored bonds must describe the same local identity.
- Changing only the advertised address would create an identity split: the host may connect to one address while RMK's security/bond state believes another identity is active.
- Switching one global local static address while connections are active would invalidate assumptions for existing connections.
- `trouble-host` 0.7 does not expose a safe public runtime API for multiple simultaneous local identities.

Therefore:

- Do not modify the local static address per profile in phase 1.
- Do not try to solve this by changing only the advertised address.
- Treat RMK BLE profiles as peer-host slots under one keyboard identity.
- Store peer bond/identity/CCCD data per slot.
- Maintain separate active connection state per slot.

If the stack/controller can advertise while connected:

- Continue serving connected slots.
- Advertise for selected empty/cleared slots in visible pairing mode.
- Advertise for unconnected remembered slots in hidden reconnect mode.

If the stack/controller cannot advertise while connected:

- Still keep the multi-connection architecture.
- Document the platform limitation.
- Reconnect/pair additional slots only when advertising is possible again.

Future per-slot local identities are only useful if RMK can also support multiple advertising/connection/security contexts where each context has a consistent local identity. That is a larger stack capability than changing an advertisement field.

### 11. Documentation, Vial Labels, And Migration Notes

Files:

- relevant docs under `docs/`
- BLE example `vial.json` files
- migration or release notes if present

Documentation must describe user tasks, not BLE internals:

- First pairing.
- Pairing another host.
- Switching back to a paired host.
- Host forgot keyboard recovery.
- Current slot unknown recovery with `Clear BT`.
- Slot mapping unknown recovery.
- Sleep and wake with any key.

Required user-facing rules:

- `BTn` short press: use that slot.
- `BTn` long press for 5 seconds: clear that slot and pair.
- `Clear BT` long press for 5 seconds: clear current slot and pair.
- Short pressing a remembered slot will not make the keyboard appear for new pairing.
- If the host forgot the keyboard, clear the keyboard slot too.
- If the keyboard sleeps after timeout, press any key to wake.

Example Vial titles:

- `BT0: tap to use, hold 5s to clear and pair`
- `BT1: tap to use, hold 5s to clear and pair`
- `BT2: tap to use, hold 5s to clear and pair`
- `Clear BT: hold 5s to clear current slot and pair`
- `Next BT: use next Bluetooth slot`
- `Prev BT: use previous Bluetooth slot`
- `Clear Split: hold 5s to forget split peer`

Migration note:

- Users upgrading from older shared-address multi-profile behavior may need to clear and re-pair BLE slots if host-side Bluetooth state is confused.
- Simultaneous multi-host support uses one consistent keyboard local identity in phase 1.
- Per-slot peer bonds are RMK-side slots; they are not separate local advertised identities.
- Do not document or imply that changing the advertised address alone creates separate BLE profiles.

## Verification Plan

### User Scenario Tests

Test 1: First pairing

```text
fresh/empty active slot -> boot -> host Add Device shows named keyboard -> pair succeeds
```

Expected:

- User can pair without pressing a slot key.
- If the window expires, pressing any key starts pairing visibility again.

Test 2: Use paired host

```text
slot 0 paired to Host A -> short press BT0
```

Expected:

- Host A reconnects.
- A different host should not see a normal named pairable keyboard for slot 0.
- The bond is not cleared.

Test 3: Pair another host using empty slot

```text
slot 1 empty -> short press BT1 -> scan on Host B
```

Expected:

- Host B sees the named keyboard.
- Pairing saves bond to slot 1.

Test 4: Pair another host by reusing a slot

```text
slot 1 already remembered -> long press BT1 for 5s -> scan on Host B
```

Expected:

- Slot 1 is cleared.
- Slot 1 remains selected.
- Host B sees the named keyboard and can pair.
- Release after long press does not cause another short-press action.

Test 5: Host forgot keyboard, slot known

```text
Host A forgets keyboard -> long press BT0 for 5s -> scan on Host A
```

Expected:

- Keyboard becomes visible for pairing.
- Host A can pair again.

Test 6: Host forgot keyboard, current slot unknown

```text
Host forgets keyboard -> user long presses Clear BT for 5s
```

Expected:

- Current active slot is cleared.
- Keyboard becomes visible for pairing.
- User did not need to know whether current slot was BT0, BT1, or BT2.

Test 7: Accidental short press safety

```text
slot remembered -> short press BTn
slot remembered -> short press Clear BT
```

Expected:

- `BTn` short press switches/reconnects only.
- `Clear BT` short press does not clear the bond.

Test 8: Slot mapping unknown

```text
slots paired to different hosts -> short press BT0/BT1/BT2 in sequence
```

Expected:

- Short presses are safe.
- User can identify which host reconnects.

Test 9: Sleep and wake

```text
pairing/reconnect window expires -> keyboard sleeps -> press any key
```

Expected:

- Empty current slot starts visible pairing.
- Remembered current slot starts reconnect.
- Wake report is delivered only if reconnect completes inside the short TTL.

Test 10: Split clear separation

```text
split build exposes Clear BT and Clear Split
```

Expected:

- `Clear BT` clears the host BLE slot.
- `Clear Split` clears the split peer.
- Labels and docs do not imply they do the same thing.

Test 11: Two hosts stay connected

```text
slot 0 paired and connected to Host A -> slot 1 paired and connected to Host B -> short press BT0/BT1
```

Expected:

- Host A and Host B can remain connected at the same time.
- Short pressing `BT0` routes keyboard reports to Host A.
- Short pressing `BT1` routes keyboard reports to Host B.
- Switching active slot does not disconnect the other host.

### Technical BLE Tests

Test 12: Hidden reconnect visibility

```text
slot 0 paired to Host A -> scan from Host B while slot 0 active
```

Expected:

- Host B does not see a normal named HID keyboard for slot 0.
- Host A can reconnect.

Test 13: Unexpected disconnect

```text
connected -> force host sleep/range loss -> firmware sees disconnect
```

Expected:

- No HDDA burst.
- Hidden reconnect fast starts.
- Hidden reconnect slow follows.
- Eventually sleeps.

Test 14: Wrong peer defense

```text
simulate unexpected accepted peer in reconnect mode
```

Expected:

- Firmware disconnects before GATT/HID work.
- Firmware logs the rejected peer.
- Firmware resumes hidden reconnect advertising.

Test 15: RPA host reconnect

```text
pair to a host that uses RPA -> wait for host address rotation -> reconnect
```

Expected:

- Reconnect succeeds if IRK was distributed.
- Pairing is not saved as a normal hidden reconnect slot if IRK is missing and RPA is required.

Test 16: BondLost

```text
host deletes bond but still initiates a connection that reaches security handling
```

Expected:

- `BondLost` clears the accepted slot.
- Firmware logs the bond-lost recovery.
- Firmware enters visible pairing.

Test 17: Multi-host report isolation

```text
Host A connected on slot 0 -> Host B connected on slot 1 -> type while BT0 active -> switch to BT1 -> type again
```

Expected:

- Reports typed while `BT0` is active go only to Host A.
- Reports typed while `BT1` is active go only to Host B.
- Reports are not broadcast to all connected hosts.

### Build/Compile

From `rmk/`:

```bash
cargo check --no-default-features --features=storage,async_matrix,_ble
cargo nextest run --no-default-features --features=split,vial,storage,async_matrix,_ble
```

If feature combinations fail for unrelated platform reasons, run the closest BLE/storage feature set that compiles locally and record the gap.

## Acceptance Criteria

The implementation is acceptable when:

- Users can explain the BLE keys as:
  - `BTn` tap to use,
  - `BTn` hold to clear that slot and pair,
  - `Clear BT` hold to clear the current slot and pair.
- Short pressing a slot never clears a bond.
- Short pressing `Clear BT` never clears a bond.
- Empty or cleared slots are visibly pairable.
- Remembered slots reconnect to their remembered host without appearing as normal pairable keyboards to unrelated hosts.
- Multiple remembered slots can be connected at the same time when the stack/controller supports it.
- The active slot determines HID input routing; reports are not broadcast to every connected host.
- Switching active slot does not disconnect other connected slots.
- Host-forgot recovery works through long pressing either the known `BTn` slot or `Clear BT`.
- Boot starts the correct mode immediately.
- After timeout sleep, pressing any key wakes the current slot.
- First wake input is replayed only if reconnect completes inside the short TTL.
- Hidden reconnect uses FAL plus software identity validation.
- Identity mismatch disconnects immediately.
- `BondLost` automatically clears the accepted slot and enters visible pairing.
- Profile switching, bond injection, FAL changes, and local identity state are not modified in a way that invalidates any active slot connection.
- Documentation and Vial labels describe user operations, not internal BLE modes.
- Current shared-address limitations and migration expectations are documented.
- Simultaneous multi-host behavior uses one consistent local BLE identity in phase 1 and does not fake profiles by changing only the advertised address.
