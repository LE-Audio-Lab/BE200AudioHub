# BE200AudioHub · Round 6：Review-2A / Review-2B HCI Host 封板

> **性质**：Round 6 技术总结、实现推进、静态审计、HIL 证据与纠偏记录。  
> **时间范围**：2026-10-15。  
> **主分支**：`main`。  
> **阶段变化**：从 Round5 已完成的 Review-1 Runtime / Async Transport 基座继续推进 HCI Host Plane，完成 Review-2A 与 Review-2B 的软件实现和真实 BE200/Buds3 Pro HIL。  
> **重要口径**：本轮只复用已有 R5/R3 成功 bond；不发起新 SMP、不重新配对、不修改持久化 BondStore。

---

## 0. 一页结论

```text
Review-1 Runtime / Async Transport         ✅ inherited COMPLETE

Review-2A
  Firmware bypass                         ✅
  resetless operational attach            ✅
  controller capabilities                 ✅
  passive extended scan                   ✅
  real BE200/B3P HIL                      ✅ PASS

Review-2B
  Resolving List / privacy                ✅
  FAL LE ACL connect                      ✅
  requester-driven cancel / race handling ✅
  typed disconnect                        ✅
  stored-LTK encryption                   ✅
  dual ACL connection-set projection      ✅
  per-handle generation fencing           ✅
  ACL completion / capacity audit         ✅
  controller identity guard               ✅
  read-only legacy bond input             ✅
  bond fingerprint / no-write proof       ✅
  real dual encrypted ACL HIL             ✅ PASS

NEXT
  Review-2C ACL / L2CAP / ATT / GATT       ← NEXT
```

最终软件 gate：

```text
cargo test  151 passed; 0 failed
cargo check PASS
cargo build PASS
```

最终 Review-2B HIL：

```text
Firmware bypass: AlreadyOperational
Controller identity guard: PASS (44:A3:BB:36:94:1B)

Member 1: identity=E8:C9:13:EF:2F:23 handle=0x0800@gen1 encrypted=true key_size=0
Member 2: identity=E8:C9:13:F6:5C:E0 handle=0x0801@gen2 encrypted=true key_size=0

Dual encrypted ACL: PASS
Cleanup disconnect: handle=0x0801@gen2 reason=0x16
Cleanup disconnect: handle=0x0800@gen1 reason=0x16
Privacy cleanup: PASS
Bond store unchanged: true
Shutdown: hci=Stopped transport=Cancelled
```

`key_size=0` 是当前 Bumble/Review 投影没有给出有效 key-size 值的表现，不代表使用 0-byte key；本轮成功判据是 connection state 真实进入 encrypted。

---

## 1. Round6 开始时状态

Round5 截止时：

- Experiment R5.3 已证明真实 LC3 First Sound；
- Experiment 同步 USB / credit-driven media 架构已 sealed；
- Review architecture contract 已冻结；
- Review-1 Runtime Kernel / Async Transport 已完成；
- HCI Host 仍为 WIP；
- synthetic resetless attach/scan 有 6 个 failing tests；
- Review 路径尚无真实 BE200 HIL。

Round6 目标：先把 `Review-2A attach/capabilities/scan → Review-2B privacy/ACL/stored security` 做成默认、bounded、typed、deterministic-test constrained 的产品组件。

---

## 2. Review-2A：HCI Host 转正

### 2.1 status-only Command Complete 根因

初始 HCI WIP：

```text
74 tests
68 pass
6 fail
```

失败集中于 resetless attach 无法推进到 Ready，scan/worker 随后超时。

根因：pinned Bumble-RS 对部分 status-only Command Complete 解析成 `ReturnParameters::Raw`，而 Device host initialization 期待 `ReturnParameters::Status`。

涉及：

- Set Event Mask；
- Set Event Mask Page 2；
- LE Set Event Mask；
- LE Write Suggested Default Data Length。

修复：只在 Review HCI adapter 边界对白名单 opcode 做 `Raw(1 byte) -> Status` normalization；未知 Raw completion 保持 Raw，不 fork upstream，并加 deterministic regression tests。

### 2.2 Address type contract

继续沿用 Round2/3 已冻结规则：地址 bytes/string 不能代替 address type；public/random 必须独立保存。

### 2.3 默认 build promotion

