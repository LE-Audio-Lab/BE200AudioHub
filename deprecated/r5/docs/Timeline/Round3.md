# BE200AudioHub Round3
## 从双成员 Pairing 收束到 Bonded Audio Set Discovery 封盘：完整发现、决策、错误修复、Quirk 与后续边界

**项目：** `STanJK/BE200AudioHub`  
**硬件：** Intel BE200 USB Bluetooth Controller (`8087:0036`) + Samsung Galaxy Buds3 Pro (`Galaxy Buds3 Pro (2F23)`)  
**主开发分支：** `r2/stack-integration-bootstrap`  
**本轮时间范围：** 2026-10-13 至 2026-10-14  
**文档定位：** Round3 技术归档。记录本轮所有重要发现、走偏路线、被推翻假设、已证实根因、修复决定、架构收束和 R3 实机结果。  
**最终状态：**

```text
Controller Bootstrap       已接入，Operational 快路径已验证
R2 Pairing                 CLOSED
Stored Bond Verification   CLOSED
R3 Bonded Audio Set        CLOSED
下一阶段                   R4.1 Codec Planner → R4.2 ASCS Config Codec
```

> 这份文档不是简单的最终架构说明，而是保留“为什么会这样设计”的完整证据链。后续维护者不应只看最后代码，还应理解哪些假设已经被 HIL 推翻，以及哪些 quirk 是 BE200 / Windows WinUSB / Bumble-RS 的真实边界。

---

# 1. 本轮开始时的状态

本轮开始时，仓库已经完成：

```text
R1:
- Windows WinUSB 绑定与 BE200 识别
- Intel Read Version
- 固件重建与冷启动
- BLE 扫描

R2 初期:
- Bumble-RS 接入
- 无 HCI Reset 的 Operational attach
- LE ACL probe
- SMP Secure Connections 基础
- JSON BondStore
- CSIS 读取
- RSI 校验
- 双成员 onboarding 初版
```

开始时分支：

```text
r2/stack-integration-bootstrap
```

当时确认的远端 HEAD：

```text
6158e9a
feat(r2.4): make dual-member onboarding CSIS-safe and resumable
```

本地存在一个长期未暂存的：

```text
 M Cargo.lock
```

它在本轮始终被刻意保留，避免无关 lockfile 变动混入硬件调试提交。

---

# 2. 本轮最终完成了什么

本轮最终完成的真实链路：

```text
Controller image 检测
→ 如已 Operational，直接复用
→ 如 Bootloader，使用固定 .\firmware 资产执行一次初始化

Primary:
Swift Pair pairing transport
→ Controller FAL 建立 ACL
→ provisional LE Secure Connections
→ encrypted CSIS read
→ rank=1 / Set Size=2 / SIRK
→ 原子提交 peer keys + set metadata

Secondary:
RSI 匹配同一 SIRK
→ Controller FAL 建立 ACL
→ provisional LE Secure Connections
→ pairing-time CSIS SIRK / Set Size / Rank 验证
→ rank=2
→ 原子提交

Stored verify:
Host IRK resolve 当前 RPA
→ FAL ACL
→ stored LTK 恢复加密
→ encrypted CSIS verify
→ 无 SMP、无持久化修改

R3:
一次扫描确认两成员可见
→ Controller Resolving List 装载固定 identity + peer IRK + local IRK
→ FAL 使用固定 identity
→ Controller 实时解析当前 RPA
→ 同时持有两条 encrypted ACL
→ 两耳分别发现 PACS / ASCS / CAS / included CSIS
→ 得到左右位置、LC3 PAC、ASE topology
→ 双断开
→ Resolving List 清理
→ BondStore fingerprint 完全不变
```

最终硬件事实：

```text
Right:
identity = E8:C9:13:EF:2F:23
CSIS rank = 1
Sink Audio Location = 0x00000002
ACL handle in final R3 HIL = 0x0800

Left:
identity = E8:C9:13:F6:5C:E0
CSIS rank = 2
Sink Audio Location = 0x00000001
ACL handle in final R3 HIL = 0x0801
```

---

# 3. 工作流决定

本轮中途明确修订了开发工作流。

## 3.1 外部参考源码不再反复读 ZIP

用户指出重复读取压缩包容易触发系统限制，因此将 6 个分卷和嵌套包全部展开到：

```text
/mnt/data/resources
```

主要源码：

```text
AOSP-Bluetooth
zephyr-main
bumble-main
bumble-rs-main
btstack-master
bt-hci-main
burble-main
trouble-main
le-audio-receiver-main
liblc3-main
libusb-master
rusb-master
```

后续规则：

```text
源码研究：读取展开目录
项目代码读取：GitHub 或本地项目
项目代码写入：GitHub connector
构建验证：ToolHub
不在 assistant sandbox 内编辑仓库
```

## 3.2 不再用 GitHub API 代替本地大规模源码阅读

曾出现过大量 GitHub connector 调用，导致系统长时间思考或被中断。根因不是代码安全问题，而是：

```text
- 过量远程源码读取
- 大量长工具调用
- 大文件/多调用链
```

