# BE200AudioHub — Round 4 完整工程纪要

> **范围**：从上一份 R2/R3 交接的 R4.1 Codec Planner 起步，到 R5.2-B 双耳 CIS 建立及完整清理成功为止。  
> **时间**：2026-10-13—2026-10-14（本回合）；记录生成：2026-10-14。  
> **性质**：工程事实记录 + 设计决定账本 + Bug/Quirk 溯源 + HIL 证据索引；不是宣传性进度汇报。  
> **主仓库**：私有 `STanJK/BE200AudioHub`；**工作分支**：`r2/stack-integration-bootstrap`。  
> **最后核对 HEAD**：`eac486691fd4d7181d36bb09a620e942d0026936`。  
> **本地工作区**：`C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub`。  
> **Git 状态**：与远端分支同步，仅 `M Cargo.lock`（此前已经存在的本地未暂存变更；本回合未主动覆盖）。  
> **核心结论**：R4.1、R4.2、R5.1、R5.2-A、**R5.2-B 全部达到真实设备 HIL PASS**。双耳控制面已经打通；**尚无 ISO Data Path / LC3 音频发送 / First Sound**。

---

## 0. 一页摘要：当前进度和不能混淆的边界

| 里程碑 | 最后状态 | 证据类型 | 本回合完成的最重要事情 |
| --- | --- | --- | --- |
| R1 | 继承基线 | 既有 HIL；本回合首次 R5.2-B 中又观测到成功冷启动 | Intel BE200 WinUSB / Operational 固件 / HCI |
| R2 | 继承 CLOSED | 上回合交接 + 本回合所有 HIL 重连 | 两耳已有 Bond、IRK/LTK、CSIS，可无 SMP 恢复加密 ACL |
| R3 | 继承 CLOSED | 本回合所有 HIL | 双耳同时 ACL+加密、PACS/ASCS/CAS 探测、左右音频位置 |
| **R4.1** | **完成** | Planner + 双耳真实数据 | 选择 LC3、1 条 Sink ASE/耳、生成合法 Codec Config 参数 |
| **R4.2** | **HIL PASS** | 左耳和双耳 Config Codec/Release | CP 成功 + ASE 通知 + GATT Readback + 全部回 Idle |
| **R5.1** | **HIL PASS** | 双耳 QoS Planner Dry Run | 读取真实 QoS Preferences，生成 CIG/CIS 与 ASCS QoS 字节（不发送） |
| **R5.2-A** | **HIL PASS** | BE200/双耳真实控制面 | Set CIG、两个 CIS Handle 分配、双耳 QoS Configured、Release、Remove CIG |
| **R5.2-B** | **HIL PASS** | 最终 `r5-establish-cis-3.log` | 两条真实 CIS Established，双耳 Streaming 状态、CIS Disconnect、Release、Remove CIG、Bond 不变 |
| **R5.3** | **未开始 HIL** | 无 | 下一步：HCI ISO Data Path + LC3 编码与 ISO 发送 + First Sound |
| R6 | 未来 | 无 | 常驻、异常恢复、重复切换、同步和稳定性测试 |

**不要误报**：

- `HCI LE Set CIG Parameters` 返回 CIS Handle，不等于 `LE CIS Established`；这一差别由 R5.2-A / R5.2-B 两个门槛刻意验证。
- R5.2-B 中 ASE 返回 `STREAMING (0x04)`、两条 CIS Established，**不等于有音频**；没有执行 ISO Data Path 配置或发送任何 LC3 帧。
- HCI 控制器报告 C→P `transport_latency=63,210 µs`，耳机支持/首选 `presentation_delay=40,000 µs`；这两个值并非可以直接相加或简单比较的端到端延迟测量。
- R5.2-B 第一次与第二次 HIL **不是**完整成功；只有第三次 `r5-establish-cis-3.log` 满足清理门槛。

```mermaid
flowchart LR
  R1["R1 Hardware / Firmware"] --> R2["R2 Bond + CSIS"]
  R2 --> R3["R3 Dual ACL / PACS ASCS CAS"]
  R3 --> R41["R4.1 Codec Plan"]
  R41 --> R42["R4.2 Config Codec"]
  R42 --> R51["R5.1 QoS Plan"]
  R51 --> R52A["R5.2-A Set CIG / Config QoS"]
  R52A --> R52B["R5.2-B Enable / Dual CIS / Teardown ✓ CURRENT"]
  R52B --> R53["R5.3 ISO Data Path / LC3 / First Sound — NEXT"]
  R53 --> R6["R6 Lifecycle / Recovery"]
```

## 1. 输入基线：本回合没有重新解决 R2/R3

### 1.1 硬件与控制器

| 项目 | 数据或决策 |
| --- | --- |
| 控制器 | Intel BE200；USB VID:PID `8087:0036`；WinUSB / libusb |
| 控制器 public address | `44:A3:BB:36:94:1B` |
| 固件模式 | Intel `Operational (image=0x03)` 优先直接复用；若真实为 `Bootloader (0x01)`，只允许已有 Guarded Bootstrap 执行一次受 PIN/Hash 约束的固件初始化 |
| 本回合冷启复测 | R5.2-B 第一次日志：Bootloader→一次 Secure Send 3960 fragments→Operational，成功且未发控制器 Reset / 未无限重试 |
| 固件文件 | `firmware\be200_intel_windows_24.60.0.4_usb_verified.sfi`；DDC `ibt-0291-0291.ddc`（文件名来自实测输出，不是要求下次重新刷新） |
| SFI SHA256（冷启日志） | `939a9b32ad11d8962abfd10cb24d5bd90d7848218af4ea438cf6490243a618d4` |
| HCI/USB 设计 | Interrupt-IN `0x81`；Bulk-IN `0x82`；Bulk-OUT `0x02`；Event/ACL 按 native header 重组，读超时若带有效 partial bytes 就保留 |
| LE ISO Buffer（真实能力报告） | **248 B × 8 packets**；LE CIS Central = true；LE 2M = true |
| 是否走 Linux | 否。目标 Windows 原生插入式自有 Host 栈。Linux/AOSP/Zephyr 只作为源码参考。 |

### 1.2 设备身份、CSIS 与物理声道

设备名称：`Galaxy Buds3 Pro (2F23)`。

| 物理耳朵 | Bonded identity | CSIS rank | PACS Sink Audio Location | 本回合使用的 ASE |
| --- | --- | --- | --- | --- |
| **右** | `E8:C9:13:EF:2F:23` | **1** | `0x00000002` | Sink ASE **1**, GATT value `0x0046` |
| **左** | `E8:C9:13:F6:5C:E0` | **2** | `0x00000001` | Sink ASE **1**, GATT value `0x0046` |

**设计决策**：左/右声道来自 **PACS Sink Audio Location**，不是 CSIS Rank；Rank 只是协调组标识。两个耳朵的 ASE ID 可以相同（都是 1），因为它们位于不同的 ACL/GATT 连接上。物理身份由已存 IRK 解析 RPA；LTK 恢复加密。避免频繁删除设备重配对；R4/R5 **没有新增 SMP 配对**，所有成功/失败路径均审计持久状态指纹。

本回合常见分配：右 ACL `0x0800`；左 ACL `0x0801`；R5.2-A/B CIG ID `1`，左 CIS ID `1`、右 CIS ID `2`，在这些 HIL 里控制器对应分配 CIS Connection Handle `0x0900` 与 `0x0901`。**这些 Handle 都是瞬时值，程序退出/资源移除后不可复用。**

### 1.3 双耳声学能力发现

