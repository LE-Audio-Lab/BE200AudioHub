# BE200AudioHub · Round 5：R5.3 Experiment 封板 → Review 架构冻结 → Runtime / Async Transport 落地

> **性质**：Round 5 技术总结、架构决定、实现推进与纠偏记录。  
> **时间范围**：2026-10-14—2026-10-15。  
> **主分支**：`main`。  
> **阶段变化**：本轮将项目从 Experiment/R5.3 的可行性验证阶段正式切换到 Review/Refactor 产品架构阶段。  
> **重要口径**：Experiment 已证明 BE200 + Galaxy Buds3 Pro 的 LE Audio First Sound 可行；Review 的目标不是重新证明协议可行性，而是把同步实验栈重构为可长期演化、可测试、可恢复、可实时运行的产品架构。

---

## 1. 本轮开始时的真实状态

Round4 历史归档截止于 R5.2-B：双耳 CIS 建立、Streaming、显式断开、Release、Remove CIG 已 HIL PASS，但尚未进入 ISO Data Path / LC3 First Sound。

Round5 开始前后的 Experiment/R5.3 已继续完成：

- Setup ISO Data Path / Remove ISO Data Path：PASS；
- LC3 48 kHz / 10 ms / 100-byte mono-per-ear：离线编码自测 PASS；
- 左耳 440 Hz、右耳 880 Hz 真实送入 Buds3 Pro，**已产生声学 First Sound**；
- 800/800 左右 SDU 可提交并完成；
- 生命周期 cleanup 可完成；
- 但持续播放存在明显 pitch glide / timing drift， nominal 8 s 音频曾需要约 9.9–10.8 s 才完成；
- 因此 R5.3 被定义为 **Experiment PASS with known realtime-quality limitation**，而不是产品级实时音频 PASS。

Experiment 最终调查基线：

`34f46809105567259f9614f648eadd0451c46b72`

提交：

`fix(r5): use event-only USB receive mode during ISO streaming`

该提交作为实验调查证据，不作为 Review 产品 transport 架构。

---

## 2. R5.3 最关键的根因判定

### 2.1 错误架构：媒体时钟被 controller credit 事件牵引

Experiment 早期 R5.3 将 source/media production 与 controller completion / credit service 紧密耦合。

直接后果：

- controller completion 抖动会改变下一帧生产时间；
- USB Event servicing 延迟会映射成可听 pitch modulation；
- nominal 10 ms frame grid 不再由独立媒体时间线保证。

本轮正式冻结原则：

> **Number Of Completed Packets 只释放 controller transport capacity；永远不能推进媒体时间。**

### 2.2 同步 USB endpoint round-robin 是第二个根因

旧 R5 USB transport 轮流同步读取：

```text
interrupt-IN Event
→ timeout / read
→ bulk-IN ACL
→ timeout / read
→ Event
→ ACL
...
```

这使得：

- HCI Event completion 可能被 ACL read timeout 延迟；
- controller credit 回收延迟；
- ISO submission 出现非媒体时钟导致的 burst / starvation；
- 调低 ACL timeout 后 nominal 8 s 发送时间明显改善，进一步证明 transport servicing 是主要因素。

Review 决定：

> **独立 RX endpoint 必须同时 pending，禁止再次出现 synchronous round-robin read loop。**

### 2.3 Bumble-RS TX Sync parser quirk

固定 Bumble-RS revision 中 `LE Read ISO TX Sync` 返回解析存在字段宽度问题：

- Core `Time_Offset` 是 24-bit；
- pinned parser 读取 `u32`；
- BE200 的合法 12-byte return 触发 `need 4 at offset 9, have 12`。

决定：

- Experiment 后续不依赖该坏 parser；
- Review 如果启用 controller-anchored clock，必须在 adapter/test 层处理或修补；
- 不能把 parser failure 误判为 controller 不支持 TX Sync。

---

## 3. Experiment 封板与仓库重构

决定不再继续扩展 R1–R5 的同步实验架构。