之后改为：

```text
外部库：本地展开源码
仓库写入：GitHub
本地编译：ToolHub
```

## 3.3 每个正在搭建的模块默认输出 verbose log

用户明确要求：

> 当前模块在搭建时，每一步都输出 verbose log，后续继续接时不依赖额外调试参数。

因此新增或保留的关键日志域：

```text
[R2/USB]
[R2/RAW-HCI]
[R2/ACL]
[R2/CONN-MGR]
[R2/BOND-MGR]
[R2/SECURITY]
[R2/SET-VERIFY]
[R2/BOND-COMMIT]
[R2/BOOTSTRAP]
[R3/PRIVACY]
[R2/HCI-RX]
```

这条决定直接帮助定位了后续三个隐藏问题：

```text
- SMP Encryption Change / Key Distribution 端点顺序
- Bumble-RS peer address type 丢失
- R3 旧 RPA 缓存失效
```

---

# 4. Pairing 前期：为什么一直连不上

## 4.1 初始错误：把同名 LE Audio advertiser 当第一次 SMP transport

最初扫描可见多个：

```text
Galaxy Buds3 Pro (2F23)
E8:C9:13:EF:2F:23/P
E8:C9:13:F6:5C:E0/P
以及若干 RPA
```

旧逻辑直接从同名 candidate 选择一个连接。

典型结果：

```text
HCI 0x2043 Command Status SUCCESS
→ 等待 LE Enhanced Connection Complete
→ 10 秒 timeout
→ cancel status=Some(12)
```

一开始容易误判为：

```text
地址错
耳机不允许连接
盒子 pairing mode 退出
第二成员需要 RPA
```

实际根因后来分成两层：

```text
第一次 Pairing：
应使用 pairing-only Microsoft Swift Pair transport

Set Member direct connection：
应使用成熟 Host 的 Filter Accept List initiating，而非 explicit peer
```

---

# 5. Advertiser 取证与三星/Buds3 Pro 广播面

## 5.1 观察到的广播类别

Buds3 Pro 在不同状态下会出现：

```text
1. Microsoft Swift Pair pairing-only RPA
2. 两个固定 public identity-looking LE Audio advertiser
3. 两耳各自轮换的 RPA
4. 盒子/无名称/厂商 beacon
5. 一批形如 xx:xx:37:23:18:46 的异常 public-looking advertiser
```

## 5.2 “18:46”异常 public advertiser

Raw HCI 中观察到：

```text
event_type=0x0041
connectable=true
data_status=2 (incomplete-truncated)
address_type=public
SID=3
data_len=0
```

地址如：

```text
13:14:37:23:18:46
27:1C:37:23:18:46
1D:1B:3F:14:18:46
```

该现象并非 Bumble 独有：用户此前在 Fedora + AX210 也见过类似 unnamed public-address 广播。

本轮结论：

```text
[UNRESOLVED QUIRK]
- 它可能与 Samsung OEM 广播面、扩展广播分片或控制器/固件报告有关。
- Raw HCI 证明 Host 收到的完整 HCI report 中已经存在这些值。
- data_status=2 且 data_len=0，说明它们并非完整有效业务广告。
- 与 Pairing 主路径无直接必要关系。
- 本轮在修好 HCI framing 后停止继续深挖。
```

不应把这些 advertiser 当成成员身份或 pairing transport。

---

# 6. USB/HCI framing 类问题

## 6.1 一个 USB transfer 内包含多个 HCI frame

曾出现：

```text
[R2/USB] one interrupt transfer contained 11 complete HCI frame(s)
```

旧 transport 假设：

```text
一次 USB interrupt read = 一个 HCI Event
```

但 BE200/WinUSB 实际会：

```text
一个 transfer 内拼接多个 HCI Event
Event 和 ACL 都可能需要 native-header reassembly
```

修复：

```text
- Event+ACL native-header reassembly
- 一次 read 可拆多个完整 frame
- incomplete frame 保留到下一次 read
```

## 6.2 `invalid HCI packet type 0x00`

Raw HCI trace 初版直接在不正确的 packet boundary 上 tap，导致：

```text
invalid HCI packet type 0x00
```

来源：

```text
USB framing / H4 packet boundary错误
而非耳机发送非法 HCI
```

修复后 trace 放在：

```text
post USB framing/HCI packet parse
pre ExternalHost/Device
```

## 6.3 partial timeout preservation

最终 R3 GATT discovery 期间出现：

```text
[R2/USB] interrupt-IN partial read preserved:
endpoint=0x81
libusb_status=-7
bytes=7
forwarding received bytes
```

`-7` 是 libusb timeout，但 transfer 超时前已经收到 7 字节。

错误实现会：

```text
看到 timeout
→ 丢弃已经收到的 bytes
```

当前决定：

```text
若 timeout 同时有 partial bytes：
→ 保存并送入 HCI reassembler
```

后续 PACS/ASCS discovery 成功，证明这是必要的 Windows/WinUSB quirk 处理，不是错误告警。

