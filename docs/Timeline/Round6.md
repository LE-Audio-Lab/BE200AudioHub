# BE200AudioHub · Round 6：Review-2A / 2B / 2C / 2D 封板 + Project Governance

> **性质**：Round 6 技术总结、实现推进、静态审计、HIL 证据与纠偏记录。  
> **时间范围**：2026-10-15—2026-10-18。Round6 跨越原网页会话与一次 successor-chat continuation；Round 边界按工程里程碑而非单一聊天窗口定义。  
> **主分支**：`main`。  
> **阶段变化**：从 Round5 已完成的 Review-1 Runtime / Async Transport 基座继续推进 HCI Host Plane；本轮最终完成 Review-2A、Review-2B、Review-2C、Review-2D 的软件实现与真实 BE200/Buds3 Pro HIL，并完成 ToolHub 构建纪律与 Apache-2.0 项目治理。Review-2D 设计阶段曾发生 connection interruption 并迁移到 successor chat，但该迁移现在只作为 Round6 内部 continuation boundary，不再视为 Round 边界。  
> **重要口径**：本轮复用已有 R5/R3 成功 bond；不发起新 SMP、不重新配对、不修改持久化 BondStore。Round7 从 **Control semantic migration** 开始。

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

Review-2C
  ACL / L2CAP / fixed ATT substrate       ✅
  async typed GATT transaction FSM        ✅
  MTU / service / include / char / desc   ✅
  short+long read / write / CCCD          ✅
  notification / indication confirm       ✅
  cancel poison / late-response fencing   ✅
  dual-ACL independent fixed bearers       ✅
  fragmented ACL→L2CAP→ATT regression     ✅
  real read-only B3P GATT HIL             ✅ PASS

Project / tooling
  Cargo --locked normal gates             ✅
  fail-closed git-sync-generated          ✅
  Apache-2.0 + NOTICE / third-party audit ✅

Review-2D
  typed CIG / CIS lifecycle              ✅
  complete ISO SDU Host boundary         ✅
  bounded pre-Bumble group admission     ✅
  dedicated bounded HciIsoTx lane        ✅
  ISO credit audit + drain gate          ✅
  ASCS/CIG/CIS/ISO HIL harness           ✅
  real BE200/Buds3 Pro HIL               ✅ PASS

Round6 boundary
  Review-2A / 2B / 2C / 2D               ✅ COMPLETE
  next engineering Round                  → Round7 / Control semantic migration
```

Review-2B checkpoint software gate：

```text
cargo test  151 passed; 0 failed
cargo check PASS
cargo build PASS
```

Review-2C 封板时的软件 gate 见 §17：`cargo test` **180/180 PASS**。Round6 最终封板 gate 已推进到 §24：`cargo test` **247/247 PASS**，`cargo check` PASS，`cargo build` PASS，working tree clean。

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


---

## 17. Round6 正式收口：Review-2C ACL / L2CAP / ATT / GATT

> §1–16 是本轮较早的 Review-2A/2B 阶段性收口。当前节记录原网页会话随后完成的 Review-2C；它是 **原网页会话中的最后一个完整实现/HIL gate**，但不是 Round6 的最终工程边界。Review-2D 在 chat migration 后继续，并于 §24 完成 Round6 final closure。

### 17.1 架构选择：复用 Bumble Device 数据面，不暴露同步 GattClient

Pinned Bumble-RS 的 `Device` 已经拥有：

- per-handle ACL reassembly；
- L2CAP framing / routing；
- fixed ATT CID 0x0004；
- `send_att_on_handle` / per-handle inbox；
- disconnect / flush；
- LE connection projection。

但 upstream `GattClient` 的 `AttTransport::request()` 是同步 request/response 形状，不满足 Review 的 HciContext ownership / no blocking wait contract。

因此 Review-2C 冻结为：

```text
typed GATT request
  -> validate ConnectionToken + encrypted state
  -> Device::send_att_on_handle()
  -> return event loop
  -> Device::poll() consumes ACL/L2CAP/ATT
  -> drain per-handle ATT inbox
  -> typed GATT report