Experiment 完整源码冻结到：

`deprecated/r5/`

关键归档提交：

- `4d9afb4` — freeze R5 snapshot under `deprecated/r5`
- `60487e3` — establish Review architecture scaffold
- `c3adabe` — organize Review docs / design principles
- `35c0a3a` — update project README for Review architecture
- `d4800e7` — freeze execution-context and mailbox contracts

原则：

- `deprecated/r5` 是实验事实和 HIL 证据；
- Review 不在实验代码上继续打补丁；
- 需要 Buds/Intel quirk 时回看 Experiment；
- 不允许把 R5 的同步 USB、credit-driven source timing 重新带回产品路径。

---

## 4. Review 总体架构冻结

### 4.1 L0 — LifeCycle / Orchestration

拥有：

- desired state；
- generation；
- session；
- startup / teardown ordering；
- retry / backoff；
- recovery escalation。

用户/UI 只向 L0 表达 desired state / intents。

### 4.2 L1 — 三个 Plane

1. **Control Plane**
2. **Media Plane**
3. **HCI Host Plane**

额外两个跨边界组件：

- **IsoTxBridge**：Media → HCI 的明确桥；
- **StreamClock**：session-scoped authoritative media timeline。

### 4.3 L2 — Transport Plane

只负责：

- HCI transport packet classes；
- packet boundary / reassembly；
- cancellation；
- transport health；
- bounded packet delivery。

不理解 Bluetooth/LE Audio semantic state。

### 4.4 L3 — Platform / Device Backend

只负责：

- Windows/libusb/WinUSB；
- BE200 device/interface；
- endpoint；
- raw asynchronous transfers；
- Intel bootstrap/device quirks。

不理解 HCI semantics。

---

## 5. RuntimeState / Control ownership

冻结为：

> **shared-readable, single-writer**

只有 Control reducer 写 `RuntimeState`。

Media / HCI / Transport：

- 不共享可变 RuntimeState；
- 发送 typed report / event；
- 高频 telemetry 在进入 Core 前聚合。

RuntimeState 是世界模型/观测投影，不是各 Plane 内部对象本身。

---

## 6. Media / StreamClock 决定

### 6.1 Media pipeline

```text
WASAPI Process Loopback
→ PCM source ring
→ clock estimator / ASRC
→ channel mapping
→ LC3 L/R
→ EncodedStereoFrame(FrameId)
```

Media 不知道：

- CIS Handle；
- HCI credits；
- USB endpoint；
- HCI ISO sequence。

### 6.2 Authoritative StreamClock

每个 active stream group/session 只有一个权威 10 ms grid：

```text
Frame N ↔ target instant N
```

初始模式：

- local monotonic/QPC。

未来模式：

- controller-anchored ISO transmit grid。

ASRC：

- 适配 Windows source clock 到 target StreamClock；
- **不是 StreamClock 本身**。

### 6.3 Media → HCI contract

Media 产出：

```rust
EncodedStereoFrame {
    frame_id,
    left,
    right,
}
```

所有 payload 对 Bridge/HCI 都是 opaque。

所有权冻结：

- Media owns `FrameId`
- HCI Host owns HCI ISO sequence / PB / fragmentation / credits
- `FrameId != HCI ISO sequence`

---

## 7. IsoTxBridge 决定

IsoTxBridge 独立成显式组件，不硬塞进 Media 或 HCI。

初始 scheduler：

**Interval Group Scheduler**

参考：

- PipeWire；
- Espressif；
- nRF Audio。

职责：

- 跟随同一个 StreamClock；
- 每 interval 原子处理左右耳；
- bounded encoded-frame backlog；
- stale drop；
- missed frame fallback；
- 非阻塞提交 HCI Host；
- 不理解 LC3。

未来可以仅替换 scheduler 为：

**Controller-Anchored Scheduler**

而不改变 Media/HCI Plane ownership。

---

## 8. Execution Context / 线程模型冻结

