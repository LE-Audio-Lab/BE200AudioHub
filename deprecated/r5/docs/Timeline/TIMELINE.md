# BE200AudioHub — 跨 Round 工程时间线 / Timeline

> **归档日期：2026-10-14** · 范围：Round 1–4 · Windows 用户态 Intel BE200 + Galaxy Buds3 Pro。
>
> 本文件是导航和对照索引，**不替代各 Round 原始纪要**。历史文档中“当前 / 下一步 / 未完成”的字样只对该 Round 当时成立。最终验证基线：`eac486691fd4d7181d36bb09a620e942d0026936`（R5.2-B HIL PASS）；其后的 Timeline 文档提交不代表新功能或新 HIL。

## 阅读入口

| 文档 | 内容 | 读法 |
|---|---|---|
| [README.md](./README.md) | 归档导览 / 文档规范 | **首先阅读** |
| [AllRounds.md](./AllRounds.md) | 四轮原文完整汇编（依序） | 要看连续原始过程 |
| [Round1.md](./Round1.md) | Rust/WinUSB、Intel firmware、HCI、BLE 广播 | R1.1–R1.6 |
| [Round2.md](./Round2.md) | Bumble-RS Host 引入、无 Reset attach、扫描、首条 ACL；后续 Pairing 的**设计** | R2.1–R2.3 |
| [Round3.md](./Round3.md) | R2.4 两成员 SMP/Bond/CSIS 落地、Stored Verify、R3 双 ACL + Audio Service discovery | R2.4、R3 |
| [Round4.md](./Round4.md) | Codec/QoS/CIG/CIS 全控制面；三次 R5.2-B HIL | R4.1–R5.2-B |
| [BUG_FIX_REGISTER.md](./BUG_FIX_REGISTER.md) | **跨四轮独立 Bug、Quirk、失败假设、修复和验证总账** | 排障与回归时阅读 |

## 1. 全程里程碑（实际实施顺序）

| Round / 时间 | 技术门槛 | 最后可证明的状态 | 代表性事实 / 记录 |
|---|---|---|---|
| Round 1 · 2026-10-11 | R1.3a/b 设备绑定 | **HIL PASS** | 仅对 BE200 `8087:0036` 手动绑 WinUSB，普通权限 Rust 可 Claim/Release Interface 0 |
| Round 1 | R1.4 Intel Vendor HCI | **HIL PASS** | 独立 `0xFC05` 读到 Operational image 0x03 |
| Round 1 | R1.5a / R1.5b 固件解析与独立冷启 | **HIL PASS** | 原厂 003 / Rust 004：3,960 条 Secure Send 逐包匹配，Bootstrap 到 Operational |
| Round 1 | R1.6a / R1.6b | **HIL PASS** | CIS Central=true；ISO buffer **248 B × 8**；LE Extended Passive Scan 看到 B3P |
| Round 2 · 2026-10-11 至 10-12 | R2.1 Bumble-RS no-reset attach | **HIL PASS** | 固定 Bumble-RS `bbac2a6`、不用 `Device::power_on`/HCI Reset |
| Round 2 | R2.2 实时 BLE scan | **HIL PASS** | Bumble `take_advertisements` 接收真实 Buds3 Pro 广播 |
| Round 2 | R2.3 首条物理 LE ACL | **HIL PASS** | 同进程 scan→connect→保持 2 s→HCI Disconnect；命令 Reason 0x13，事件 Reason 0x16 |
| Round 2 截止点 | R2.4 SMP/双耳配对 | **当时仅规划** | 不能引用 Round2 为“Bond 已完成”的证据；由 Round3 后续完成 |
| Round 3 · 2026-10-13 至 10-14 | R2.4 Primary Swift Pair + 次成员 pairing | **HIL PASS** | 两成员独立 IRK/LTK；ProvisionalBond→encrypted CSIS→原子 Commit；每耳只配一次 |
| Round 3 | Stored Bond Verify | **HIL PASS** | IRK 解析当前 RPA→FAL→stored LTK 加密→CSIS SIRK/Rank；**零 SMP** |
| Round 3 | R3 双加密 ACL / PACS / ASCS / CAS | **HIL PASS** | Resolver List + identity FAL；右 rank1/Location=2，左 rank2/Location=1；同一时间两 ACL |
| Round 4 · 2026-10-13 至 10-14 | R4.1 Codec Plan | **HIL PASS** | 每耳 ASE1；LC3 48 kHz/10 ms/100 octets/Mono，Media Context |
| Round 4 | R4.2 Config Codec + Release | **HIL PASS** | CP Success + ASE Notification + GATT Readback；两耳 Release Idle；解决 Idle 多余 3 byte |
| Round 4 | R5.1 QoS Plan | **HIL PASS（Dry Run）** | 读取真机偏好 RTN=13、Max Transport Latency=95 ms、固定 Presentation Delay=40 ms |
| Round 4 | R5.2-A HCI Set CIG + ASCS Config QoS | **HIL PASS** | CIG1 分配 CIS1/2，双耳 QoS Configured、Release、Remove CIG；**还未建立 CIS** |
| Round 4 | R5.2-B ASCS Enable + Create Dual CIS | **HIL PASS（第 3 次）** | 两条 LE CIS Established、两耳 Streaming、显式断开两 CIS、Release 两耳、Remove CIG、Bond fingerprint unchanged |
| **下一步** | **R5.3 ISO Data Path + LC3 First Sound** | **未 HIL / 未产生声学证据** | Setup/Remove ISO Path，HCI ISO transport，编码和音频时序仍要实施 |
| 未来 | R6 Lifecycle / Stability / Daily | 未开始产品验收 | 异常恢复、重复运行、声像/时延/稳定性等 |