```

不重复实现 ACL/L2CAP assembler/router，也不把 raw ATT bytes 暴露给 Control。

### 17.2 typed GATT surface 与 fixed-bearer 约束

实现的操作面：

- Exchange MTU；
- Discover Primary Services；
- Discover Included Services；
- Discover Characteristics；
- Discover Descriptors；
- Read / Long Read (Read Blob)；
- Write Request / Write Command；
- Subscribe / Unsubscribe；
- Notification / Indication。

关键约束：

1. 每条 ACL 的 fixed ATT bearer 同时最多一个 request in flight；两条 ACL 可独立各跑一个；
2. 所有请求都使用 `ConnectionToken { handle, generation }` fencing；
3. 未加密 connection 不允许 GATT；
4. Notification / Indication 独立于 active request；Indication 自动发 Handle Value Confirmation；
5. requester cancel 后，wire 上没有 ATT cancel，因此该 fixed bearer **poisoned until real disconnect**；late response 明确 drop/report，禁止误配给后续请求；
6. ATT Error Response 的 `request_opcode_in_error` 必须与当前 request opcode 匹配；
7. Service / Characteristic / Descriptor discovery 支持分页；
8. Included Service 同时支持 6-byte inline UUID 形式和 4-byte declaration + follow-up Read UUID 形式；
9. Long Read 按 MTU 继续 Read Blob，Attribute Not Long / Invalid Offset 在合法上下文中结束累计值。

### 17.3 Engine / mailbox / dual-bond HIL substrate

Review HCI mailbox 增加 typed GATT begin/cancel command 与 `HciReport::Gatt`；Engine 维护 per-handle transaction / MTU / poisoned bearer / bounded report queue。

为了避免 2C HIL 复制 2B 的连接逻辑，原有双加密 bond-smoke 被抽成可复用 session：

```text
firmware bypass
-> resetless attach
-> controller identity guard
-> privacy / resolving list
-> dual FAL ACL
-> dual stored-LTK encryption
-> caller-specific work
-> reverse disconnect
-> privacy cleanup
-> ordered shutdown
-> bond fingerprint unchanged
```

2C harness 只在上述已验证 session 上做 read-only GATT。

### 17.4 deterministic tests

本轮新增/确认的关键测试包括：

- encrypted-current-connection gate；
- MTU round-trip 走真实 synthetic ACL→L2CAP→ATT；
- 两 ACL fixed bearer transaction isolation；
- unsolicited Notification / Indication connection-local；
- indication confirmation；
- cancel poison + late response drop + disconnect reset；
- disconnect 只失败 matching transaction；
- ATT Error Response opcode correlation；
- Included Service 两种编码形式；
- Write Request / Unsubscribe；
- fragmented HCI ACL response 必须完整 reassemble 后才完成 GATT。

直接依赖 `bumble-att` 固定到与其它 Bumble crates 相同 revision；canonical `Cargo.lock` 同步提交。

### 17.5 ToolHub locked gate 抓到的真实回归

在把 normal Cargo operation 改为 `--workspace --locked` 后，第一次 `cargo-test` 抓到：

```text
error[E0063]: missing field declaration_handle in initializer of GattCharacteristic
```

根因不是 ToolHub，而是此前 warning-cleanup 时误删了 `GattCharacteristic.declaration_handle` initializer 字段。补回后：

```text
cargo test   180 passed; 0 failed
cargo check  PASS
cargo build  PASS
git status   ## main...origin/main
```

这次 failure 也证明新的 locked build path 会直接暴露代码/manifest 不一致，而不会通过改写 `Cargo.lock` 隐藏状态。

### 17.6 Review-2C 真实 HIL PASS

用户在 Windows/BE200/Buds3 Pro 上执行 `review2 gatt-smoke`，结果：

```text
Review-2C read-only GATT HIL
Firmware bypass: AlreadyOperational
Controller identity guard: PASS (44:A3:BB:36:94:1B)

Member: identity=E8:C9:13:EF:2F:23 handle=0x0800@gen1
  mtu=247 services=14
  PACS=0x004E..0x0060 ASCS=0x0041..0x004D CAS=0x007D..0x007E
  PACS chars=6 ASCS chars=4 CAS includes=1 CSIS=true
  descriptors=1 supported-contexts-handle=0x0053 value-bytes=4

Member: identity=E8:C9:13:F6:5C:E0 handle=0x0801@gen2
  mtu=247 services=14
  PACS=0x004E..0x0060 ASCS=0x0041..0x004D CAS=0x007D..0x007E
  PACS chars=6 ASCS chars=4 CAS includes=1 CSIS=true
  descriptors=1 supported-contexts-handle=0x0053 value-bytes=4