## 6.4 控制器切错导致 `Operation not supported`

曾出现：

```text
Bumble attach failed:
USB transport error:
Operation not supported or unimplemented on this platform
```

最终不是代码问题，而是用户打电话时把 Bluetooth 切到了 MTK 控制器，忘记切回 BE200。

决定：

```text
- fast preflight 必须验证唯一 8087:0036
- 不把该错误归因于 Bumble 或 BE200
```

---

# 7. 首次 Pairing transport：Swift Pair

增加 Swift Pair probe 后找到：

```text
Microsoft company ID
beacon = 0x03
subscenario = 0x02
display_name = Galaxy Buds3 Pro (2F23)
```

Swift Pair RPA 可以立即建 ACL：

```text
HCI 0x2043 Command Status SUCCESS
LE Enhanced Connection Complete SUCCESS
ACL hold 2s PASS
```

第一次正式 SMP 也走通：

```text
WaitPairingResponse
→ WaitPublicKey
→ WaitPairingConfirm
→ WaitPairingRandom
→ WaitDhKeyCheck
→ WaitEncryption
→ KeyDistribution
→ Complete
```

Primary 最终分发：

```text
identity = E8:C9:13:EF:2F:23
address_type = public
LTK = present
IRK = present
CSIS rank = 1
Set Size = 2
```

这证明：

```text
[CONFIRMED]
首次 Pairing 入口是 Swift Pair pairing transport，
不是两个 public LE Audio identity advertisement。
```

---

# 8. 第二成员发现：RSI 是候选证据，不是最终信任

Primary 完成后获得 SIRK，扫描第二成员。

日志证明：

```text
E8:C9:13:F6:5C:E0/P
RSI = MATCH
```

本轮最终确定的证据层级：

```text
RSI MATCH:
- 说明 advertiser 很可能属于目标 coordinated set
- 用于创建 ExpectedSetMember
- 不足以永久信任或提交 bond

Encrypted CSIS SIRK read:
- authoritative membership proof
- SIRK 必须与已存 set 完全一致
- Set Size 必须一致
- rank 必须合法
```

最终决策：

```text
SMP 只产生 ProvisionalBond
→ CSIS gate PASS
→ commit

SIRK mismatch / read failure
→ discard provisional keys
→ disconnect
→ 恢复 discovery
```

---

# 9. 被推翻的第二成员假设

本轮曾提出并随后撤回的假设：

## 9.1 “第二只必须用 RPA”

被推翻。

实际第二成员成功 Pairing 使用：

```text
E8:C9:13:F6:5C:E0/P
address_type = public
```

## 9.2 “public advertiser 只是 identity，不可连接”

被推翻。

它通过 Controller Filter Accept List 可在约 15–46 ms 内建链。

## 9.3 “第二只必须再次出现 Swift Pair anchor”

被推翻。

Swift Pair pairing-only transport用于 primary ceremony。Set member 由 RSI + 通用 Bond Manager 进入。

## 9.4 “第二只 timeout 是因为 local IRK 丢失”

这是一个合理但未证实的中间假设。

稳定 local identity / local IRK 确实是成熟 Host 的必要基础，因此最终保留并持久化。但 HIL 最终证明第二只 ACL timeout 的直接原因是：

```text
explicit-peer initiating 错误
```

只切换到 FAL，未变更 local identity，第二成员立即连接。

因此：

```text
[DECISION]
稳定 local IRK：保留，作为正确 Host 基础设施。
“它是 ACL timeout 根因”：撤回。
```

---

# 10. 成熟栈源码研究带来的架构转向

在反复 HIL 后，正式阅读：

```text
AOSP Fluoride CSIS / LE Audio / Security / ACL
Zephyr CSIP Set Coordinator / Host connection
Bumble-RS
```

得到核心结论：

```text
CSIS Client 不应自己决定 HCI initiating、privacy、SMP 或持久化。
```

AOSP 逻辑：

```text
连接 primary
→ 读取 SIRK
→ RSI 搜索 expected member
→ 暂停扫描
→ 通知上层
→ 上层 generic LE bond
→ pairing-time SIRK verify
→ connect profile
→ 继续找剩余成员
```

Zephyr 逻辑同样是：

```text
Coordinator 只识别 RSI 和 set membership
连接、privacy、bond 交给通用 Host
```

本轮架构决定：

```text
Bumble-RS 提供 protocol primitives
AOSP / Zephyr 提供 control-plane 结构
BE200AudioHub 只实现薄 orchestration 和 BE200/Windows transport quirks
```

---

# 11. Filter Accept List：第二成员 ACL 的真正根因

旧连接：

```text
LeExtendedCreateConnection
initiator_filter_policy = 0
peer_address = E8:C9:13:F6:5C:E0
```

结果：

```text
Command Status SUCCESS
无 Connection Complete
10s timeout
cancel status=0x0C / Some(12)
```

新 generic connection manager：

```text
LeClearFilterAcceptList
→ LeAddDeviceToFilterAcceptList
→ LeExtendedCreateConnection
   initiator_filter_policy = 1
   peer_fields = ANY
```

