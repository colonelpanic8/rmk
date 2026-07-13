# BLE 多 Profile 用户操作计划

## 目标

从用户角度，让 RMK BLE 多 profile 行为变得容易理解：

- 用户不需要学习 BLE 内部机制，也能配对一把新键盘。
- 用户可以在已经配对过的主机之间切换，而不会误清除 bond。
- 用户可以让多个已记住的主机同时保持连接，并选择哪一个接收键盘输入。
- 用户可以从“主机忘记了键盘”的状态恢复，而不需要猜固件当前处于什么状态。
- 用户即使不记得当前 BLE profile，也有一个明确的恢复按键。

实现上仍然必须保持 BLE 行为合理且可辩护：

- 如果键盘在主机的添加设备界面中以具名键盘显示，它就必须可配对/可连接。
- 如果某个 slot 已经 bonded，并且只是尝试重连它保存过的主机，它就不应该在无关主机上看起来像普通可配对键盘。

## 非目标

本计划不定义硬件相关的指示行为。

RMK 应该为重要的 BLE profile 状态转换复用现有 status events，并用 logs 表达瞬时诊断。各个键盘可以自行决定是否消费这些信号，以及如何消费。

## 用户心智模型

RMK 把 BLE profiles 暴露为 slots。

对于有 `N` 个 BLE profiles 的键盘：

- `BT0..BT(N-1)` 表示“使用这个蓝牙槽”。
- `Clear BT` 表示“清除当前正在使用的蓝牙槽”。
- 一个 slot 可以是空的，也可以是已经记住过主机的。
- 空 slot 可以和新主机配对。
- 记住过主机的 slot 可以处于 connected、reconnecting 或 asleep。
- 多个记住过主机的 slots 可以同时 connected。
- active slot 是当前输入目标。普通 keyboard reports 只发送给 active connected slot，不发送给所有 connected hosts。
- 切换 slot 不应该断开其他 connected hosts。

用户不应该需要理解 advertising、bonds、FAL、HDDA、RPA、IRK 或 stack state。

## 必需用户操作

### 首次配对

用户故事：

> 我第一次打开键盘。我想把它和电脑配对。

预期操作：

1. 打开键盘。
2. 打开主机的蓝牙添加设备界面。
3. 选择具名键盘。

预期行为：

- 如果当前 BLE slot 是空的，键盘在启动后可见并可配对。
- 如果配对窗口超时，键盘会休眠以省电。
- 按任意键会唤醒键盘。
- 唤醒后，如果当前 slot 仍然是空的，键盘会再次变为可见并可配对。

重要用户规则：

- 超时不是故障。按任意键即可重新唤醒当前 slot。

### 使用已经配对过的主机

用户故事：

> 我已经把这把键盘配对到笔记本上。我想再次使用那台笔记本。

预期操作：

1. 短按 slot key，例如 `BT0`。
2. 该 slot 被选中后使用键盘。

预期行为：

- 如果该 slot 已经 connected，输入路由会立即切到它。
- 如果该 slot 尚未 connected，RMK 会尝试重连这个 slot 记住的主机。
- 其他 connected slots 保持 connected。
- 在这个重连过程中，键盘不应该在其他主机上显示为新的可配对设备。
- 这种情况下，用户不需要打开主机的添加设备界面。

重要用户规则：

- 短按表示“使用这个 slot”；它不会清除这个 slot。

### 配对另一台主机

用户故事：

> 我想把这把键盘配对到另一台电脑、手机或平板。

如果目标 slot 为空，预期操作：

1. 短按目标 slot key，例如 `BT1`。
2. 打开新主机的蓝牙添加设备界面。
3. 选择具名键盘。

如果目标 slot 已经记住过主机，预期操作：

1. 长按目标 slot key 5 秒，例如 `BT1`。
2. 该 slot 被清除并被选中。
3. 打开新主机的蓝牙添加设备界面。
4. 选择具名键盘。

预期行为：

- 长按 `BTn` 会清除 slot `n`，切换到 slot `n`，并进入可见配对。
- 如果 slot `n` 已经 connected，只断开该 slot 的连接。其他 connected hosts 保持 connected。
- 一次成功长按之后的 release event 不能再触发短按切换动作。

重要用户规则：

- 要把某个 slot 重新用于新主机，长按该 slot 5 秒。

### 主机忘记了键盘

用户故事：

> 我在电脑上点击了“忘记此设备”。现在我想重新配对键盘。

如果用户知道 slot，预期操作：

1. 长按该 slot key 5 秒，例如 `BT0`。
2. 在主机蓝牙界面中重新配对具名键盘。

如果用户不知道当前 slot，预期操作：

1. 长按 `Clear BT` 5 秒。
2. 在主机蓝牙界面中重新配对具名键盘。

预期行为：

- 短按一个记住过主机的 slot，不得自动让它变为公开可配对。
- 只有当 BLE stack 为当前连接报告明确的 bond loss 时，键盘才应该自动清除。
- 否则，用户恢复路径是长按清除。