Encrypted GATT service access: PASS
Cleanup disconnect: handle=0x0801@gen2 reason=0x16
Cleanup disconnect: handle=0x0800@gen1 reason=0x16
Privacy cleanup: true
Bond store unchanged: true
Shutdown: hci=Stopped transport=Cancelled
```

该 HIL 实际跑通：MTU、Primary Service、Characteristic、Included Service、Descriptor、Characteristic Read。没有 CCCD write、没有 ASCS Control Point write、没有 SMP、没有 bond mutation。

> **Review-2C software + real HIL 正式封板。**

---

## 18. ToolHub 构建/同步纪律在本轮完成收敛

本轮遇到的 `Cargo.lock` dirty / ff-only pull 阻塞不通过开放远程 shell 解决，而保持 ToolHub 的 fixed-capability 设计：

```text
cargo-test  = cargo test  --workspace --locked
cargo-check = cargo check --workspace --locked
cargo-build = cargo build --workspace --locked
```

新增 fail-closed `git-sync-generated`：

- 只允许当前 branch=main；
- staged changes 必须为空；
- working tree 必须恰好只有 unstaged `Cargo.lock`；
- fetch 后必须证明 origin/main 是 fast-forward descendant；
- incoming diff 必须包含 canonical `Cargo.lock`；
- 任一条件不满足直接拒绝；
- 不暴露通用 `git reset/restore` 能力。

真实验证中，当工作树已经 clean 时它返回：

```text
refused:
Working tree must contain exactly one unstaged Cargo.lock modification; got []
```

这是预期的 fail-closed 行为。随后 normal `git-pull` + locked Cargo gate 全部通过，且 build/test 后 working tree 仍 clean。

---

## 19. 开源治理：Apache-2.0

本轮正式给项目建立许可证链：

- `LICENSE`：Apache License 2.0 全文；
- `NOTICE`：项目 + bumble-rs attribution；
- `THIRD_PARTY_NOTICES.md`：第三方依赖与发行边界；
- `Cargo.toml`：`license = "Apache-2.0"`；
- README 增加 License 段。

对应提交：

- `751983f` — add Apache-2.0 LICENSE；
- `445792b` — add NOTICE；
- `58c3ac6` — third-party license boundaries；
- `84a78cf` — Cargo package license；
- `fd95666` — README license section。

许可证选择理由：

1. 与 bumble-rs / Google Bumble 的 Apache-2.0 provenance 对齐；
2. 显式 patent grant / retaliation 条款更适合 Bluetooth / controller / codec 这种专利密集基础设施；
3. 商业/OEM/研究集成友好，不引入全项目 copyleft；
4. contributor 默认按 Apache-2.0 contribution 规则进入；当前不引入 CLA，未来需要时可考虑 DCO。

边界：

- Intel `.sfi` / `.ddc` firmware **不属于** BE200AudioHub Apache-2.0；
- `rusb` 启用 `vendored` 会带入 libusb；libusb LGPL-2.1-or-later 的发行义务不能被项目 Apache-2.0 覆盖；
- binary release 需要保留第三方 license/source/relinking 合规路径。

---

## 20. 社区定位与公开 claim 边界

本轮专门调研 BlueZ / BTstack / Zephyr / NimBLE / Google Bumble / bumble-rs 的项目历史和 Windows/LE Audio 能力。

时间尺度：

```text
BlueZ       2001-
BTstack     2009-
Mynewt/NimBLE roots 2015- / NimBLE 1.0 2018
Zephyr      2016-
Google Bumble public 2022-
bumble-rs   2026-07-
```

重要结论：Core Bluetooth / ATT / GATT / SMP 已经成熟很多年，但 commodity-controller LE Audio Host、CIG/CIS/USB ISO、commercial TWS integration 普遍只有近 2–4 年的公开工程历史；bumble-rs 更是只比本项目早约一个半月开始 Rust port。

公开 claim 必须收窄：

- **不能**宣称“第一个 Windows Bluetooth Host”或“唯一 Windows LE Audio stack”；BTstack/Bumble 都有 Windows/direct-HCI precedent；
- 当前可以事实性描述为：native Windows userspace Host takeover of commodity Intel BE200，绕过 Windows Bluetooth Host，已完成 firmware/dual encrypted ACL/real LE Audio GATT，以及 real dual-CIS / HCI ISO transport + teardown HIL；
- 等 Review 路径重新完成 CIG/CIS/ISO/LC3/WASAPI end-to-end 后，才适合使用：
  **“To our knowledge, the first publicly documented open-source implementation to take over a commodity Intel PC Bluetooth controller on native Windows and run the complete LE Audio Unicast host path to commercial TWS earbuds without the Windows Bluetooth host stack.”**

社区判断：这是 niche systems infrastructure，不应以大众 star 数为成功标准；最大价值来自 generic Intel backend、可复现 First Sound、真实 controller/earbud quirk、以及向 Bumble/其它 upstream 回馈最小复现和修复。Consumer adoption 受 WinUSB takeover、内置 Bluetooth 丢失风险、firmware/driver setup 门槛强烈限制。

---

## 21. 项目动机 / North Star 在本轮重新明确

早期 Windows LE Audio 调试的关键证据链是：

```text
同一 B660M + AX210 + Buds3 Pro

