# BE200AudioHub — Round 1–9 Bug / Fix / Quirk Register

> 活动总账：Round1–4 历史 Experiment + Round5–9 Review / product-path continuation。源文可查 [Round1](./Round1.md)–[Round9](./Round9.md)；Round1–4 原文全量汇编仍见 [AllRounds](./AllRounds.md)。
>
> **证据原则**：HIL = 真实 BE200/B3P 或工具执行器实际结果；Code = 编译或字节/单元级；Review = 静态审查；Hypothesis = 未经证实或后来被推翻；Open = 尚未解决。Experiment historical baselines 与当前 Review baselines 分开记录。当前 Round9 deterministic gate 为 410/410 tests、cargo check/build PASS；真实硬件的 30 s graceful teardown 仍 FAIL，ACL lower-boundary audit 等待下一次 HIL。
>
> 每项 ID 是本归档新增的检索编号，不代表仓库 Issue ID。一个问题若经历数次失败，会保留中间失败和最终解法，避免只留下“修好了”。

## 0. 特别容易误判的错误码

| 值 | 正确含义/上下文 | 区分 |
|---|---|---|
| HCI `0x0C` | Command Disallowed | R3 的 Initiator Cancel、R5.2-B 的 Create CIS、Remove CIG 三处可能分别出现，根因不同 |
| HCI `0x12` | Invalid HCI Command Parameters | 在第二次 R5.2-B 的 CIS Disconnect 是命令 Reason 错用 `0x16` |
| HCI `0x13` | 此处可用于 `HCI_Disconnect` 命令的 Reason | 第三次 R5.2-B 改用后，CIS Disconnect PASS |
| HCI `0x16` | 本地收到的 `Disconnection Complete` 事件 Reason | **不能当 Disconnect 命令的 Reason** |
| SMP `0x0B` | DHKey Check Failed | Round3 第二成员 public/random 地址类型 bit 不一致 |
| libusb `-7` | Transfer timeout | Timeout 同时可能携带已收到的字节，必须保留 partial bytes |
| Rust `E0282` | 类型推断失败 | Round4 `#[cfg(test)]` Fixture，正常 `cargo-check` 不一定发现 |
| PowerShell `NativeCommandError` | Native EXE 非零退出的外层呈现 | 真正根因以 Rust `Error:` 与更早 HCI/ASCS 状态为准 |

## 1. 已确认的问题 / 修复 — Round 1

