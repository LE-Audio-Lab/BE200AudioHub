# BE200AudioHub · Round 7：Control Semantic Migration + Product Data Plane / First Sound HIL Ready

> **时间范围**：2026-10-18，本网页会话至 chat migration checkpoint。  
> **工程 Round 状态**：**ACTIVE / NOT CLOSED**。本网页会话结束不等于 Round7 结束；下一 chat 继续同一 Round7。  
> **起点**：Round6 已完成 Review-2A/2B/2C/2D software + real BE200/Buds3 Pro HIL。  
> **当前终点**：正式 Control semantic path、transport-neutral StreamBinding、LocalStreamClock、valid LC3 Media producer、IsoTxBridge、logical HCI ISO lane 与 Round7 First Sound HIL CLI 已完成软件实现；**350/350 deterministic tests PASS，check/build PASS，working tree clean**。  
> **硬件口径**：本 Round7 的新 valid-LC3 product data plane **尚未执行最终 First Sound HIL**。不得提前声称 Round7 First Sound PASS。

---

## 1. 本轮目标与施工方向

Round6 将 HCI Host / async transport / CIG-CIS-ISO substrate 真机打穿后，Round7 不再向下扩 HCI 协议细节，而是从既有 HCI substrate 向上生长产品语义和数据面。

本轮实际形成两条逐步汇合的主线：

```text
Control semantic plane
existing bond
→ controller attach / bit32 / discovery / privacy
→ dual encrypted ACL
→ PACS / ASCS / CAS / CSIS
→ Codec
→ CIG
→ QoS
→ Enable
→ Create CIS
→ ISO Input Path
→ confirmed ASE STREAMING
→ HCI logical route ACK
→ StreamBinding

Product data plane
SyntheticToneSource
→ PCM 48 kHz / 10 ms
→ valid LC3 100 B/ear
→ EncodedStereoFrame(FrameId)
→ LocalStreamClock absolute grid
→ IntervalGroupScheduler
→ LogicalIsoTxBridge
→ HciLogicalIsoTx
→ shared bounded ISO lane
→ logical route resolve
→ CisRef
→ raw CIS handle only inside HCI Host
```

最终两条线在 `StreamBinding + logical HCI route` 处闭合。

---

## 2. 冻结边界再次落实

### 2.1 Control 与 HCI 的语义边界

Round7 正式实现了此前冻结但尚未落地的原则：

- Control 拥有 LE Audio semantic lifecycle；
- HCI Host 拥有 raw CIS handles、HCI ISO sequence、PB/fragmentation、controller credits 与 USB；
- Media / StreamClock / IsoTxBridge 不接触 raw CIS handle；
- `Number Of Completed Packets` 只释放 transport capacity，**永远不推进 media time**；
- Control 必须等 HCI logical route 的可靠 ACK 后，才能暴露最终 `StreamBinding`。

当前 `ControlPhase` 已实际覆盖：

```text
Idle
→ Attaching
→ ReadingControllerIdentity
→ EnsuringCisHostFeature
→ Starting/Discovering/StoppingDiscovery
→ ConfiguringPrivacy
→ Connecting/Securing member 0/1
→ SetConnected
→ DiscoveringAudioMember
→ AudioSetDiscovered
→ CodecPlanned
→ PreparingCodecAse / ConfiguringCodec L/R
→ CodecConfigured
→ IsoPlanned
→ ConfiguringCig
→ CigConfigured
→ QosPlanned
→ ConfiguringQos L/R
→ QosConfigured
→ EnablePlanned
→ Enabling L/R
→ Enabled
→ CreatingCis
→ CisEstablished
→ SettingUpIsoPath L/R
→ IsoPathsReady
→ AwaitingStreaming L/R
→ StreamingReady
→ BindingStreamRoute
→ StreamBoundReady
```

### 2.2 StreamBinding 必须 transport-neutral

正式 `StreamBinding` 只包含：