Windows:
  CIS effective capability = False
  native LE Audio unavailable

Fedora:
  cis-central / cis-peripheral available
  LE bearer connected
  BAP-sink
  LC3
```

此前 BIOS IFR、SST、NHLT、Intel private DevProp、BATS registry、driver-version A/B 与 property spoofing 已逐层排除“硬件不支持”的解释。Fedora 证明同一物理硬件可跑 LE Audio 后，项目的根本动机从“继续猜 Windows private gate”转为：

> **绕过 Windows Bluetooth Host，直接拥有 controller / Host / LE Audio 全链路。**

长期系统边界可概括为：

```text
Windows:
  USB host
  PCM source

Bluetooth control/data:
  owned by BE200AudioHub
```

这不是当前所有功能已经完成的声明，而是项目的产品/研究 North Star。

---

## 22. Round6 内部 Chat Migration Boundary：Review-2D 设计交接

2C HIL PASS 后开始设计 Review-2D；在实际落第一笔 2D code 前多次 connection interruption，随后迁移到 successor chat 继续。**这里是聊天边界，不再作为工程 Round 边界。** Round6 的正式结束点改为 §24 的 Review-2D real HIL closure。

冻结拆分：

```text
2D-A  ISO capabilities + typed CIG/CIS lifecycle
2D-B  ISO Data Path + complete HCI ISO SDU transport
2D-C  teardown / fault / credit / dual-CIS invariants
```

本轮已完成的旧 R5 / reference audit：

- CIS Host Support bit32 必须在任何 LE ACL 前建立；
- stale/inactive CIG 不能靠 ACL disconnect 等价清除，需显式 lifecycle；
- Set CIG Command Complete wire 语义：status / cig_id / count / CIS handles；
- Create CIS 是 Command Status + later LE CIS Established terminal events，不能把 command acceptance 当 established；
- complete ISO SDU 是 HCI Host boundary；Host owns ISO sequence / PB / fragmentation / controller credits；
- Number Of Completed Packets 只释放 transport capacity，绝不推进 FrameId / StreamClock；
- old R5 unidirectional QoS 仍给未使用 P→C 方向合法 interval/latency/PHY，同时 Max_SDU=0、RTN=0，避免 mandatory-range controller rejection；
- 新 2D 不应把 ASCS Control semantic 混回 HCI Host；Control Plane migration 是后续 phase。

Engine integration 阅读到的边界：

- Command Complete / Status 使用 Review `HciObservation`；
- LE CIS Established / disconnect 属于 Bumble DeviceEvent；
- `QueueHostTransport` 已有 native ISO ↔ H4 adaptation；
- 2D 应继续使用 `Transaction + typed Action + Engine owner + bounded report` 风格。

> **原网页会话到此结束；Round6 工程阶段继续。** 后续 §23–§24 来自 successor chat，但仍计入 Round6，因为它们完成的是同一个 Review-2D gate。

---

## 23. Round6 continuation after chat migration：Review-2D 实现（2026-10-18）

> 本节来自 successor chat，是 Round6 的工程 continuation。它不改写前面“当时尚未实现 2D”的历史快照，但**正式计入 Round6 最终成果**。事实源仍按当前 `main` 源码、Git commit、deterministic test 与真实 HIL。

§16 写下时，Review-2C 仍标记为 NEXT、2D 为 pending。仓库随后继续推进，实际代码已经越过该状态。

### 23.1 Review-2C — ACL / L2CAP / ATT / GATT

当前 Review 路径已经具备：

- 真实 ACL → L2CAP → fixed ATT bearer 数据路径；
- fragmented HCI ACL reassembly；
- per-ACL request isolation；
- MTU exchange；
- primary / included service discovery；
- characteristic / descriptor discovery；
- short / long read；
- write request / write command；
- subscribe / unsubscribe；
- notification / indication；
- encrypted-current-connection gate；
- bounded GATT mailbox / report surface；
- read-only real-HIL harness `review2 gatt-smoke`。

Review-2C 已在本 Round §17 达到 **software + real HIL PASS**。本 continuation 只承接 Review-2D，不重复声明新的 2C gate。

### 23.2 Review-2D — CIG / CIS / ISO Host

Review-2D 当前实现完成：

1. typed CIG configure / remove transaction；
2. typed Create CIS request + asynchronous Command Status / LE CIS Established reducer；
3. dual-CIS partial-failure collection与显式 disconnect lifecycle；
4. HCI ISO Data Path setup / remove transaction与 Device projection consistency check；
5. strict ISO packet submitted/completed audit，per-handle completion、over-completion fault、disconnect-local flush；
6. complete ISO SDU Host boundary：HCI sequence、PB、fragmentation、controller packet queue 由 pinned Bumble Host owning；
7. **bounded interval-group admission**：在进入 Bumble queue 前按真实 controller ISO packet length/window 计算完整 group 所需 fragments；
8. 独立 bounded `HciIsoTx` realtime data lane，不复用 reliable control mailbox；
9. HCI worker budget/fairness：stop → bounded control → bounded controller events → bounded ISO group service；
10. controller credit 满时只保留一个 HCI-worker pending group，并停止继续 drain data lane，避免把 realtime backlog 搬进 Bumble 内部 unbounded waiting `VecDeque`；
11. reliable ISO drain/resume gate：teardown 前先 block 对应 CIS、整组丢弃尚未 Host-admit 的 stale realtime data，再等待已经提交的 Host/controller packets 真实 drain；
12. stop/control 在 ISO data pressure 下仍保持可服务，确定性测试锁定该优先级。

关键实现锚点：

- `b450f6ee` — complete ISO SDU through HCI engine；
- `f9335386` — bounded interval-group admission model；
- `260f7458` — engine-side pre-Bumble group admission；
- `2678827a` / `6bc8e696` — dedicated ISO data lane + worker fairness；
- `7d97df43` — 248 B × 8 controller-window deterministic integration；
- `9ee2e575` — drain gate / teardown protection；
- `498fb5a7` — ISO data pressure fairness tests；
- `fbbd9327` — keep public submission boundary group-only。

### 23.3 为什么额外做 Host admission

Pinned Bumble `DataPacketQueue` 只限制 controller `max_in_flight`；其 waiting packet `VecDeque` 本身没有容量上限。因此仅在 Bridge→HCI 入口使用 bounded mailbox 仍不够：如果 HCI worker 持续消费 mailbox，realtime backlog 会被转移到 Bumble 内部并重新变成无界延迟。

Review-2D 的规则因此冻结为：

> **在调用 `Device::send_iso_sdu()` 之前，对完整 interval group 做 admission。正常 group 不得使 Host pending packet 数超过 controller window；一个自身 fragmentation 已大于 window 的合法 group 只能从完全 drained 状态进入。**

对于 sealed B3P First Sound 参数，历史 HIL 已知 BE200 报告 ISO pool = **248 B × 8 packets**，48 kHz / 10 ms / 100 B mono-per-ear 每耳各占一个 HCI ISO packet，因此正常情况下一个 8-packet window 最多容纳四个 stereo intervals。该数值只作为已知硬件 fixture；runtime 使用实际 controller capability，不硬编码 8。

### 23.4 Review-2D deterministic gate

完成本节代码后，本地 ToolHub 当前软件门槛：

```text
cargo test --workspace --locked
233 passed; 0 failed; 0 ignored
```

Review-2D 当前状态：

```text
Review-2A attach / capabilities / scan       ✅ COMPLETE + HIL PASS
Review-2B privacy / ACL / stored security    ✅ COMPLETE + HIL PASS
Review-2C ACL / L2CAP / ATT / GATT           ✅ COMPLETE + HIL PASS
Review-2D CIG / CIS / ISO Host               ✅ SOFTWARE COMPLETE
                                              → real HIL closure in §24