| ID | 现象与故障来源 | 根因及修复过程 | 验证 / 原文 |
|---|---|---|---|
| **R1-01** | PnP Restart/USB Port Cycle 曾被混作 Controller 真正冷启 | 001 未下载完整固件、002 多在 Operational；003 真正断电进入 Bootloader。用 Image Type 决定是否进行一次受保护的 Firmware Boot，而非每次复位或反复刷写 | **HIL** [R1 `3.1–3.6](./Round1.md) |
| **R1-02** | 只依赖 003 USBPcap 无法恢复“完整 SFI” | USB 上只传 ECDSA+DATA，缺 644B RSA 头。再从 Intel 24.60.0.4 `ibtusb.sys` 恢复完整 980,840B Windows OEM SFI，和 003 的 3,960 片完全对比；004 Rust 真实冷启成功 | **PCAP+HIL** [R1 `3.5–3.6](./Round1.md) |
| **R1-03** | R1.4 Intel Read Version 的 HCI Event 可能超过一个 USB 64B packet | 把单次 USB 返回等同完整 HCI Event 不可靠；实现事件拼接、长度和 command-complete 匹配，最终读到 Operational 和 Intel TLV | **HIL** [R1 `3.3](./Round1.md) |
| **R1-04** | R1.5a/b 第一次编译失败：`AppError::Firmware` 未定义 | 代码使用新错误分支但漏改 `src/error.rs` enum。Commit `97a2bd8` 补错误类型；GitHub 修改不是编译证据 | **Code** [R1 `7.4](./Round1.md) |
| **R1-05** | 加入 `sha2` / Git deps 后本地 `Cargo.lock` 长期 dirty | 依赖解析使 lockfile 与远端暂不同步；决策是不猜 Cargo checksum、也不把无关 Lock 变更混进 HIL Commit | **Open version-control hygiene** [R1 `7.4/8.4](./Round1.md) |
| **R1-06** | `ISO Broadcaster=false` 被误读为没有 ISO | Broadcast BIG/BIS 与 Unicast CIS Central 是不同控制器能力位；以 `CIS Central=true`、ISO Buffer `248B × 8` 为事实，不冒充已发送 ISO | **认知修正/HIL capability** [R1 `3.7](./Round1.md) |
| **R1-07** | Scan `unique` 数量被看成实际物理设备数，或 0 reports 认为扫描坏了 | 广播地址/RPA 会轮换，truncated Data Status/0B 记录不等于新成员；后续身份应由 IRK/CSIS/PACS 证实 | **认知修正** [R1 `3.8](./Round1.md) |
| **R1-08** | 因 BTstack Lite 不是标准 BAP 就断言整个 LE Audio Client 必须自写 | 局部演示不代表整个开源生态；纠偏为调研成熟标准 Profile，然后 Round2 固定 Bumble-RS | **架构修正** [R1 `5.2–5.3](./Round1.md) |
| **R1-09** | 对 Windows 设备驱动切换的作用范围和恢复能力过度乐观 | 同 Root Hub 尚有日用 MT7925；只对 `8087:0036` 手动换 WinUSB，保留 OEM 备份；真正 Intel 驱动恢复 HIL **仍未演练** | **部分 HIL / Open** [R1 `2](./Round1.md) |

## 2. 已确认的问题 / 修复 — Round 2

| ID | 现象与故障来源 | 根因及修复过程 | 验证 / 原文 |
|---|---|---|---|
| **R2-01** | 常规 Bumble `ExternalHost::initialize_device` 初始化会 HCI Reset 已 Operational 的 BE200 | 固定上游默认先 Reset；改 `initialize_existing_operational`：机械跟随其余命令/缓冲池配置，**只省略 Reset** | **HIL** [R2 `4](./Round2.md) |
| **R2-02** | BE200 物理失电后变 Bootloader，Host Attach 无法继续 | WinUSB 状态下不会自动运行 Intel BTHUSB 固件加载；Round2 手动 Firmware Boot，Round3 添加单次受控 Bootstrap，Round4 复现通过 | **HIL** [R2 `4.3](./Round2.md)、[R3 `19](./Round3.md) |
| **R2-03** | Public 地址字符串带 `/P`，早期格式化/保存未正确区分类型 | 字符串不是完整地址上下文；修正输出并将 `address_type` 放进 `ScanObservation`，后续 SMP f5/f6 更依赖正确地址类型 | **HIL** [R2 `5.3](./Round2.md) |
| **R2-04** | 每条 R2 命令前 10–15 秒蓝色 PowerShell Progress | R1 的 `driver::verify` 每次启动 PowerShell `Get-PnpDevice` 枚举 PnP/CIM；不是 Bumble 扫描慢。Commit `bb8331f` 改成纯 Rust `rusb` fast preflight | **用户复测** [R2 `6](./Round2.md) |
| **R2-05** | `Device::connect_le_extended()` 隐式 own address = random | 现有 no-reset Host 未执行上游 `Device::power_on` 设置随机身份；R2.3 直发 own_type=public，Round3 统一 Generic FAL 连接 | **HIL** [R2 `7.3](./Round2.md) |
| **R2-06** | 上游 `PairingConfig::default()` 的 MITM=true 与 Buds3 Pro `NoInputNoOutput` 组合不合适 | 明确 Secure Connections/Bonding，`mitm=false`、真实 BE200 public Host Identity；防止依赖不合适默认值 | **预防性源审+Round3 pairing HIL** [R2 `15](./Round2.md) |
| **R2-07** | BondStore 以 stable identity 为 key，但 LE 连接元信息可能是当前 RPA | 直接用临时 RPA 字符串 lookup 会 miss；Round3 选择 IRK Host 解析、Controller Resolving List + identity FAL，并从存储 LTK 加密 | **后续 HIL** [R2 `22](./Round2.md)、[R3 `21–22](./Round3.md) |
| **R2-08** | ToolHub `frontend-npm-build` `spawn EINVAL` 后项目所有操作一直 `busy` | 子进程 spawn 失败时没有释放 in-flight project lock；RAII/`finally` Guard 覆盖 spawn error/timeout/crash/normal exit。**原始 spawn EINVAL 根因仍是独立问题** | **ToolHub 复测** [R2 `29](./Round2.md) |
| **R2-09** | GitHub Connector 返回 GraphQL 错误，远端实际上已提交 | 远程调用结果和服务器提交结果可能不一致；先核对 HEAD/Blob 再 Retry，防重复写入 | **工具行为实测** [R2 `34](./Round2.md) |
| **R2-10** | 未被允许的 ToolHub operation 触发审批，而非静态禁止 | ToolHub Allowlist 与宿主平台授权层是两道边界；自此明确助手只做代码/Check/Build，**HIL 由用户本机显式执行** | **工作流纠偏** [R2 `30–31](./Round2.md) |

## 3. 已确认的问题 / 修复 — Round 3

| ID | 现象与故障来源 | 根因及修复过程 | 验证 / 原文 |
|---|---|---|---|
| **R3-01** | 第一次 SMP 从同名 LE Audio public advertiser 发起却卡连接 | 广告类型多种，Primary ceremony 实际使用 Microsoft Swift Pair pairing-only RPA；改匹配 company ID/beacon=0x03/subscenario=0x02 的 transport | **HIL** [R3 `4–7](./Round3.md) |
| **R3-02** | Secondary explicit-peer `0x2043` Command Status=0，却 10s 无 Connection Complete | 绕开成熟 Connection Manager，错误地 `filter_policy=0`；用 `Clear FAL→Add identity→Extended Create Connection(filter_policy=1)` 后 15–46ms 建链。Commit `f224488` | **HIL** [R3 `11](./Round3.md) |
| **R3-03** | 前述超时 Cancel 时 `0x0C` | Initiating 生命周期处在不允许取消的状态边界，是错误连接路径伴随症状；不能误判为 peer 主动拒绝 | **HIL 纠偏** [R3 `11](./Round3.md) |
| **R3-04** | 一次 BE200 Interrupt transfer 包含 11 个 HCI Frame | `USB read=one HCI event` 假设错误；按 native headers 拆合包、跨 read 保存未完成 Event+ACL | **HIL** [R3 `6.1](./Round3.md) |
| **R3-05** | Raw HCI Tap 报 `invalid HCI packet type 0x00` | Tap 边界错，把 native bytes 当 H4 header；改为 USB framing 完成后、ExternalHost/Device 解析前 | **HIL** [R3 `6.2](./Round3.md) |
| **R3-06** | `libusb_status=-7 bytes=N` 仍携有效 HCI 字节 | Partial timeout 不能丢帧；向 reassembler 转发有效 byte；Round4 多次依赖该行为 | **HIL** [R3 `6.3](./Round3.md) |
| **R3-07** | `Operation not supported` 被误认为 BE200 Bumble Bug | 当时用户实际上切到了 MTK 控制器；以唯一 VID/PID fast preflight 防错，不改共用 Root Hub | **现场原因确认** [R3 `6.4](./Round3.md) |
| **R3-08** | Primary SMP `WaitEncryption` 时先收到 `IdentityInformation` 导致本地主动 PairingFailed | BE200 Bulk-IN SMP 先于 Interrupt-IN Encryption Change 交给 Host；代码先处理 SMP Queue、后检查 encrypted。加窄 handoff 路线先调整软件状态，**仍等待 HCI Encryption Change Controller 确认才进入 GATT**。Commit `046d74f` | **HIL** [R3 `13](./Round3.md) |
| **R3-09** | Secondary `PairingFailed 0x0B` / DHKey Check Failed | 固定 Bumble-RS 丢失 public peer address type，错误建模 random；f5/f6 地址类型 bit 不匹配。**仅在 expected public identity 与地址 bytes 完全相同、只有类型错时**修正为 public。Commit `4fa415c` | **HIL** [R3 `14–15](./Round3.md) |
| **R3-10** | 曾推测“第二耳必须 RPA / 第二次 Swift Pair / public 不可连接” | HIL 推翻：secondary 以 public identity + generic FAL 成功建链，RSI 只做候选、CSIS 是最终证明 | **Hypothesis 撤回** [R3 `9](./Round3.md) |
| **R3-11** | 曾推测 Local IRK 丢失导致第二耳 ACL timeout | 单独改成 FAL 就成功、未改变 Local Identity；保留稳定 IRK 作为基础设施，**撤销直接根因声明** | **Hypothesis 撤回** [R3 `9.4](./Round3.md) |
| **R3-12** | 共用 8s 扫描缓存的 RPA 后续直连失效 | RPA 会轮换，扫描→第二耳连接有时间差。实现 Controller Resolving List 装 IRK + FAL 固定 public identity。Commit `a80cf1e` | **HIL** [R3 `21–22](./Round3.md) |
| **R3-13** | Pairing 完成或 RSI MATCH 就要持久化 | 信任证据不充分；必须 `ProvisionalBond→encrypted CSIS SIRK/SetSize/Rank gate→唯一 commit.rs 原子提交`；失败丢弃 provisional 而不删旧 Bond | **HIL** [R3 `8/12](./Round3.md) |
| **R3-14** | Windows PowerShell `ΓÇö` 乱码 | UTF-8 em dash `E2 80 94` 在 Native Pipeline 用错代码页。引入输出边界 ASCII sanitizer，将 Unicode 标点降级 `-` / `->` | **Code/HIL** [R3 `17](./Round3.md) |
| **R3-15** | v1–v4 BondManager、旧 explicit-peer、重复 commit 均仍处活动路径 | 实验快速迭代遗留；静态审计保留唯一 Connection/Security/Commit 路线，旧模块归档 `deprecated/r2-pairing-experiments`，净删除约 4,690 行 | **Code/Review** [R3 `18](./Round3.md) |
| **R3-16** | 自动 Bootloader 初始化误触发/反复刷写风险 | `ensure_operational` 先查 image=03 直接复用、image=01 仅一次 PIN/Hash/Hardware Guarded Bootstrap、其他拒绝，无 HCI Reset/自动重试；Round4 有一次真实冷启证明 | **HIL** [R3 `19](./Round3.md)、[R4 `1](./Round4.md) |

