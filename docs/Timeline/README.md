# BE200AudioHub — Engineering Timeline Archive

本目录是 BE200AudioHub 从第一次 Windows BE200 Bring-up 到 **双耳 LE CIS 全生命周期 HIL PASS** 的历史资料归档。保存原始过程、失败、被推翻的假设和问题修复，而不是仅保存最后的成功结果。

**时间范围**：2026-10-11—2026-10-14（四个 Round）；**归档日期**：2026-10-14。当前有效开发分支：`r2/stack-integration-bootstrap`。由于仓库长期以 **`docs/` 小写**存放文档，本归档也使用 `docs/Timeline/`，避免 Windows 上同时创建大小写不同的 `Docs`/`docs`。

## 目录

| 文件 | 作用 |
|---|---|
| **[TIMELINE.md](./TIMELINE.md)** | **跨四轮总索引**：里程碑、状态边界、重大技术决策的前后演变、关键 commit、各类证据导航 |
| **[BUG_FIX_REGISTER.md](./BUG_FIX_REGISTER.md)** | **独立故障与修复记录**：48 个按轮编号的故障/纠偏项、17 个未解决或需小心处理的 Quirk/风险；包含现象、根因、错误修复过程、最终证据 |
| [AllRounds.md](./AllRounds.md) | **原文全量合订版**：四份 Round 文档依序拼接，有章节跳转和来源身份 |
| [Round1.md](./Round1.md) | **Round1 原文**：驱动/WinUSB、Intel OEM Signed SFI 冷启、HCI、BLE Passive Scan、初期选型反思 |
| [Round2.md](./Round2.md) | **Round2 原文**：Bumble-RS Host no-reset、扫描、首次真实 LE ACL、后续 R2.4 Pairing 的设计（当时未完成） |
| [Round3.md](./Round3.md) | **Round3 原文**：真正完成 R2.4 两成员 SMP/Bond/CSIS、Stored LTK 验证、R3 双加密 ACL/PACS/ASCS/CAS、USB/SMP/Privacy Quirk |
| [Round4.md](./Round4.md) | **Round4 原文**：Codec→QoS→CIG→Enable→双 CIS Established→规范 Disconnect/Release/Remove CIG，及三次 R5.2-B HIL |

## 推荐阅读顺序

需要快速接手 R5.3：**README → TIMELINE → BUG_FIX_REGISTER → Round4 → Round3**。调查固件/硬件再看 Round1；调查 BLE Host/SMP 初期决策再看 Round2。

需要完整追溯：直接阅读 **AllRounds.md**，或四份 Round 原文。它们是**历史快照**：Round1 文档的“尚无 ACL”和 Round2 文档的“尚无 SMP”是当时真实状态，但都已在后续 Round 得到新的 HIL 证明；**不得**截取早期状态冒充项目当前状态。遇新旧结论矛盾，以更晚的独立 HIL/源码证据更新当前视图，同时保留原文。

## 归档校验（四份原文未经编辑）

四个独立 Round 文件内容与用户上传的原始 Markdown **Git Blob SHA 完全一致**：

| 文件 | 上传 SHA256 | Git Blob SHA-1 |
|---|---|---|
| Round1.md | `26e07c4a68e63019ee72b126359ca682ba86d93e056c56db935a5959a794bff7` | `cc23b3a95f0edaa596192ebbd819134e401fa9ac` |
| Round2.md | `44c3198688a071a6427bea9988387775687da1efb34a3cbde5449d319e960be5` | `30debb39e488960e7d9c67f4770d5e20011b99ac` |
| Round3.md | `b8e3d184b79d900ccffb851217dab2c7aa6e95ca33f964a776f4492e8b77180c` | `e5c1b322e2d377422e9ea71c905031f8168bb8ac` |
| Round4.md | `89676bcc3eb86400fa91360fa9fb88a8edf7aa64132f79e9f8e6e471bb07c06d` | `352b1f6160af7623dc6f26c1c16595d7b6c93719` |

`AllRounds.md` 将四份原文置于明确的 BEGIN/END 源文件标记内，额外加了合订版目录、版本和证明信息；因此它自身不等于任一原始文件的字节副本。以后校对以 **Round1.md–Round4.md** 独立文件为准。

**不包含** Intel 专有 SFI、私有 pairing-keys/IRK/LTK/SIRK、Windows USBPcap 二进制或原始日志完整副本。这些在工程私有的单独证据包中；Round4 `11 给出了七份 HIL 日志的对应索引和 HIL 证据含义。

## 里程碑冻结状态

| 门槛 | 是否真实硬件验证 |
|---|---|
| R1 Firmware/Controller HCI/Scan | **HIL PASS** |
| R2 双成员 Bond / Stored LTK / R3 双 ACL、PACS/ASCS | **HIL PASS（在 Round3 完成）** |
| R4.1 Codec Planner、R4.2 Config Codec | **HIL PASS** |
| R5.1 QoS Planner、R5.2-A Set CIG + Config QoS | **HIL PASS** |
| **R5.2-B Enable + 双 LE CIS + 规范 Teardown** | **HIL PASS** |
| **R5.3 ISO Data Path + LC3 双耳实际发声** | **未验证 / 未完成** |
| R6 日用长稳、自动重连/恢复 | 未验收 |

**最后完成真实 R5.2-B HIL 的源码基线**：`eac486691fd4d7181d36bb09a620e942d0026936`。后续本目录的文档提交只是归档，不是音频开发新版本。不要重新做 Pairing，不要清 Bond；所有 HIL 继续由用户在 Windows 本机显式执行，助手可提交代码/执行 allowlisted Build，但不能将 Build 当成 HIL。

**下一步**：R5.3-A 首先验证 Setup/Remove ISO Data Path、HCI ISO Bulk-Out framing/flow control；然后 R5.3-B 让 LC3 48kHz、10ms、100 octets/mono SDU 通过两条已经 HIL 的 CIS 发声。详见 [TIMELINE `6](./TIMELINE.md) 和 [Round4 `13](./Round4.md)。

## 新 Round 维护规则

1. **原文单独存档**：新阶段以 Round5.md、Round6.md 追加，不覆盖此前 Round 原文。
2. **状态由 HIL 更新**：更新 TIMELINE、BUG_FIX_REGISTER、README 时写明测试证据和 commit；旧文档中的历史计划不要悄悄改写。
3. **Bug/Fix 分离**：明确每次失败的原始现象、错误码、控制层级、根因证据、做过的错误尝试、最终修复、修复后的测试；不能把“清理 UNCONFIRMED”包装为 PASS。
4. **能力层级严格区分**：Intel HCI 支持 ISO ≠ Set CIG ≠ CIS Established ≠ 可用 ISO Data Path ≠ 实际可听 First Sound。
5. **防止多项目混合**：BE200AudioHub 是自己的 Windows Rust BLE/LE Audio Host；独立 `LE-Audio-Relay` 是基于 Windows 系统音频渲染的旧项目，不能以其数据面替代本项目的实验结果。