采用 AOSP Handler 思路：

> 模块依赖 serialized execution context，而不是把 `thread::spawn` 写进架构边界。

逻辑 Context：

- CoreContext
- HciContext
- MediaRtContext
- CaptureContext
- TransportContext

初始物理映射为 5 个线程：

1. CoreThread
2. HciHostThread
3. MediaRtThread
4. WASAPI CaptureThread
5. UsbReactorThread

以后可 profiling 后合并/重映射线程，但 ownership 不改变。

---

## 9. Execution / mailbox contract

Review contract 冻结于：

`docs/design/EXECUTION_MODEL.md`

关键规则：

- 每份 mutable subsystem state 只有一个 owner；
- 禁止为方便直接 `Arc<Mutex<SubsystemState>>` 多 owner；
- 所有 runtime queue bounded；
- state-changing traffic 不能 silent drop；
- cross-context request/reply 异步 correlation；
- timeout 是 requester-owned timer event；
- generation/session/stream scoped message 到达时验证；
- stale callback 只统计，不改变当前状态；
- Media RT hot path 不做 HCI/USB/GATT/log/filesystem/general mutex；
- USB completion callback 不调用 Bumble；
- HCI Event 不得被 ISO TX 饿死；
- stop/reset/teardown 不得被 data backlog 饿死。

---

## 10. Review-1 — Runtime Kernel 完成

新 Runtime Kernel 已实现并以软件测试封板。

包括：

- GenerationGate；
- CancellationToken；
- bounded nonblocking mailbox；
- serial execution context；
- single-owner mutable state；
- request/reply correlation；
- supervisor fault channel；
- panic → fault；
- idle context stop wakeup。

这部分定义成产品基础设施，后续只有在：

- HIL 证明 contract 错误；
- profiling 证明 deployment policy 不合理；
- 新 Plane 引入新 invariant；

时才重新打开。

---

## 11. Review-1 — L3/L2 Async Transport 完成

### 11.1 deterministic TestBackend

先以纯软件模型证明：

- Event / ACL endpoint 可同时 pending；
- completion 顺序由 arrival 决定；
- cancel / detach 明确传播；
- TX class 映射明确。

### 11.2 HCI packet transport

实现：

- Event / ACL / ISO packet classes；
- partial read preservation；
- multi-packet transfer split；
- impossible declared length rejection；
- endpoint-local reassembly。

### 11.3 async libusb backend

Windows BE200 backend 使用：

`rusb::ffi / libusb async transfer`

映射：

| HCI traffic | USB |
|---|---|
| Command | class control OUT |
| Event | interrupt-IN |
| ACL RX | bulk-IN |
| ACL TX | bulk-OUT |
| ISO TX | bulk-OUT |

### 11.4 callback ownership

中间实现曾出现危险结构：

- callback 直接 free；
- backend 同时保留 raw transfer pointer map。

本轮纠正为：

> backend pending table 是唯一 owner；callback 只记录 terminal completion；TransportContext 消费 completion 后统一 reclaim/reuse。

避免 cancel/completion race 与 UAF。

### 11.5 persistent transfer reuse

为减少高频 malloc/free 抖动：

- 每个 RX endpoint 一个 reusable persistent transfer slot；
- write transfer 按 class bounded pool；
- idle write pool cap = 8 / class。

### 11.6 UsbReactor

UsbReactor：

- bounded Host TX budget；
- 再 service backend completion；
- cancellation 每 turn 可见；
- 不允许无限 TX drain 饿死 Event completion。

---

## 12. Review-1 最终验证

默认 Review build：

```text
cargo test --workspace
48 passed; 0 failed

cargo check --workspace
PASS

cargo build --workspace
PASS
```

Review-1 正式封板文档：

`docs/design/REVIEW1_RUNTIME_TRANSPORT.md`

依赖 lockfile 提交：

`8598c3a6b9877f43ab853fe63fcd981ee4c8c605`

`chore(review1): lock runtime and transport dependencies`

---

