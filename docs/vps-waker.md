# VPS 定时唤醒

ClaudeTile 将 [claude-waker](https://github.com/weidwonder/claude-waker) 的“定时发一条短消息”方式集成到磁贴。沿用磁贴已有的官方 Claude Code CLI 调用，不额外安装 Python Agent SDK。CLI 运行在 VPS；Windows 关机不影响定时任务。

## 填写 Token

已完成 VPS 部署和 SSH 配对的电脑：

1. 在已有 Claude Code 的终端执行 `claude setup-token`，授权需要唤醒的订阅账号。
2. 右键磁贴 → **VPS 定时唤醒 / 填写 Token…**。
3. 在 SSH 窗口输入 `1`，粘贴完整 OAuth Token，按回车。隐藏输入不会显示字符。有多个订阅账号时，每个账号各执行一次 `setup-token`，再各用 `1` 添加。
4. 输入 `0` 关闭。保存 Token 不会立即发送消息，等待下一个执行时间。
5. 菜单中“立即刷新”或等待最多一分钟，磁贴“唤醒”行显示“已启用 · 下次 …”，下面每个 Token 一行，显示末 4 位和它最后一次执行结果。时间按右键菜单“时区”显示。

再次输入已保存且已启用的同一个 Token 时，窗口提示 “already saved and enabled; nothing changed”，不重复添加。同一窗口的 `2` 暂停、`3` 恢复全部 Token，`4` 按编号删除一个 Token。Token 只保存到 VPS 的 `/var/lib/claudetile-waker/config.json` 的 `tokens` 列表（权限 0600），不会下载到磁贴、写入源码或日志；只有单个 `token` 字段的配置照常使用，下次保存时改写为列表。该文件的 `enabled` 决定是否执行，`expected_egress_ip` 固定允许的公网出口。

## 时间与额度的边界

- cron 使用 `CRON_TZ=Asia/Shanghai`，每天北京时间 08:00、13:01、18:02、23:03 各执行一次，依次处理每个 Token；每次只在开始后 5 分钟内（如 08:00–08:04、23:03–23:07）每分钟重试失败的 Token，成功或已有窗口的 Token 本次不再请求。错过时间不补跑。
- 5 小时窗口从首条消息所在的整点开始，所以每次在上个窗口重置后执行，目标是固定在 13:00、18:00、23:00、04:00 重置，长时间不用 Claude 也不变；为确保上个窗口已经重置，每次比上一次多顺延 1 分钟（13:01、18:02、23:03）。04:00 重置后不再执行，到 08:00 间隔很长，次日仍从 08:00 开始。服务器无法强制修改 Anthropic 的重置时间，也不调整周额度。
- 能查询额度且已有窗口时，跳过唤醒，显示服务端实际重置时间。`setup-token` 可能只有推理权限；额度接口返回 403 时仍可尝试发送短消息，但明确显示“重置时间未确认”。
- 夜间手动使用同账号、其他机器自动打卡、周额度用尽，都可能使目标无法达到。连接 VPS 后，本机“空闲时自动打卡”被禁用，避免两个调度器竞争；其他电脑也应关闭该选项。
- 磁贴分别展示本机账号和 VPS Token 账号。VPS 每 5 分钟通过固定 ISP 查询 OAuth profile 和 usage，只返回邮箱、套餐、额度与重置时间；每个 Token 只有取得有效额度与重置时间才显示一张 VPS 账号卡片（标注 Token 末 4 位），查询失败或没有权限时隐藏；与本机账号或另一张卡同邮箱的不重复显示。不会从 Token 字符串推断身份，也不会借用本机邮箱。切换本机账号不会替换 VPS Token。
- 多 Token、账号详情、Token 末 4 位和 cron 检查需要部署新版 worker；旧版服务器仍可显示任务状态，但只显示一个 Token（最早的版本显示为“已保存”），也无法查询账号详情。
- “唤醒请求成功”需要官方 CLI 的成功 Result；超时、错误 Result、网络失败不算成功。

## 网络与权限

```text
VPS 上的 cron → Python 调度器 → 官方 Claude Code CLI
                                     ↓ HTTPS_PROXY
                             127.0.0.1:17898（独立 Xray）
                                     ↓ SOCKS5
                                固定 ISP → Claude
```

每次唤醒前检查出口 IP；代理失败或 IP 不符时不发送 Token。子进程仅继承明确配置的环境变量，不继承 API Key、其他服务地址或 `NO_PROXY` 绕过规则。此代理只有一个 SOCKS5 出站，不配置直连回退。

Windows 用系统 OpenSSH 获取精简状态、打开配置窗口。专用 SSH 私钥和已固定的服务器公钥在本机；`status` 只回传每个 Token 的末 4 位与执行结果，以及 cron 文件 `/etc/cron.d/claudetile-waker` 是否存在；文件缺失时磁贴显示“定时任务未安装”。服务器 authorized_keys 将密钥限定为 `status` / `configure` / `profile`，禁止端口、Agent 和 X11 转发，不授予 root shell。SSH 不传输本机 Claude 的登录凭证。

`%APPDATA%\ClaudeTile\waker-ssh.json` 是连接配置，格式见 `scripts/waker/waker-ssh.example.json`。不包含密码或 OAuth Token。要回到本地自动打卡模式，先在 VPS 设置窗口暂停任务，再移走这个连接文件并重启磁贴。

## 部署与维护

仓库内 `scripts/waker/` 包含调度器、测试、cron、代理服务和示例配置。部署前准备：

1. `/opt/claudetile-waker/claude`：可执行的官方 Claude Code CLI 或启动脚本。
2. `/usr/bin/python`：Python 2.7 或 Python 3；调度器只用标准库。只有 `python3` 的系统（如 Ubuntu）先安装 `python-is-python3`。
3. `/usr/local/bin/xray`；将你自己的 ISP 设置填入 `/etc/claudetile-waker/proxy.json`（格式见 `proxy.example.json`），只监听 `127.0.0.1:17898`。
4. 正在运行的 cron。cronie（CentOS / RHEL）支持 `CRON_TZ`；Debian / Ubuntu 自带的 cron 忽略 `CRON_TZ`、按系统时区触发，需要系统时区为 Asia/Shanghai。

然后以 root 在 `scripts/waker/` 目录执行 `sh install.sh`。安装不会导入 Token；已有 config.json 会保留。脚本只安装/重启 `claudetile-waker-proxy`，不更改已有 Xray 服务或系统时区。

为 `claudetile-waker` 用户配置专用 SSH 公钥时，在 `authorized_keys` 的公钥前加：

```text
no-agent-forwarding,no-port-forwarding,no-X11-forwarding,command="/usr/bin/python /opt/claudetile-waker/waker.py ssh"
```

验证服务器公钥后将其写入专用 known_hosts 文件，并将本机连接配置放到 `%APPDATA%\ClaudeTile\waker-ssh.json`。私钥不要放进仓库或安装包。

glibc 过旧、官方 CLI 无法直接运行的系统，可改用官方 `linux-x64-musl` CLI 加独立的 musl/libgcc 运行库，由 `claude` 启动脚本指定加载器，不替换系统 glibc。

检查任务与代理：

```sh
cat /etc/cron.d/claudetile-waker
systemctl status claudetile-waker-proxy
runuser -u claudetile-waker -- /usr/bin/python /opt/claudetile-waker/waker.py status
```

状态文件 `/var/lib/claudetile-waker/state.json` 仅保存最后一次尝试的结果、北京时间所属日期和可用的重置时间。不要把 config.json 发到聊天或 issue 中。

测试：

```powershell
python -m unittest discover -s scripts/waker -p 'test_*.py' -v
cargo test --release
```

部署后检查真实 ISP 出口、SSH 主机校验与受限命令、执行时间计算、缺少 Token 时不发送请求。单元测试不代表已经验证订阅认证或实际整点重置。