### 状态不能越级

```text
WinUSB Open → Intel Operational → BLE Advertisement → LE ACL
           → SMP/CSIS Bond → dual encrypted ACL + PACS/ASCS
           → Codec Configured → QoS Configured
           → CIG allocated → 2 × CIS Established → CIS teardown verified
           --------------------------------------- Round4 LAST HIL GATE
           → ISO Data Path setup → LC3 SDU TX → First Sound → R6
           --------------------------------------- NOT YET DONE
```

尤其：`ISO Buffer` 只说明控制器有能力；`Set CIG` 只分配 Handle；`LE CIS Established` 也**不等于**已经有音频。R5.2-B 在没有 ISO Data Path/LC3 数据的情况下能使两耳 ASE 自行达到 `Streaming (0x04)`。

## 2. 四轮技术选型和决策演化

| 问题 | 最初思路 | 后续实证修正 / 当前决定 | 原文 |
|---|---|---|---|
| Windows LE Audio 控制权 | 在系统 Host 上 Keepalive Relay 回避随机声像问题 | 独立 BE200 Rust/WinUSB Host，Relay 继续独立项目 | [R1 §1](./Round1.md) |
| Intel 固件 | 外部 Linux SFI 先实验 | 从 `ibtusb.sys` 复原与 003 完全匹配的 Windows OEM Build 176 SFI；只在 Bootloader 守卫加载一次 | [R1 §3](./Round1.md) |
| Bluetooth Host | 审 BTstack Lite，一度倾向自写 BAP Client | 优先复用现成标准实现；最后固定 Bumble-RS `bbac2a6`，对照 AOSP/Zephyr/Linux | [R1 §5](./Round1.md)、[R2 §2](./Round2.md)、[R3 §10](./Round3.md) |
| Controller Attach | 常规 Host power_on / HCI Reset | BE200 已 Operational 时采用 no-reset `initialize_existing_operational` | [R2 §4](./Round2.md) |
| 第一次 Pairing | 直接连同名公有 LE Audio 广播 | Primary 使用 Microsoft Swift Pair pairing-only transport；次成员 RSI 候选 + generic bond/CSIS | [R3 §4–§8](./Round3.md) |
| Initiating | Explicit Peer `LE Extended Create Connection` | 成熟 generic **Filter Accept List** Connection Manager | [R3 §11](./Round3.md) |
| BLE Privacy | Scan 到 RPA 后留着稍后连接 | IRK 解析可见性；Controller Resolving List + FAL **固定 identity** 实时匹配 RPA | [R3 §21–§22](./Round3.md) |
| Bond 持久化 | Pairing 成功即保存 | ProvisionalBond 先经加密 CSIS SIRK/Size/Rank gate，再唯一 `commit.rs` 原子提交；以后只用存储 LTK | [R3 §12](./Round3.md) |
| 物理左/右 | CSIS Rank 直觉代表左右 | PACS Sink Audio Location 是唯一真相源；**rank1=Right, rank2=Left**（实测仅此型号） | [R3 §24](./Round3.md) |
| ASCS Idle 解析 | Bumble-RS 全值严格解析 | **只有 Idle** 解析 ASE ID/State，记录并忽略剩余字节；其它状态仍严格 | [R4 §4](./Round4.md) |
| 写命令成功判据 | ATT Write 返回 | CP Response + ASE Notification + 独立 Readback，缺一不可 | [R4 §4](./Round4.md) |
| QoS 策略 | 以默认值硬编码 | 耳机真实返回偏好与约束，BE200 能力 gate 和双 CIS 可调度性由控制器 HIL 最终判断 | [R4 §5](./Round4.md) |
| CIS 建立 | 同时验证所有阶段 | R5.2-A 先证明 CIG+QoS，R5.2-B 再 Enable+Create CIS+Cleanup | [R4 §6–§7](./Round4.md) |
| CIS Host 前置条件 | 只检查 CIS Central bit28 | **LE CIS Host Support bit32** 先于任何 ACL 设置并读回 | [R4 §7.2](./Round4.md) |
| Teardown | 直接 HCI Disconnect reason 0x16 | **命令 Reason 0x13，事件 Reason 0x16**；确认 CIS 消失后 Release/Remove CIG | [R4 §7.3–§7.5](./Round4.md) |