结果：

```text
Command Status SUCCESS ~15 ms
Connection Complete SUCCESS ~15–46 ms
```

结论：

```text
[CONFIRMED ROOT CAUSE]
第二成员地址、RSI、connectable 属性均正确。
失败来自 Host 绕过通用 LE Connection Manager，
错误使用 explicit peer initiating。
```

`cancel=status=0x0C` 的解释：

```text
Command Disallowed
```

通常发生在 timeout 边界，控制器 initiator 已经不处于可取消状态或状态机没有按 Host 假设 settle。它是错误 initiating 生命周期的伴随症状，不是对端拒绝码。

---

# 12. Pairing 事务化重构

新增/收束的主要概念：

```text
LeConnectionManager
LeBondManager
ProvisionalBond
SetMemberVerifier
commit_primary_bond
commit_verified_set_member
BondStore
```

最终 Pairing 主线：

```text
ACL
→ provisional SMP
→ encrypted GATT CSIS
→ trust gate
→ atomic commit
```

Primary 与 member 的差异：

```text
Primary:
- CSIS 建立 set 的初始 SIRK / Set Size / Rank
- 读取成功后才 commit

Member:
- 必须与已有 SIRK / Set Size 完全匹配
- PASS 后 commit
```

持久化边界：

```text
commit.rs 是唯一 peer keys + set metadata 写入口
```

若 metadata 写入失败：

```text
- 尝试恢复旧 keys
- 报告 rollback 结果
```

---

# 13. SMP Bug 1：BE200 USB endpoint delivery order

第一次 transactional primary SMP 失败在：

```text
WaitDhKeyCheck
→ LE Start Encryption
→ WaitEncryption
→ RX IdentityInformation
→ local PairingFailed UnspecifiedReason
```

关键证据：

```text
IdentityInformation 是 Key Distribution PDU。
它合理出现的前提是链路加密已生效。
```

但 Host 内部：

```text
Bulk-IN 0x82:
加密后的 SMP IdentityInformation 先到

Interrupt-IN 0x81:
HCI Encryption Change 后到
```

旧 Bond Manager 顺序：

```text
Device::poll
→ 先处理 queued SMP PDU
→ 后检查 encrypted state
```

导致：

```text
ScPairingSession 仍为 WaitEncryption
→ 把合法 Key Distribution 判为乱序
→ 本地主动发送 PairingFailed
```

修复决定：

```text
正常路径：
Encryption Change
→ mark_encrypted
→ KeyDistribution

BE200 endpoint-order path：
LE Start Encryption 已提交
+ 收到合法 Key Distribution PDU
→ 推断链路实际上已加密
→ mark_encrypted
→ 处理该 PDU
→ 仍必须等待 HCI Encryption Change 作 controller confirmation
→ confirmation 后才允许进入 GATT verifier
```

这不是降低安全标准：

```text
PDU 只用于修正 Host 状态机时序
HCI Encryption Change 仍是 GATT 前必要确认
SIRK gate 仍是持久化前信任边界
```

---

# 14. SMP Bug 2：Bumble-RS 丢失 LE public peer address type

第二成员在 DHKey Check 阶段收到：

```text
Pairing Failed reason=0x0B
```

0x0B：

```text
DHKey Check Failed
```

源码审计发现：

```text
Bumble-RS 的 LE Meta Event address parser
先把 peer address 构造成 RANDOM_DEVICE

虽然 HCI Enhanced Connection Complete
另有正确 peer_address_type 字段，
Host metadata 路径没有保留/使用它。
```

Primary 是 RPA：

```text
真实 random
错误标 random
碰巧一致
```

Secondary 是 public：

```text
真实 public = 0
Host metadata random = 1
```

Secure Connections f5/f6 输入包含：

```text
address bytes + address type bit
```

因此双方只差一个 bit：

```text
本机计算 responder type=random
耳机计算 responder type=public
→ DHKey Check mismatch
→ peer PairingFailed 0x0B
```

修复：

```text
仅当：
- caller 提供 expected public identity
- Host peer address bytes 与 expected identity 完全一致
- 只有 type 不同

才把 SC link address type 修正为 public。
```

严禁：

```text
把所有 random 地址强制改 public
把 RPA 冒充 identity
```

修复后：

```text
RX PairingDhKeyCheck
→ Encryption
→ Key Distribution
→ ProvisionalBond READY
→ CSIS PASS
→ commit
```

---

# 15. Pairing 完整成功

最终第二成员成功日志核心：

```text
SC address context:
initiator=44:A3:BB:36:94:1B type=0
responder=E8:C9:13:F6:5C:E0 type=0
host_metadata_responder=... type=1

corrected pinned Bumble-RS LE address-type loss

RX PairingDhKeyCheck
LE Start Encryption
Encryption Change status=0
IdentityInformation
IdentityAddressInformation

provisional transaction READY
persistent store unchanged

SET-VERIFY PASS:
set_size=2
rank=2

BOND-COMMIT PASS
```

最终 set：

