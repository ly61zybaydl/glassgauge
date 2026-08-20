# mirasim 订阅账号切换器

本地小工具：把 Mirasim 的登录态存成快照，随时一条命令切换账号，不用每次退出重登、收验证码。

零依赖，只需本机装有 Node.js（实测 v24，v18+ 均可）。

## 原理

Mirasim 的订阅登录态保存在 `~/.mirasim/setting.json` 的 `auth` 字段里
（`token` / `refreshToken` / `userId` / `name`，其中令牌是 `mrs1:` 前缀的密文，
由同目录的 `secret.key` 在本机加解密）。

本工具做的事就是把整个 `auth` 块存成快照、需要时写回去：

- 快照存在 `~/.mirasim/_account_switcher/profiles/<名字>.json`
- 每次改写 `setting.json` 前，先备份到 `~/.mirasim/_account_switcher/backups/`（保留最近 20 份）
- 切换前会自动把**当前**账号的最新登录态回存到它的快照（保证 refreshToken 始终是最新的）
- 写入用「临时文件 + 原子改名」，不会留下写了一半的 setting.json
- 只动 `auth` 字段，不碰 `secret.key`、设备密钥、mirachannel、agent 账号等其它配置

## 用法

```powershell
cd glassgauge\cli

# 交互模式：列出所有快照，输入序号即切换
.\mirasim-accounts.cmd

# 常用子命令
.\mirasim-accounts.cmd whoami        # 我现在登录的是谁
.\mirasim-accounts.cmd save 主号     # 把当前登录保存为快照「主号」
.\mirasim-accounts.cmd list          # 列出快照（● 标记当前账号）
.\mirasim-accounts.cmd use 小号      # 切换到「小号」（也可以用序号：use 2）
.\mirasim-accounts.cmd rm 小号       # 删除快照
.\mirasim-accounts.cmd restore       # 查看 / 恢复 setting.json 备份
```

想在任何目录直接敲 `mirasim-accounts`，把本目录加进 PATH，或在 PowerShell 配置里加：

```powershell
Set-Alias mirasim-accounts glassgauge\cli\mirasim-accounts.cmd
```

## 首次收集账号

1. 当前已登录账号 A → `save A号`
2. 在 Mirasim 里退出登录，改用账号 B 登录 → `save B号`
3. 之后 `use A号` / `use B号` 随意来回，不再需要邮箱验证码。

## 注意事项

- **建议在 Mirasim 退出后切换。** 服务端会定期从 setting.json 热重载配置，运行中切换通常也能生效，
  但正在进行的会话可能短暂异常；且如果恰好碰上应用刷新令牌并回写文件，本次切换可能被覆盖。
  工具检测到 Mirasim 在运行时会先询问（`--force` 跳过）。切完建议重启 Mirasim。
- **快照只在本机有效。** 令牌密文绑定本机 `secret.key`，拷到别的电脑没用，新电脑请重新登录再 save。
- **快照可能过期。** 服务端如果轮换了 refreshToken，长期没用过的快照可能失效——现象是切过去后
  Mirasim 要求重新登录。重新登录一次然后 `save` 同名快照即可修复。
- 快照文件里的令牌虽是密文，但配合本机 `secret.key` 即可还原，`_account_switcher` 目录请当机密对待，
  不要提交进任何仓库或同步盘。
- 数据目录不在默认位置时，用 `--home D:\某目录` 或设置环境变量 `MIRASIM_HOME`。

## 卸载

删除本工具目录，以及 `~/.mirasim/_account_switcher/`（快照和备份都在里面）即可，
不影响 Mirasim 本身。