- PACS：每耳 Sink PAC=1、Source PAC=1；ASCS Sink ASE=2、Source ASE=1；CAS 存在且包括 CSIS。
- Sink LC3 Coding Format = `0x06`，Company ID / Vendor Codec ID 均 0。
- 支持采样频率字段为 `0x00B4`（日志 Debug 表示为 `180`）；支持 10 ms 帧长度；单声道通道数 `[1]`。
- 支持每 LC3 帧 **26..155 octets**，每 SDU 最多 **1 codec frame**。
- Sink Audio Context supported/available = `0x0F7F`，Media `0x0004` 可用。
- ASCS Control Point handle `0x0043`，Sink ASE1 value `0x0046`、ASE2 `0x0049`。

详细 R2/R3 历史（包括 Pairing Transaction、身份解析、f5/f6、USB race 等）在交接包内附带的**上一份 R2-R3 handoff**；本文件不把旧结论冒充本回合新发明。

---

## 2. 本回合架构与职责分界

```text
Windows CLI: be200hub.exe
  src/main.rs                 -- CLI 解析；不同写入阶段独立确认开关
  src/audio_set.rs            -- 扫描、双加密 ACL、Bond 指纹、HIL 生命周期编排
  src/controller_bootstrap.rs -- Operational 复用 / guarded cold boot
        |
        +--> crates/bumble-adapter/
        |    init.rs / usb_fixed.rs       控制器与 HCI USB 接入
        |    gatt_session.rs             GATT 同步请求与 CP/ASE 异步通知泵
        |    iso_capabilities.rs         LE CIS/ISO 能力门槛
        |    cig_control.rs              HCI Set CIG / Remove CIG；解析 Raw Completion
        |    cis_host_feature.rs         bit32 Host Support（ACL 建立之前）
        |    cis_control.rs              HCI Create CIS / Established / Disconnect
        |
        +--> crates/bap-client/
        |    discovery.rs                PACS/ASCS/CAS 发现
        |    codec_plan.rs               纯 LC3 Codec Planner
        |    ase_read.rs                 AOSP 风格 Idle 容错，其他状态严格
        |    codec_config.rs             Codec/Config QoS/CP gate/Release 事务
        |    qos_plan.rs                 双耳 CIG/CIS/ASCS QoS 纯规划器
        |    cis_session.rs              Enable/dual CIS/Disable/Release 全事务
        |
        +--> crates/device-policy/      产品拓扑模型（双单声道）
        +--> crates/iso-scheduler/      目前仅 frame quantum 的最小骨架
        +--> pinned upstream Bumble-RS  不重造 HCI/GATT/PACS/ASCS/BAP 模型
```

固定上游：`chaitanyarahalkar/bumble-rs` commit `bbac2a6803b8cab0920ab725a23aa408fc4fed85`。重要原则：**协议类型、序列化、解析尽量使用已固定 Bumble-RS**；产品层只补充控制器桥接、状态机不变量、设备兼容层与组合事务。不在每一轮更新依赖；跨域兼容修复尽量放在 `bumble-adapter` 或 `bap-client`，不直接分叉改写第三方大库。

Bumble-RS `Device::configure_cig_with_parameters`/`create_cis_pairs` 的一些 API 专用于 `LocalLink` 模拟链路，不能不经适配直接套在自有 `ExternalHost` 控制器上。本回合使用了 `ExternalHost::send_command`、`Device::take_cis_control_events()` 等通道，并沿用上游 `Command`、`AseOperation`、`AseStatus`、`CigParameters`、`CisParameters`。

---

## 3. R4.1 — 确定性 LC3 Codec Planner

### 目标和决定

从 **真实** `AudioSetSnapshot` 生成纯数据 `CodecConfigurationPlan`：

- 双耳、双单声道（`StereoDualMonoAse`），各选一个 Idle Sink ASE（本机为 ASE1）。
- LC3，48,000 Hz，10,000 µs 帧，每耳每帧 100 octets，`1 frame/SDU`。
- Media Context `0x0004`，左右 LTV 分别设置 Front Left `0x00000001` / Front Right `0x00000002`。
- 对耳机 PAC、音频位置、可用 Context、ASE ID/Identity 冲突进行验证；计划不能凭 rank 猜左右。
- R4.1 本身只读：扫描/加密/发现后生成计划，不发送 ASCS Codec/GATT 控制写入。

对应代码：`crates/bap-client/src/codec_plan.rs`；CLI：

```powershell
.\target\debug\be200hub.exe r4 codec-plan `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)"
```

**源头 Bug（命令解析）**：最初的 CLI 匹配曾把 `codec-plan` 六 token 的实际参数数组与错误的 arity 分支混淆，出现帮助列出了命令但无法正确执行的风险。解决：`src/main.rs` 的 `parse_r4_invocation` 纯解析和 CLI 参数回归测试，要求**硬件 Bootstrap 之前**决定命令是否合法。后续 R5.1、R5.2-A、R5.2-B 延续这个结构，避免“已写好却无法进入分支”的错误。

---

## 4. R4.2 — ASCS Config Codec、事件泵与 Release

### 4.1 实际事务设计

```text
R3 同时持有两个 encrypted ACL
  → MTU Exchange (请求 247，确保 Config Codec CP payload 可发送)
  → ASCS re-discovery，确认 attribute handles 未变化
  → CP CCCD + 每个 ASE CCCD 全部订阅
  → 严格确认 selected ASE Idle
  → Config Codec（两耳逐个，可选 left/right/both）
  → 每个 ASE 同时要求 Control Point SUCCESS + ASE status notification
  → 独立 GATT Readback 确认 CodecConfigured 和实际 codec + QoS Preferences
  → 在当前加密 ACL 有效时反向顺序 Release
  → CP SUCCESS + Idle Notification + Idle Readback
  → ACL Disconnect + Resolving List 清理 + Bond 指纹不变
```

事务保留 `--side left|right|both` 与 `--confirm-ascs-write` 显式确认；单耳故障可能导致远端状态不确定，rollback 包含**所有尝试写入**的 ASE，不只包含已经收到成功响应的 ASE。

**GATT 事件泵选择**：固定 Bumble-RS 的 `ExternalAttTransport` 在执行同步 ATT 请求时会把 unsolicited PDU 放到临时队列；若临时 transport 结束时未提取，CP/ASE 通知可能丢失。于是新增 `with_encrypted_gatt_events`、`drain_att_value_events`、`wait_for_att_value_events`，按 ACL Handle 保持事件关联，对 Handle Value Indication 发送 Confirmation，拒绝意外 ATT PDU。CP 与状态通知顺序不固定；两者都收到、ASE ID/opcode/code 匹配且 Readback 成功才计 PASS。

上游 `AudioStreamControlServiceProxy::write_operation` 使用 ATT Write Command，而非 Write Request；**不能**把本地 write 返回当作设备已经接受配置。真正的协议证据是 CP Response + ASE Notification + Readback。

### 4.2 首次单耳成功与初次双耳失败

左耳 `r4-config-codec-left-1.log`（交接包日志 01）：`Config Codec`、`Release→Idle` 均成功，真实 QoS Preferences 首次拿到。

初次双耳：右耳 Discovery 成功，左耳 `ASCS Sink ASE read` 报 `ASE status has 3 trailing bytes`；尚未进入 `[R4.2/PREP]`，所以不是 LC3 编码参数被拒绝。初始观测的异常为：

```text
LE ACL=0x0801, ASE Value Handle=0x0046
raw = [01, 00, 01, 00, 00]
       ASE=1, State=Idle, 后三字节剩余