```text
E8:C9:13:EF:2F:23 rank=1 verified=true
E8:C9:13:F6:5C:E0 rank=2 verified=true
```

---

# 16. Stored Bond Verification

Pairing 完成后新增只读 verify：

```text
IRK resolve
→ FAL ACL
→ stored-LTK encryption
→ encrypted CSIS read
→ SIRK / Set Size / Rank exact compare
```

严格不执行：

```text
SMP Pairing Request
新 bond
key update
set metadata update
local identity update
```

最终实机：

```text
Member 1:
RPA resolve PASS
ACL PASS
stored LTK PASS
CSIS rank=1 PASS

Member 2:
RPA resolve PASS
ACL PASS
stored LTK PASS
CSIS rank=2 PASS
```

文件 fingerprint：

```text
pairing-keys.json unchanged
bond-sets.json unchanged
local-identity.json unchanged
```

因此 Pairing 正式封盘。

---

# 17. Windows PowerShell mojibake

日志中的：

```text
ΓÇö
```

原文是：

```text
—
```

UTF-8 字节：

```text
E2 80 94
```

Windows PowerShell 5.1 native pipeline / `Tee-Object` 用错误代码页解释，显示为：

```text
Γ Ç ö
```

决定：

```text
CLI 不依赖 Unicode 标点。
统一在输出边界转换：
— / – → -
→ → ->
… → ...
```

新增：

```text
src/console_ascii.rs
crates/bumble-adapter/src/console_ascii.rs
```

避免逐字符串清理。

---

# 18. ACL 以上静态审计与路线收束

Pairing 成功后，活动树中仍存在：

```text
- 旧 bond-onboard / bond-verify
- 独立 FAL probe
- pairing-v2 onboarding / verify
- bond_manager v1/v2/v3/v4
- explicit-peer connection
- local-RPA connection experiment
- 未接真实 HIL 的 pairing-orchestrator
- primary/member 两套重复 commit
```

正式静态审计后：

```text
新增/重写约 677 行
删除约 5,367 行
净减少约 4,690 行
```

活动模块收束为：

```text
connection_manager.rs
    唯一 ACL 创建入口

acl_session.rs
    linger / disconnect

security.rs
    provisional Secure Connections

stored_security.rs
    stored LTK 恢复

csis.rs
    RSI 和 CSIS

set_member_verifier.rs
    SIRK / Set Size / Rank gate

commit.rs
    唯一持久写入口

bond.rs
    BondStore / local identity / set metadata / Host IRK resolve

gatt_session.rs
    encrypted ATT ownership bridge

src/pairing/onboard.rs
    write-capable pairing orchestration

src/pairing/verify.rs
    read-only verify
```

删除/弃用：

```text
bond_manager.rs
bond_manager_v2.rs
bond_manager_v3.rs
旧 connect.rs
旧 pairing.rs
primary_commit.rs
pairing-orchestrator crate
r2_pairing_refactor.rs
legacy pairing CLI
```

归档：

```text
deprecated/r2-pairing-experiments/
```

---

# 19. 自动固件初始化

用户希望：

```text
如果 BE200 断电后仍在 Bootloader，
运行 R2/R3 命令时自动初始化。
```

新增统一 gate：

```text
controller_bootstrap::ensure_operational()
```

固定 firmware 路径：

```text
.\firmware\
be200_intel_windows_24.60.0.4_usb_verified.sfi
ibt-0291-0291.ddc
```

状态判断：

```text
image_type=0x03:
- 已 Operational
- 不读固件文件
- 不下载
- 继续原命令

image_type=0x01:
- 读取固定资产
- SHA-256 校验
- 硬件/SBE 校验
- 单次 boot
- 确认 0x03
- 继续原命令

其他:
- 拒绝
```

安全约束：

```text
无 retry
无 USB reset
无 power cycle
无 reset-to-bootloader
不搜索其他 SFI
不联网下载
```

已验证：

```text
Operational fast path
```

尚待单独回归：

```text
真正断 3.3V 后，
直接运行 R2/R3 命令，
验证自动 Bootloader → Operational。
```

这是 bootstrap HIL，不是 Pairing reopen。

固件资产由用户手动添加/推送；本轮没有对 Intel firmware redistribution 作法律结论。

---

# 20. R3 初版：直接接已有 PACS/ASCS/CAP 实现

用户要求：

> 先读已有代码库，只往里面接东西，不重写协议。

读取 Bumble-RS：

```text
pacs.rs
ascs.rs
cap.rs
bap.rs
gatt client
```

确认已有：

```text
PublishedAudioCapabilitiesServiceProxy
AudioStreamControlServiceProxy
CommonAudioServiceProxy
PacRecord
CodecSpecificCapabilities
AudioLocation
AudioContexts
AseStatus
AseStatusDetails
AseOperation
```

因此决定：

```text
BE200AudioHub 不重写：
- PAC parser
- ASE parser
- Audio Location
- Context
- Metadata
- GATT Read Blob
- CAS include discovery
```

只新增：

```text
gatt_session.rs
bap-client snapshot.rs
bap-client discovery.rs
src/audio_set.rs
```