- Bumble HCI/Host 与 crossbeam 进入 normal dependencies；
- `hci_host` / `review2` 默认编译；
- 默认 `cargo test/check/build` 直接覆盖 HCI Host；
- 移除 `hci-wip` 作为正常运行前提。

---

## 3. Firmware Bypass

Review-2A HIL 前确认 cold BE200 可能仍处 Intel Bootloader image `0x01`。

决定：firmware bootstrap 不进入 HCI Host 或 async Transport；使用短生命周期 L3 bypass：

```text
Review L0
  +-- FirmwareBypassPort
  |     +-- Be200FirmwareLoader
  |            +-- PinnedFirmware
  +-- release interface 0
        -> Async Transport
        -> HCI Host
```

保留 frozen R5 proven sequence：

```text
FC05 Read Version
  -> Operational 0x03: true no-op
  -> Bootloader 0x01:
       HW/Secure-Boot guard
       preboot DDC
       Secure Send
       FF/06
       FC01 Boot
       FF/02
       DSBR default
       FC05 require Operational
       postboot DDC
       release
```

不复用旧 Experiment 的 cwd magic、多模式 CLI、自动 retry/reset/power-cycle、旧 AppError/logging。

---

## 4. Review-2A HIL PASS

`review2 hci-smoke`：

```text
Firmware bypass: Booted
Capabilities: PASS
Scan: PASS reports=2 observations=2 truncated=false
  addr=6B:FE:B1:FC:C5:A6 type=1 rssi=-80 name=Galaxy Buds3 Pro (2F23)
  addr=72:B5:83:DC:64:40 type=1 rssi=-71 name=Galaxy Buds3 Pro (2F23)
Target seen: true
Shutdown: hci=Stopped transport=Cancelled
```

该 HIL 没有建立 ACL、没有 SMP、没有 Bond mutation、没有 HCI Reset。

> **Review-2A software + HIL 正式封板。**

---

## 5. Review-2B：Privacy / FAL / connection lifecycle

### 5.1 Resolving List transaction

参考 `deprecated/r5/crates/bumble-adapter/src/privacy.rs`，重写为非阻塞 typed FSM：

```text
disable address resolution
→ clear RL
→ read RL capacity
→ add fixed identity + peer IRK + local IRK
→ RPA timeout 900 s
→ enable address resolution
```

任意中途 failure：`disable → clear → Failed`。另有 explicit cleanup transaction，disable 失败也仍尝试 clear。

### 5.2 FAL connect

完全保持 R5 真机成功参数：

```text
Read FAL Size
→ Clear FAL
→ Add stable bonded identity
→ LE Extended Create Connection
→ terminal connection event
→ Clear FAL

filter_policy       = 1
own_address_type    = public
peer fields         = ANY / zero
PHY                 = 1M
scan interval/window= 0x0010 / 0x0010
conn interval       = 24..40
latency             = 0
supervision timeout = 42
CE length           = 0 / 0
```

### 5.3 Requester-owned cancel

timeout 不在 HciContext 内 sleep；requester 超时后显式发 `LE Create Connection Cancel`。

FSM 覆盖：cancel accepted、cancel rejected、late connection success、`ConnectedAfterCancel`。

### 5.4 Disconnect

Round4/R5 proven rule 固化为类型层约束：

- command reason 固定 `0x13`；
- received Disconnection Complete `0x16` 只作为 event result；
- API 不允许调用者把 0x16 再传回 command。

---

## 6. Stored-LTK encryption

参考 frozen R5 `stored_security.rs`：

```text
keys.ltk
→ ltk_central
→ ltk_peripheral
```

LE Enable Encryption 参数直接使用 stored `LTK + EDIV + RAND`；缺省 EDIV/RAND 为 0。

成功门槛：Command Status success + Bumble EncryptionChange success + Device handle encrypted。

等待期间 ACL 先断则 terminal `ConnectionLost`。静态审计后把临时 5 s wait 恢复成 R5 proven 的 **10 s**，避免 HIL 假超时。

---

## 7. ConnectionToken generation fencing

引入：

```rust
ConnectionToken {
    connection_handle,
    generation,
}
```

每次新 LE Connection Established 分配新 generation；handle 被 controller 复用时旧 token 必须失效。disconnect / stored encryption 都要求当前 token。

回归测试：`0x0800@g1 → disconnect → 0x0800@g2` 后，g1 的 disconnect/encrypt 请求都必须返回 `StaleConnection`。