Bumble-RS: invalid profile value: ASE status has 3 trailing bytes
```

第一轮改动：新增 `ase_read.rs`，对格式错误**有限重读三次**，且记录 ACL、Attribute Handle、长度、raw Hex；真实日志连续三次相同，表明这是稳定返回，而非随机 USB 丢包。不能把“重试通过”当成解决问题。

### 4.3 AOSP 对照 + 两次策略迭代

- Bumble-RS `AseStatus::from_bytes` 解析完 `ASE_ID + State=Idle` 后执行“全部输入必须读完”的严格长度校验。标准 Idle 不需要额外 state-specific 字段；上游保守做法拒绝残余字节。
- 对照 AOSP Fluoride 的 LE Audio `ParseAseStatusHeader` / `ProcessGattNotifEvent` / Idle state machine：只要能读取 ASE ID 与 Idle State，就会分派 Idle，不要求残余 payload 长度严格等于 0。**这是一个兼容策略，不意味着设备额外 3 字节一定符合规范。**
- 第一版 compat 为了过度保守，只接受 `ASE_ID,00,01,00,00`；双耳一次确实 HIL PASS（日志 02）。
- **真实回归**：下一轮 R5.1 又出现 `ASE_ID,00,00,00,00`（第三字节变化），三次重读仍一致，狭义白名单再次失败；说明不能根据第一次数据为尾部字节建立恒定语义。
- **最终决定**：**当且仅当 State 为 Idle 且 ASE ID 非零，至少先正确解析前 2 字节，忽略任何多余 Idle 尾部，但保留原始值日志**；Codec Configured / QoS Configured / Enabling / Streaming 等全部继续使用 Bumble-RS 严格解析。不无条件截掉所有状态的尾部。
- 新策略在后续日志覆盖了如下真实样本：

```text
右/左： [01, 00, 00, 00, 00]
右/左： [01, 00, 01, 00, 00]
左：    [01, 00, 01, 01, 00]
右：    [01, 00, 01, 02, 00]
ASE 2: [02, 00, 01, 00, 00]
ASE 3: [03, 00, 01, 00, 00]
```

**根因判定强度**：确定是固定上游的 Idle 全长度解析与本次 GATT 读值不兼容；额外字节最终来自**耳机固件还是更低层栈封装**，尚无专门 HCI/ATT byte-for-byte 双端对照，不能绝对断言厂商违例。兼容层位于 `bap-client/src/ase_read.rs`，readback 与选中 ASE 通知共用。日志关键字 `[R4/ASE-COMPAT]`。

### 4.4 R4.2 最终硬件证明

`r4-config-codec-both-3.log`（交接日志 02）：双耳 CP + Notification + Readback 均 PASS；两耳 Release→Idle；Bond 指纹 `UNCHANGED`。该文件还包含 `cargo test -p bap-client` 输出：**17 tests passed**（这是当时版本，不等于最终 HEAD 的完整测试覆盖报告）。

首轮 R4.2 合入后的 Commit：`ef4167291637d9444a0ab6cb797dd1382e5c5460`。随后出现并修复 Idle 解析问题；关键提交包括 `31670d3`（有限重读和诊断）、`4f0d9f9`（最早窄白名单）、`59c4a58`（通用 Idle 头部兼容）、`7e36dca`（确保非零 ASE ID）。

---

## 5. R5.1 — 真实 QoS Preferences → 两耳 CIG/CIS 计划

### 5.1 真正的输入与参数

来自 R4.2 两耳 Configured 读回：

| QoS Preferred / Supported | 左耳 | 右耳 |
| --- | ---: | ---: |
| Preferred Framing | 0 | 0 |
| Preferred PHY（比特掩码） | 2 | 2 |
| Preferred Retransmission Number | 13 | 13 |
| Preferred Max Transport Latency | 95 ms | 95 ms |
| Supported Presentation Delay | **40000..40000 µs** | **40000..40000 µs** |
| Preferred Presentation Delay | **40000..40000 µs** | **40000..40000 µs** |

**这些是设备返回的配置信息，不是端到端测量**。R5.2-A 在真实 BE200 上成功配置后，才知道其控制器接受了该 CIG；R5.2-B Established 后才拿到实际链路时延。

### 5.2 Planner policy（与上游协议职责划分）

- 输入：`CodecConfigurationPlan`、`CodecConfiguredSetSnapshot`、`IsoControllerCapabilities`。
- Controller Gate：必须 CIS Central、HCI Set CIG/Create CIS/Remove CIG、Setup ISO Data Path 指令标志、ISO Buffers；对于 R5 首次声音的单 packet ISO 路径，100-byte SDU 还需考虑 HCI ISO SDU Info **4-byte header**，不能仅以 100 与 248 比较。
- 单一 CIG ID `1`；左 CIS ID `1`，右 CIS ID `2`；C→P 每 CIS 100-byte SDU、10ms Interval、2M PHY（设备首选且 BE200 支持），每 CIS RTN 13。
- P→C 不发送麦克风音频：`max_sdu_p_to_c=0`、`rtn_p_to_c=0`；反方向 PHY/Interval/Latency 填入合法非零的控制器参数，避免不兼容。
- 两耳必须拥有共同的**有效** Presentation Delay 交集；首台实测为固定 40ms。若 preferred 区间不交但 hard supported 区间有交集，则允许从支持交集中选，不因偏好冲突拒绝合法方案。
- 源 LTV（LC3）与两耳身份必须匹配 Configured Readback；拒绝重复/混合 Identity、非法 PHY/SDU/时间范围等。
- 复用上游 `CigParameters`、`CisParameters`、`ConfigQosParameters` 与 `Command::LeSetCigParameters`，验证 ASCS encode/decode roundtrip 和 HCI 完整 wire packet。
- **R5.1 不发送 CIG/QoS**，只是为了获得实时 QoS Preferences 复用 R4.2 的 Config Codec→Release，随后打印 Dry Run 字节。

真实计划（日志 03）说明如下：

```text
StereoQosPlan: CIG=1 CIS=2 interval=10000us
PHY_C_to_P=2 framing=0 max_SDU=100
transport_latency=95ms presentation_delay=40000us
left  CIS=1 ASE=1 RTN=13
right CIS=2 ASE=1 RTN=13
HCI LE Set CIG Parameters wire (NOT SENT):
  [01,62,20,21,01,10,27,00,10,27,00,00,00,00,5F,00,5F,00,
   02,01,64,00,00,00,02,01,0D,00,02,64,00,00,00,02,01,0D,00]
```

ASCS Config QoS 对两耳各为 18 字节，因 CIS_ID 左 1 / 右 2 不同而对应不同 payload。Real HIL `R5.1 QOS PLAN PASS`，无 CIG/QoS/CIS/ISO 发送，ASE 返回 Idle，Bond 不变。

### 5.3 本轮构建/测试 Bug

`cargo-check` / `cargo-build` 通过后，首次 `cargo test -p bumble-adapter -p bap-client -p be200audiohub` 在 `qos_plan.rs:379` 遇到 `E0282`：测试 Fixture 闭包 `let sink = |side, id, acl, rank|` 里 `id.into()` 缺乏具体类型，Rust 无法推断。修复为显式闭包参数类型（`side: StereoSide, id: &str, acl: u16, rank: u8`），提交 `f69eff1`；**生产 QoS 逻辑未因此改变**。必须记住：`cargo check` 默认不编译 `#[cfg(test)]`；不能把 Build PASS 写成 Unit Tests PASS。

---