## 4. 已确认的问题 / 修复 — Round 4

| ID | 现象与故障来源 | 根因及修复过程 | 验证 / 原文 |
|---|---|---|---|
| **R4-01** | `r4 codec-plan` CLI 出现 token 数不匹配 | Rust `match` arity 错、文档列出了命令却走不到分支。用纯 `parse_r4_invocation` 且在 Boot 前校验；扩展 CLI 回归测试 | **HIL/Code** [R4 `3](./Round4.md) |
| **R4-02** | ASCS Write 返回却漏 CP/ASE Notification | Bumble-RS 临时 `ExternalAttTransport` 同步请求缓存 unsolicited PDU，生命周期结束可能丢失。新增按 ACL Handle 的 Value Event Pump / Indication Confirmation；必须 CP SUCCESS + ASE Notify + GATT Readback | **HIL** [R4 `4.1](./Round4.md) |
| **R4-03** | 双耳 R4.2 Discovery 报 `ASE status has 3 trailing bytes` | Bumble-RS Idle `reader.finish()` 要求严格 2 字节，B3P 读回是可变 5 字节。先最多三次重读+原始 Hex：连续重复确认不是瞬时 USB 截断；再对照 AOSP Idle header | **HIL** [R4 `4.2–4.3](./Round4.md) |
| **R4-04** | 只支持尾部 `01 00 00` 的白名单修复后，R5.1 又遇到尾部 `00 00 00` 失败 | 错把不稳定尾部字段当固定签名。最终只在 **Idle、ASE ID 非零**时取前两个字节并日志记录额外尾部；Codec/QoS/Enabling/Streaming 继续严格解析 | **HIL 至 R5.2-B** [R4 `4.3](./Round4.md) |
| **R4-05** | `cargo test` 编译失败 `E0282`，但 `cargo check/build` 绿 | `qos_plan.rs:379` 测试闭包 `id.into()` 缺明确参数类型；加 `side:StereoSide,id:&str,acl:u16,rank:u8`。Commit `f69eff1`；**最终 HEAD 完整 cargo test 日志未附** | **Code；最终测试 Open** [R4 `5.3](./Round4.md) |
| **R4-06** | Set CIG Command Complete 不支持上游 typed CIS handles 返回 | 固定 Bumble-RS 的 `ReturnParameters::Raw`；在 Adapter 严格解 Status/CIG_ID/CIS_Count/Handle 数组，验证 Handle 唯一/合法和 CIS ID 对应 | **HIL R5.2-A/B** [R4 `6.2](./Round4.md) |
| **R4-07** | 单包 SDU 100B 容量判断忘了额外 header | HCI ISO first packet 还有 4B SDU info；ISO buffer gate 采用 100+4B ≤ 248B，避免错误单包判断；实际 ISO TX 仍未 HIL | **Code/Review** [R4 `5.2](./Round4.md) |
| **R4-08** | 第一次 R5.2-B `HCI LE Create CIS Status=0x0C` | Controller `CIS Central` 设定不够，**LE CIS Host Support Feature bit32** 未设置。新增专用 Pre-ACL `HCI LE Set Host Feature(0x2074,bit32=1)`、Readback；第二轮 bit32 false→true、Create CIS status0、2×Established | **HIL 干预对照** [R4 `7.2](./Round4.md) |
| **R4-09** | 第二次 R5.2-B 已 2×Established，但 CIS Disconnect 均 `0x12` | 错把 `Disconnection Complete` 事件 Reason `0x16` 填入 `HCI_Disconnect` 命令。改成合法命令 Reason `0x13`，增加 HCI 字节测试 `01 06 04 03 00 09 13`；第三次 2×Disconnect PASS | **HIL** [R4 `7.3](./Round4.md) |
| **R4-10** | 第二次 Release CP 已成功但 Idle Notify 缺失；Remove CIG 返回 `0x0C` | **CIS 未被断开**的连锁症状，不应强行忽略 Notify/伪报 CIG 成功。先修 Disconnect，再双耳 Release→Idle、Remove CIG | **HIL 第三轮** [R4 `7.3–7.5](./Round4.md) |
| **R4-11** | ACL 最终断开，下一轮仍有残留 CIG1 | 断开 ACL 隐式终止 CIS ≠ Remove CIG。加仅在无 ACL 的 Preflight 对自有 CIG1 Remove：Status0/ID匹配或不存在 0x02 可继续；0x0C 停止。第三轮显示 removed leftover inactive CIG=1 | **HIL** [R4 `7.4](./Round4.md) |
| **R4-12** | 误把 HCI Create CIS 的 Command Status 成功视作 CIS Established | 异步 Procedure 必须分别收到两条 `LE CIS Established(status0)`，检查 CIS→ACL Mapping 和 Device Link；部分建立也要清理 | **HIL** [R4 `7.1/7.5](./Round4.md) |
| **R4-13** | PowerShell `Tee-Object` 在 native CLI 非零退出时展示 `NativeCommandError` | 不是另一种 Windows 崩溃；定位日志中更早 HCI/ASCS 错误文本，而不是外层 Pipeline Stack | **诊断纠偏** [R4 `8](./Round4.md) |

## 5. 关键修复全过程（因果链）

### A. Pairing：广告、ACL、SMP 三种故障不是一个故障

1. Primary 选错同名 LE Audio advertiser；正确第一次 SMP 应使用 Swift Pair pairing-only RPA。
2. Secondary 使用 explicit-peer Filter Policy 0 导致 10s timeout，**改 FAL** 即成功；原先“必须换 RPA/重复 Swift Pair/Local IRK 丢了”均被 HIL 推翻。
3. 真正进入 SMP 后，**USB Bulk SMP 先于 Encryption Change Event** 被 Host 消费；软件状态不得据此立即 PairingFailed，但必须继续等待控制器加密确认。
4. Secondary DHKeyCheck 0x0B 是 **public/random 地址类型 bit** 丢失而非 SIRK 不匹配；修正只针对已验证 public identity，不对所有随机地址做强制类型转换。
5. Bond 仍先 Provisional，再由加密 CSIS SIRK/Size/Rank gate 才允许 Commit；后来 R3、R4 重复连接均验证 stored LTK 无新 SMP。

**负责代码**：`bumble-adapter/connection_manager.rs`、`security.rs`、`stored_security.rs`、`set_member_verifier.rs`、`commit.rs`、`privacy.rs`。见 [R3 `4–22](./Round3.md)。

### B. ASCS Idle：三次修正才真正闭环