---

# 21. R3 初版失败：扫描时 RPA 到连接时已失效

第一次 R3：

```text
共享扫描 8 秒
→ resolve 两只 RPA
→ 选择 rank 1 RPA
→ FAL 放该 RPA
→ 10 秒 timeout
```

这不是 FAL 本身回归，因为 Pair verify 曾成功。

差异：

```text
Pair verify:
每只临连接前重新扫描
→ 立即用刚看到的 RPA

R3 初版:
一次共享扫描
→ 后续连接两个缓存 RPA
→ 时间窗口更长
```

结论：

```text
[CONFIRMED]
缓存 RPA 不是 bonded long-lived connection target。
RPA 或 Samsung 广告面可能在 scan→connect 间变化。
```

---

# 22. Controller Resolving List：R3 正确 privacy 路线

最终 R3 privacy：

```text
Host scan + IRK resolve：
用于确认两只成员当前可见

Controller Resolving List：
- peer identity
- peer IRK
- local IRK

FAL：
放固定 public identity

Controller：
实时解析当前空口 RPA
→ 与 FAL identity 匹配
```

配置：

```text
LeSetAddressResolutionEnable(0)
LeClearResolvingList
LeReadResolvingListSize
LeAddDeviceToResolvingList × 2
LeSetResolvablePrivateAddressTimeout
LeSetAddressResolutionEnable(1)
```

连接目标：

```text
E8:C9:13:EF:2F:23 type=public
E8:C9:13:F6:5C:E0 type=public
```

最终 HCI Enhanced Connection Complete 同时报告：

```text
resolved identity
peer RPA
```

rank 1：

```text
identity = E8:C9:13:EF:2F:23
RPA = 62:3C:EE:E4:EB:24
handle = 0x0800
```

rank 2：

```text
identity = E8:C9:13:F6:5C:E0
RPA = 50:35:66:2B:A4:96
handle = 0x0801
```

这是一条硬证据：

```text
Controller-side address resolution 正常工作。
```

退出时：

```text
disable address resolution
clear resolving list
```

---

# 23. R3 双 ACL 与 Profile Discovery 最终结果

## 23.1 两条 ACL 同时存在

```text
rank 1 handle=0x0800 encrypted
rank 2 handle=0x0801 encrypted
```

第二只连接和加密期间第一只没有断开。

结论：

```text
BE200 + 当前 Host + USB event pump
可以同时维持双成员 encrypted ACL。
```

## 23.2 PACS

两耳对称：

```text
Sink PAC records = 1
Source PAC records = 1
```

Sink PAC：

```text
Coding Format = 6 (LC3)
Company ID = 0
Vendor Codec ID = 0
SupportedSamplingFrequency = 180
SupportedFrameDuration = 3
SupportedAudioChannelCount = [1]
MinOctetsPerCodecFrame = 26
MaxOctetsPerCodecFrame = 155
MaxCodecFramesPerSdu = 1
```

按本轮解释：

```text
Sampling:
16 / 24 / 32 / 48 kHz

Frame duration:
7.5 / 10 ms

Channel:
mono

Octets:
26..155

Frames/SDU:
1
```

## 23.3 ASCS

每耳：

```text
Sink ASE = 2
Source ASE = 1

Sink ASE 1:
value_handle = 0x0046
state = Idle

Sink ASE 2:
value_handle = 0x0049
state = Idle

ASCS Control Point:
0x0043
```

第一版播放只需：

```text
每耳选择一个 Idle Sink ASE
```

当前最小 deterministic policy：

```text
每耳选择最低 ASE ID：
ASE ID 1
```

## 23.4 CAS / included CSIS

两耳：

```text
CAS = 0x007D..0x007E
included CSIS = 0x006E..0x0079
```

证明：

```text
两成员是 CAP coordinated audio set，
不再依赖名称启发式证明属于同一音频集合。
```

## 23.5 Context

两耳：

```text
Supported Sink   = 0x0F7F
Available Sink   = 0x0F7F
Supported Source = 0x026F
Available Source = 0x026F
```

Supported = Available，当前无上下文被临时屏蔽。

---

# 24. 左右耳映射的重要决定

实际：

```text
rank 1 = Right
rank 2 = Left
```

因此：

```text
CSIS Rank 只用于 set 内序号/排序。
Audio Location 才是 Left/Right 的唯一真相源。
```

严格禁止：

```text
rank 1 = left
rank 2 = right
```

后续 Stream Planner：

```text
Right:
E8:C9:13:EF:2F:23
rank 1
location 0x2

Left:
E8:C9:13:F6:5C:E0
rank 2
location 0x1
```

---

# 25. R3 Snapshot 与 ownership 决定

新增：

```text
AudioSetSnapshot
AudioMemberSnapshot
PacsSnapshot
AscsSnapshot
CapSnapshot
AseSnapshot
```

保留的关键 handles：

```text
PACS service range
Sink/Source PAC handles
Sink/Source Audio Location handles
Supported/Available Context handles

ASCS service range
Sink/Source ASE value handles
ASCS Control Point handle

CAS service range
included CSIS range
```