---

## 8. ACL completion / capacity correctness audit

Pinned Bumble 已有 queue/credit substrate，但静态精读发现 Number Of Completed Packets 路径可能吞掉内部 over-completion error。

Review 不复制 scheduler，只加旁路 audit：

```text
Transport successful ACL enqueue -> AclSubmitted(handle)
Number Of Completed Packets      -> completed(handle,count)
```

Invariant：

- failed enqueue 不计 submitted；
- unknown handle completion = fault；
- completed > in-flight = fault；
- disconnect 只 flush 当前 handle；
- global Flush 才 flush all。

冻结原则：controller completion 只释放 transport capacity，绝不能推进 Media/StreamClock。

---

## 9. DeviceEvent fan-out / dual ACL projection

DeviceEvent router 从单 consumer 改成 typed fan-out，因为同一个 `Disconnected` 可能同时终结 disconnect transaction、stored encryption transaction并改变 ConnectionSet。

无人消费的高层事件进入 bounded deferred queue，cap=64，overflow=fault。

Dual ACL 不另造 mutable registry；唯一 truth 是 `Bumble Device::le_connections()`，Review 只派生 `LeConnectionSetSnapshot`。

软件测试覆盖：两 handle 共存、单独 encryption、单 handle disconnect 后另一个继续存在。

---

## 10. Controller identity / existing bond compatibility

### 10.1 Side-effect-free Read BD_ADDR

直接把 Bumble `ReadBdAddr` completion 喂给 Device 会触发 upstream key-store / Resolving List refresh，违反 Review ownership。

因此增加 one-shot journal-only completion suppression：Review 得到 public address，但 completion 不进入 Bumble Device；测试确认 suppression 只生效一次。

### 10.2 Read-only bond loader

只读：

```text
%LOCALAPPDATA%\BE200AudioHub\bonding\
  controller-identity.txt
  local-identity.json
  bond-sets.json
  pairing-keys.json
```

提取 controller identity、local IRK、两个 CSIS-verified identities、address type、peer IRK、LTK、EDIV、RAND。`pairing-keys.json` 直接复用 pinned Bumble `JsonKeyStore` schema。

没有 save / commit / reseed / pairing / clear bond API。

### 10.3 Set completeness

最后静态审计增加纯本地 gate：

```text
stored set_size == 2
CSIS-verified member count == set_size
```

零额外扫描/RF 时间。Rank 不作为 Review-2B blocker，因为 ACL/security 不依赖 rank，左右声道未来仍以 PACS Audio Location 为真值。

---

## 11. Persistent-state proof / secret redaction

HIL 前后 SHA-256 fingerprint：

- controller-identity.txt；
- local-identity.json；
- bond-sets.json；
- pairing-keys.json。

任一变化直接判 Review-2B failure。

IRK/LTK/RAND 的 Debug 输出全部 redacted；正常 HIL log 只显示 identity/handle/generation/state。

---

## 12. Review-2B HIL harness

CLI：`review2 bond-smoke`，必须显式 `--confirm-stored-bond-hil`。

```text
fingerprint bond store
→ load existing verified bond
→ firmware bypass
→ resetless attach
→ side-effect-free Read BD_ADDR
→ identity guard
→ configure Resolving List

member 1: stable identity FAL → stored-LTK
member 2: stable identity FAL → stored-LTK

→ verify exactly 2 live encrypted ACL
→ reverse-order disconnect
→ disable address resolution
→ clear RL
→ ordered HCI/Transport shutdown
→ fingerprint bond store again
```

任何主流程 failure 仍执行 best-effort live-handle disconnect、privacy cleanup、ordered shutdown 与 bond fingerprint。

---

## 13. R5 successful baseline static audit

Smoke 前逐项重读：

- `deprecated/r5/src/audio_set.rs`；
- `deprecated/r5/crates/bumble-adapter/src/privacy.rs`；
- `connection_manager.rs`；
- `stored_security.rs`；
- `bond.rs`。

确认 Review 沿用 R5 proven：RPA timeout 900 s、stable identity FAL、filter policy 1、public own address、ANY peer、1M、scan 0x10/0x10、interval 24–40、latency 0、supervision 42、stored LTK/EDIV/RAND、Disconnect command 0x13。