```text
SessionId
StreamId
LeAudioFormat
presentation_delay_us
StreamTopology::StereoSplit
```

不包含：

```text
ASE handle
CIG ID
CIS ID
raw CIS handle
HCI ISO sequence
controller credit
USB endpoint
```

Control 的 `begin_stream_handoff(session_id, stream_id)` 先发可靠 `BindStreamRoute`，收到 `StreamRouteState::Bound` 后才进入 `StreamBoundReady`，此时 `stream_binding()` 才允许成功。

---

## 3. C1：正式 Control 连接 / security path

第一段 Control 从已有 HCI typed surface 向上构建，不复制 HCI primitive：

```text
Attach
→ Controller identity guard
→ CIS Host Support bit32
→ passive scan
→ host-side stored-IRK RPA resolve
→ 2/2 connectable member gate
→ controller RL / privacy
→ FAL ACL member 0
→ stored-LTK member 0
→ FAL ACL member 1
→ stored-LTK member 1
→ encrypted ConnectionSet projection
→ SetConnected
```

重要 policy：

- 每个 stored identity 的扫描候选只看 connectable observation；
- 同一 identity 命中多个当前地址时取 strongest RSSI；
- 两个 set member 不允许解析到同一 transport address；
- timeout/cancel 不在 HCI worker 内 sleep；
- connect timeout 先 Cancel FAL；
- 已有 ACL/security failure 走反向清理，不直接把脏 controller/privacy state留给上层。

---

## 4. C2：PACS / ASCS / CAS / CSIS discovery

每个 member 使用独立异步 `AudioDiscoveryMachine`，总 Control 只编排 member 顺序。

路径：

```text
Exchange MTU
→ Discover Services
→ PACS characteristics
→ Sink PAC
→ Sink Audio Location
→ Available Audio Contexts
→ Supported Audio Contexts
→ ASCS characteristics
→ read every Sink ASE
→ CAS Included Service
→ CSIS
→ AudioMemberSnapshot
```

产品 snapshot 保存 semantic information，而不是底层对象引用。

### Buds3 Pro 已固化的真实 quirk

每个 member 实际有 **2 个 Sink ASE**。产品规则继承 sealed R5 / Round6 HIL：

```text
enumerate all Sink ASE
→ ASE ID must be nonzero + unique
→ read state
→ among Idle Sink ASE choose minimum ASE ID
```

Idle ASE 对 Buds3 Pro 的 trailing-byte quirk 继续使用兼容 parser；非-Idle 状态仍走严格 Bumble parser。

---

## 5. C3：Codec / QoS / Enable semantic transactions

### 5.1 Codec planner

`StereoCodecPlan` 从 `AudioSetSnapshot` 决定：

- physical Left / Right 来自 PACS Sink Audio Location，不来自 member 顺序；
- LC3；
- 48,000 Hz；
- 10 ms；
- mono per ASE；
- 100 octets / codec frame / SDU；
- Media Context；
- lowest-ID Idle Sink ASE；
- presentation policy继续沿用已验证参数。

### 5.2 ASCS three-gate transaction

正式 `AseOperationMachine` 不以一次 ATT Write Response 作为 semantic success。

每个 Config Codec / Config QoS / Enable 操作必须同时满足：

```text
ATT Write Response
+
ASCS Control Point response = SUCCESS
+
ASE expected-state notification
        ↓
independent ReadValue readback
        ↓
strict semantic verification
```

三门槛可乱序到达；Control 有单独的 semantic operation timer，解决了“ATT write 已完成但 CP/ASE notification 尚未到达时没有 timeout owner”的空窗。

### 5.3 ASCS Release

为了正确 rollback，事务机被泛化为：

```text
expected notification state
!=
expected final readback state
```

Release 使用：

```text
notification = RELEASING
readback     = IDLE
```

因此 stream rollback 可复用同一个 typed ASCS transaction substrate。

---