NEXT after Round6 closure: Control semantic migration
      → StreamClock + IsoTxBridge
      → Synthetic Media / First Sound
```

本节记录的是 Review-2D software-complete 截点；随后 §24 已完成新的 Review-2D 真机 HIL。sealed Experiment/R5 的 LC3 First Sound 仍是独立历史证据，不能与 Review-2D transport smoke 的 codec-opaque payload 混为同一声学 gate。


---

## 24. Round6 final closure：Review-2D Real HIL PASS + Round7 Handoff

### 24.1 HIL harness 的最终边界

为避免把产品 Control / Media 提前塞回 HCI Host，最终新增的 `review2 iso-smoke` 明确是 **HIL-only oracle bridge**。它复用既有 stored bond，并只为证明 Review-2D 的 controller/Host/ISO 生命周期而临时执行最小 LE Audio semantic preamble：

```text
existing stored bond
→ resetless attach / controller identity guard
→ pre-ACL CIS Host Support bit32 set/readback
→ bounded passive extended scan
→ stored peer IRK host-side resolve current RPA for both members
→ controller RL / privacy
→ dual stored-LTK encrypted ACL
→ PACS Sink Audio Location → physical Left/Right
→ ASCS Sink ASE selection
→ Config Codec
→ Set CIG
→ Config QoS
→ Enable
→ Create dual CIS
→ Setup HCI Input ISO Path ×2
→ 100 stereo interval groups through HciIsoTx
→ exact submitted/completed audit
→ ISO drain
→ Remove ISO Path ×2
→ Disconnect CIS ×2
→ Disable/Release ASE
→ Remove CIG
→ reverse ACL disconnect / privacy cleanup
→ bond fingerprint unchanged
```

该 smoke **不属于产品 Control implementation**。Round7 仍需把 PACS/ASCS/CIG/CIS orchestration 正式迁入 Control semantic owner。

### 24.2 HIL 前发现并修复的真实迁移遗漏

#### A. CIS Host Support bit32

旧 R5 已证明：BE200 在任何 LE ACL 前需要 `HCI LE Set Host Feature (0x2074)` 将 bit32 置 1，并通过 LE feature readback 验证；否则 Create CIS 可返回 `0x0C Command Disallowed`。

Review-2D 最初没有这个 primitive，因此新增 typed transaction：

- pre-ACL guard；
- cached feature no-op if already enabled；
- Set Host Feature bit32=1；
- Command Complete status；
- LE Read Local Supported Features readback；
- bit32 must be observed set。

#### B. 多 Sink ASE 假设错误

第一次 ISO smoke 真机到达 ASCS 后失败：

```text
ASCS Sink ASE: expected exactly one characteristic UUID 0x2BC4, found 2
```

Buds3 Pro 每个 member 实际暴露 **2 个 Sink ASE**。修复重新采用 sealed R5 deterministic policy：

```text
enumerate all Sink ASE
→ every ASE ID nonzero and unique
→ read current state
→ choose the lowest ASE ID among Idle Sink ASEs
→ leave all unselected ASEs untouched
```

真机随后选择：

```text
Sink ASEs=2
selected ASE=1
handle=0x0046
```

#### C. pre-connect discovery gate 遗漏

修复多 ASE 后，ISO smoke 连续三次在第一只 member 的 identity-FAL ACL 上 timeout，即使重新入盒/出盒仍一致。

回读 sealed R5 发现真实成功顺序是：

```text
bit32 pre-ACL
→ passive extended scan
→ host-side IRK resolve current member RPA
→ require 2/2 members currently connectable
→ configure controller RL/privacy
→ identity FAL connect
```

Review ISO smoke 原先漏掉 scan/resolve gate。恢复该 gate 后，真机观测：

```text
[R2D/DISCOVERY]
identity=E8:C9:13:EF:2F:23 current=4E:D9:B1:8B:15:FF connectable=PASS
identity=E8:C9:13:F6:5C:E0 current=57:A6:A5:F2:30:00 connectable=PASS
```

这一步使用 pinned Bumble SMP privacy helper 验证 RPA，不自行重写 `ah()`。

### 24.3 Review-2D 最终真实 HIL

最终用户在 Windows 本机执行 `review2 iso-smoke --frames 100 --confirm-iso-hil`，得到：

```text
Firmware bypass: AlreadyOperational
Controller identity guard: PASS (44:A3:BB:36:94:1B)