R5 的 host-side scan→IRK resolve current RPA 是 Experiment observability gate；Round6 决定不为 checklist 新造 host resolver。真实产品连接路径依赖 controller Resolving List + stable-identity FAL，并由真实 dual-ACL HIL验证。

---

## 14. 本轮工程纠偏

### 14.1 worker state publisher 参数扩张

新增 privacy/FAL/disconnect/encryption/connection-set state 时，多次出现某个 worker branch 漏传新 `last_*` 参数。Cargo typecheck 每次在编译期抓住。后续应考虑把 publisher last-state 聚合成 struct。

### 14.2 HIL orchestration duplicate draft

长会话断点恢复时曾误判 `review2_bond_smoke.rs` 后半段尚未实现，重复追加 executor/helper。Cargo 报 duplicate type/function；纠正为重新精读完整文件并删除重复 draft，不叠补丁掩盖结构问题。

### 14.3 Cargo.lock / rust-analyzer

direct `bumble` / `serde_json` 加入后，本地 Cargo/rust-analyzer 会把旧 lock 自动重写；`git restore` 后又立即 dirty，导致 `git pull --ff-only` 拒绝覆盖。

最终：远端提交 canonical lock，本地确认只有 Cargo.lock dirty 后 `git fetch origin → git reset --hard origin/main`，恢复 clean。

### 14.4 set_size fixture regression

增加 `set_size == 2` guard 后旧测试 fixture 自身缺字段，导致 1 个 unit test failure。生产 guard 正确；修 fixture 加 `"set_size": 2`，最终 151/151。

---

## 15. Review-2B 最终 HIL PASS

```text
Review-2B stored-bond HIL: identity guard -> privacy -> dual ACL -> stored-LTK -> cleanup
Target set: Galaxy Buds3 Pro (2F23)
Connect timeout: 12 s
Policy: existing bond only; no pairing/SMP start; no bond writes

Firmware bypass: AlreadyOperational
Controller identity guard: PASS (44:A3:BB:36:94:1B)
Member 1: identity=E8:C9:13:EF:2F:23 handle=0x0800@gen1 encrypted=true key_size=0
Member 2: identity=E8:C9:13:F6:5C:E0 handle=0x0801@gen2 encrypted=true key_size=0
Dual encrypted ACL: PASS
Cleanup disconnect: handle=0x0801@gen2 reason=0x16
Cleanup disconnect: handle=0x0800@gen1 reason=0x16
Privacy cleanup: PASS
Bond store unchanged: true
Shutdown: hci=Stopped transport=Cancelled
```

证据意义：

1. Review async Transport + HCI Host 在真实 BE200 上复用了已有 R5 bond；
2. Controller RL + stable identity FAL 同时建立 B3P 双耳 ACL；
3. 两耳 stored-LTK 恢复加密成功；
4. 没有新 SMP/Pairing；
5. cleanup 使用合法 0x13 command，收到 local 0x16 event reason；
6. privacy cleanup 完成；
7. 四个 bond 文件 fingerprint 完全不变；
8. HCI/Transport ordered shutdown 完成。

> **Review-2B software + HIL 正式封板。**

---

## 16. Round6 最终状态 / 下一步

```text
Experiment R1–R5.3                     ✅ SEALED

Review-1
Runtime Kernel                         ✅ COMPLETE
Async Transport                        ✅ COMPLETE

Review-2
2A attach / capabilities / scan        ✅ COMPLETE + HIL PASS
2B privacy / ACL / stored security     ✅ COMPLETE + HIL PASS
2C ACL / L2CAP / ATT / GATT            ← NEXT
2D CIG / CIS / ISO Host                pending

Phase 3 Control Plane migration         pending
Phase 4 StreamClock + IsoTxBridge       pending
Phase 5 Synthetic Media                 pending
Phase 6 Synthetic First Sound HIL       pending
Phase 7 Windows Process Loopback        pending
Phase 8 Clock discipline / ASRC         pending
```

下一轮最短路径：

> **Review-2C：在已经封板的 dual encrypted ACL / generation / credit substrate 上迁移 ACL routing、L2CAP、ATT、GATT 与 encrypted service access。**

除非新的 HIL 或 deterministic regression 给出明确证据，不重新打开 Pairing、Bond、Review-2A attach 或 Review-2B privacy/connection/stored-LTK。
