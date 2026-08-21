[English](README.en.md) | **简体中文**

# glassgauge

> A liquid-glass usage widget for the [mirasim](https://mirasim.ai) relay on Windows,
> with one-click subscription-account switching. Tauri v2 + native DirectX.

Windows 桌面上的 **mirasim 用量挂件**，iOS 26 风格液态玻璃：玻璃透出的是窗口
后面**实时的真实内容**（不是壁纸贴图），带边缘位移折射。Tauri v2 + 原生
DirectX 渲染管线，常驻内存 ~40 MB。附带一个零依赖的账号切换命令行工具（见 `cli/`）。

![glass](docs/design/glass-sample.png)

## 仓库结构

- `ui/`、`src-tauri/` — glassgauge 挂件本体（前端 + Tauri/Rust 原生引擎）
- `cli/` — 同款账号切换的命令行版（Node，零依赖，与挂件共用磁盘布局，可混用）
- `docs/design/` — 挂件与折射引擎的设计文档

## 它做什么

- 自动发现本机 mirasim relay（端口动态分配，扫描 + 响应形状认领），轮询
  `GET /v1/limits`；
- 常驻展开面板：5 小时 / 7 天 / 30 天三窗口卡，各含用量百分比、**credits 原值 · 估算 API 费用 并排**
  （v0.10.0，如 `441 / 39.2k · ≈ $4.4 / $392`；费用按 `creditsPerUsd` 折算，默认 100 credits = $1，
  费率为估算非官方）、匀速线刻度、超前/落后、重置倒计时；
- **订阅账号切换**（v0.2.0）：面板里的「账号」行点开即列出已存快照，点击
  一键切换 mirasim 登录账号，免重复收验证码——额度用尽换小号的场景一步到位。
  账号以**邮箱**标识（v0.4.0，从令牌 JWT 解出，见下）；
- **透明度调节**（v0.3.0）：头部 ⚙ 弹出滑杆，白纱（veil α，0 = 纯玻璃全透）
  与磨砂（blur σ）拖动即时生效并写回 config.json——refract 模式经引擎
  `SetCfg` 热更效果链，wallpaper 模式重建 CSS 滤镜，live 模式模糊由 DWM
  固定、只提供白纱；
  白纱可一路拖到 **100% = 纯白实底面板**（v0.7.0，白纱 ≥ 50% 自动转深色字/淡黑分层，
  深色壁纸下也读得清）；
- **开机自愈**（v0.7.0）：挂件常随开机自启、可能比 mirasim 先起，此时显示
  「等待 Mirasim 启动…」并快重试（≤5s），relay 一就绪即秒级恢复；取数循环包了
  try/finally + 看门狗，任何异常都不会让它卡死在"不可用"。
- 自由拖动、双屏感知（含不同 DPI 缩放）、位置记忆、断线降级显示最后数据。

## 账号切换

登录态即 `~/.mirasim/setting.json` 的 `auth` 块（令牌是绑定本机 `secret.key`
的 `mrs1:` 密文，快照仅本机有效）。glassgauge 与仓库内的 `cli/`
命令行工具共用同一套磁盘布局
（`~/.mirasim/_account_switcher/{profiles,backups}`），两边可混用。

- 首次收集：账号 A 登录状态下点「＋ 保存当前登录为快照」；在 mirasim 里退出
  换账号 B 登录，再存一次。之后点谁切谁。
- 每次切换前自动把当前账号最新登录态回存快照（refreshToken 保持最新），并
  备份整份 setting.json（保留 20 份，出问题可用 CLI `restore` 回滚）。
- 切换只改 `auth` 字段；mirasim 服务端会在几秒内热重载。正在跑的会话可能
  短暂异常；若恰逢应用刷新令牌回写文件，本次切换可能被覆盖，重点一次即可。
- relay 断开时面板仍会展开，账号切换照常可用（额度卡显示空态）——
  这正是登录失效需要切号自救的场景。
- 快照列表与删除（✕ 两步确认）都在面板内完成；令牌永不进入 WebView，
  invoke 只传元数据。
- 账号名显示为**邮箱**：`auth.token`（`mrs1:` 密文）本机解开后是一枚 JWT，
  `email` claim 即账号邮箱。解密链路 = DPAPI 还原 `secret.key`（`CryptUnprotectData`，
  当前用户可解）→ AES-256-GCM 解令牌 → 读 JWT。全程本机、不联网，密文不出进程；
  解不出（异机快照/密钥不可读）时回退显示账号名。新快照默认以邮箱本地部分命名。
- **套餐徽章与到期跟着账号走**（v0.5.0）：头部的套餐徽章和「套餐到期」取自同一枚 JWT
  的 `plan` / `plan_exp` claim，切换账号即刷新；解不出时才回退到 config 里的
  `planLabel` / `validUntil`（所以这两项现在只是兜底默认值）。
- **用量按账号归属，不串号**（v0.6.0）：`/v1/limits` 响应带 `subject`（= 账号 userId）。
  切号后 relay 要过几秒到几十秒才把 limits 换到新账号，这段时间 `subject` 仍是旧账号——
  挂件据此判定：`subject` 不等于当前登录 userId 时不显示这批用量（否则会看到上一个账号
  的数字），改显示「正在同步新账号用量…」，并从每 60s 提到每 3s 快轮询（最多 ~150s）
  直到 relay 追上。若长时间不更新，重启 Mirasim 可强制刷新。

## 液态玻璃引擎

三种玻璃模式（`mode` 配置）：

| 模式 | 玻璃内容 | 说明 |
| --- | --- | --- |
| `refract`（默认） | 窗口后面的**实时画面** | DXGI 桌面复制抓帧（事件驱动+脏区过滤，静止零功耗）→ Direct2D 高斯模糊 → 位移贴图折射 → 饱和度 → 20px 圆角 AA → DirectComposition 垫在 WebView 内容后 |
| `wallpaper` | 壁纸按窗口物理位置裁剪折射 | refract 不可用时的自动兜底（锁屏/UAC/独占全屏/远程桌面），也可手动指定 |
| `live` | DWM 亚克力实时模糊 | 系统材质，圆角固定 ~8px |

**截图注意**：refract 模式下挂件必须从屏幕捕获中剔除（否则玻璃拍到自己形成
回环），因此**截图/录屏里看不到它**。要截它：托盘勾选"截图模式（玻璃暂用
壁纸）"，截完取消即回实时玻璃。

## 构建

```powershell
# 依赖：Rust (MSVC)、tauri-cli 2.x、WebView2 运行时（Win10 2004+ 自带）
cd src-tauri && cargo build          # 调试
tauri build                          # release + NSIS 安装包
cargo test                           # Rust 单测（几何/位移图/发现协议）
node --test ui/tests/*.test.js       # JS 单测（派生计算/裁剪映射/位移场）
```

## 配置

`%APPDATA%\glassgauge\config.json`（首次运行自动生成），托盘"立即刷新"热载：

```jsonc
{
  "mode": "refract",          // refract | live | wallpaper
  "expand": "always",         // always 常驻展开 | hover 悬停展开
  "autostart": true,          // 开机自启（HKCU Run 键，随 exe 位置自动更新）
  "accent": "auto",           // 主色：auto 壁纸取色（绕开绿）| blue | amber | ink | "#hex"
  "ink": "#000000",           // 可选：钉死字色（省略 = 随壁纸明暗自动黑/白字）
  "planLabel": "MAX",         // 徽章文字（仅当无法从账号令牌解出套餐时的兜底）
  "validUntil": "2027-08-11", // 套餐到期兜底（正常取自账号令牌的 plan_exp）
  "refreshSeconds": 60,
  "creditsPerUsd": 100,       // 额度→估算 API 费用的换算：多少 credits 折 $1（估算，非官方）
  "alwaysOnTop": true,
  "glass": {
    "alpha": 0.03,            // 白纱浓度（0 = 纯玻璃，1 = 纯白实底；滑杆量程 0–1）
    "blur": 4,                // 磨砂程度（0 = 全透，14 = 重磨砂）
    "displacement": 24,       // 边缘折射弯曲强度
    "band": 16,               // 折射边带宽
    "radiusCollapsed": 20,    // 玻璃圆角
    "saturate": 1.12
  }
}
```

## 设计文档

- [挂件整体设计](docs/design/2026-08-18-mirasim-usage-glass-widget-design.md)
- [原生实时折射引擎设计](docs/design/2026-08-18-glassgauge-native-refraction-engine-design.md)
- [引擎实施计划](docs/design/2026-08-18-glassgauge-refraction-engine-plan.md)

调试构建带验证入口：环境变量 `GG_SPIKE=b|a|cap|pipe`（层序/捕获剔除/抓取
通道/整条管线自检），`GG_DUMP_ONCE=1`（启动 2.5s 后自动导出玻璃帧 PNG），
托盘"导出玻璃帧"。

## 许可

MIT，见 [LICENSE](LICENSE)。

## 已知边界

- 鼠标指针不会映在玻璃里（捕获不含指针，iOS 同）；
- DRM 保护的视频区域在玻璃里是黑的；
- 需要 Windows 10 2004+（捕获剔除 API），更早系统自动落到壁纸模式。