| 阶段 | 实验数据/决定 | 结论 |
|---|---|---|
| 初次失败 | `01 00 01 00 00` 报 `3 trailing bytes` | 失败发生在 Discovery，尚未 Config Codec |
| 诊断 | 同一个 ASE 三次 Read raw 完全相同 | 不是单次 USB timeout；重试不能作为根因修复 |
| 过窄兼容 | 仅接受固定尾部 `01 00 00` | 一次双耳 HIL 过了，但后续发现 `00 00 00`、`01 01 00`、`01 02 00` |
| 最终策略 | 只对 Idle 状态读前 2 byte，其余所有非 Idle 仍严格 | R4.2、R5.1、R5.2-A/B 全部 HIL 通过 |

**限制**：无法最终断言额外 Idle 尾部来自三星固件还是更低层封装；没有独立双端 byte-for-byte 抓包。AOSP 的宽容解析只是兼容策略，不等于宣称该值符合 Core 规范。

### C. R5.2-B 三次 HIL：不要用第二次结果冒充最终成功

| HIL | 完成的部分 | 当时失败 | 纠正 |
|---|---|---|---|
| **1** `r5-establish-cis-1.log` | 双 Codec、双 QoS、双 Enable、Set CIG | Create CIS `0x0C`；未真正建立 CIS | 发现漏了 Host bit32，ACL 前 Set Host Feature 并 Readback |
| **2** `r5-establish-cis-2.log` | **双 CIS Established**、双耳 Streaming `0x04` | Disconnect 命令 0x12、Release Idle 通知 Timeout、Remove CIG 0x0C | 把 HCI Disconnect 命令 Reason 0x16 改 0x13，检查遗留 CIG |
| **3** `r5-establish-cis-3.log` | **双 CIS Established、显式断开两条、两耳 Idle、Remove CIG、ACL/Privacy Clean、Bond Unchanged** | 无未确认 Cleanup | **R5.2-B HIL PASS** |

HIL 第三次的 Controller 实测：C→P PHY 2M，Transport Latency **63,210 µs**，NSE2/BN1/FT7、Max PDU100、ISO Interval8 = 10ms。耳机 QoS Presentation Delay 固定 **40,000 µs**，不宜直接相加得出端到端延迟。本轮既无 ISO Data Path 也无任何 LC3 发送。

**重要资源事实**：第二次末尾断 ACL 使两个 CIS 被隐式终止，**没有证明 CIG 已 Remove**；第三次 Pre-ACL 实测清掉上一轮的 inactive CIG。不能改成“断 ACL 就算全资源释放”。

## 6. 尚未解决的 Quirk / 静态风险 / 假设（不当成已修复 Bug）

| ID | Quirk / 风险 | 当前结论 / 停止条件 |
|---|---|---|
| **Q01** | 三星 Swift Pair/identity/RPA/box beacon 多种广播共存 | 只用各自用途做证据，不仅凭广告名确认成员 |
| **Q02** | `xx:xx:37:23:18:46` 等 unnamed public、DataStatus=2、data_len=0 | **Open**。Raw HCI 证明确有数据，源头未证实。不得当真实成员 identity |
| **Q03** | 部分 RPA 无 RSI、盒灯变化后成员不可见 | **Observed**，不要删 Bond；以后解决产品化广告状态适配 |
| **Q04** | Bumble PACS Proxy 每方向只支持 1 个 PAC characteristic | **Known limitation**，B3P 当前每方向恰 1 个；多实例真实设备出现再扩展 |
| **Q05** | `libusb timeout -7` 携带 partial bytes | **已处理**：必须保留并传入 HCI reassembler，不能为了清爽日志删除逻辑 |
| **Q06** | RPA 频繁轮换、rank1 恰为右而不是左 | **设备事实**：Physical Left/Right 一律用 PACS Sink Audio Location |
| **Q07** | 两耳 `ASE STREAMING(0x04)`，却还没有声音 | **分层事实**：ASCS Streaming 不等于 ISO Data Path 或音频 TX |
| **Q08** | 63.210ms Controller Transport Latency vs 40ms Presentation Delay | **Open**：时基定义待 R5.3 研究，不能武断称端到端 103.21ms |
| **Q09** | `iso-scheduler` 只有 frame quantum，没有真实 ISO 时钟/credit | **Open**：R5.3 |
| **Q10** | USB ISO Bulk-Out 分支存在但从未 HIL | **Open**：验证 packet/header/short write/credit/backpressure |
| **Q11** | LC3 Encoder + 100B SDU + 双耳实际可听测试 | **Open**：First Sound 尚未发生 |
| **Q12** | Intel Firmware 的发行许可与恢复原厂驱动 | **Open**：发布前处理，不能由硬件成功推出合法可再分发 |
| **Q13** | 最终 HEAD 的全部 `cargo test` 输出未归档 | **Open**：前面 17/17 仅代表旧版本，R5.3 开工再完整执行 |
| **Q14** | 长稳/多次循环、单耳丢失、异常 Recovery | **Open**：R6，当前最少一次完整 R5.2-B PASS |
| **Q15** | ToolHub 底层 `frontend-npm-build spawn EINVAL` | 只修了 stale operation lock 释放；**EINVAL 本身没有充分根因证据** |
| **Q16** | BTstack `Setup.Length=sizeof(size)` 潜在 bug | **仅静态审计可疑**，未在现行 Bumble-RS/BE200 线路上 HIL 验证 |
| **Q17** | 部分早期猜想被撤回（第二耳要 RPA、local IRK 丢失、第二次 Swift Pair） | **不要再用这些猜想作为默认修复指南**；回 R3 原文审证据 |

## 7. 防回归原则：以后遇到问题先读这里

1. **不要清 Bond 或重复配对**：正常 HIL 从已有 LTK/IRK 恢复；配对键写入只允许经过 CSIS gate 的事务。
2. **不要每次重置 BE200**：Operational 使用已签名固件现态；Bootloader 才执行一次守卫的 Intel Secure Send。
3. **不要默认 explicit-peer 连接**：Controller Resolving List + identity FAL；左/右根据 PACS Location，Handle 只是当次生命周期产物。
4. **不要丢 unsolicited ATT**：ASCS CP Success + Notification + GATT Readback 缺一不可；失败回滚含可能已送达的写命令。
5. **不要把所有 ASE 都截短成 2B**：只有 Idle 采用头部容错，其它状态完整解码。
6. **不要以 Set CIG 得到 Handle 代表 Established**：必须等两条 LE CIS Established 异步事件。
7. **不要在 ACL 存在后才设 Host CIS bit32**：Pre-ACL 初始化并独立 LE Features Readback。
8. **不要再向 HCI Disconnect 命令写 Reason 0x16**：这只在本地 Disconnection Complete 事件有意义；命令使用规范允许的 0x13。
9. **不要以断 ACL 代替 Remove CIG**：有失败或异常时、CIG 不确定则不能回报完整 PASS。Pre-ACL 只检查本程序自己的 CIG1，不得干扰别的 Controller owner。
10. **不要把 Build、Unit Tests、HIL、First Sound 混成一个状态**：远程 GitHub/ToolHub 做源码检查与编译，用户本机执行射频实验，保存日志。
11. **ToolHub/GitHub 错误优先核查状态后重试**：检查 HEAD、blob、operation lock；不要盲目重复写入。
12. **不修改独立 Win11 Quick Settings 实验分支或 LE-Audio-Relay 产品**：本归档只记录 BE200AudioHub Round1–4。

