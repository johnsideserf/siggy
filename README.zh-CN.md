<p align="center">
  <img src="siggy-banner.png" alt="siggy" width="600">
</p>

<p align="center">
  <a href="https://github.com/johnsideserf/siggy/actions/workflows/ci.yml"><img src="https://github.com/johnsideserf/siggy/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/johnsideserf/siggy/releases/latest"><img src="https://img.shields.io/github/v/release/johnsideserf/siggy" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/johnsideserf/siggy" alt="License: GPL-3.0"></a>
  <a href="https://crates.io/crates/siggy"><img src="https://img.shields.io/crates/v/siggy" alt="crates.io"></a>
  <a href="https://johnsideserf.github.io/siggy/"><img src="https://img.shields.io/badge/docs-siggy-blue" alt="Docs"></a>
  <a href="https://ko-fi.com/johnsideserf"><img src="https://img.shields.io/badge/Ko--fi-Support%20siggy-ff5e5b?logo=ko-fi&logoColor=white" alt="Ko-fi"></a>
  <a href="https://x.com/siggyapp"><img src="https://img.shields.io/badge/follow-@siggyapp-000000?logo=x&logoColor=white" alt="Follow @siggyapp"></a>
</p>

> This translation was last updated against commit `1553609`. The English version is authoritative.