## 6. CIG / CIS / ISO Path：raw handle 不上浮

### 6.1 StereoIsoPlan

Control 只规划 semantic identity：

```text
CIG ID 1
Left  CIS ID 1
Right CIS ID 2
10 ms SDU interval
100 B C→P
2M
RTN 13
95 ms max transport latency
```

HCI Set CIG 返回的 raw controller handles只用于 HCI内部验证；Control 不保存 raw CIS handle。

### 6.2 sanitized semantic CIS API

新增 `CisRef { cig_id, cis_id }` 与 semantic reports：

- `SemanticCisCreateState`
- `SemanticCisDisconnectState`

raw `CisCreateState / CisDisconnectState` 继续保留给 sealed Review-2 HIL oracle。

HCI Engine 内部解析：

```text
CisRef
→ current established CIS
→ raw handle
```

raw handle 不进入 Control。

### 6.3 semantic ISO Path API

同样增加：

- `SetupIsoInputPathByRef { cis }`
- `RemoveIsoPathByRef { cis, directions }`
- `SemanticIsoPathState`

Engine regression 已证明：

```text
CisRef {1,1}
→ HCI Host内部 resolve
→ raw 0x0900
→ HCI LE Setup ISO Data Path
```

而 Control/report surface 仍只有 `CisRef`。

---

## 7. Streaming gate

sealed Review-2D / HIL 证明的实际顺序是：

```text
Enable
→ Create CIS
→ Setup ISO Input Path
→ ASE ENABLING 自动进入 STREAMING
→ 才能发送 ISO media
```

产品实现新增非阻塞 `StreamingGateMachine`：

```text
Read ASE
→ ENABLING
→ RetryLater
→ Core-owned probe timer
→ Read again
→ STREAMING notification 可提前触发 readback
→ final ReadValue must confirm STREAMING
```

STREAMING 最终验证不仅看 state byte，还验证：

- ASE ID；
- CIG ID；
- CIS ID；
- Media Context metadata。

semantic deadline / GATT timeout 都会进入完整 stream rollback。

---

## 8. 正式 rollback 顺序

Round7 将 Control rollback 从简单 ACL/privacy cleanup 提升为完整 LE Audio reverse lifecycle。

已知 stream state 的正常失败清理顺序：

```text
logical stream route unbind（若已 bound）
→ ISO Path Right / Left
→ CIS Right / Left
→ ASE Release Right / Left
→ Remove CIG
→ ACL Right / Left
→ clear controller privacy
```

关键 fail-closed policy：

- Create CIS timeout：controller真实状态无法证明时，不猜测 Remove CIG；
- ISO Path remove failure/timeout：path 可能仍 live，不继续断 CIS；
- CIS disconnect failure/timeout：不继续 Remove CIG；
- route cleanup timeout / failure：记录 cleanup failure，再进入后续已知安全 cleanup；
- GATT fixed-bearer timeout 继续先 CancelGatt，再 teardown。

---

## 9. HCI logical stream route

### 9.1 Route key

HCI Host 内部新增 session-fenced route：

```text
(SessionId, StreamId, AudioSide)
→ CisRef
→ raw CIS handle
```

必须包含 SessionId，防止新 session 复用 StreamId 时误命中旧 route。

### 9.2 bind policy

绑定前必须证明：

- Left/Right `CisRef` 不重复；
- 同一 CIG；
- 两个 CIS 当前 live；
- 两个 HCI Input Data Path 当前 ready。

exact replay 是幂等的；conflicting rebind fail-closed。

### 9.3 invalidation

route 在 teardown 时不会变成 stale pointer：

- Input ISO Path remove 会 invalidate 相关 logical route；
- CIS retirement/disconnect 会 invalidate引用该 CIS 的 route；
- worker stale logical ISO classification与 teardown互不耦合。

### 9.4 route ACK

`BindStreamRoute / UnbindStreamRoute` 通过 reliable control lane，并返回显式：