## 8. 原始证据和章节定位

| 资料 | 最有效的章节 |
|---|---|
| [Round1.md](./Round1.md) | `2–3 USB/Firmware、`5 BTstack 研究纠偏、`7.4 编译与流程故障 |
| [Round2.md](./Round2.md) | `4 无 Reset、`6 Hot Path PowerShell、`7.3 Own Addr、`15–22 SMP/RPA 风险、`29/34 工具故障 |
| [Round3.md](./Round3.md) | `4–17 Pairing、`21–22 RPA/FAL、`28 Q1–Q20 完整 Quirk 表 |
| [Round4.md](./Round4.md) | `4 Idle/通知、`5 E0282、`6 Set CIG、`7 三次 CIS、`8 B01–B10 / Q01–Q06 账本、`11 七份 HIL 记录索引 |
| [TIMELINE.md](./TIMELINE.md) | 里程碑状态、commit、证据索引以及 R5.3 接续点 |

**最后状态**：四轮控制面到双 CIS/规范清理全部经真机验证；紧接着应研究 R5.3 HCI ISO Data Path 和 LC3 发包，不再反复解决已关闭的 R1–R5.2-B 问题。


---

## 9. Round 5 / Review — architecture and implementation corrections

| ID | Symptom / wrong assumption | Root cause / final correction | Verification |
|---|---|---|---|
| **R5R-01** | R5.3 pitch glide and nominal 8 s audio taking ~9.9–10.8 s | Media/source production was coupled to controller credit/completion timing. Freeze rule: completions release transport capacity only; StreamClock owns media time | Experiment timing comparison + Review architecture |
| **R5R-02** | Event/completion servicing latency changed when ACL read timeout changed | Event-IN and ACL bulk-IN were synchronously round-robin polled. Review L3/L2 uses concurrent pending async transfers | Review-1 48/48 software tests |
| **R5R-03** | Intermediate async backend could free transfer in callback while backend retained raw pointer | Split ownership created cancel/completion UAF risk. Pending transfer table became sole owner; callback only records terminal state | callback ownership tests |
| **R5R-04** | High-rate async path would allocate/free libusb transfer objects every completion | Allocation jitter risk. Reuse persistent RX slots and bounded per-class idle TX transfer pools | transfer reuse tests |
| **R5R-05** | Bumble `ExternalHost` looked like easiest Review Host reuse | It owns a blocking reader thread and synchronous `send_command` waits, violating HciContext contract. Review uses `Device + HostTransport` substrate instead | source audit; HCI WIP |
| **R5R-06** | Cargo dependency fetch appeared broken | Rule proxy did not cover Cargo/GitHub/crates.io; switch to global proxy | dependencies fetched; local compile reached 74 tests |
| **R5R-07** | Async ToolHub run returned before result, initially impossible to verify exit status | MCP exposed only run creation. Added get/list/cancel run tools and process-tree cancellation | run_id/get_run/list_runs/cancel_run verified |
| **R5R-08** | Third-block HCI WIP caused default test suite failures during Review-1 sealing | HCI attach/scan synthetic fixtures are not Review-1 acceptance criteria. Gate HCI WIP behind `hci-wip`; keep default Review-1 baseline green | default 48/48; hci-wip separately tracked |
| **R5R-09** | Upstream research repeatedly required large source sweeps | No implementation-facing function index. Built `docs/reference-index/` with exact project/file/function/type anchors and test obligations | docs/reference-index complete |
| **R5R-10** | Active `docs/Timeline/` disappeared during Review docs reorganization | Historical Timeline existed only in `deprecated/r5/docs/Timeline/`. Restore it byte-for-byte into main docs, preserve archive as immutable source, append Review as new rounds | Git blob reuse + Round5 append |

### Open Review risks

| ID | Risk | Current policy |
|---|---|---|
| **QR5-01** | `hci-wip` synthetic resetless attach does not yet reach Ready in six tests | **CLOSED in Round6**：status-only host-init completion normalization；HCI Host promoted；Review-2A HIL PASS |
| **QR5-02** | Bumble-RS LE Read ISO TX Sync parser has 24-bit Time_Offset width bug | Do not trust current parser; patch/adapter plus golden test before controller-anchored clock |
| **QR5-03** | Async transport is software-tested but not yet re-HILed as the active Review Host path | **CLOSED in Round6**：Review-2A and Review-2B both passed real BE200/B3P HIL on the async Review path |
| **QR5-04** | Exact controller-anchored StreamClock strategy remains future work | Initial scheduler uses local authoritative interval grid; controller anchor is pluggable future scheduler |

---

## 10. Round 6 / Review-2A + Review-2B corrections

| ID | Symptom / wrong assumption | Root cause / final correction | Verification |
|---|---|---|---|
| **R6R-01** | HCI WIP had 6 synthetic attach/scan failures; host-init stalled | Pinned Bumble parsed several status-only Command Complete packets as Raw while Device init expected Status. Add narrow opcode allowlist normalization at Review adapter boundary; unknown Raw remains Raw | **Code**; default HCI tests green; Review-2A HIL PASS |
| **R6R-02** | Cold BE200 cannot be assumed Operational when async Transport opens | Firmware bootstrap is a separate short-lived L3 owner: FC05 probe, optional guarded one-shot Intel load, release interface, then async Transport reopen | **Code + HIL**; Review-2A cold path Booted PASS and Review-2B AlreadyOperational PASS |
| **R6R-03** | Firmware/bootstrap logic risked leaking into HCI Host/Transport ownership | Create `FirmwareBypassPort` + pure `Be200FirmwareLoader` + pinned planner; no retries/reset/power-cycle fallback | **Code + HIL** |
| **R6R-04** | Controller Resolving List mutation could leave partial state on failure | Typed privacy transaction with disable→clear→capacity→add→timeout→enable and mandatory disable+clear rollback; explicit cleanup transaction attempts clear even if disable fails | **Code 151/151 + Review-2B HIL** |
| **R6R-05** | Reimplementation might drift from R5-proven FAL connection parameters | Static audit against frozen `connection_manager.rs`; keep filter_policy=1, own public, ANY peer, 1M, scan 0x10/0x10, interval 24–40, latency0, supervision42 | **Static audit + Review-2B HIL** |
| **R6R-06** | Timeout handling inside HCI worker would violate requester-owned timer contract | HCI transaction has explicit cancel event only; requester timeout triggers LE Create Connection Cancel. Accepted/rejected/late-success races modeled explicitly | **Code tests** |
| **R6R-07** | Historical disconnect bug could recur by using event reason 0x16 as command parameter | Disconnect API accepts only ConnectionToken; command reason fixed 0x13, event reason 0x16 recorded only as result | **Code + HIL** |
| **R6R-08** | Stored-LTK restore could be mis-modeled as local command success | Success requires Command Status + real Encryption Change + Bumble encrypted state; connection loss while waiting is terminal | **Code + Review-2B HIL** |
| **R6R-09** | Controller may reuse ACL handle; stale work could hit a new connection | Add `ConnectionToken { handle, generation }`; reused handle increments generation; stale disconnect/encrypt rejected | **Code regression test** |
| **R6R-10** | Pinned Bumble may swallow ACL over-completion accounting errors | Add non-scheduling audit ledger: only successful enqueue counts submitted; per-handle completions retire exact capacity; unknown/overflow completion = EngineFault | **Code regression tests** |
| **R6R-11** | DeviceEvent single-consumer routing could hide one event from another active transaction | Change to typed fan-out; unconsumed high-level events use bounded deferred queue cap64; overflow=fault | **Code tests** |
| **R6R-12** | Read BD_ADDR through Bumble could trigger upstream key-store/privacy refresh and violate Review ownership | One-shot journal-only completion suppression for controller identity probe; Bumble does not see the completion | **Code test + Review-2B identity HIL** |
| **R6R-13** | Reusing old BondStore implementation would reintroduce write/reseed/pairing surface | Add L0-only read-only loader for four existing bond files; use pinned JsonKeyStore schema; no mutation API | **Code + bond fingerprint HIL** |
| **R6R-14** | Review-2B HIL could run against incomplete stored coordinated-set metadata | Preflight requires stored `set_size=2` and exactly two CSIS-verified members; pure local JSON check, zero RF time | **Code test + HIL preflight** |
| **R6R-15** | IRK/LTK/RAND could leak through derived Debug output in HIL logs | Custom redacted Debug for privacy/security key material | **Code tests** |
| **R6R-16** | Temporary Review encryption wait was 5 s, shorter than R5 proven success budget | Restore stored-LTK wait to R5 baseline 10 s to avoid false HIL timeout | **Static audit + final HIL PASS** |
| **R6R-17** | Adding new worker state repeatedly caused a branch to miss the new publisher argument | Cargo exhaustive typecheck caught every omission. Future refactor should bundle last-published states instead of linearly growing function parameters | **Compile-time catches** |
| **R6R-18** | Long-chat resume caused a duplicate HIL executor/helper draft | Full-file reread showed existing executor; remove duplicate rather than patching conflicts. Cargo duplicate-definition errors prevented merge | **Code + final 151/151** |
| **R6R-19** | `Cargo.lock` was immediately redirtied after restore, blocking ff-only pull | Cargo/rust-analyzer regenerated lock for new direct dependencies. Commit canonical lock remotely, then reset local branch to origin after confirming Cargo.lock was the only local diff | **Local Git verification** |
| **R6R-20** | New set_size guard broke an old test fixture | Production guard was correct; fixture lacked `set_size`. Add `set_size: 2` to fixture; no production relaxation | **151/151 tests** |
| **R6R-21** | Upstream synchronous `GattClient/AttTransport::request()` was tempting to expose as Review GATT API | Synchronous request/wait violates HciContext ownership and timeout policy. Keep Bumble `Device` as ACL/L2CAP/ATT substrate but port discovery/read/write reducer into event-driven typed GATT FSM | **Code + 180/180 + Review-2C HIL** |
| **R6R-22** | ATT requester timeout/cancel could let a late response be correlated to a later request on the same fixed bearer | ATT has no wire cancel. After local cancel, mark that handle's fixed bearer poisoned until real disconnect; late response is reported/dropped; new request gets `BearerPoisoned` | **Deterministic tests** |
| **R6R-23** | CAS Included Service discovery cannot assume every include declaration carries UUID inline | ATT include declaration may be 6-byte form with UUID or 4-byte form requiring a follow-up Read of the included service declaration. Implement both, required for CAS→CSIS | **Deterministic tests + B3P HIL CSIS=true** |
| **R6R-24** | ATT Error Response could be accepted solely because an error arrived while a request was active | Error response also carries `request_opcode_in_error`; require exact match with current request opcode, otherwise Protocol failure | **Deterministic regression test** |
| **R6R-25** | A happy-path ATT response in one HCI ACL packet would not prove real fragmentation/reassembly ownership | Add response fragmented across HCI ACL pieces; first fragment must not complete transaction, final continuation completes only after Bumble ACL/L2CAP reassembly | **Deterministic regression test** |
| **R6R-26** | Direct `bumble-att` dependency initially left local lock regeneration fighting ff-only sync | Commit canonical lock remotely and make normal Cargo operations use `--workspace --locked`; dependency changes must land manifest+lock together before local pull | **ToolHub local verification** |
| **R6R-27** | Generic recovery via `git reset --hard` would violate ToolHub's fixed-capability design | Add fail-closed `git-sync-generated`: only sole unstaged Cargo.lock, clean index, incoming canonical lock, fast-forward; otherwise refuse. Keep generic reset/restore unavailable | **ToolHub operation verified; clean tree correctly refused** |
| **R6R-28** | First locked `cargo-test` after ToolHub hardening failed with E0063 missing `GattCharacteristic.declaration_handle` | Earlier warning-cleanup accidentally removed the initializer field. Restore `declaration_handle`; rerun test/check/build | **180/180; check/build PASS; git clean** |

| **R6R-29** | A bounded Bridge→HCI mailbox looked sufficient to bound realtime ISO backlog | Pinned Bumble `DataPacketQueue` bounds controller `max_in_flight` but its waiting `VecDeque` is unbounded. Add pre-Bumble complete-group admission based on real ISO packet length/window; normal traffic may not move Host pending beyond controller capacity | **Deterministic window test + real HIL 8-packet window** |
| **R6R-30** | Realtime ISO traffic risked starving teardown/control or transferring backpressure into HCI worker | Add dedicated bounded `HciIsoTx` lane, explicit per-turn data budget, priority stop→control→controller events→ISO, and at most one worker-local backpressured group | **Worker fairness tests + HIL** |
| **R6R-31** | A retained backpressured group could be retried after ISO Path removal and fault the HCI worker during teardown | Introduce reliable ISO drain/resume gate. Drain blocks affected CIS, drops only not-yet-admitted stale groups atomically, waits for already-admitted packets to truly complete before path/CIS teardown | **Deterministic drain tests + HIL reverse teardown PASS** |
| **R6R-32** | Review HCI Host initially lacked the R5-proven CIS Host Support bit32 pre-ACL gate; Create CIS can return 0x0C without it | Port a typed `HCI LE Set Host Feature (0x2074)` transaction with zero-ACL guard and LE feature readback; Command Complete alone is not sufficient proof | **Unit tests + real ISO HIL** |
| **R6R-33** | First `iso-smoke` assumed exactly one ASCS Sink ASE per Bud and failed: `expected exactly one ... 0x2BC4, found 2` | Restore sealed R5 policy: enumerate all Sink ASEs, require nonzero unique ASE IDs, read state, choose lowest-ID Idle Sink ASE, leave all others untouched | **Regression test + B3P HIL selected ASE=1/handle=0x0046** |
| **R6R-34** | After multi-ASE fix, ISO smoke repeatedly timed out on the first identity-FAL ACL even after three retries and re-boxing earbuds | Review smoke omitted the R5 pre-connect discovery gate. Restore passive extended scan + host-side IRK resolution of current RPA; require 2/2 bonded members currently connectable before controller RL/privacy + identity FAL | **RPA resolver test + HIL 2/2 discovery PASS; subsequent ACL/CIS/ISO PASS** |
| **R6R-35** | Failure rollback treated controller-allocated CIS handles as if CIS links were already Established, creating false cleanup errors when Create CIS failed early | Track allocation and actual `EstablishedCis` separately; only real established handles are drained/disconnected, while CIG cleanup still follows may-exist semantics | **Code review + 245+ regression suite** |
| **R6R-36** | Direct `bumble-smp` dependency for the RPA helper left local `Cargo.lock` dirty because the crate already existed only transitively | Add canonical direct root lock entry, push remotely, then use fail-closed `git-sync-generated` to prove Cargo.lock was the sole generated diff and fast-forward safely | **ToolHub sync + clean git status** |
| **R6R-37** | Using valid LC3 inside Review-2D smoke would blur HCI Host validation with future Media/codec ownership and add an unnecessary root codec dependency | Keep ISO smoke payload deterministic but codec-opaque 100-byte data. Treat audible digital output only as supplementary endpoint evidence, never as valid-LC3/acoustic First Sound | **HIL 200/200 ISO packets + audible endpoint evidence; claim explicitly scoped** |

### Round6 closed gates

```text
Review-2A attach/capabilities/scan     software + HIL PASS
Review-2B privacy/dual ACL/stored LTK software + HIL PASS
Review-2C ACL/L2CAP/ATT/GATT          software + HIL PASS
Review-2D CIG/CIS/ISO Host             software + HIL PASS
Bond mutation                         NONE; fingerprint unchanged
ToolHub normal Cargo gates            --workspace --locked
Round6 final software gate             247/247 + check/build PASS
Round6 engineering boundary            Review-2D real HIL closure
```

Round6 internally crossed a chat-migration boundary after Review-2D design/oracle audit, but the successor-chat implementation and HIL are now formally counted as the same Engineering Round. Round7 begins at Control semantic migration.

See [Round6.md](./Round6.md) §22–§24 for the chat boundary, Review-2D continuation, final HIL and Round7 handoff.


---

## 11. Round 7 / Control semantic path + product data-plane corrections

| ID | Symptom / wrong assumption | Root cause / final correction | Verification |
|---|---|---|---|
| **R7R-01** | It was tempting to let Media/Control carry raw CIS handles because Review-2D APIs already used them | This violates the frozen ownership boundary. Introduce `CisRef { cig_id, cis_id }`, sanitized CIS/ISO-path reports, and keep raw controller handles inside HCI Host only | semantic CIS/path tests + Control regression |
| **R7R-02** | ASCS Config Codec/QoS/Enable could appear complete after ATT Write Response | ATT success is only transport success. Product ASCS op now requires ATT Write Response + CP SUCCESS + expected ASE notification + independent ReadValue readback | out-of-order three-gate tests + rejection tests |
| **R7R-03** | Control snapshot initially kept only ASCS Control Point value handle | CCCD discovery needs declaration/end-group range. Preserve semantic `GattCharacteristicRange` for CP and selected ASE descriptor range | compile + deterministic descriptor/subscription tests |
| **R7R-04** | A semantic ASCS operation timer tied only to the active GATT request left a timeout gap after Write Response while waiting for CP/ASE notifications | Separate GATT-request timer from whole-operation `AseOperation { side }` semantic timer | ConfigCodec/QoS/Enable timeout regressions |
| **R7R-05** | Generic failure cleanup could jump directly to CIG/ACL while ISO Path, CIS, or remote ASE state remained live | Formalize reverse stream cleanup: route unbind → ISO Path R/L → CIS R/L → ASE Release R/L → Remove CIG → ACL R/L → privacy | stream rollback ordering tests |
| **R7R-06** | Create CIS timeout tempted speculative cleanup | Timeout does not prove whether controller created a CIS. Fail closed instead of guessing RemoveCig; only explicit partial-failure evidence is rolled back via known live `CisRef` | Create-CIS timeout + partial failure tests |
| **R7R-07** | ISO Path setup/remove failure can be ambiguous even when command result is malformed | Preserve `may_have_configured`; uncertain setup enters path-first rollback, and remove failure/timeout prevents unsafe CIS teardown | path uncertainty + rollback tests |
| **R7R-08** | Exposing `StreamBinding` immediately after ASE STREAMING would allow Media to run before HCI logical route admission is proven | Add reliable `BindStreamRoute` request/ACK. Control exposes binding only in `StreamBoundReady` after `StreamRouteState::Bound` | route-ack gating tests |
| **R7R-09** | Session/stream route could become stale after Input Path removal or CIS teardown | HCI route table is session-fenced and invalidated by path/CIS retirement; exact replay is idempotent, conflicting rebind rejected | route readiness/invalidation tests |
| **R7R-10** | Adding a separate “logical” ISO queue would bypass Review-2D bounded-lane/fairness/admission guarantees | Raw and logical ISO requests share one bounded data-lane envelope and the same worker/admission path; logical groups resolve to raw handles inside HCI Host | shared-lane tests + logical/raw envelope tests |
| **R7R-11** | Stale logical ISO payload could fault worker during teardown after route/path invalidation | Classify stale logical data as superseded payload and keep teardown/control independent from payload validation; do not resurrect stale data | stale/drain classification regressions |
| **R7R-12** | Realtime HCI lane backpressure could tempt retry/wait and therefore move media time | Product bridge drops the complete stereo interval on `Full`, never retries it, and continues the next absolute FrameId/deadline | bridge runtime backpressure test |
| **R7R-13** | Relative sleep scheduling would reintroduce R5 drift | Implement `LocalStreamClock` as an absolute 10 ms grid. Callback jitter/late wake skips expired intervals rather than stretching time | StreamClock absolute-grid tests |
| **R7R-14** | Missing Media frame fallback must still be valid codec data | Generate real LC3 silence fallback with the same 48k/10ms/100B format and verify decoder output near zero | LC3 silence decode regression |
| **R7R-15** | A new `stream_route.rs` file existed but was not actually exported from `hci_host::mod`; test count stayed unchanged and created a false-green risk | Explicitly export module and require expected test-count delta; after fix route tests entered normal suite | gate moved 306 → 310 with four route tests |
| **R7R-16** | Stream route commands/ACK types were exposed in mailbox before worker command/report wiring was complete, temporarily breaking `main` exhaustive matches | Close worker routing and explicit `StreamRouteState::{Bound,Unbound,Failed}` ACK mapping; legacy Review-2 tracker explicitly ignores product route reports | compile recovery + worker/mailbox route tests |
| **R7R-17** | Adding pinned LC3 dependency left recurring dirty `Cargo.lock` despite no version change | Root dependency order was not Cargo-canonical. Canonicalize remote lock then use fail-closed `git-sync-generated`; keep normal gates `--locked` | final 350/350 locked tests + clean tree |
| **R7R-18** | A valid-LC3 HIL could be overclaimed as full product runtime acceptance | Dedicated `round7 first-sound` explicitly reuses sealed Review-2D setup/teardown and replaces only data plane. Claim is HIL READY until real run; full Core/Lifecycle remains future integration | parser scope + CLI text + code audit |

### Round7 current gate / open hardware gate

```text
Control semantic path                        SOFTWARE COMPLETE
HCI logical route / logical ISO lane         SOFTWARE COMPLETE
Synthetic PCM + valid LC3                    SOFTWARE COMPLETE
LocalStreamClock + IsoTxBridge               SOFTWARE COMPLETE
ControlAction -> HciControl dispatcher       SOFTWARE COMPLETE