一个基于终端的 Signal 即时通讯客户端，采用 IRC 风格设计。后端基于 [signal-cli](https://github.com/AsamK/signal-cli) 的 JSON-RPC 通信。

![siggy 截图](screenshot.png)

## 安装

### Homebrew（macOS）

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### 预编译二进制文件

从 [Releases](https://github.com/johnsideserf/siggy/releases) 下载对应平台的最新版本。

**Linux / macOS**（一行命令）：

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows**（PowerShell）：

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

两个脚本都会下载最新的 Release 二进制文件，并检查 signal-cli 是否已安装。

### 通过 crates.io 安装

需要 Rust 1.70+。

```sh
cargo install siggy
```

### 从源码构建

克隆并本地构建：

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# 二进制文件位于 target/release/siggy
```

## 前置条件

- 已安装 [signal-cli](https://github.com/AsamK/signal-cli) 并可在 PATH 中访问（或通过 `signal_cli_path` 配置路径）
- 一个已作为辅助设备关联的 Signal 账户（首次设置向导会引导完成此步骤）

## 使用方法

```sh
siggy                        # 启动（使用配置文件）
siggy -a +15551234567        # 指定账户
siggy -c /path/to/config.toml  # 自定义配置文件路径
siggy --setup                # 重新运行首次设置向导
siggy --demo                 # 使用模拟数据启动（无需 signal-cli）
siggy --incognito            # 无本地消息存储（仅内存模式）
```

首次启动时，设置向导会引导你找到 signal-cli、输入手机号，并通过 QR 码关联设备。

## 配置

配置文件加载路径：
- **Linux/macOS：** `~/.config/siggy/config.toml`
- **Windows：** `%APPDATA%\siggy\config.toml`

```toml
account = "+15551234567"
signal_cli_path = "signal-cli"
download_dir = "/home/user/signal-downloads"
notify_direct = true
notify_group = true
desktop_notifications = false
inline_images = true
mouse_enabled = true
send_read_receipts = true
theme = "Default"
proxy = ""
```

所有字段均为可选。`signal_cli_path` 默认为 `"signal-cli"`（通过 PATH 查找），`download_dir` 默认为 `~/signal-downloads/`。在 Windows 上，如果 signal-cli.bat 不在 PATH 中，请使用完整路径。

## 功能特性

- **消息收发** -- 发送和接收一对一及群组消息
- **附件** -- 图片预览以内联半块字符艺术渲染；非图片附件显示为 `[attachment: filename]`
- **可点击链接** -- URL 和文件路径采用 OSC 8 超链接格式（可在 Windows Terminal、iTerm2 等终端中点击）
- **输入提示** -- 显示正在输入的联系人，并解析联系人姓名
- **消息同步** -- 从手机发送的消息会同步显示在 TUI 中
- **消息持久化** -- SQLite 存储，采用 WAL 模式；对话和已读标记会在重启后保留
- **未读跟踪** -- 侧边栏显示未读计数，对话中有"新消息"分隔线
- **通知** -- 新消息时终端响铃（可按单聊/群聊/单聊进行配置，支持单独静音），并发送操作系统级别的桌面通知
- **联系人解析** -- 使用 Signal 通讯录中的姓名；群组在启动时自动填充
- **消息反应** -- 在普通模式下按 `r` 进行反应；表情选择器带计数显示（`👍 2 ❤️ 1`）
- **回复/引用** -- 在某条消息上按 `q` 可以引用该消息进行回复
- **编辑消息** -- 按 `e` 编辑自己已发送的消息
- **删除消息** -- 按 `d` 删除本地或远程消息（仅限自己发送的消息）
- **消息搜索** -- `/search <关键词>`，用 `n`/`N` 在结果间跳转
- **@提及** -- 在群聊中输入 `@` 来提及成员，带自动补全
- **消息选择** -- 滚动时高亮当前聚焦的消息；`J`/`K` 在消息间跳转
- **已读回执** -- 发出消息上的状态符号（发送中 → 已发送 → 已送达 → 已读 → 已查看）
- **阅后即焚** -- 遵循 Signal 的阅后即焚计时器；可通过 `/disappearing` 按对话设置
- **群组管理** -- 创建群组、添加/移除成员、重命名、退出，通过 `/group` 操作
- **消息请求** -- 接受或删除来自陌生人的消息
- **拉黑/取消拉黑** -- 使用 `/block` 和 `/unblock` 拉黑联系人或群组
- **鼠标支持** -- 点击侧边栏对话、滚动消息、点击定位光标
- **颜色主题** -- 通过 `/theme` 或 `/settings` 选择主题
- **设置向导** -- 首次运行的引导流程，支持 QR 码设备关联
- **Vim 键位** -- 模态编辑（普通模式/插入模式），完整的光标移动
- **命令自动补全** -- Tab 补全弹出框，支持斜杠命令
- **设置覆盖层** -- 可在应用内切换通知、侧边栏、内联图片
- **响应式布局** -- 可调整大小的侧边栏，窄终端（<60 列）时自动隐藏
- **无痕模式** -- `--incognito` 使用内存存储，退出后不留下任何痕迹
- **代理支持** -- 通过 `proxy` 配置项设置 Signal TLS 代理，适用于受限制的网络环境
- **演示模式** -- 无需 signal-cli 即可体验 UI（`--demo`）

## 命令

| 命令 | 别名 | 描述 |
|---|---|---|
| `/join <名称>` | `/j` | 按联系人姓名、号码或群组名切换到对话 |
| `/part` | `/p` | 离开当前对话 |
| `/attach` | `/a` | 打开文件浏览器选择附件 |
| `/search <关键词>` | `/s` | 在当前（或所有）对话中搜索消息 |
| `/sidebar` | `/sb` | 切换侧边栏显示 |
| `/bell [类型]` | `/notify` | 切换通知（`direct`、`group` 或两者） |
| `/mute [时长]` | | 静音/取消静音当前对话（如 `1h`、`8h`、`1d`、`1w`） |
| `/block` | | 拉黑当前联系人或群组 |
| `/unblock` | | 取消拉黑当前联系人或群组 |
| `/disappearing <时长>` | `/dm` | 设置阅后即焚计时器（`off`、`30s`、`5m`、`1h`、`1d`、`1w`） |
| `/group` | `/g` | 打开群组管理菜单 |
| `/theme` | `/t` | 打开主题选择器 |
| `/contacts` | `/c` | 浏览已同步的联系人 |
| `/settings` | | 打开设置覆盖层 |
| `/help` | `/h` | 显示帮助覆盖层 |
| `/quit` | `/q` | 退出 siggy |

输入 `/` 打开自动补全弹出框。用 Tab 补全，方向键导航。

向新联系人发送消息：`/join +15551234567`（E.164 格式）。

## 键盘快捷键

应用采用 Vim 风格的模态编辑，有两种模式：**插入模式**（默认）和**普通模式**。

### 全局（两种模式通用）

| 键 | 功能 |
|---|---|
| `Ctrl+C` | 退出 |
| `Tab` / `Shift+Tab` | 切换到下一个/上一个对话 |
| `PgUp` / `PgDn` | 滚动消息（5 行） |
| `Ctrl+Left` / `Ctrl+Right` | 调整侧边栏大小 |

### 普通模式

按 `Esc` 进入普通模式。

| 键 | 功能 |
|---|---|
| `j` / `k` | 下/上滚动 1 行 |
| `J` / `K` | 跳转到上一条/下一条消息 |
| `Ctrl+D` / `Ctrl+U` | 下/上滚动半页 |
| `g` / `G` | 滚动到顶部/底部 |
| `h` / `l` | 光标左/右移动 |
| `w` / `b` | 光标前移/后移一个词 |
| `0` / `$` | 行首/行尾 |
| `x` | 删除光标处的字符 |
| `D` | 从光标处删除到行尾 |
| `y` / `Y` | 复制消息内容/整行 |
| `r` | 对聚焦的消息进行反应 |
| `q` | 引用/回复聚焦的消息 |
| `e` | 编辑自己已发送的消息 |
| `d` | 删除消息（本地或远程） |
| `n` / `N` | 跳转到下一个/上一个搜索匹配项 |
| `i` | 进入插入模式 |
| `a` | 进入插入模式（光标右移 1 位） |
| `I` / `A` | 在行首/行尾进入插入模式 |
| `o` | 进入插入模式（清空缓冲区） |
| `/` | 进入插入模式并预先输入 `/` |

### 插入模式（默认）

| 键 | 功能 |
|---|---|
| `Esc` | 切换到普通模式 |
| `Enter` | 发送消息 / 执行命令 |
| `Shift+Enter` / `Alt+Enter` | 插入换行符（用于多行消息） |
| `Backspace` / `Delete` | 删除字符 |
| `Up` / `Down` | 调出输入历史 |
| `Left` / `Right` | 移动光标 |
| `Home` / `End` | 跳转到行首/行尾 |

## 架构

```
键盘 --> InputAction --> App 状态 --> SignalClient (mpsc) --> signal-cli (JSON-RPC stdin/stdout)
signal-cli --> JsonRpcResponse --> SignalEvent (mpsc) --> App 状态 --> SQLite + Ratatui 渲染
```

```
+------------+   mpsc 通道   +----------------+
|  TUI       | <-----------> |  Signal        |
|  (主       |   SignalEvent  |  后端          |
|  线程)     |   UserCommand  |  (tokio 任务)  |
+------------+               +--------+-------+
                                      |
                                stdin/stdout
                                      |
                              +--------v-------+
                              |  signal-cli    |
                              |  (子进程)      |
                              +----------------+
```

基于 [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) 构建，运行于 [Tokio](https://tokio.rs/) 异步运行时。

## 许可证

[GPL-3.0](LICENSE)