重要用户规则：

- 如果主机忘记了键盘，键盘这边的 slot 也必须清除。

### 不知道当前 Slot

用户故事：

> 我不记得当前激活的是哪个 BLE slot，但我想重置当前蓝牙连接。

预期操作：

1. 长按 `Clear BT` 5 秒。
2. 当前 active slot 被清除。
3. 键盘在同一个 slot 上进入可见配对。

预期行为：

- 即使已经有 `BT0..BT(N-1)` 长按清除，`Clear BT` 仍然是必需的。
- `Clear BT` 清除当前 active slot，而不是固定 slot。
- `Clear BT` 应该是长按动作，避免误触破坏 bond。

重要用户规则：

- 如果你不知道当前 slot，长按 `Clear BT`。

### 不知道 Slot 和主机的对应关系

用户故事：

> 我不记得 `BT0`、`BT1`、`BT2` 分别对应哪台电脑。

预期操作：

1. 短按一个 slot，看预期主机是否重连。
2. 如有需要，再尝试另一个 slot。
3. 如果要重用某个 slot，长按该 slot 5 秒并重新配对。

预期行为：

- 短按 slot 是安全且非破坏性的。
- 只有 5 秒长按才会清除。

重要用户规则：

- 用短按尝试 slots。用长按重用 slot。

### 休眠和唤醒

用户故事：

> 键盘停止 advertising 或重连了。我想继续使用它。

预期操作：

1. 按任意键。

预期行为：

- 启动后的配对或重连窗口超时后，键盘会休眠。
- 任意键都会唤醒键盘。
- 如果当前 slot 是空的，唤醒后开始可见配对。
- 如果当前 slot 已经记住主机，唤醒后开始重连。
- 如果重连很快完成，RMK 应该在技术上安全的情况下保留唤醒按键 report。
- 如果重连耗时太久，RMK 应该丢弃唤醒按键 report，而不是之后发送过期输入。

重要用户规则：

- 休眠是正常行为。按任意键即可唤醒当前 slot。

## 必需按键语义

### `BT0..BT(N-1)`

短按：

- 切换到该 slot。
- 如果该 slot 是空的，进入可见配对。
- 如果该 slot 已记住主机且已经 connected，立即把 keyboard reports 路由到该连接。
- 如果该 slot 已记住主机但 disconnected，尝试重连记住的主机。
- 不断开其他 connected slots。
- 不清除任何东西。

长按 5 秒：

- 清除该具体 slot。
- 如果该 slot 当前 connected，断开该 slot 的连接。
- 切换到该 slot。
- 进入可见配对。
- 抑制 release event，避免触发短按行为。

### `Next BT` / `Previous BT`

短按：

- 切换到下一个或上一个 slot。
- 被选中的 slot 遵循和 `BTn` 短按相同的规则：
  - 空 slot -> 可见配对，
  - 记住过主机的 slot -> 重连。

长按：

- phase 1 中没有特殊清除行为。
- 只在 `BTn` 和 `Clear BT` 上保留清除行为，可以让用户模型更简单。

### `Clear BT`

短按：

- 没有破坏性动作。
- 可以忽略，或保留给未来的非破坏性状态动作。

长按 5 秒：

- 清除当前 active slot。
- 保持该 slot 为 active。
- 进入可见配对。
- 抑制 release event。

这个按键是必需的，因为用户可能不记得当前激活的是哪个 profile。

### Split Peer Clear Key

现有 split peer 清除行为必须和 BLE host slot 清除分开。

如果 split build 暴露了一个“clear split peer”按键，它的用户可见标签不应该和 `Clear BT` 混淆：

- `Clear BT`：清除当前 host BLE slot。
- `Clear Split`：忘记 bonded split peer。

两者都可以使用 5 秒长按，但必须在文档中说明它们是不同操作。

## 事件和类型复用

优先复用现有 public status types。

RMK 已经有：

- `BleStatus { profile, state }`
- `BleState::{Advertising, Connected, Inactive}`
- `ConnectionStatusChangeEvent(ConnectionStatus)`

用 `ConnectionStatusChangeEvent` 表达 profile/state 转换：

- Slot selected：`ble.profile` 改变。
- Active slot pairing 或 reconnect started：`ble.state = Advertising`。
- Active slot connected：`ble.state = Connected`。
- Active slot sleep/inactive：`ble.state = Inactive`。

除非真实下游消费者无法使用 `ConnectionStatusChangeEvent`，否则不要新增 public `PairingStarted`、`ReconnectStarted`、`Sleeping` 或 `SlotSelected` events。

在 simultaneous host connections 下，`BleStatus` 仍然是 active output slot 的粗粒度 public status。它不尝试描述每一个 connected host。

BLE task 需要按 slot 保存内部运行时状态：