## 13. HCI Host WIP：已开工但未算 Review-2 PASS

本轮在 Review-1 之后提前探索了 HCI Host。

核心决定：

- 固定复用 Experiment 已 HIL 的 Bumble-RS revision：
  `bbac2a6803b8cab0920ab725a23aa408fc4fed85`
- **不复用 `ExternalHost`**
  - 它内部自己 spawn blocking reader；
  - `send_command()` 同步等待 completion；
  - 违反 HciContext contract。
- 直接复用：
  - Bumble `Device`
  - `HostTransport`
  - HCI/profile parser/state substrate。

已形成 WIP：

- QueueHostTransport；
- native USB HCI ↔ Bumble H4 adapter；
- HciEngine；
- resetless operational attach；
- controller capability snapshot；
- HCI worker lane 设计；
- passive extended scan transaction；
- bounded scan aggregation；
- HciStack composition。

但该代码仍有 synthetic attach/scan 测试未解决：

```text
cargo test --features hci-wip
74 tests total
68 pass
6 fail
```

失败集中在 synthetic minimal operational attach fixture 未正确推进到 `Ready`，而不是 Review-1 Runtime/Transport。

决定：

- HCI Host 暂时 gated behind Cargo feature：
  `hci-wip`
- 默认 main 只验收 Review-1；
- Review-2A 第一件事就是修绿 HCI WIP，再转正。

---

## 14. ToolHub 工作流本轮演进

### 14.1 cargo-test allowlist

新增：

```text
cargo.exe test --workspace
```

用于远程纯软件回归。

### 14.2 旧 busy lock 问题

此前同步 ToolHub operation 在 spawn/error 边界可能留下：

`Project ... already has a running operation`

本轮 ToolHub 改成异步 run registry：

- `local_build_run` → run_id
- `local_build_get_run`
- `local_build_list_runs`
- `local_build_cancel_run`

经本轮实际验证：

- run 可查询实时 stdout/stderr；
- completion 可读 exit_code；
- cancel 可终止 Windows process tree；
- 不再需要把“operation 启动”误当“operation 成功”。

### 14.3 Cargo TLS 失败

一次 `cargo test` 拉 Bumble/crates.io 时连续：

`schannel SSL/TLS handshake failed`

根因不是 Cargo/依赖，而是用户当时使用规则代理，Cargo 流量未正确代理。

切换全局代理后：

- Bumble-RS；
- crates.io dependencies；

正常拉取并进入 cache。

---

## 15. Function-level Reference Index 建立

本轮额外完成：

`docs/reference-index/`

共 15 份文档。

目标：

> 下一阶段不再反复大范围考古上游，而是从 BE200AudioHub contract 直接跳到 upstream 文件/函数/type 和对应 cargo-test invariant。

索引域包括：

- Runtime / execution；
- async USB transport；
- HCI Host；
- ACL/L2CAP/ATT/GATT；
- security/privacy；
- LE Audio Control；
- ISO data plane；
- Media/StreamClock/ASRC；
- Bridge scheduler；
- Windows audio；
- lifecycle/recovery；
- LC3；
- deterministic test strategy；
- source trust boundaries。

主要上游精读：

- AOSP Bluetooth；
- Bumble-RS；
- Zephyr；
- BTstack；
- libusb/rusb；
- PipeWire；
- BlueZ；
- Linux ISO；
- nRF Audio；
- Espressif；
- liblc3；
- LE-Audio-Relay。

规则：

- architecture contract 优先于 upstream layout；
- upstream 是 precedent/oracle，不是 ownership authority；
- 能 fake 外部世界的逻辑必须优先 Cargo-test 约束；
- HIL 只验证 Windows/USB/controller/earbuds/real-clock/acoustic assumptions。

---

## 16. Timeline 归档结构恢复

Review 初期曾只保留：

`docs/history/`

而原 Experiment 的完整 Timeline 位于：

`deprecated/r5/docs/Timeline/`

本轮恢复为活动目录：