新增 encrypted GATT bridge：

```text
AttachedController::with_encrypted_gatt()
```

职责：

```text
确认 ACL connected
确认 ACL encrypted
创建 ExternalAttTransport
创建 GattClient
把一次 GATT 操作交给 bap-client
```

`bap-client` 不直接拥有：

```text
ExternalHost
Device
USB transport
HCI
```

保持 adapter 边界。

---

# 26. 已知上游限制

Bumble-RS PACS Proxy 当前模型：

```text
sink_pac: Option<CharacteristicProxy>
source_pac: Option<CharacteristicProxy>
```

每方向只保留一个 PAC characteristic instance。

但一个 PAC characteristic 内可包含多个 `PacRecord`。

Buds3 Pro 当前：

```text
每方向一个 PAC characteristic
```

因此本轮无影响。

决定：

```text
不提前修未来通用设备问题。
出现多同方向 PAC characteristic 的真实设备时再扩展 upstream/proxy。
```

---

# 27. R3 封盘条件全部满足

最终 R3：

```text
- 两成员可见
- Controller Resolving List 配置成功
- 两 identity FAL 连接成功
- 两 ACL 同时 encrypted
- PACS mandatory PASS
- ASCS mandatory PASS
- CAS present
- included CSIS present
- Sink PAC LC3 PASS
- 每耳至少一个 Sink ASE
- 左右 Audio Location 唯一明确
- 双断开成功
- Controller Resolving List 清理
- Pairing state fingerprint unchanged
```

结束：

```text
R3 AUDIO-SET INSPECT PASS
```

R3 正式封盘。

---

# 28. 本轮 Quirk / Bug 总表

| 编号 | 现象 | 来源 | 最终处理 |
|---|---|---|---|
| Q1 | `Operation not supported` | 实际切到了 MTK 控制器 | 强化 BE200 fast preflight |
| Q2 | HCI packet type `0x00` | USB/HCI framing boundary 错 | Event+ACL reassembly |
| Q3 | 单次 interrupt transfer 有多个 frame | BE200/WinUSB batching | 拆分多 HCI frame |
| Q4 | timeout 同时有 partial bytes | WinUSB/libusb completion 语义 | 保留 partial bytes |
| Q5 | `ΓÇö` | PowerShell 5.1 native pipe 编码 | 输出边界 ASCII sanitizer |
| Q6 | 第一次同名 public adv 无法 Pair | pairing ceremony transport 选错 | Swift Pair primary |
| Q7 | explicit peer 0x2043 timeout | 绕过通用 connection manager | FAL direct connect |
| Q8 | cancel `0x0C` | initiator 生命周期/状态不匹配 | 通用 cancel + settle；根因移除 |
| Q9 | 第二只被要求 RPA | 错误上层策略 | 删除；CSIS 不决定地址类型 |
| Q10 | 第二只被要求 Swift Pair anchor | 错误 ceremony 假设 | 删除；RSI → generic bond |
| Q11 | Key Distribution 先于 Encryption Change | BE200 Bulk-IN/Interrupt-IN delivery order | infer handoff + 后续 controller confirm |
| Q12 | public peer 被 Bumble 当 random | pinned Bumble-RS address type 丢失 | 有条件修正 SC address type |
| Q13 | 共享扫描 RPA 稍后失效 | RPA/广告面轮换 | Controller Resolving List + identity FAL |
| Q14 | 盒灯熄灭后成员不可见 | pairing mode/广告状态变化 | 不破坏已有 bond；提示重新进入 formal mode |
| Q15 | 某些 RPA 广告无 RSI | 扩展广播片段/广告状态并非每包含 RSI | 只在完整 RSI 可见时建候选 |
| Q16 | `18:46` unnamed public advertisers | Samsung/扩展广播/控制器行为未完全解释 | 归档为 unresolved forensic quirk |
| Q17 | local IRK 被怀疑是 ACL timeout 根因 | 合理但未证实的 privacy 假设 | 保留稳定 local identity，撤回直接根因判断 |
| Q18 | 多代 BondManager 同时存在 | 快速 HIL 演化遗留 | 静态审计后只保留 `security.rs` |
| Q19 | Pairing 和 Profile discovery 混杂风险 | 单体 orchestration | Pairing / GATT / BAP 分层 |
| Q20 | `Cargo.lock` 长期 dirty | 本地依赖锁变动 | 始终不随无关提交 staged |

---

# 29. 最终决策清单

## 29.1 已锁死

```text
- Pairing CLOSED
- R3 discovery CLOSED
- Filter Accept List 是唯一 generic direct-connect 路线
- Bonded privacy 使用 Controller Resolving List
- Stored verify 不发送 SMP
- SMP keys 在 CSIS gate 前始终 provisional
- commit.rs 是唯一持久写边界
- Left/Right 只看 PACS Audio Location
- 外部协议优先复用 Bumble-RS
- 不在产品层重写 PACS/ASCS/CAP parser
```

## 29.2 明确禁止回归

