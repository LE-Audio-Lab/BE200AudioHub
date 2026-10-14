# R5.3 A/B/C — 本地实现候选（尚未编译 / 尚未 HIL）

## 状态与边界

本补丁基于用户上传的 `BE200AudioHub.zip`，其 Git HEAD 为
`d047269e9c9c945301a66afeeb1f7a08631306e7`。最后一份既有真实硬件证据仍是
R5.2-B 双 CIS 全生命周期 PASS；本补丁没有改变这个历史验收边界。

本次只在 `/mnt/data` 的快照上修改，没有 GitHub 推送、用户电脑文件写入或 BE200 操作。
沙箱无 `rustc` / `cargo`，并且不能解析外部下载域名，因此 **Rust 编译、Rust 单测、
Windows 链接及真实硬件测试均未运行**。新增测试是可执行的 Rust 测试源码，不是已经
通过的测试结果。已执行的静态检查不替代编译或 HIL。

用户快照的 `Cargo.lock` 字节完整保留，补丁不覆盖它。首次 Cargo 构建会为新增的
workspace 依赖边更新 lock 中的本地包依赖列表。第三方 Bumble-RS 仍固定为
`bbac2a6803b8cab0920ab725a23aa408fc4fed85`，没有引入新版本或新种类的第三方库。

## 一次运行完成 A/B/C

```powershell
cargo test --workspace
if ($LASTEXITCODE -ne 0) { throw "Tests/build failed; do not run HIL" }
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "Build failed; do not run HIL" }

.\target\release\be200hub.exe r5 first-sound `
  --seconds 8 `
  --name "Galaxy Buds3 Pro (2F23)" `
  --play-seconds 8 `
  --confirm-first-sound `
  2>&1 | Tee-Object ".\r5-first-sound-1.log"
```

`--seconds` 是既有发现扫描窗口；`--play-seconds` 是音频时长，允许 1–30 秒。
先调低耳机音量。本次 PCM 峰值约 -24 dBFS，但它不是声压校准，不能保证具体听感响度。
不要删除设备或清 Bond。测试需要两耳在线，继续使用既有 WinUSB / Operational bootstrap。

完整路径：

```text
严格 CLI 解析 / 独立确认参数
  -> LC3 编解码离线自检（PCM 形状、能量、左右频率）
  -> 预编码整段双耳测试音
  -> 既有 controller bootstrap / no-reset attach
  -> Host Feature bit32 + 残留 CIG 核对（ACL 之前）
  -> stored bond / 双 ACL 加密 / PACS-ASCS / Codec-QoS / Enable / 双 CIS
  -> A: 每个 CIS Setup ISO Input Data Path，并核对 Command Complete + Device 状态
  -> 双 Sink ASE Streaming readback
  -> C: 按 10 ms 绝对时限发送预编码 LC3，逐耳序号和 HCI credits 审计
  -> 等待真实 HCI 完成 / 可选 Read ISO TX Sync
  -> 再检查双耳 ASE 状态
  -> Drain -> Remove ISO Paths -> Disconnect CIS
  -> Disable / Release ASE -> Idle -> Remove CIG -> 断 ACL / 清 privacy
  -> 确认 persistent-state fingerprint 不变
```

编解码 B 提前执行是为了在编码失败时不碰硬件，也避免在实时发送路径等待编码线程。
A、B、C 不要求分成三次硬件测试；独立命令仅供定位失败层级。

## 独立诊断入口

```powershell
# 完全离线，不访问 USB/Bond。
.\target\release\be200hub.exe r5 lc3-self-test

# 只做 A 和既有双 CIS 生命周期，不发 LC3/ISO 音频数据。
.\target\release\be200hub.exe r5 iso-path-probe `
  --seconds 8 --name "Galaxy Buds3 Pro (2F23)" --confirm-iso-path
```

既有 `r5 establish-cis --confirm-cis-create` 保持 R5.2-B 控制面行为，不安装 ISO Path，
不发送音频。确认参数不能互换。

## 实现导航

| 文件 | 本次职责 |
|---|---|
| `crates/audio-bridge/src/first_sound.rs` | 固定 48 kHz / 10 ms / 100 B mono LC3；左右独立持续编码状态；440/880 Hz；50 ms 淡入淡出；离线解码频率/能量自检 |
| `crates/iso-scheduler/src/first_sound.rs` | 绝对时刻节拍；双耳成对 credit 准入；超过 30 ms 的迟到终止测试而非无限追赶 |
| `crates/bumble-adapter/src/iso_io.rs` | ISO Setup/Remove 确认、发送、drain、Tx Sync；限定双 CIS 和每帧 100 B |
| `crates/bumble-adapter/src/iso_audit.rs` | 独立 HCI 报文计数；逐 handle 完成事件、序号、credit 上限和错误锁存；不记录音频内容或密钥 |
| `crates/bumble-adapter/src/probe_guard.rs` | Windows 协作式 Ctrl+C/Ctrl+Break；配对申请/释放 1 ms timer；不保证硬实时 |
| `crates/bumble-adapter/src/init.rs` | 在现有 packet source/sink 外增加默认关闭的审计封装，不替换 USB framing |
| `crates/bap-client/src/cis_session.rs` | 单一共享 CIS 事务；R5.3 opt-in 阶段；ISO 在 CIS teardown 前清理；失败仍沿既有 rollback |
| `src/audio_set.rs`、`src/main.rs` | 前置 CLI/离线检查；R5.3 调度、日志、控制面/数据面/人工听音的明确区分 |