[R2D/ASCS]
E8:C9:13:EF:2F:23 Sink ASEs=2 selected ASE=1 handle=0x0046
E8:C9:13:F6:5C:E0 Sink ASEs=2 selected ASE=1 handle=0x0046

Left:  identity=E8:C9:13:F6:5C:E0 CIS=0x0900
Right: identity=E8:C9:13:EF:2F:23 CIS=0x0901

Controller ISO packet window: Some(8)
ISO Host audit:
  frames=100
  submitted=200
  completed=200
  in_flight_after=0

ISO drain + reverse teardown: PASS

Cleanup ACL disconnect:
  handle=0x0801@gen2 reason=0x16
  handle=0x0800@gen1 reason=0x16

Privacy cleanup: true
Bond store unchanged: true
Shutdown: hci=Stopped transport=Cancelled
```

这证明新的 Review 架构已经真实打通：

```text
HciIsoTx
→ bounded Host admission
→ Bumble HCI ISO sequence/PB/fragmentation
→ Transport
→ Windows BE200 USB bulk OUT
→ BE200
→ dual CIS
→ Galaxy Buds3 Pro
→ completion credit return
→ bounded drain
→ reverse teardown
```

100 个 stereo intervals 对应每耳 100 个 100-byte SDU；在该 verified controller configuration 下每耳 SDU 各占一个 HCI ISO packet，因此 audit 的 **200 submitted / 200 completed / 0 in-flight** 与真实 controller window=8 完全闭合。

### 24.4 可听输出：额外 endpoint evidence，但不是 LC3 gate

为保持 Review-2D scope，smoke payload 是 deterministic codec-opaque 100-byte pattern，而不是合法 LC3：

```text
Left  byte[i] = frame_index + i
Right byte[i] = frame_index + i + 0x55
(mod 256)
```

Buds3 Pro 被 ASCS 配置为 LC3/48 kHz/10 ms/100 B，因此会尝试将这些 opaque bytes 当 LC3 frame 解码。用户即使未佩戴耳机、仅拿在手上也听到了明显数字噪声。

该现象只作为**补充 endpoint evidence**：SDU 已经跨过 controller/CIS 到达耳机 codec/output pipeline。它**不**计作合法 LC3、音质、时序或 acoustic First Sound 验收；Review 路径的合法 LC3 synthetic First Sound 属于 Round7 后续 Media 阶段。

### 24.5 Round6 final software gate

最终修复 pre-connect discovery gate 后，ToolHub 结果：

```text
cargo test --workspace --locked
247 passed; 0 failed; 0 ignored