- 每个 connected slot 的 connection handle/task，
- per-slot encryption/readiness，
- per-slot CCCD/client table state，
- 如有需要，per-slot pending first-wake report cache，
- accepted peer identity 到 slot 的映射。

这个内部 per-slot connection table 不持久化，也不是 public event stream。

phase 1 中不要新增 public BLE profile event type。

以下都是瞬时结果，不是需要存储或 replay 的状态：

- 某个 slot 被清除，
- `BondLost` 导致 slot 清除，
- 意外 peer 被拒绝。

持久事实应该体现在现有 storage/profile data 中。用户可见的粗粒度状态继续通过现有 `ConnectionStatusChangeEvent` 表达。意外 peer 和 bond-lost recovery 这类瞬时诊断使用 logs。

phase 1 中不要新增独立 public `reason` 或 `source` enums。如果 logs 或内部分支选择需要 reason，把它留在 `rmk/src/ble/mod.rs` 内部。

## 支持用户模型所需的 BLE 行为

### Visible Pairing

使用场景：

- active slot 是空的。
- 某个 slot 刚刚通过长按 `BTn` 被清除。
- 当前 slot 刚刚通过长按 `Clear BT` 被清除。
- stack 报告了 `BondLost`，并且 RMK 清除了 accepted slot。

行为：

- Legacy connectable scannable undirected advertising。
- advertising payload 和 scan response 合起来包含正常主机配对所需的信息：
  - Flags。
  - HID UUID。
  - Battery UUID。
  - Keyboard appearance。
  - 如果放得下，使用 complete local name；否则把 name 移到 scan response，或使用 shortened name。
- Filter policy 是 unfiltered。
- advertising 前清空 controller filter accept list。
- `bondable = true`。
- 任意主机都可以发现并配对。

用户含义：

- “这个 slot 已准备好配对。”

重要实现规则：

- Visible pairing 不能只因为产品名放不进 31-byte legacy advertising payload 就失败。

### Hidden Reconnect

使用场景：

- 一个或多个 remembered slots 当前未 connected，并且 RMK 正在尝试把它们重连到保存的主机。

行为：

- Legacy connectable undirected advertising。
- Advertising payload 极简，通常只有 flags。
- 没有 local name。
- 没有 HID UUID。
- 没有 appearance。
- scan response 为空。
- Controller filter accept list 包含当前未 connected 的 remembered slots 保存的 peer identity addresses，受 controller capacity 限制。
- Advertising filter policy 是 `FilterConnAndScan`。
- `bondable = false`。
- accept 之后，RMK 将 peer identity 映射到对应 slot，并在 GATT/HID 工作前用软件验证。
- 如果验证失败，立即断开连接并恢复 reconnect advertising。

用户含义：

- “记住过主机的 slots 正在尝试重连它们保存的主机。”

重要实现规则：

- 不要创建一个无关主机可见、看起来可以连接、但无法成功配对的具名设备。

### HDDA Wake Burst

High-duty directed advertising 是优化，不是用户心智模型。

只有满足所有条件时才允许使用：

- Active slot 已记住主机。
- BLE inactive 的原因是之前配对/重连超时，或键盘已休眠。
- 唤醒原因是直接用户输入，例如按键或 pointing movement。
- Active slot 尚未 connected。
- 当前没有 active advertising。
- stack idle。
- target peer identity 可用，并且适合 directed advertising。

行为：

- 尝试一个有界 high-duty directed advertising 窗口，约 1.28 秒。
- `bondable = false`。
- 如果失败，回退到 hidden reconnect fast，然后 hidden reconnect slow。
- 对同一次唤醒不要重复尝试 HDDA。

不要在以下场景使用 HDDA：

- Startup。
- 空 slot 配对。
- Clear-and-pair flow。
- Host-forgot-bond recovery。
- Link loss。
- Host sleep。
- Host reboot。
- Out-of-range disconnect。

用户含义：

- 没有直接含义。用户只会感受到“按任意键唤醒并重连”。

## 映射到固件模式的终端用户流程

### Boot

如果 active slot 为空：

```text
Boot -> PairingVisible -> timeout -> Sleep
```

如果 active slot 已记住主机：

```text
Boot -> HiddenReconnectFast/Slow for remembered unconnected slots -> timeout -> Sleep if active slot is still disconnected
```

必需行为：

- Boot 和 reboot 必须立即启动正确的 active BLE 模式。
- 键盘不得先进入 sleep，并等待用户输入之后才为 empty slot advertising 或为 remembered slot 重连。
- boot 时不使用 HDDA。
- boot 窗口应该足够长，让用户有时间打开键盘，然后操作主机 UI。

建议时序：

- 空 slot visible pairing：约 300 秒。
- remembered unconnected slots fast reconnect：约 5 秒，interval 约 20 ms。
- boot 时 remembered unconnected slots slow reconnect：约 180-300 秒，interval 约 1 秒。

### Slot 短按

如果选中的 slot 为空：

```text
ShortPress(BTn) -> select slot n -> PairingVisible
```