## 6. R5.2-A — 首次真实 HCI Set CIG + 双耳 ASCS Config QoS

### 6.1 为什么先分开 A/B

旧 roadmap 把 CIG/Config QoS/Create CIS 放在同一大阶段；本回合将 R5.2 拆为：

- **R5.2-A**：`Config Codec → HCI Set CIG Parameters → 两耳 ASCS Config QoS → 读回 QoS Configured → Release → Remove CIG`。**不执行 Enable/Create CIS**。
- **R5.2-B**：承接 A 成功的路径，留在 QoS Configured → `Enable → HCI Create CIS → 两条 Established → 显式 Disconnect → Release → Remove CIG`。

拆分的好处：将 Codec 配置有效性、Controller CIG 可调度性、耳机 Config QoS 接受性、空口 CIS 建立/清理分成独立的真实门槛。R4.2 原先会完成一次配置后立即 Release，进入 R5.2 时重构为**可保持 Live Codec Configured**的事务，最终在统一 cleanup 中释放，不复用已关闭 ACL 的旧 snapshot 伪装 live 状态。

### 6.2 上游缺口：CIG Command Complete 的 Raw 返回值

固定版本 Bumble-RS 支持 `Command::LeSetCigParameters` 序列化，但 Set CIG 的 Command Complete 参数并没有独立的 typed `ReturnParameters::LeSetCigParameters` 分支，而是 `ReturnParameters::Raw`。产品层在 `bumble-adapter/src/cig_control.rs` 按规范解析：

```text
Status [1 byte] | CIG_ID [1] | CIS_Count [1] | CIS_Handle[N × u16 little-endian]
```

要求 Status SUCCESS、CIG ID 与请求一致、CIS 个数与两个请求一致、CIS ID 不重复、返回 CIS Handle 合法且互不相同，并按命令中 CIS ID 顺序映射，而不是想象 Handle 一定连续或永恒不变。

### 6.3 R5.2-A 的 HIL 事实

`r5-config-qos-1.log`（交接日志 04）：

```text
[R5.2/CIG] HCI Set CIG PASS: CIG=1 CIS_count=2
[R5.2/CIG] allocation: CIS ID=1 handle=0x0900 (not established)
[R5.2/CIG] allocation: CIS ID=2 handle=0x0901 (not established)
[R5.2/QOS] left  ASE=1 CIS=1 QoSConfigured PASS
[R5.2/QOS] right ASE=1 CIS=2 QoSConfigured PASS
[R4.2/RELEASE] right ... Idle PASS
[R4.2/RELEASE] left  ... Idle PASS
[R5.2/CIG] Remove CIG=1 PASS
Persistent-state fingerprint: UNCHANGED
R5.2-A CONFIG QOS PASS
```

从这里开始，可以严谨地说 **BE200 接受了这一次 RTN=13、latency=95ms、双 CIS 规划**，且 Galaxy Buds3 Pro 两耳均接受并返回了匹配的 Config QoS。**仍无等时空口连接或音频。**

---

## 7. R5.2-B — 三次硬件探针与两个根因级修复

### 7.1 实施决定

- 已建立两个 encrypted ACL；Selected Sink ASE 各走 Codec Configured → QoS Configured。
- `ASCS Enable` 使用 pinned Bumble-RS `Metadata`，Streaming Audio Context `0x0004`（Media），要求 CP SUCCESS + ASE 状态通知 + Readback。
- **一个 HCI LE Create CIS 命令同时提交两对 `(CIS_Handle, ACL_Handle)`**：左 `(0x0900,0x0801)`、右 `(0x0901,0x0800)`。Command Status 0 **不是 Established 成功**，必须分别收到 `LE CIS Established` 异步事件 status=0 且 `Device::cis_link(handle)` 存在。
- 对 **Audio Sink** ASE 不人为调用 Source 端专用的 Receiver Start Ready。实际设备在建立 CIS 后两耳可自行进入 `STREAMING (0x04)`。
- 不调用 Setup ISO Data Path、不发 LC3 帧。建完立即进入受控 teardown；本轮目标是控制链与资源清理，而不是可听见声音。
- 控制器可能已建立一条而另一条失败：必须逐条检查已建立 CIS 并按逆序断开、验证 `Disconnection Complete`；后续才能 Disable（若仍 Enabling/Streaming）、Release→Idle、Remove CIG、断 ACL / 清 Privacy。

### 7.2 第一次 HIL：Create CIS Command Status `0x0C`

日志 05：两耳都已经成功 `Config Codec`、`Config QoS`、`Enable`，`HCI Set CIG` 成功；执行 `HCI LE Create CIS` 时，BE200 直接返回：

```text
HCI LE Create CIS rejected: Command Status=0x0C
```

`0x0C = Command Disallowed`。没有 LE CIS Established 事件，耳机没有走真实 CIS 建立；两耳 Disable/Release、Remove CIG、Bond 指纹仍全部清理成功。

**查明的遗漏**：项目 `initialize_existing_operational()` 只读取 LE Features，并未像 Linux `net/bluetooth/hci_sync.c` 那样在 LE ACL 建立前配置 **LE CIS (Host Support) Feature bit 32**。Bluetooth Core 对 Create CIS 在 Host Support bit 不启用时允许/要求返回 Command Disallowed；这与现场现象吻合。

**修复/决定**：新增 `bumble-adapter/src/cis_host_feature.rs`，只在 R5.2-B 及未来 ISO 路线的专用 Preflight 调用：

```text
Before ANY ACL:
  Read cached LE Features: bit32 false
  HCI LE Set Host Feature, opcode 0x2074, bit_number=32, bit_value=1
  Require successful Command Complete
  Independently read Local LE Features
  Confirm bit32 = true
  THEN begin bonded RPA scan / dual encrypted ACL
```

真实第二次 HIL 验证 `bit32=false → Set Host Feature PASS → Readback PASS → Create CIS Command Status 0 → 两条 CIS Established PASS`；因此本问题在本设备上不再仅是假设，而有**干预前后对照**。bit32 是 controller-local 的 Host Feature，不需要碰 Bond，也不能等到两耳已经连上才设置。

### 7.3 第二次 HIL：两条 CIS 成功，但 teardown 多重失败

日志 06（**失败测试，严禁记成完整 PASS**）：

```text
[R5.2/CIS] CIS=1 ... Established PASS
[R5.2/CIS] CIS=2 ... Established PASS
两耳随后 ASE state=0x04 (Streaming)
HCI Disconnect CIS1/CIS2 Command Status = 0x12
两耳 Disable → QoSConfigured PASS
两耳 Release: CP=true, Idle status notification=false → timeout
HCI Remove CIG=1 status=0x0C
最终 ACL 断开；伴随 0x0900/0x0901 Disconnection Complete
Persistent-state fingerprint: UNCHANGED (failure path)
cleanup UNCONFIRMED
```

**核心根因**：`bumble-adapter/src/cis_control.rs` 错把本地主机最终收到的 `Disconnection Complete` **事件** Reason `0x16` 填到了 `HCI_Disconnect` **命令**里的 `Reason` 参数。按 Bluetooth Core 的 Disconnect Command 参数约束，`0x16` 不属于此处可用值；所以 Controller 返回 `0x12 = Invalid HCI Command Parameters`。这是我们桥接层的 HCI 编码语义错误，**不是耳机音频兼容性问题**。

**修复决定**：`Command::Disconnect { reason: 0x13 }`（命令侧允许的 Remote User Terminated Connection），仍然期望接收的本地 `Disconnection Complete` Reason 可以为 `0x16`。代码里改成明确的 `cis_disconnect_command(handle)` 与字节级回归测试（`01 06 04 03 00 09 13`）。