## 已在源码核对的易错点

1. **Setup 与 Remove 的方向编码不同。** Setup `direction=0` 表示 Host→Controller；
   Remove 使用 `0x01` 的位掩码。HCI Path ID 为 0，Codec ID 是 **TRANSPARENT**，不是
   LC3 DSP offload。编码发生在 Host。
2. **不另外手写 ISO packetizer 或另建 credit 池。** 继续用 pinned
   `Device::send_iso_sdu` 和 `DataPacketQueue`；本地层限制为一 SDU 一 HCI packet：
   PB=2、TS=0、100 B LC3 + 4 B SDU 信息、108 B native USB 数据；USB 层剥离 H4 类型字节。
3. **独立审计不替代 upstream credits。** 上游 `Device` 忽略
   `on_packets_completed` 的部分错误返回，因此新增独立计数器拒绝超量 completion、
   重复序号、超窗发送和部分写失败。审计在 USB 写前预留计数，避免 reader 先观察到
   completion 的线程竞态；任何 write 失败都会使本次审计失败，不能当成传输成功。
4. **断链后的空队列不是发送完成。** Device 在断链时会隐式 flush；正常结果必须在
   CIS 仍活着时观察每耳 `submitted == completed == requested_frames`。
5. **不向 Sink ASE 发送 Receiver Start Ready。** 依旧要求每耳 Streaming 状态和 CIG/CIS
   映射匹配；不以 Streaming 状态本身证明有音频。
6. **Setup 超时是状态不确定。** 跟踪可能存在的 Path；cleanup 尝试等待迟到的 Setup
   完成并显式 Remove。无法证实时输出 `UNCONFIRMED`，继续尝试 CIS/ASE/CIG 清理，绝不
   用重配对或偷偷清 Bond 掩盖失败。

## 输出判读

`R5.3-B/LC3 ... PASS`：仅说明本机实际编码/解码自检通过。

`R5.3-A/SETUP ... PASS`：仅说明特定 CIS 的 HCI Setup completion 及 Host 路径记录一致。

`R5.3-C/TX`：第一帧和每 100 帧打印 submitted/completed/pending/max_late。

`R5.3/RESULT transport probe PASS`：每耳 HCI 提交和 Controller 完成计数满足门槛。
**不表示 RF 无丢失、耳机解码成功、左右声道正确，或用户已经听到声音。**

`FIRST SOUND LISTENING RESULT: USER VERIFICATION REQUIRED`：需要人工反馈是否两耳发声、
是否左低右高、是否断续/失真；它不能由日志自行变成 HIL PASS。

`R5.3 CLEANUP PASS`：正常退出的瞬时状态已按路径清理且 Bond fingerprint 不变。
失败中出现 `UNCONFIRMED` 时，不能将前面的局部 PASS 当成完整验收。

## 测试与现存边界

本次新增 13 个 Rust 测试：PCM/LC3 2 个、节拍/双耳准入 2 个、独立审计 4 个、
ISO 方向编码与 scripted HCI 路径 3 个、CLI 2 个。它们尚未运行。
scripted HCI 测试不是硬件测试；语法词法和 TOML 检查也不是 Rust 编译。

目前是有界 First Sound 技术验证，不是 R6 日用：没有 Windows PCM capture 接入、
没有长时间音频时钟恢复、没有自动重连、没有麦克风或下行以外的 ISO 数据接收。
保留既有 QoS，不在此补丁顺带调低 Transport Latency。Windows 错过发送时限会失败并
清理，具体硬件 USB/credits/声像行为等待第一次 HIL。

Windows Ctrl+C/Ctrl+Break 会设置取消标志，在安全检查点退出并清理；编码阶段和正在
进行的同步 HCI/GATT 调用可能先完成。结束进程、关窗口、拔 USB、驱动失败不能保证显式
Remove 成功，日志仍可能是 `UNCONFIRMED`。不要依赖强杀代替生命周期清理。

## 本地源码证据定位（不通过 GitHub 重新抓取）

资源路径 `/mnt/data/BE200_Indexed/resources/bumble-rs-main/`：

- `bumble-host/src/lib.rs`：`IsoDataPathParameters::hci`、`setup_iso_data_path_with_parameters`、
  `remove_iso_data_path`、`send_iso_sdu`、`iso_packets_pending`、NumberOfCompletedPackets 处理。
- `bumble-host/src/data_queue.rs`：`pending` 包括等待与 in-flight；completion overflow；disconnect flush。
- `bumble-transport/src/host.rs`：Device command journal、独立 reader、transport failure 行为。
- `bumble-codecs/src/lc3.rs`：纯 Rust 有状态 mono encoder/decoder 的 owned worker API。
- `bumble-hci/src/command.rs`：Setup/Remove wire 编码和 `CodingFormat::TRANSPARENT`。

项目历史依据：`docs/Timeline/Round4.md`，尤其双耳 `state=0x04` 与实际音频未开始的边界。
本地资源是上传的源码归档；本次没有额外在线核验该归档与 Git commit 的逐文件一致性，
真正构建仍由项目 manifest 中的固定 revision 决定。