## 3. 关键 Git 锚点（不是逐提交日志）

| 阶段 | Commit / 基线 | 用途 |
|---|---|---|
| Round1 最后核实 | `04a089e6a3d9476350e856ea322953e3f934a087` | HCI/BLE Passive Scan 基线 |
| Round2 最后核实 | `e883f70315f17a605f266e90e7e5618b5a7ea02d` | Bumble LE ACL |
| Round3 开始 | `6158e9a` | 当时的 R2.4 初版 |
| Round3 结束 | `a80cf1e8fe7d07078ae158e0f18aefcc73e80aed` | R3 双加密 ACL/音频服务发现完成 |
| R4.2 初版 | `ef4167291637d9444a0ab6cb797dd1382e5c5460` | 双耳 ASCS Config Codec 主体 |
| R5.1 | `15c68dcd4b5364fce81366d8c07ce72f50ba1579` | QoS Planner |
| R5.2-A | `7ef216eecb937f5e60e5c645c3d40b3928ef40cc` | CIG+ASCS QoS |
| R5.2-B 初版 | `c31c7f76588870a61c0ca76a10d76170817d53e8` | 双 CIS 实验入口 |
| 0x0C 解决 | `61851a01141fc586b0b27b9abc1425cb0d337d1b` | CIS Host bit32 |
| 0x12 / CIG 清理修复 | `5848597`、`2ef6ccf` | CIS Disconnect Reason 及 Pre-ACL CIG Recovery |
| **最终 HIL 基线** | **`eac486691fd4d7181d36bb09a620e942d0026936`** | R5.2-B 第三轮全生命周期成功 |

Round4 文档核对过的 Git 比较：R3 基线 `a80cf1e8` → Round4 最终 HIL 提交 `eac4866`，Ahead 68 commits / 22 files。**不要**将历史 HIL 基线误写成当前归档分支新 HEAD。

## 4. 核心实现路径索引（相对于仓库根）