Release CP 成功但没有最终 Idle Notification 与 Remove CIG `0x0C` 是**前述 CIS 未真正断开的后续症状**；不应该用忽略缺失通知、跳过 Controller 资源清理的方式掩盖。正确做法是先修 HCI Disconnect 并确认 CIS 不再活动，然后执行 ASE Release 和 Remove CIG。

**重要现场**：程序末端 ACL 断开造成两个 CIS 的 `Disconnection Complete`，证明 BLE 生命周期会随 ACL 终止隐式关闭 CIS；但 **没有证明 CIG 已移除**，并且上一轮 Remove CIG 已明确返回 `0x0C`。所以必须保存 `cleanup UNCONFIRMED` 的错误语义，不能改成 PASS。

### 7.4 为避免历史资源污染而引入的 Pre-ACL CIG Reconciliation

因为第二次 HIL 没能证明 Remove CIG 成功，第三次运行前增加**针对本程序独占 CIG ID=1 的无 ACL 前置检查**：

- 执行 `HCI LE Remove CIG(1)`；若成功（`Status=0` 且 `CIG_ID=1`），说明成功清理一个 inactive CIG；如果标准 `Unknown Connection Identifier (0x02)`，则说明 CIG 不存在，二者都允许进入新事务。
- 若为 `0x0C`（CIG 仍 active/Command Disallowed）、状态/返回值不符或超时，则**拒绝继续建立新的 ACL/CIS**，保留诊断；不复位控制器、不盲目重连、不重新配对。
- 这个 Preflight 只作用于 R5.2-B（及之后合理演化的 ISO 路径），不破坏已通过的 R5.2-A，也不能在控制器仍有别的 Owner 时滥用。

第三次日志实际出现：`[R5.2/RECOVERY] removed leftover inactive CIG=1 before ACL PASS`，确认第二次留下的资源确实需要且已能清理。这一点同时印证“ACL 隐式断 CIS ≠ Remove CIG”。

### 7.5 第三次 HIL：完整 R5.2-B PASS（本回合最终里程碑）

`r5-establish-cis-3.log`（交接日志 07）：

```text
[R5.2/HOST] bit32 already enabled; READBACK PASS
[R5.2/RECOVERY] removed leftover inactive CIG=1 before ACL PASS
[R4.2/CONFIG] left + right CodecConfigured PASS
[R5.2/CIG] HCI Set CIG PASS, 2 CIS Handle assigned
[R5.2/QOS] left + right QoSConfigured PASS
[R5.2/ENABLE] left + right Enabling PASS
[R5.2/CIS] LE Create CIS Command Status: SUCCESS
[R5.2/CIS] CIS=1 ACL=0x0801 handle=0x0900 Established event PASS
[R5.2/CIS] CIS=2 ACL=0x0800 handle=0x0901 Established event PASS
[R5.2-B/CIS] DUAL CIS ESTABLISHED: 2/2
[R5.2/STATE] left + right state=0x04 CIG/CIS mapping PASS
[R5.2/CIS] CIS=2 handle=0x0901 disconnected PASS
[R5.2/CIS] CIS=1 handle=0x0900 disconnected PASS
[R4.2/RELEASE] right + left Idle PASS (CP + notify + readback)
[R5.2/CIG] Remove CIG=1 PASS
Dual ACL Disconnection Complete PASS; Resolving List cleared
Persistent-state fingerprint: UNCHANGED
R5.2-B DUAL CIS PASS
```

两耳真实 CIS Established 事件回报（这里是**建立时的 Controller 实测**，而不是规划器预测）：

| 参数 | Left CIS 1 | Right CIS 2 |
| --- | ---: | ---: |
| Established CIS Handle（仅此次运行） | `0x0900` | `0x0901` |
| Assigned ACL Handle（仅此次运行） | `0x0801` | `0x0800` |
| PHY C→P / P→C | 2M / 1M | 2M / 1M |
| C→P Transport Latency | **63,210 µs** | **63,210 µs** |
| NSE | 2 | 2 |
| BN C→P / P→C | 1 / 0 | 1 / 0 |
| FT C→P / P→C | 7 / 0 | 7 / 0 |
| Max PDU C→P | 100 B | 100 B |
| ISO Interval | `8 × 1.25 ms = 10 ms` | `8 × 1.25 ms = 10 ms` |

**另一个设备行为 Quirk**：两耳 CIS 建立之后均可自行进 `STREAMING (0x04)`，即便我们没有 `Receiver Start Ready` 或任何 ISO 音频发送。第三次测试在 CIS 显式断开后，两耳直接能 Release→Idle（未必需要先执行 Disable），这要成为未来 R6 生命周期状态机的输入条件。

---

## 8. Bug 与 Quirk 根因账本（本回合必读）

| ID | 层级与症状 | 根因 / 源头 | 处理决定 | 最终证据 / 限制 |
| --- | --- | --- | --- | --- |
| B01 | CLI `codec-plan` 指令 token 个数错误 | Rust `match` 分支 arity 设计错误 | 纯 CLI 解析器 + 参数测试，Bootstrap 前校验 | R4.1 命令路径已投入使用 |
| B02 | ASCS ASE Idle `has 3 trailing bytes` | Bumble-RS 严格 `reader.finish()` 与实际 GATT Idle 多字节不兼容 | 有限重读并抓 raw；最终仅 Idle 解前 2 byte，其他状态严格 | 两耳 ASE1/2/3 多种尾部成功；**不证明额外字节根因来自固件** |
| B03 | 第一次兼容仅接纳 `[01,00,01,00,00]` 后复发 | 假设尾部有固定值，实际 `00`/`01`/`02` 等会变化 | 转成基于 State 的兼容，而非按字节白名单 | R5.1/5.2-A/5.2-B 真实 HIL PASS |
| B04 | GATT CP/ASE Notification 易丢失 | 同步 ATT 临时 transport 的 unsolicited PDU 队列被生命周期吞掉 | ATT Value Event pump，处理 CC/ASE 两种事件；必要时 Indication Confirmation | R4.2、R5.2-A/B 均能完成 CP+Notify+Readback |
| B05 | `cargo test` E0282，`id.into()` 不知道类型 | 测试 Fixture 闭包参数未标注；`cargo check` 未编译 `#[cfg(test)]` | 显式闭包参数类型；新增 CLI 回归测试 | R5.1 代码编译通过，测试问题在日志中有报错；最终 HEAD 仍需完整本地测试日志 |
| B06 | Set CIG 结果不能用 typed 返回直接取得 CIS Handles | 固定 Bumble-RS 将 LE Set CIG Complete 留作 `ReturnParameters::Raw` | 本地仅解析固定规范字段 + 强制 Handle 一致性检查 | R5.2-A/B HIL 返回 CIG1/CIS1/2 |
| B07 | 第一次 `LE Create CIS → 0x0C` | 自有 Host 初始化漏设 LE CIS Host Support feature bit32 | `HCI LE Set Host Feature (0x2074)` 必须 **双 ACL 建立前**执行，且读回验证 | 第二次 bit32 false→true、Create CIS 成功，第三次保持 true |
| B08 | 第二次 CIS Disconnect → `0x12` | 把 `Disconnection Complete` 事件 Reason `0x16` 错作为 `HCI_Disconnect` 命令 Reason | 命令 Reason 改 `0x13`；增字节级回归测试 | 第三次 2/2 CIS Disconnection Complete PASS |
| B09 | Release CP 成功但无 Idle 通知；Remove CIG `0x0C` | 上游 CIS 未真正断开导致状态/资源无法释放 | **不**忽略通知，不**假装** Remove 成功；先修 Disconnect 与关闭 CIS | 第三次先双 CIS Disconnect 再 Release/Remove，全 PASS |
| B10 | 隔次运行 CIG 1 残留 | 前轮 ACL 断开隐式终止 CIS，但 CIG 未 Remove | 无 ACL 前置 Reconcile，仅认可 Status 0 或 0x02，遇 0x0C 拒绝开新连接 | 第三次捕获“removed leftover inactive CIG=1” |
| Q01 | `interrupt-IN ... libusb_status=-7 bytes=N` | USB interrupt timeout 仍带部分已收到字节 | 保留字节并交给 HCI reassembler；不能当成整包丢弃 | 本回合各 HIL 均有该诊断，链路照常完成 |
| Q02 | RPA 地址变动、CSIS Rank 与声道不等同 | BLE privacy 与 CSIS 语义各自独立 | 先 IRK 解析，FAL 控制器身份连接；左右基于 PACS Location | 多轮 RPA 变化，固定 Identity 与音频声道 |
| Q03 | 冷启时 BE200 image=0x01 | Controller 可在 Bootloader，不能假设永远 Operational | PIN/hash 校验，一次固件初始化，无循环 Retry/Reset | 第一次 R5.2-B 完整冷启成功 |
| Q04 | `ASE STREAMING (0x04)` 但没有声音 | ASCS 状态机与 HCI ISO Data Path 是两层概念 | 不把 Streaming state 解释成音频数据已传输 | R5.2-B 双耳 0x04；R5.3 仍未开始 |
| Q05 | `transport_latency=63210us` 与 `presentation_delay=40000us` | 不同规范时间参考、与 SDU 调度关联 | 保留各自原始定义；以后 ISO/audio HIL 分析，不能武断相加或称“不兼容” | 目前只有控制器 HCI 链路时序，无播放时刻测量 |
| Q06 | PowerShell `Tee-Object` 输出 `NativeCommandError` | EXE 非零退出时 PowerShell 把 native 失败展示为外层报错 | 读取最终 `Error:` 和先前 HCI/ASCS 证据；不把其误判为 Git/Windows 运行时崩溃 | R4.2/R5.1/5.2-B 失败记录均可复现 |