如果选中的 slot 已记住主机：

```text
ShortPress(BTn) -> select slot n -> already connected OR HiddenReconnectFast/Slow for slot n
```

必需行为：

- 对 remembered slot 不做公开配对 fallback。
- 如果选中的 slot 已经 connected，切换只是 routing change。
- 其他 connected slots 保持 connected。
- 用户必须长按清除该 slot 后，才能把新主机配对到该 slot。

### Slot 长按

```text
LongPress(BTn, 5s) -> clear slot n -> select slot n -> PairingVisible
```

必需行为：

- 当用户知道目标 slot 时，这是恢复路径。
- 只断开/清除 slot `n`；其他 connected slots 保持 connected。
- 成功长按后的 release 被消费。

### Clear Current 长按

```text
LongPress(Clear BT, 5s) -> clear current slot -> PairingVisible
```

必需行为：

- 当用户不记得当前 active profile 时，这是恢复路径。
- 短按不得清除 bond。
- 成功长按后的 release 被消费。

### 意外断开

```text
Connected -> unexpected disconnect -> HiddenReconnectFast -> HiddenReconnectSlow -> Sleep
```

必需行为：

- 普通 link loss 不使用 HDDA。
- 不要因为 remembered host 断开，就变成公开可配对。
- 某个 slot 断开不得 tear down 其他 connected slots。

### 从 Sleep 唤醒

如果 active slot 为空：

```text
Sleep + any key -> PairingVisible
```

如果 active slot 已记住主机：

```text
Sleep + any key -> if active slot connected, route input; otherwise optional one HDDA burst -> HiddenReconnectFast/Slow
```

必需行为：

- 任意键都可以唤醒。
- 唤醒输入只应在短暂且安全的重连窗口内保留。

## 首个唤醒 Report 策略

用户预期：

- 如果某个按键唤醒键盘后主机立即重连，这个按键通常应该生效。
- 如果重连耗时太久，这个按键不应该在几秒后突然出现。

实现方向：

- 增加一个很小的 BLE-only pending report cache，独立于 `BLE_REPORT_CHANNEL`。
- 只在 `Sleep + user input` reconnect 时启用。
- 只存储那些本来应该发往 active BLE slot、但该 slot 尚未 connected 的 reports。
- 不要重复存储已经成功路由到 USB 的 reports。
- 存储前几个 reports 的有界 ring，例如 4 个 reports，并带 timestamp 和 slot/profile generation。
- active slot encryption 完成后、该 slot 正常 BLE report forwarding 前，flush 一次。
- 在以下情况丢弃 cache：
  - active profile 改变，
  - slot 被清除，
  - visible pairing 开始，
  - identity validation 失败，
  - reconnect 超时，
  - USB 变成 active transport，
  - TTL 过期。

建议 TTL：

- 约为 HDDA window 加上一点 margin，例如 2 秒。

这不能变成通用离线 typeahead buffer。

## 当前本地状态

仓库目前基本仍处于 plan 前的 BLE profile 实现：

- `rmk/src/ble/profile.rs` 中的 `BleProfileAction::ClearBond` 没有 slot 参数。
- `UPDATED_CCCD_TABLE` 只携带 raw CCCD table bytes，并在保存时更新当前 active profile。
- `rmk/src/keyboard.rs` 发送旧形态的 `BleProfileAction::ClearBond`。
- `rmk/src/ble/mod.rs` 只有单一的 visible、bondable、undirected advertising path。
- 现有文档把 `User0..User(N-1)` 描述为 profile switching，把 `User(N+2)` 描述为 clear current profile。
- 现有 Vial 示例标题使用了类似 "Bluetooth Channel 0" 和 "Clear bond info for current channel" 的标签。

第一个实现步骤应该引入面向用户的 action model 和 slot-aware save model，然后在一个保持可编译的步骤中更新所有 call sites。

## 代码修改计划

### 1. 复用现有 Profile Action 形状

文件：

- `rmk/src/ble/profile.rs`
- `rmk/src/keyboard.rs`
- `rmk/src/ble/mod.rs`

任务：

- 让 `BleProfileAction` 尽量接近当前设计：
  - 保留 `Switch(u8)`，
  - 保留 `Previous`，
  - 保留 `Next`，
  - 保留现有 `ClearBond` 作为 current-slot clear action，
  - 只为 `BTn` 长按新增 `ClearAndSwitch(u8)`。