```text
StreamRouteState::Bound
StreamRouteState::Unbound { was_bound }
StreamRouteState::Failed
```

Control 只有收到正确 Bound ACK 后才暴露 `StreamBinding`。

---

## 10. logical ISO product data lane

### 10.1 handle-free request

产品 Media/Bridge 向 HCI 提交：

```text
LogicalIsoSduGroupRequest {
    session_id,
    stream_id,
    left,
    right,
}
```

不带 raw handle / CIS ID。

HCI Host 在共享 bounded ISO lane 内执行：

```text
logical request
→ route lookup
→ Left/Right CisRef
→ live route/path validation
→ raw IsoSduGroupRequest
→ existing Review-2D admission
→ Bumble send_iso_sdu
```

旧 raw `IsoSduGroupRequest` 与新 logical request 共用同一 bounded lane / worker fairness / controller admission，不创建第二条绕过 Review-2D correctness gate 的数据面。

### 10.2 stale request policy

如果 route/path已 teardown，未进入 controller 的 stale logical payload不会反向破坏 teardown。

该约束通过：

- logical stale/drain classification tests；
- teardown-independent stale ISO handling；
- route invalidation regression

固定。

---

## 11. Product Media：Synthetic PCM → valid LC3

### 11.1 SyntheticToneSource

当前 First Sound fixture：

```text
48 kHz
10 ms / frame
480 samples
Left  = 400 Hz
Right = 600 Hz
peak ≈ -50 dBFS
100 ms fade-in
100 ms fade-out
default 100 frames ≈ 1 s
```

`FrameId` 从 producer 开始拥有并单调递增。

### 11.2 LC3 encoder

新增正式 `Lc3StereoEncoder`，复用 pinned `bumble-codecs`：

- persistent mono encoder per ear；
- 48 kHz；
- 10 ms；
- 100 B / ear；
- 1 codec frame / SDU；
- output = `EncodedStereoFrame<Vec<u8>>`；
- 无 Bluetooth / CIS / HCI / USB / credit / scheduler dependency。

测试包括真实 LC3 decode roundtrip：

- Left 400 Hz energy显著高于 600 Hz；
- Right 600 Hz energy显著高于 400 Hz；
- exact 100 B / ear；
- FrameId不丢失。

同时生成合法 LC3 silence fallback，并验证 decode near-zero。

---

## 12. LocalStreamClock

Round7 初始产品 clock 使用本地 monotonic absolute grid，不等待 controller completion。

核心 contract：

```text
epoch_us
period = 10,000 us
FrameId N
→ target = epoch + N * 10,000 us
```

callback jitter 不改变 logical grid。

时间跳跃时：

- old FrameId 成为 stale；
- scheduler直接跳到当前 absolute interval；
- 不通过延长 period 来“追赶”。

controller-anchored clock仍是未来扩展点，不是本轮 scope。

---

## 13. IntervalGroupScheduler / LogicalIsoTxBridge

`IntervalGroupScheduler`：

- deterministic / I/O-free；
- bounded frame slots；
- exact FrameId scheduling；
- stale frame drop；
- missing target frame使用 valid-LC3 fallback；
- Left/Right作为一个完整 stereo interval group。

`LogicalIsoTxBridge` 是 scheduler 到 HCI Host 的非阻塞 runtime edge。

### 已冻结 backpressure policy

```text
HCI logical ISO bounded lane Full
→ DROP 当前完整 stereo interval
→ 不等待
→ 不 retry
→ 不改变 media clock
→ 下一 FrameId / deadline照常继续
```

也就是说：

> transport backpressure永远不能成为 media time。

对正常产品 runtime，drop 是允许的 realtime policy；对当前 First Sound HIL，任何 drop 都直接判 HIL failure，因为 acceptance要求指定的 100 个 intervals 全部提交。

---

## 14. ControlAction → HCI thin dispatcher

新增 `src/control/dispatch.rs`：