**关于 5 字节“直接丢弃”的措辞**：对于观察到的 *总长 5 字节* Idle 状态，**保留 2 字节 ASE_ID+State、忽略后 3 字节**；绝不可把“后面五个字节全丢了”理解为丢弃数据本身，或对 Codec/QoS/Streaming 状态作同样处理。

---

## 9. 所有工程决策及原因（未来不要默默推翻）

1. **先 BAP 控制面、后 ISO 数据面**：R4.1→R4.2→R5.1→R5.2-A→R5.2-B→R5.3。每个门槛需要真正 HIL 证据，而不是纯 Build。
2. **优先现成协议实现**：ASCS/GATT/HCI/CIG/CIS 等复用 pinned Bumble-RS；对照 AOSP/Zephyr/Linux 查调用顺序、策略和 Host 初始化；仅针对 ExternalHost/BE200 补产品适配。
3. **配对次数最少**：沿用 R2 已有 LTK/IRK，禁止无理由重新 Bond、清空本地 Identity 或重配对。持久状态变更只有 R2 Pairing Transaction 允许。
4. **严格身份隔离**：控制器 FAL/Resolving List 使用已验证 Identity，Handle 只在单轮连接中有意义。所有 R4/R5 应审计 Bond/Set/Local Identity 指纹。
5. **Topology**：两个分立 Sink ASE，下行左/右单声道、一个 CIG 两个 CIS。Audio Location 是通道依据，CSIS Rank 不作为 Left/Right。
6. **LC3 首声优先策略**：48 kHz、10ms、100B、1 Frame/SDU，Media，先求可听验证，不提前讨论复杂多码率优化。
7. **QoS 来自真实 Config Codec 回执**：40ms delay 是硬支持范围；RTN13/95ms 是偏好/候选，BE200 成功 Set CIG 后证明这一次可接受，但实际时延需 CIS Established 数据。
8. **双耳统一建链**：HCI LE Create CIS 一条命令包含左右 `(CIS Handle, ACL Handle)`，分别等待完整异步 Established。
9. **CP / Status / Readback 三门槛**：GATT Write Command 本身不证明耳机状态转移成功；任何一步缺一不可。
10. **Idle 特例，其他状态严格**：AOSP 风格仅限 Idle 头部容错，同时保留 Raw Hex；严格保留 Codec/QoS/Enable 状态的字节完整性。
11. **隐式断链不是清理证明**：不接受通过简单断 ACL 掩盖 CIS/CIG 泄漏。先合法 Reason `0x13` 显式断 CIS、Release ASE、Remove CIG，再断 ACL。
12. **所有不确定事务回滚有声**：CP 写入后即使缺 Response，也视为可能改过远端状态；清理失败输出 `UNCONFIRMED` 而非继续报 PASS。
13. **Host Feature 设置时机**：CIS Host Support bit32 必须在创建双 ACL 之前检查/设置，并独立读回；只在确需 ISO 的命令路径执行，不改变老路径。
14. **控制器资源恢复**：已知独占 CIG 1 在下一次 R5.2-B 前做幂等 Reconciliation，成功/不存在才继续；不做全局 Reset、不假设系统其它 CIG 属于我们。
15. **明确 Stage 的写入权限**：R4.1 只读；R4.2 Config Codec/Release 须 `--confirm-ascs-write`；R5.1 只做 QoS dry-run 但为得到偏好允许同样 Codec/Release；R5.2-A 另外用 `--confirm-cig-qos-write`；R5.2-B 再用不同的 `--confirm-cis-create`。
16. **不因便利混入额外 Scope**：R5.2-B 不 setup ISO Path、不发送 LC3、不引入新的 Source ASE、无 Receiver Start Ready，不耦合其他 Windows Relay 产品；第一次声音是 R5.3。
17. **同时保留 CLI、可自动化与日志**：PowerShell + `Tee-Object` 作为回归入口；Rust test 测 pure parsers、source invariants 和 HCI bytes。ToolHub 用于 Git Pull/Check/Build；无 cargo-test allowlist 就在用户 Windows 本地执行。
18. **Branch Isolation**：本回合一直写 `r2/stack-integration-bootstrap`，工具输出曾显示独立 `experiment/win11-quick-settings` 分支更新；该实验分支未纳入本 Round4 成果，不应误操作。

---

## 10. 关键仓库文件、所参考上游与提交锚点

### 10.1 本项目核心文件