- 除非实现证明 current-slot `ClearBond` 形状不安全，否则不要同时引入 `ClearBond(slot)` 和 `ClearCurrentAndPair`。
- 将 `ClearBond` 的用户可见效果定义为：清除当前 active slot，保持它为 active，然后进入 visible pairing。
- 将 `ClearAndSwitch(slot)` 的用户可见效果定义为：清除 `slot`，切换到 `slot`，然后进入 visible pairing。
- slot 实际清除后，更新 storage/profile cache；如果 profile/state 改变，则 publish 对应的 `ConnectionStatusChangeEvent`。
- 在 connection 被 accepted 时捕获 slot/profile generation。
- 给 CCCD updates 和 bond save events 标记 accepted connection 的 slot/generation。
- connection 已经 accepted 后，避免在 GATT save paths 中使用 `current_profile()`。
- 将 profile data saving 和 idle stack bond injection 拆开。
- bond save 和 CCCD save 可以在 connected 时更新 cache/flash，但不得切换 active slot。
- 维护内部 slot -> connection table，让多个 accepted connections 可以并发服务。
- 一个 connected slot 拥有自己的 GATT/HID writer state、CCCD state 和 connection task。
- HID input reports 只发送给 active slot 的 connected HID writer。不要广播给所有 connected hosts。
- Host output reports，例如 LED state，按 slot 追踪；用户可见状态默认跟随 active slot，除非某个键盘明确选择其他策略。

### 2. 实现长按用户动作

文件：

- `rmk/src/keyboard.rs`

任务：

- 对 `User0..User(NUM_BLE_PROFILE - 1)`：
  - press 时启动 5 秒 long-press wait。
  - 如果 timeout 前发生 release，发送 `Switch(slot)`。
  - 如果 timeout 先到，发送 `ClearAndSwitch(slot)`。
  - 记住这次 key press 已被 long press 消费。
  - long press 后 release 时，抑制普通 short-press switch。
  - 如果 timeout 前来了另一个 key event，把它 push 到 `unprocessed_events` 并保持正常行为。
- 对 `User(NUM_BLE_PROFILE + 2)` / `Clear BT`：
  - press 时启动 5 秒 long-press wait。
  - 如果 timeout 前发生 release，不执行破坏性动作。
  - 如果 timeout 先到，发送现有 `BleProfileAction::ClearBond`。
  - `ClearBond` 清除当前 active slot 并进入 visible pairing。
  - 成功 long press 后抑制 release。
- 保持 `Next`、`Previous`、output toggle 和 split clear peer 行为兼容。
- 保持 split peer clearing 和 host BLE slot clearing 分离。

### 3. 增加 Multi-Connection Advertising And Accept Loop

文件：

- `rmk/src/ble/mod.rs`

任务：

- 用 mode-aware advertising 替换单一 `advertise()` path。
- BLE loop 必须能在继续服务已有 slot connections 的同时，为另一个 empty 或 remembered slot advertising，前提是 controller/stack 支持 connected 时 advertising。
- 不要新增 public advertising mode types。
- 优先在 `rmk/src/ble/mod.rs` 内使用 private helper functions，例如：
  - visible pairing advertising，
  - hidden reconnect advertising，
  - optional one-shot HDDA wake burst。
- 如果 private enum 能让 loop 更容易写，可以保留 file-local 且 minimal；它不应该变成 public event/status type。
- advertising 开始前 snapshot active slot 和 active bond。
- 根据用户意图选择 advertising sequence：
  - empty active slot -> visible pairing，
  - remembered unconnected slots on startup -> hidden reconnect，
  - remembered selected slot on slot switch -> 如果尚未 connected，则 hidden reconnect，
  - remembered slot on link loss -> 为该 slot hidden reconnect，同时其他 slots 保持 connected，
  - remembered active slot on user wake -> 如果 disconnected，则 optional one HDDA burst, then hidden reconnect，
  - cleared slot -> visible pairing。
- 不要在 startup、clear-and-pair、host-forgot recovery 或普通 link loss 中使用 HDDA。
- profile 或 coarse BLE state 改变时，通过 `set_ble_profile()` / `set_ble_state()` publish 现有 `ConnectionStatusChangeEvent`。
- 当所有有用目标都 connected，且没有被选中的 empty/cleared slot 正在等待 pairing 时，停止 advertising。

### 4. 增加 Filter Accept List 处理

文件：

- `rmk/src/ble/mod.rs`

任务：

- 增加 `Peripheral::set_filter_accept_list()` 所需的 controller command bounds：
  - `LeClearFilterAcceptList`
  - `LeAddDeviceToFilterAcceptList`
- visible pairing advertising 前：
  - 清空 FAL，
  - 使用 `AdvFilterPolicy::Unfiltered`。
- hidden reconnect advertising 前：
  - 将 FAL 设置为未 connected remembered slots 的 peer identity addresses，受 controller capacity 限制，
  - 使用 `AdvFilterPolicy::FilterConnAndScan`。
- hidden advertising payload 只保留 flags。
- 为 hidden advertising 显式清空 scan response data。
- 如果 `trouble-host` 会跳过 zero-length scan response commands，不要依赖给 `Peripheral::advertise()` 传 `scan_data: &[]`。
- 增加显式清空 scan response 所需的 command bound：
  - `LeSetScanResponseData`
- 让 visible advertising payload construction 能应对 31-byte legacy advertising 限制。