cargo check --workspace --locked
PASS

cargo build --workspace --locked
PASS

git status
## main...origin/main
```

相关后段 commit 锚点：

- `0eab62b6` — typed CIS Host Feature bit32 transaction；
- `c7e03520` — ISO HIL session pre-ACL bit32 gate；
- `59a2e5a4` — cumulative ISO submitted/completed audit snapshot；
- `17c32549` — HIL-only ASCS preparation bridge；
- `1202e2e4` — cleanup only real Established CIS links；
- `729d92f1` — `review2 iso-smoke` CLI；
- `f84c5fc1` — multiple Sink ASE → lowest Idle ASE ID；
- `3e2f890e` — restore pre-connect bonded-member discovery/RPA resolution；
- `c361a2d1` — canonical direct `bumble-smp` lock entry。

### 24.6 Round6 正式结束 / Round7 起点

Round6 最终状态：

```text
Review-1  Runtime / Async Transport          ✅ inherited COMPLETE
Review-2A Attach / Capabilities / Scan       ✅ COMPLETE + HIL PASS
Review-2B Privacy / Dual ACL / Stored LTK    ✅ COMPLETE + HIL PASS
Review-2C ACL / L2CAP / ATT / GATT           ✅ COMPLETE + HIL PASS
Review-2D CIG / CIS / ISO Host               ✅ COMPLETE + HIL PASS
```

Round6 在此正式封板。**Round7 从以下工作开始：**

```text
Control semantic migration
        ↓
StreamBinding
        ↓
StreamClock + IsoTxBridge
        ↓
Synthetic valid-LC3 First Sound
        ↓
WASAPI + ASRC
        ↓
Lifecycle / Recovery
        ↓
Profiling / HIL hardening
```

Round7 的核心边界：HCI Host 以下视为 sealed substrate。Control / Media 不得重新拥有 raw CIS handle、HCI ISO sequence、PB/fragmentation、controller credit 或 USB endpoint；Number Of Completed Packets 继续只代表 transport capacity，不得推进 media time。