| 相对路径 | 作用 / 要防止的回归 |
| --- | --- |
| `src/main.rs` | R4/R5 CLI arity、确认开关，不允许不明命令触发硬件写入 |
| `src/audio_set.rs` | Dual ACL、Live transaction、R5.2-B 前 ACL 的 Host bit32 + CIG 恢复、最终持久指纹 |
| `crates/bap-client/src/codec_plan.rs` | 依据 PACS 而非 Rank 选择 LC3 Sink ASE |
| `crates/bap-client/src/ase_read.rs` | 仅 Idle 的多字节特例和原始值诊断 |
| `crates/bap-client/src/codec_config.rs` | GATT CP/ASE 双事件 gate + 独立状态 Readback、统一 Release |
| `crates/bap-client/src/qos_plan.rs` | QoS 参数与 HCI CIG/ASCS 配置的一致性 |
| `crates/bap-client/src/cis_session.rs` | Enable + Dual CIS + Disable/Release/Remove 的短生命周期 |
| `crates/bumble-adapter/src/gatt_session.rs` | 同步 ATT 请求期间保留 unsolicited PDU，防漏 CP/ASE |
| `crates/bumble-adapter/src/iso_capabilities.rs` | BE200 CIS/ISO Feature、buffer、命令能力前置检查 |
| `crates/bumble-adapter/src/cig_control.rs` | HCI Set CIG Raw completion 解析、Remove CIG、前 ACL Recovery |
| `crates/bumble-adapter/src/cis_host_feature.rs` | **CIS Host Support bit32** 必须在 ACL 前设置 |
| `crates/bumble-adapter/src/cis_control.rs` | Create CIS 事件映射及**合法 Disconnect Reason 0x13** |
| `crates/bumble-adapter/src/usb_fixed.rs` | BE200 原生 USB HCI 分帧、partial timeout，已有 ISO Bulk-Out 分支**尚未 HIL** |
| `crates/iso-scheduler/src/lib.rs` | 当前只有 48k×10ms→480 frames 的 minimal quantum，**不是完整 ISO scheduler** |
| `docs/r4-ascs-config-codec.md`、`docs/r5-qos-plan.md`、`docs/r5-config-qos.md`、`docs/r5-dual-cis.md` | 每阶段安全边界和 HIL 命令 |

### 10.2 上游来源（定位用；不是要求更新依赖）