`docs/Timeline/`

恢复文件：

- README.md
- TIMELINE.md
- BUG_FIX_REGISTER.md
- AllRounds.md
- Round1.md
- Round2.md
- Round3.md
- Round4.md

复制方式：

- 直接复用 Git blob SHA；
- Round1–4 等文件字节级保持历史原样；
- immutable 原副本仍留在 `deprecated/r5/docs/Timeline/`。

新规则：

- `deprecated/r5/docs/Timeline` = frozen archive；
- `docs/Timeline` = active continuation；
- 新阶段从本文件 `Round5.md` 开始追加；
- 不回写旧 Round 原文。

---

## 17. 当前项目状态

```text
Experiment
R1–R5.3 feasibility                     ✅ SEALED
  └─ real Buds3 Pro First Sound proven

Review architecture contract             ✅
Execution model                          ✅

Review-1
Runtime Kernel                           ✅ COMPLETE
Async L3/L2 Transport                    ✅ COMPLETE

Review-2
HCI Host Plane                           🟡 WIP
  2A attach / capability / scan          🟡 synthetic tests pending
  2B ACL / stored security               ⬜
  2C ATT / GATT                          ⬜
  2D ISO Host / credits / fragmentation  ⬜

Control Plane migration                  ⬜
StreamClock + IsoTxBridge                ⬜
Synthetic realtime First Sound           ⬜
WASAPI + ASRC                            ⬜
Lifecycle / Recovery                     ⬜
Profiling / HIL hardening                ⬜
```

---

## 18. 下一轮最短接续路径

### Review-2A

先只处理：

```text
HCI Host resetless attach
→ controller capabilities
→ passive extended scan
```

验收：

`cargo test --features hci-wip`

全部通过。

然后：

- 去掉/收窄 WIP gate；
- HCI Host 成为默认 main 一等组件。

### Review-2B

迁移：

- LE ACL connect/disconnect；
- FAL / Resolving List；
- stored LTK encryption；
- dual ACL。

### Review-2C

迁移：

- ACL/L2CAP；
- ATT；
- GATT；
- encrypted service discovery。

### Review-2D

实现/迁移：

- CIG/CIS primitive；
- ISO Data Path primitive；
- ISO sequence；
- fragmentation；
- per-CIS queue；
- controller credit / completion。

完成后进入 Control semantic migration。

---

## 19. 本轮防回归原则

1. **不重新打开 Experiment 架构**：R5 同步 USB/credit-driven media 不再修补。
2. **不让 controller completion 当媒体时钟**。
3. **不让 USB callback 进入 Bumble/Control semantics**。
4. **不让 Media 知道 CIS/HCI credits**。
5. **不让 HCI Host 知道 PCM/ASRC/LC3 production timing**。
6. **不让 Control 构造 raw HCI command**。
7. **所有 mutable subsystem state 保持 single-owner**。
8. **所有 runtime queue bounded**。
9. **state-changing reliable traffic full = correctness fault，不 silent drop**。
10. **能够用 fake backend/fake clock/packet replay 约束的逻辑，先 Cargo-test，再 HIL**。
11. **HIL 失败必须尽量还原成软件 regression fixture**。
12. **任何新实现先查 `docs/reference-index/`，再决定是否需要重新精读 upstream。**

---

## 20. Round 5 结论

本轮真正完成的不是“又加了一些 LE Audio 功能”，而是完成了项目性质转换：

```text
能发声的同步 Experiment
        ↓
冻结问题来源
        ↓
明确 Plane / ownership / clock / execution contracts
        ↓
Runtime + Async Transport 产品底座
        ↓
建立函数级 upstream oracle / test index
        ↓
开始按层迁移 HCI / Control / Media
```

从此后新的功能实现不再以“能跑一次”为主要成功标准，而以：

- ownership 正确；
- timing responsibility 正确；
- bounded；
- deterministic-test constrained；
- HIL 只验证真实世界假设；

作为默认工程门槛。