```text
- explicit-peer connection 作为默认路线
- 缓存 RPA 后延迟直连
- 第二成员必须 RPA
- 第二成员必须 Swift Pair
- Pairing 完成即持久化
- rank 推导左右
- CSIS 层决定 HCI privacy
- 一个大函数同时做 scan/connect/SMP/GATT/persistence
```

## 29.3 暂缓

```text
- 麦克风上行
- 通话双向
- MCP/TBS/MICS
- Broadcast/Auracast
- 多品牌泛化
- GUI
- 自适应码率
- 空间音频
```

---

# 30. 当前固件与持久化路径

固件：

```text
C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub\firmware\
```

BondStore：

```text
%LOCALAPPDATA%\BE200AudioHub\bonding\
├─ controller-identity.txt
├─ local-identity.json
├─ pairing-keys.json
├─ bond-sets.json
└─ archive\
```

当前 bond 是有价值状态，不应随意 reset。

---

# 31. 当前 CLI

```powershell
# Pairing
.\target\debug\be200hub.exe r2 pair onboard `
    --seconds 5 `
    --name "Galaxy Buds3 Pro (2F23)"

.\target\debug\be200hub.exe r2 pair verify `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)"

# R3
.\target\debug\be200hub.exe r3 audio-set inspect `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)"
```

`r2 pair reset --confirm` 只用于明确的重新配对迁移，不能作为普通修复手段。

---

# 32. 重要 commit 里程碑

本轮选择性里程碑：

```text
6158e9a  初始 R2.4 resumable dual-member onboarding
ffca729  matching RSI second-member candidate
f224488  FAL generic connection manager
046d74f  endpoint-order encryption handoff
4fa415c  public address type correction for SC
cd7f1f3  stored dual-member verification
81f62f5  ASCII console cleanup
7ddc554  above-ACL route consolidation
7db8078  remove redundant public APIs
b307cc3  automatic controller bootstrap
5391134  initial R3 integration
a80cf1e  controller-resolved identity connection for R3
```

用户后续可能另有 firmware asset commit；`a80cf1e` 是本轮 assistant 明确完成 HIL 验证的 R3 integration commit。

---

# 33. 下一步：R4.1 Codec Configuration Planner

输入：

```text
AudioSetSnapshot
```

输出：

```text
CodecConfigurationPlan
```

当前 first-sound candidate：

```text
LC3
48 kHz
10 ms
mono per ear
100 octets/frame
1 frame/SDU
Media context
每耳最低编号 Idle Sink ASE
```

为什么可行：

```text
48 kHz supported
10 ms supported
mono supported
100 ∈ [26, 155]
1 frame/SDU supported
```

Planner 必须：

```text
- 纯计算
- 验证两耳 capability intersection
- 用 Audio Location 分左右
- 选择 Idle Sink ASE
- 输出 deterministic plan
- 单元测试覆盖 capability rejection
```

不得：

```text
- GATT
- ASCS write
- HCI
- persistence
```

---

# 34. R4.2 ASCS Config Codec

R4.1 后：

```text
两耳保持 encrypted ACL
→ 复用 ASCS Proxy
→ 订阅 chosen ASE
→ 订阅 ASCS Control Point
→ Config Codec
→ 等待两耳 ASE 进入 Codec Configured
→ 读取 peer preferred QoS / presentation delay
```

此阶段不做：

```text
CIG
CIS
ISO Data Path
LC3 发送
```

成功输出：

```text
CodecConfiguredSetSnapshot
```

作为 R5 QoS Planner 输入。

---

# 35. 未完成但需要记住的事项

## 35.1 自动冷启动 gate 实机回归

代码已接，Operational fast path 已过。

仍需：

```text
真正断 BE200 3.3V
→ 重新上电至 Bootloader
→ 直接运行 R2/R3 命令
→ 验证自动固件初始化后继续业务
```

## 35.2 固件 redistribution

用户决定将固件提交仓库。

技术层已有：

```text
固定路径
固定 hash
固定硬件/SBE 校验
```

但正式公开发布前应单独处理：

```text
Intel license / attribution / redistribution
```

本轮不作法律结论。

## 35.3 upstream bug 处理

本地修复了：

```text
- Bumble-RS LE public peer address type 丢失
- Windows USB partial timeout
- BE200 endpoint-order handoff
```

后续可考虑：

```text
- 提交最小 upstream issue/patch
- 或保留 pinned adapter patch seam
```

当前不应为了 upstream 清洁度牺牲已验证 HIL。

---

# 36. 本轮最终一句话总结

> Round3 将 BE200AudioHub 从“能看到并尝试配对 Buds3 Pro”的实验性 Host，收束为一条经过真实硬件验证的完整控制平面：自动启动 BE200、事务化 Pair 两个 CSIS 成员、持久恢复两条加密 ACL、由控制器处理 RPA privacy，并在不修改 bond 的情况下读取两耳完整 PACS/ASCS/CAS 音频拓扑；Pairing 与 R3 均已封盘，下一阶段只应向 Codec Config 与 QoS/CIS/ISO 前进。