### 5. 强制 Bondable 状态和 Peer Validation

文件：

- `rmk/src/ble/mod.rs`

任务：

- Visible pairing：
  - `conn.raw().set_bondable(true)`。
- Hidden reconnect 和 HDDA：
  - `conn.raw().set_bondable(false)`。
  - 根据这次 advertising 捕获的 active bond 检查 accepted peer identity。
  - 如果 mismatch：
    - log warning，
    - 立即 disconnect，
    - 返回 reconnect advertising。
- 在以下操作前做 identity check：
  - 恢复 CCCD table，
  - 更新 PHY，
  - 运行 GATT task，
  - 启动 HID writer task。

### 6. 保持 Profile、Advertising 和 Connection State 一致

文件：

- `rmk/src/ble/profile.rs`
- `rmk/src/ble/mod.rs`

Profile switch：

```text
receive Switch/Next/Previous
update active output slot
if selected slot is connected: route reports to it
if selected slot is empty: make it the visible pairing target
if selected slot is remembered but disconnected: include it in reconnect advertising
```

Clear target slot：

```text
receive ClearBond or ClearAndSwitch(slot)
resolve target slot
disconnect only that slot if connected
wait for that slot's connection task to finish
clear that slot's bond/cache/CCCD data
make that slot the visible pairing target
keep other slot connections running
```

任务：

- Profile switch actions 不应该断开其他 slots。它们只改变 active output slot，并可能为被选中的 slot 启动 reconnect/pairing。
- Clear actions 只在目标 slot connected 时断开目标 slot，然后清除该 slot 的 storage/cache，并让该 slot 进入 visible pairing。
- `ProfileManager::add_profile_info()` 不得以会 invalidate 其他 active slot connections 的方式重写 global stack bond state。
- Stack bond/security state 必须支持所有当前 connected slots 加 reconnect candidates，或者只以不会影响这些连接的方式更新。
- connection 引发的所有 save 和 clear 决策都必须使用捕获的 slot/generation，而不是 event handling 时的 current profile。

### 7. 处理 BondLost

文件：

- `rmk/src/ble/mod.rs`

任务：

- 将 `gatt_events_task()` 改成返回 connection-end reason，而不只是 `Result<(), Error>`。
- 在 `GattConnectionEvent::BondLost` 上：
  - 使用 accept 时捕获的 slot/generation 返回 `ConnectionEnd::BondLost { slot, generation }`。
- BLE connection loop 处理 `BondLost`：
  - 如果该 slot 仍 connected，则 disconnect 该 slot 的 connection，
  - 仅当 generation 仍匹配时清除 accepted connection 的 slot，
  - log 这次 recovery，
  - 只让该 slot 进入 visible pairing。

这是唯一自动 host-forgot recovery 路径，因为它基于明确的 stack 证据。

### 8. 验证 Pairing Identity

文件：

- `rmk/src/ble/mod.rs`

任务：

- 在 `PairingComplete` 中检查返回的 bond。
- 只有 bond 具有能支持可靠 reconnect 的 identity data 时才保存：
  - public identity address，
  - random static identity address，
  - 带有 distributed peer IRK 的 resolvable private address。
- 不要把每个 random address 都当成稳定地址。
- 仔细按 `bt-hci`/`BdAddr` byte order 检查 random address type bits：
  - static random 的两个最高 random-address bits 已设置，
  - RPA 是 `01` pattern，
  - NRPA 是 `00` pattern。
- 没有 peer IRK 的 RPA/NRPA 不得保存为普通 hidden reconnect slot。
- 如果 pairing 缺少 usable identity：
  - 拒绝 profile save，
  - disconnect，
  - 返回 visible pairing，
  - log 一个可被 integrators 展示的原因。

如果 `trouble-host` 后续暴露 pairing key-distribution policy，把这部分从 post-pairing validation 移到 SMP negotiation。

### 9. 保留首个唤醒 Report

文件：

- `rmk/src/ble/mod.rs`
- `rmk/src/channel.rs`
- 可能还有 keyboard/report channel code

任务：

- 验证 wake key/pointing event 在 BLE reconnect 时是否仍会到达 report generation。
- 验证 report routing 是否会因为 BLE 直到 encrypted/connected 前都不是 active，而丢弃第一个 report。
- 如果第一个 report 可能丢失：
  - 在 report-routing boundary 附近增加 pending report cache，
  - 只为 user-input wake from sleep 启用，
  - 只在 BLE 是 intended transport 且 USB 没有接受该 report 时 cache，
  - 存储带 slot generation 和 timestamp 的有界 report ring，
  - encryption 完成后 flush 一次，
  - 在 timeout、profile switch、clear slot、identity mismatch、visible pairing 或 USB becoming active 时清空。

### 10. Simultaneous Multi-Host 的 Address Identity

Simultaneous multi-host support 是 phase 1 设计要求。

Phase 1 应该为键盘使用一个一致的 local BLE identity address，并在该 identity 下支持多个 central connections。