cargo test --workspace --locked              350/350 PASS
cargo check                                  PASS
cargo build                                  PASS
git status                                   clean/synced

Round7 valid-LC3 First Sound HIL             READY / NOT YET RUN
full Core/Lifecycle runtime                  NOT YET INTEGRATED
```

See [Round7.md](./Round7.md) for the full chronology, claim boundary, HIL command and next-chat handoff point.

## 12. Round 8 / Structural + live-relay baseline

| ID | Symptom / wrong assumption | Root cause / correction | Verification / status |
|---|---|---|---|
| **R8R-01** | Windows Process Loopback activation failed with `E_ILLEGAL_METHOD_CALL` | Hand-written completion handler did not expose `IAgileObject`; add agile COM QI support | **FIXED**; two consecutive 10 s live HIL PASS |
| **R8R-02** | First 300 s longevity run reached 100% data window but normal Stop timed out releasing both ASEs | Normal Stop had inherited failure-rollback ordering. Split graceful route→Release→path→CIS→CIG from fail-closed failure rollback | **PARTIAL CORRECTION** in `742eb75c`; data window proven, teardown handed open to Round9 |
| **R8R-03** | Release terminal readback was treated as IDLE-only | Peer may cache codec state after Release; accept terminal `CODEC_CONFIGURED` for Release as well | **FIXED** in `a48b2a54`; regression covered |
| **R8R-04** | Long-run telemetry was too coarse to localize media/ISO degradation | Add ten equal longevity checkpoints and per-window PCM/bridge/ISO conservation snapshots | **ADDED** in `a11efdff`; observer causality investigated and exonerated in Round9 |

## 13. Round 9 / Teardown + recovery + ACL boundary

| ID | Symptom / wrong assumption | Root cause / correction | Verification / status |
|---|---|---|---|
| **R9R-01** | Ctrl+C killed the sole userspace Host while BE200/peer state survived; following launches repeatedly failed member visibility until BE200 power-cycle | Resetless next-generation attach trusted an unknown controller state; userspace Host/controller ownership can split-brain after abnormal death | **CONFIRMED OPEN DEFECT**; graceful Ctrl+C + durable DIRTY/CLEAN marker + conditional HCI Reset/reinit + supervisor required |
| **R9R-02** | Graceful Stop waited for terminal Idle/Codec Configured before ISO path/CIS teardown | Reference choreography uses ASE `RELEASING` as the transport-teardown pivot | **FIXED MODEL** in `96cf90a5`; did not alone eliminate 30 s failure |
| **R9R-03** | `CleanupAse` timeout hid which Release gate stalled | Cleanup timer expired before the equally-budgeted inner GATT timer and erased transaction detail | **FIXED DIAGNOSTICS**; `AseOperationGateSnapshot` preserves phase/pending/write/CP/ASE state |
| **R9R-04** | 10%-heartbeat was suspected of causing teardown failure through active IsoTxAudit/report draining | 30 s **normal mode without longevity polling** reproduced the same failure | **EXONERATED**; observer path is not root cause |
| **R9R-05** | Release depended on ATT Write Response and 30 s teardown showed `write_complete=false` | ASCS Control Point should use Write Without Response; map product ASCS ops to `GattOperation::WriteCommand` while retaining CP/ASE semantic gates | **FIXED / reference-aligned** in `85300c88`; failure moved to `write_complete=true, cp_success=false, notified=None` |
| **R9R-06** | Local WriteCommand completion could be mistaken for proof the packet reached BE200 | Bumble L2CAP/ACL can queue above the actual HCI submit boundary | **OPEN DIAGNOSTIC**; `d1fb532c` + `084fa605` expose Bumble pending vs actual ACL in-flight; next HIL pending |
| **R9R-07** | Some 30 s live runs show isolated 480-frame PCM underflows while drop/bridge/ISO conservation remain clean | Negative drift / scheduling gap is not yet corrected; current drift guard primarily handles positive accumulation | **OPEN Wave-2**; independent from teardown failure |


## 14. Round 10 / graceful-teardown investigation closure and cleanup

| ID | Symptom / wrong assumption | Evidence / correction | Verification / status |
|---|---|---|---|
| **R10R-01** | Release timeout was still being treated as an isochronous teardown problem | Final `acl-only` model reproduced simultaneous dual-ATT death with no CIG/CIS/ISO/media. Host→controller ACL, NCP, HCI Event, RSSI and LL Remote Features remained live while Controller→Host HCI ACL data stopped | **LOCALIZED / ROOT CAUSE UNRESOLVED**; sampled transition in approximately `(20 s, 25 s]` |
| **R10R-02** | Resetless attach may have inherited stale Controller→Host HCI flow-control state | Explicitly disabled Controller→Host flow control before connection creation; Model A failed at the same boundary | **FALSIFIED** |
| **R10R-03** | One pending libusb ACL Bulk-IN read may leave a re-arm gap that wedges BE200 | Implemented depth-two A/B with two genuine outstanding reads; at failure both reads remained pending and no new ACL-IN completion arrived | **FALSIFIED** |
| **R10R-04** | Round9 forensic/A-B code had become a large permanent cross-layer surface | Audited `96cf90a5..941092e3`; archived full tracked tree; removed Review profiles, final models, probe/report APIs, flow-control A/B, Bulk-IN depth A/B, cross-layer transport audit and related tests | **CLEANED** in `ca12d763` + `8176e635` |
| **R10R-05** | Cleanup could accidentally remove unrelated product architecture | Compared post-cleanup active `src/` against pre-investigation baseline `a11efdff`; only three intentional files differ | **PASS** |
| **R10R-06** | Cleanup could silently break deterministic behavior | Re-ran full locked software gate after cleanup | **408/408 tests PASS; check PASS; build PASS** |
| **R10R-07** | Current ungraceful normal-Stop implementation could be mistaken for a finalized Round10 architecture decision | Round10 documentation records it only as current code state and explicitly defers the final teardown contract to the next chat | **DECISION DEFERRED** |