| 路径 | 功能 |
|---|---|
| `src/controller_bootstrap.rs` | Bootloader / Operational Guarded Bootstrap |
| `src/audio_set.rs` | 双加密 ACL、上层工作流、Bond 指纹和 Cleanup |
| `src/main.rs` | 独立 CLI consent gate、参数解析 |
| `crates/bumble-adapter/src/init.rs`, `usb_fixed.rs` | 无 Reset 初始化、WinUSB Event/ACL 原生分帧与 ISO 尚待 HIL 的 Bulk-Out |
| `crates/bumble-adapter/src/connection_manager.rs`, `privacy.rs` | FAL、Resolving List |
| `crates/bumble-adapter/src/security.rs`, `stored_security.rs`, `commit.rs` | SMP 临时密钥、既有 LTK 复用、唯一持久提交 |
| `crates/bumble-adapter/src/gatt_session.rs` | 多 ACL 的 GATT/异步 CP-ASE 事件桥 |
| `crates/bap-client/src/codec_plan.rs`, `codec_config.rs`, `ase_read.rs` | LC3 Planner、Codec/ASCS 事务、Idle 特例 |
| `crates/bap-client/src/qos_plan.rs`, `cis_session.rs` | 双耳 QoS 计划、Enable/CIS 生命周期 |
| `crates/bumble-adapter/src/iso_capabilities.rs` | CIS Central / ISO Buffer 能力检查 |
| `crates/bumble-adapter/src/cig_control.rs`, `cis_host_feature.rs`, `cis_control.rs` | Set/Remove CIG、Host bit32、Create/Disconnect CIS |
| `crates/iso-scheduler/src/lib.rs` | **仅 frame quantum 骨架**，不是完整 ISO 时序系统 |

## 5. 证据归档位置与强弱

- Round1: 003 原厂 ColdBoot vs 004 Rust ColdSend USBPcap 与签名固件重建：见 [Round1 §3](./Round1.md)；仓库原有 [EXPERIMENT_004_R1_5B.md](../EXPERIMENT_004_R1_5B.md) 和 [EXPERIMENT_005_R1_6.md](../EXPERIMENT_005_R1_6.md)。
- Round2: R2.1/2.2/2.3 用户 Windows 实机日志，原始输出摘录见 [Round2 §4–§7](./Round2.md)。
- Round3: Stored Bond / Pairing / R3 完整审计见 [Round3 §15–§27](./Round3.md)；仓库已有 [R2_PAIRING_V2_HIL.md](../R2_PAIRING_V2_HIL.md)、[R2_ABOVE_ACL_STATIC_AUDIT.md](../R2_ABOVE_ACL_STATIC_AUDIT.md)。
- Round4: 七份原始 HIL 日志索引见 [Round4 §11](./Round4.md)；详细阶段门槛见仓库原有 [r4-ascs-config-codec.md](../r4-ascs-config-codec.md)、[r5-qos-plan.md](../r5-qos-plan.md)、[r5-config-qos.md](../r5-config-qos.md)、[r5-dual-cis.md](../r5-dual-cis.md)。**这次归档只收入四份 MD，不把敏感 Bond/固件/PCAP 拷入 Git。**
- 可复现事实强度：USBPcap/HCI 实际返回、用户提供的 CLI/Cargo 输出 > 当前源码/Commit > 文档推论。早期 Round 的“计划”不等于后期已经通过 HIL。

## 6. R5.3 接续点 / 尚未闭环

最短路径仍是：`R5.3-A ISO Data Path Setup/Remove HIL → R5.3-B 合成 PCM→LC3(100B/10ms)→2×ISO SDU TX → 可听左右耳 → R5.3-C 时序/抖动/稳定性 → R6`。

**待做/不能冒充成功**：ISO Data Path 和 LE HCI ISO Bulk-Out 发送均未硬件验证；LC3 Windows Encoder 未实际接到耳机；实测 Transport Latency `63,210 µs` 与 Presentation Delay `40,000 µs` 的时基关系仍需专门分析；最终 HEAD 的完整 `cargo test` 尚缺留档；长稳、异常单耳恢复、B3P 以外泛化均未验收。不得自动清除 Bond 或因清理失败直接热复位硬件。

**推荐下一轮阅读顺序**：`README.md → TIMELINE.md → BUG_FIX_REGISTER.md → Round4.md → Round3.md`，遇 R1/R2 的旧假设再回原文。