- Control状态机仍是纯 reducer；
- dispatcher不拥有 Control state；
- reliable HCI actions nonblocking提交给 `HciControl`；
- `ArmTimer/DisarmTimer` 只返回给未来 Core timer owner；
- reliable control lane Full 是显式 error，不 silently drop。

因此未来 Core 不需要重新写一套 Control→HCI mapping。

---

## 15. Round7 valid-LC3 First Sound HIL harness

### 15.1 Scope

新命令：

```text
round7 first-sound
```

setup/teardown继续复用 sealed Review-2D HIL oracle，但数据面替换为当前产品路径。

真实链：

```text
sealed Review-2D setup oracle
→ dual ASE STREAMING
→ bind logical stream route
  SessionId(7) / StreamId(11)
→ SyntheticToneSource
→ valid LC3
→ LocalStreamClock
→ LogicalIsoTxBridge
→ HciLogicalIsoTx
→ shared bounded ISO lane
→ logical route resolve
→ CisRef
→ raw CIS handle inside HCI Host
→ BE200
→ Buds3 Pro
```

### 15.2 Acceptance

HIL要求：

- no scheduler skipped interval；
- no stale frame drop；
- no HCI logical lane backpressure drop；
- exact `frames × 2` ISO packets submitted；
- exact `frames × 2` completed；
- final in-flight = 0；
- logical route unbind confirmed；
- ISO drain confirmed；
- reverse teardown PASS；
- privacy cleanup true；
- BondStore fingerprint unchanged。

### 15.3 CLI

Windows repo root：

```powershell
$fw = 'C:\STanJK\Development\LEAudioLab\BE200\firmware'
$bond = Join-Path $env:LOCALAPPDATA 'BE200AudioHub\bonding'

.\target\debug\be200hub.exe `
    round7 first-sound `
    --sfi (Join-Path $fw 'be200_intel_windows_24.60.0.4_usb_verified.sfi') `
    --ddc (Join-Path $fw 'ibt-0291-0291.ddc') `
    --bond-root $bond `
    --name "Galaxy Buds3 Pro (2F23)" `
    --connect-seconds 12 `
    --frames 100 `
    --confirm-first-sound-hil
```

专门的 consent flag 不能用 Review-2D 的 `--confirm-iso-hil` 代替。

**安全要求**：第一次把 Buds3 Pro 拿在手上，不佩戴。-50 dBFS 是数字幅度，不是校准后的实际 SPL 保证。

### 15.4 当前状态

**HIL READY / NOT YET RUN。**

因此当前公开 claim 只能是：

> Round7 product Control + valid-LC3 Media/Clock/Bridge/logical-HCI data plane is software-complete and has a dedicated real-hardware First Sound HIL entry point.

不能写：

> Round7 First Sound HIL PASS.

---

## 16. Cargo / ToolHub 锁文件修复

加入 pinned LC3 dependency 后，`Cargo.lock` 曾持续出现 root dependency order 的 generated diff。

根因不是版本漂移，而是 dependency entry order 非 Cargo canonical output。

最终修复：

```text
remote Cargo.lock canonicalize
→ git-sync-generated
→ cargo test --workspace --locked
→ clean
```

最终相关提交：

- `7267164f0e` — add pinned Bumble LC3 codec dependency；
- `17298fc4df` — lock LC3 dependency chain；
- `57de8faed3` — normalize root Cargo.lock dependency order。

---

## 17. 关键 commit anchors

### Control semantic path

- `d156b17658` — nonblocking ASE streaming gate；
- `a66b58223f` — integrate dual-ear streaming gate；
- `de73d9880c` — transport-neutral StreamBinding；
- `8f5e0da0e1` — Control projects StreamBinding；
- `7268970fdb` — route-acknowledged stream handoff contract；
- `6b9b507e51` — HCI route ACK required before media handoff；
- `668b874566` / `2f290b3c13` — route-first rollback + cleanup ACK loop。