- Bumble-RS pinned: [`chaitanyarahalkar/bumble-rs@bbac2a6`](https://github.com/chaitanyarahalkar/bumble-rs/tree/bbac2a6803b8cab0920ab725a23aa408fc4fed85)，重点 `bumble-profiles/src/ascs.rs`、`bumble-profiles/src/bap.rs`、`bumble-gatt/src/client.rs`、`bumble-transport/src/host.rs`、`bumble-hci/src/{command,event,return_parameters}.rs`、`bumble-host/src/lib.rs`。
- AOSP Fluoride：`packages/modules/Bluetooth/system/bta/le_audio/client_parser.cc` 与 `state_machine.cc`，用于 Idle 解析与 Audio Sink 状态机实践；引用的是本回合源码对照结论，项目实际代码无需移植整套 Android。
- Zephyr：`subsys/bluetooth/audio/bap_unicast_client.c` 相关 QoS 校验、BAP→CIG/CIS 映射、单向音频策略；仅作比较。
- Linux：[`net/bluetooth/hci_sync.c`](https://codebrowser.dev/linux/linux/net/bluetooth/hci_sync.c.html) 中 CIS Host Feature 初始化路径。
- Bluetooth Core HCI 规范：LE Set Host Feature `0x2074`（bit32）；LE Set CIG Parameters `0x2062`、LE Create CIS `0x2064`、LE Remove CIG `0x2065`；HCI Disconnect `0x0406` Reason 参数及 Disconnection Complete 事件 Reason 的区别。**以上 HCI opcode 基于本次源码/日志和规范对照；不要根据本文推测其它供应商行为。**

### 10.3 关键 Commit 与版本锚点（精选；不是 68 次提交的逐条清单）

| Commit | 内容 |
| --- | --- |
| `a80cf1e8fe7d07078ae158e0f18aefcc73e80aed` | 上一份 R2/R3 交接的已验证起点 |
| `ef4167291637d9444a0ab6cb797dd1382e5c5460` | R4.2 Config Codec/Release 代码集成（初始 Build PASS） |
| `31670d3` | ASE 格式异常的有限重读和 Hex 诊断 |
| `4f0d9f9` / `f8aa89e` | 第一版窄 Idle 尾部兼容及构建类型修正 |
| `59c4a58` / `7e36dca` | 修为通用 Idle 前两字节解析；拒绝 ASE ID=0 |
| `15c68dcd4b5364fce81366d8c07ce72f50ba1579` | R5.1 QoS Planner + 文档构建里程碑 |
| `f69eff1e3961af586a0f25f5b3d62f43b12d987a` | 测试 Fixture 闭包 `E0282` 修复 |
| `7ef216eecb937f5e60e5c645c3d40b3928ef40cc` | R5.2-A CIG + ASCS Config QoS 完整实现 |
| `c31c7f76588870a61c0ca76a10d76170817d53e8` | R5.2-B 初版 Enable + Dual CIS + Teardown 实现 |
| `61851a01141fc586b0b27b9abc1425cb0d337d1b` | Host Feature bit32 修复（解决 Create CIS 0x0C） |
| `5848597` / `2ef6ccf` | CIS Disconnect Reason 0x13 与残留 CIG 的前置恢复门槛 |
| **`eac486691fd4d7181d36bb09a620e942d0026936`** | 本回合最终核对 HEAD；R5.2-B 第三次 HIL PASS 采用的基线 |

GitHub Compare：自 `a80cf1e8` 到本回合 HEAD **ahead 68 commits、22 个文件发生变更**（比较统计，不代表 68 个全部都是本次新功能的独立里程碑）。

---

## 11. HIL 证据：七份用户原始日志的定位

交接包 `evidence/hil/` 中存放所有七份上传日志，**不修改原文**（仅做有意义的文件重命名）；若需要逐字追溯，以日志内容为准。

| 日志索引 | 原始文件 | 这份日志证明什么 |
| --- | --- | --- |
| 01 | `Pasted text(1).txt` | R4.2 左耳 Codec Configured+Release HIL PASS；第一份 QoS Preferences |
| 02 | `Pasted text(2).txt` | 当时 `bap-client` **17/17 test PASS**；R4.2 双耳 Config Codec/Release HIL PASS；第一版窄 Idle 兼容 |
| 03 | `Pasted text(3).txt` | R5.1 双耳 QoS Planner Dry Run HIL PASS；可变 Idle 尾部已兼容 |
| 04 | `Pasted text(4).txt` | R5.2-A Set CIG + 2× Config QoS + Remove CIG HIL PASS |
| 05 | `Pasted text(5).txt` | R5.2-B 第一次 Create CIS `0x0C` Command Disallowed，Rollback CONFIRMED |
| 06 | `Pasted text(6).txt` | Host bit32 修复后 **2/2 CIS Established PASS**，但 Disconnect Reason `0x16` 导致 0x12、Release/Remove 失败，Cleanup UNCONFIRMED |
| 07 | `Pasted text(7).txt` | **R5.2-B 第三次端到端控制面 HIL PASS**，成功恢复历史 CIG、成功断开双 CIS，Bond UNCHANGED |

额外来自本回合对话但不在这七份附件里的中间失败：R4.2 最早的双耳 ASCS trailing bytes Discovery Error、R5.1 窄白名单导致 `[01,00,00,00,00]` 三次重试失败、以及测试 Fixture `E0282`。本文件已依据当时对话原文保留错误文本和修复决策；若新 Chat 需要逐字完整命令输出，可返回本项目旧会话查询，不把此处重述当成独立完整原始附件。

---

## 12. 可复现实验命令与风险等级

**先运行测试**（ToolHub 不保证允许 `cargo-test`；须用户本地 Windows 执行）：

```powershell
cargo test -p bumble-adapter -p bap-client -p be200audiohub
```

**只读 R4.1**：

```powershell
.\target\debug\be200hub.exe r4 codec-plan --seconds 8 --name "Galaxy Buds3 Pro (2F23)"
```

**R4.2：仅 Codec 配置并 Release，需同意写入**：

```powershell
.\target\debug\be200hub.exe r4 config-codec --seconds 8 --name "Galaxy Buds3 Pro (2F23)" --side both --confirm-ascs-write
```

**R5.1：需短暂 Codec 写入以取得实时 Preferences；不发 QoS/CIG**：

```powershell
.\target\debug\be200hub.exe r5 qos-plan --seconds 8 --name "Galaxy Buds3 Pro (2F23)" --confirm-ascs-write
```

**R5.2-A：会真实写 CIG + ASCS QoS，但不 Create CIS**：

```powershell
.\target\debug\be200hub.exe r5 config-qos --seconds 8 --name "Galaxy Buds3 Pro (2F23)" --confirm-cig-qos-write
```

**R5.2-B：会真实建立双 CIS；不是读-only；需先确保没有其它进程占用 BE200**：

```powershell
.\target\debug\be200hub.exe r5 establish-cis `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)" `
    --confirm-cis-create `
    2>&1 | Tee-Object ".\r5-establish-cis-repeat.log"
```

R5.2-B 的通过门槛：bit32 pre-ACL Readback、双 Codec、双 QoS、双 Enable、**2× CIS Established**，之后 **2× HCI Disconnect、2× ASE Idle、Remove CIG、双 ACL Disconnection、Bond 指纹未变**。一旦出现 `cleanup UNCONFIRMED`，保留日志、停止复跑，先查资源状态。所有 HIL 由用户在 Windows 本地显式运行，本回合助手只远程提交源码并调用 `git-pull`/`cargo-check`/`cargo-build`，**未远程向耳机执行上述危险写操作**。

---

## 13. R5.3 First Sound：新 Chat 该做什么，不该做什么

当前接口准备度：**GATT/BAP 控制面及双 CIS 已 HIL 证明；数据面尚未实施。** R5.3 建议继续按小门槛：

### R5.3-A：确认 HCI ISO Data Path 边界和数据链路

1. 读取 pinned Bumble-RS `Command::LeSetupIsoDataPath` / `LeRemoveIsoDataPath`、`IsoDataPacket` / ISO transport buffer 与 `Number Of Completed Packets` 流控实现；不从零发明格式。
2. 阅读项目 `usb_fixed.rs`：`HCI_ISO_DATA_PACKET` 已有 Bulk-Out 发包分支，但**从未证明 BE200 HCI ISO 发送实际可用**；需要检查 H4 到 USB native wire 的 Header 去除、最大单包与短写行为、Controller-credit 节奏。避免把这当成既成能力。
3. 从当前已验证 R5.2-B 的 live 双 CIS 流程中抽出可持有 Session 的 Stage；在两个 Established 后执行 Host→Controller (downlink) 的 ISO Data Path 设置（明确 Direction、Data_Path_ID、Codec_ID/Codec Configuration、Controller Delay 的真实上游定义），回读 HCI Command Complete。
4. 首先单做 **Setup → Remove ISO Data Path** 的受控实验；不急着发音频，沿用完整 Disconnect/Release/Remove CIG cleanup。

### R5.3-B：第一次 LC3 100-byte SDU

1. 复用稳定 LC3 实现，先调研 `bumble-codecs` 及可用 Encoder 对 Windows/MSVC 的要求，确认实际产生 **100 B、48k/10ms 单声道**帧；禁止把裸 PCM 当 LC3。
2. 纯合成 source（例如双耳不同频率或先分别单侧验证），每耳 480 PCM frames → 100 B LC3 frame → 1 SDU/10ms。**先不整合 WASAPI/桌面音频链**；这属于后续产品化。
3. 按正确 CIS Handle 分配独立 ISO Packet Sequence Number、调度和 HCI Number Of Completed Packets 信用；验证左右立体声映射与送包统计，不能简单假设 `write_bulk` 返回就代表空口/耳机收到了音频。
4. 实际聆听/可选外部麦克风回录，确认左/右分离、持续 1–3 秒的稳定输出；再拓展失败回滚、静默保活和时延测量。

### R5.3-C：时序与稳定性

- 以 Controller Established 的 `transport_latency_c_to_p=63.210ms`、`ISO_Interval=10ms` 和耳机 `presentation_delay=40ms` 为数据入口，查清协议的时间参考点；别直接相加成“103.21ms 端到端”，也别用 `40 < 63.21` 推断必然不可用。
- 排查 CIS 建立后 Idle/Streaming/Disable/Release 的竞态、ACL 异常终止时 CIG 残留，以及重复 10+ 次 HIL 的幂等性；R6 单独系统化。
- 不动 Bond，不把 BE200 原生 Host 产品和单独的 Windows LE-Audio-Relay 软件渲染项目混成一个程序；后者可以以后作为音频源模块。

### 当前尚未解决或没有测试证据的事项

| 风险 | 现状 | 下一步 |
| --- | --- | --- |
| HCI ISO Data Path Setup/Remove | 尚无 HIL | R5.3-A 首先证明 |
| USB HCI ISO native Packet 发送 | 存在代码入口，无 HIL | 独立发包 / credit / header / buffer 验证 |
| LC3 Encoder 与同步 PCM 产生 | 尚无真实 100B 音频编码 HIL | R5.3-B |
| 实际可听的左右独立声音 | 尚无任何声学证据 | First Sound |
| 硬件长稳 / 连续复现 | 当前仅一次完整 R5.2-B Cleanup PASS | R5.3 后重复 / R6 |
| 最终 HEAD 完整 `cargo test` | 仅部分先前测试有真实输出，最后 HEAD 没附独立完整日志 | 新 Chat 开工先跑一次测试并存档 |
| Idle 多余字节来源（耳机/传输） | 倾向设备/栈兼容差异，但未做双端逐字节对照 | 后期抓 ATT/HCI；目前保留 Hex 证据和特例 |

---

## 14. 交接原则 / 冻结现状

1. **当前工作应直接从 `eac4866` 的 R5.2-B HIL PASS 启动**，不要把 `r5-establish-cis-2.log` 的失败结论当成最终状态。
2. 除非有回归，**不要重新解 R2 Pairing、R3 双 ACL/PACS、R4 Codec、R5 QoS/CIG/CIS 的已验证问题**；所有改动应受回归测试与真机状态门槛约束。
3. 最终一次测试确证：Controller Host feature bit32 true、Pre-ACL 残留 CIG 清除、2/2 CIS Established、2/2 CIS Disconnected、两耳 Idle、Remove CIG、Bond unchanged。下一步**只剩 R5.3 First Sound 的数据面关键路径**，不是把整个 LE Audio 从头再学一遍。
4. 代码变更原则：GitHub Connector 写入 `r2/stack-integration-bootstrap`，ToolHub 只做有 allowlist 的 Pull/Check/Build，用户手工完成 Windows/HIL；不可宣称 Build 等于 Test、Test 等于 HIL、HIL 等于真正可听声音。
5. **不清 Bond、不频繁重配对、不直接硬复位控制器**。新 Chat 如果面对 Cleanup UNCONFIRMED 必须优先排查/恢复 CIG/CIS/ASE 状态，而不是建议一股脑删除设备。

**一句话总结**：这回合真正完成的是 BE200 与 Galaxy Buds3 Pro 的**双耳 LE Audio Unicast 全套控制面**（Codec→QoS→CIG→Enable→2×CIS Established→规范回滚），并把协议不兼容、Host 初始化遗漏和 HCI Disconnect 参数错误三个真正会阻断推进的源头问题逐一查清和修好；**First Sound 尚未发生，下一轮进入 R5.3。**