原因：

- 一个 bonded BLE HID device 的 advertised address、pairing identity、encryption/security manager state、resolving-list state 和 stored bonds 必须描述同一个 local identity。
- 只修改 advertised address 会制造 identity split：主机可能连接到一个 address，但 RMK 的 security/bond state 认为另一个 identity 是 active。
- 在 connections active 时切换一个全局 local static address，会破坏现有连接所依赖的假设。
- `trouble-host` 0.7 没有暴露安全的 public runtime API 来支持多个同时 local identities。

因此：

- phase 1 不按 profile 修改 local static address。
- 不要尝试通过只修改 advertised address 解决这个问题。
- 将 RMK BLE profiles 视为同一个 keyboard identity 下的 peer-host slots。
- 每个 slot 存储自己的 peer bond/identity/CCCD data。
- 每个 slot 维护独立 active connection state。

如果 stack/controller 支持 connected 时 advertising：

- 继续服务 connected slots。
- 为被选中的 empty/cleared slots 以 visible pairing mode advertising。
- 为 unconnected remembered slots 以 hidden reconnect mode advertising。

如果 stack/controller 不支持 connected 时 advertising：

- 仍然保留 multi-connection architecture。
- 文档记录该平台限制。
- 只有在 advertising 再次可用时才 reconnect/pair additional slots。

未来 per-slot local identities 只有在 RMK 同时支持多个 advertising/connection/security contexts，且每个 context 都有一致 local identity 时才有意义。这比修改 advertisement field 是更大的 stack capability。

### 11. 文档、Vial 标签和迁移说明

文件：

- `docs/` 下相关文档
- BLE 示例 `vial.json` 文件
- migration 或 release notes，如果存在

文档必须描述用户任务，而不是 BLE 内部机制：

- 首次配对。
- 配对另一台主机。
- 切回已配对主机。
- Host forgot keyboard recovery。
- 使用 `Clear BT` 做 current slot unknown recovery。
- Slot mapping unknown recovery。
- 任意键 sleep and wake。

必需用户可见规则：

- `BTn` 短按：使用该 slot。
- `BTn` 长按 5 秒：清除该 slot 并配对。
- `Clear BT` 长按 5 秒：清除当前 slot 并配对。
- 短按一个 remembered slot 不会让键盘为新配对而出现。
- 如果主机忘记了键盘，也要清除键盘 slot。
- 如果键盘超时休眠，按任意键唤醒。

示例 Vial titles：

- `BT0: tap to use, hold 5s to clear and pair`
- `BT1: tap to use, hold 5s to clear and pair`
- `BT2: tap to use, hold 5s to clear and pair`
- `Clear BT: hold 5s to clear current slot and pair`
- `Next BT: use next Bluetooth slot`
- `Prev BT: use previous Bluetooth slot`
- `Clear Split: hold 5s to forget split peer`

迁移说明：

- 从旧的 shared-address multi-profile 行为升级的用户，如果主机侧 Bluetooth state 混乱，可能需要清除并重新配对 BLE slots。
- Simultaneous multi-host support 在 phase 1 使用一个一致的 keyboard local identity。
- Per-slot peer bonds 是 RMK-side slots；它们不是分开的 local advertised identities。
- 不要在文档中暗示只修改 advertised address 就能创建独立 BLE profiles。

## 验证计划

### 用户场景测试

Test 1：首次配对

```text
fresh/empty active slot -> boot -> host Add Device shows named keyboard -> pair succeeds
```

预期：

- 用户不需要按 slot key 即可配对。
- 如果窗口超时，按任意键会重新开始 pairing visibility。

Test 2：使用已配对主机

```text
slot 0 paired to Host A -> short press BT0
```

预期：

- Host A 重连。
- 另一个主机不应该看到 slot 0 的普通具名可配对键盘。
- bond 不会被清除。

Test 3：使用空 slot 配对另一台主机

```text
slot 1 empty -> short press BT1 -> scan on Host B
```

预期：

- Host B 看到具名键盘。
- pairing 将 bond 保存到 slot 1。

Test 4：通过重用 slot 配对另一台主机

```text
slot 1 already remembered -> long press BT1 for 5s -> scan on Host B
```

预期：

- Slot 1 被清除。
- Slot 1 保持 selected。
- Host B 看到具名键盘并可以配对。
- long press 后 release 不会导致另一个 short-press action。

Test 5：主机忘记键盘，slot 已知

```text
Host A forgets keyboard -> long press BT0 for 5s -> scan on Host A
```

预期：

- 键盘变为可见并可配对。
- Host A 可以重新配对。

Test 6：主机忘记键盘，当前 slot 未知

```text
Host forgets keyboard -> user long presses Clear BT for 5s
```

预期：

- 当前 active slot 被清除。
- 键盘变为可见并可配对。
- 用户不需要知道当前 slot 是 BT0、BT1 还是 BT2。

Test 7：误短按安全性