### HCI logical route / logical ISO lane

- `87ed38395e` — session-fenced logical stream route table；
- `06143bcdde` — Engine owns/validates logical routes；
- `f62a2fdb33` — invalidate routes when Input Path removed；
- `f7bbf18a8d` / `ef20c5defc` — explicit route ACK + public control API；
- `08a670adc8` — resolve logical ISO through live routes；
- `de0c71ee90` — unify raw/logical ISO lane envelope；
- `a408def78c` — shared bounded worker lane；
- `901cf4465b` — expose logical ISO product handle；
- `7287f2f14f` — teardown independent from stale ISO payload validation。

### Clock / Media / Bridge

- `f5141cbcc2` — LocalStreamClock core；
- `35a764c40f` — offline stereo LC3 encoder；
- `3119b66e9c` — valid LC3 silence fallback；
- `f1f827ff57` — pure interval-group scheduler；
- `2470a9d09e` — scheduled intervals → logical ISO groups；
- `c2008ec6da` — end-to-end LC3→Bridge→logical ISO deterministic test；
- `0adfc4295b` — Bridge runtime to logical HCI lane。

### Runtime edge / HIL

- `174b1d8310` — ControlAction→HciControl thin dispatcher；
- `a5578bc95a` — product LC3 HIL data-plane helpers；
- `44f15dbf1a` — selectable ProductLc3 / OpaqueReview2 mode；
- `d2b57012b6` — Round7 First Sound parser；
- `98d06b99b7` — expose Round7 First Sound CLI；
- `57de8faed3` — canonical Cargo.lock。

---

## 18. Final software gate at chat migration

当前 `main` / 本地重新实测：

```text
cargo test --workspace --locked
350 passed; 0 failed; 0 ignored

cargo check
PASS

cargo build
PASS

git status
## main...origin/main
```

warnings 主要来自尚未接入最终 executable/runtime composition 的 architecture scaffolds，以及 HIL helper 的一处 unused assignment；当前无编译错误。

---

## 19. 当前精确状态

```text
Round6
  Review-2A / 2B / 2C / 2D       ✅ COMPLETE + REAL HIL

Round7
  Control semantic path           ✅ SOFTWARE COMPLETE
  StreamBinding / route ACK       ✅ SOFTWARE COMPLETE
  LocalStreamClock                ✅ SOFTWARE COMPLETE
  Synthetic PCM                   ✅ SOFTWARE COMPLETE
  valid LC3 encoder               ✅ SOFTWARE COMPLETE
  IntervalGroupScheduler          ✅ SOFTWARE COMPLETE
  LogicalIsoTxBridge              ✅ SOFTWARE COMPLETE
  HCI logical ISO route/lane      ✅ SOFTWARE COMPLETE
  ControlAction dispatcher        ✅ SOFTWARE COMPLETE

  valid-LC3 First Sound HIL       🟡 READY / NOT YET RUN
  full Core/Lifecycle runtime     ⏳ NOT YET INTEGRATED
  egui frontend                   ⏳ NOT STARTED
  WASAPI / ASRC                   ⏳ FUTURE
```

---

## 20. Chat migration boundary / next chat

本网页会话在 **Round7 First Sound HIL 已准备完成但尚未执行** 时结束。

这只是 chat migration，不是 Engineering Round closure。

下一 chat 应直接从：

```text
run Round7 valid-LC3 First Sound HIL
        ↓
if PASS:
    record real HIL
    then integrate ControlMachine + dispatcher + Core timer owner
if FAIL:
    debug only the product data-plane / HIL boundary indicated by evidence
    do not reopen sealed Round6 HCI substrate without concrete evidence
```

开始。

不要回头重做：

- Review-2D raw ISO Host；
- Control semantic planners/FSM；
- logical route；
- LC3 producer；
- LocalStreamClock；
- interval scheduler；
- logical ISO lane。

除非新的 HIL evidence明确证明其中某一层存在问题。