```text
slot remembered -> short press BTn
slot remembered -> short press Clear BT
```

预期：

- `BTn` 短按只 switch/reconnect。
- `Clear BT` 短按不清除 bond。

Test 8：slot mapping unknown

```text
slots paired to different hosts -> short press BT0/BT1/BT2 in sequence
```

预期：

- 短按是安全的。
- 用户可以识别哪台主机会重连。

Test 9：sleep and wake

```text
pairing/reconnect window expires -> keyboard sleeps -> press any key
```

预期：

- Empty current slot 开始 visible pairing。
- Remembered current slot 开始 reconnect。
- wake report 只在 reconnect 于短 TTL 内完成时才发送。

Test 10：split clear separation

```text
split build exposes Clear BT and Clear Split
```

预期：

- `Clear BT` 清除 host BLE slot。
- `Clear Split` 清除 split peer。
- labels 和 docs 不暗示它们做同一件事。

Test 11：两个主机保持 connected

```text
slot 0 paired and connected to Host A -> slot 1 paired and connected to Host B -> short press BT0/BT1
```

预期：

- Host A 和 Host B 可以同时保持 connected。
- 短按 `BT0` 将 keyboard reports 路由到 Host A。
- 短按 `BT1` 将 keyboard reports 路由到 Host B。
- 切换 active slot 不会断开另一个 host。

### 技术 BLE 测试

Test 12：hidden reconnect visibility

```text
slot 0 paired to Host A -> scan from Host B while slot 0 active
```

预期：

- Host B 看不到 slot 0 的普通具名 HID 键盘。
- Host A 可以重连。

Test 13：unexpected disconnect

```text
connected -> force host sleep/range loss -> firmware sees disconnect
```

预期：

- 没有 HDDA burst。
- hidden reconnect fast 启动。
- hidden reconnect slow 跟随。
- 最终 sleep。

Test 14：wrong peer defense

```text
simulate unexpected accepted peer in reconnect mode
```

预期：

- Firmware 在 GATT/HID work 前 disconnect。
- Firmware log rejected peer。
- Firmware 恢复 hidden reconnect advertising。

Test 15：RPA host reconnect

```text
pair to a host that uses RPA -> wait for host address rotation -> reconnect
```

预期：

- 如果 IRK 已分发，reconnect 成功。
- 如果缺少 IRK 且需要 RPA，pairing 不会保存为普通 hidden reconnect slot。

Test 16：BondLost

```text
host deletes bond but still initiates a connection that reaches security handling
```

预期：

- `BondLost` 清除 accepted slot。
- Firmware log bond-lost recovery。
- Firmware 进入 visible pairing。

Test 17：multi-host report isolation

```text
Host A connected on slot 0 -> Host B connected on slot 1 -> type while BT0 active -> switch to BT1 -> type again
```

预期：

- `BT0` active 时输入的 reports 只发给 Host A。
- `BT1` active 时输入的 reports 只发给 Host B。
- Reports 不广播给所有 connected hosts。

### Build/Compile

从 `rmk/`：

```bash
cargo check --no-default-features --features=storage,async_matrix,_ble
cargo nextest run --no-default-features --features=split,vial,storage,async_matrix,_ble
```

如果 feature combinations 因无关平台原因失败，运行本地可编译的最接近 BLE/storage feature set，并记录 gap。

## 验收标准

实现可接受的条件：

- 用户可以这样解释 BLE keys：
  - `BTn` tap to use，
  - `BTn` hold to clear that slot and pair，
  - `Clear BT` hold to clear the current slot and pair。
- 短按 slot 永远不会清除 bond。
- 短按 `Clear BT` 永远不会清除 bond。
- 空 slot 或已清除 slot 都是可见且可配对的。
- remembered slots 会重连它们记住的主机，而不会在无关主机上显示为普通可配对键盘。
- 当 stack/controller 支持时，多个 remembered slots 可以同时 connected。
- active slot 决定 HID input routing；reports 不广播给所有 connected hosts。
- 切换 active slot 不会断开其他 connected slots。
- host-forgot recovery 可以通过长按已知的 `BTn` slot 或 `Clear BT` 完成。
- Boot 立即启动正确模式。
- timeout sleep 后，按任意键会唤醒当前 slot。
- first wake input 只在 reconnect 于短 TTL 内完成时 replay。
- hidden reconnect 使用 FAL 加软件 identity validation。
- identity mismatch 会立即 disconnect。
- `BondLost` 自动清除 accepted slot 并进入 visible pairing。
- Profile switching、bond injection、FAL changes 和 local identity state 不会以 invalidate 任意 active slot connection 的方式被修改。
- 文档和 Vial labels 描述用户操作，而不是内部 BLE modes。
- 当前 shared-address 限制和迁移预期有文档说明。
- Simultaneous multi-host behavior 在 phase 1 使用一个一致的 local BLE identity，不通过只修改 advertised address 来伪造 profiles。
