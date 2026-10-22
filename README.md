# keyforge

跨平台键盘改键工具（Rust）。已实现 **Windows / Linux** 后端；macOS 尚未实现。
Linux 已接入 evdev/uinput，真实桌面与键盘仍需实机验证。

- 后台守护进程（`keyforged`）配套 CLI（`keyforge`）；Windows 提供系统托盘菜单
- 配置为 TOML：支持**组合键**、单键替换、层（layer）、轻按/长按双角色（tap-hold）、
  执行命令、屏蔽按键、按前台程序过滤
- 配置热重载：**默认关闭**；可从托盘菜单手动重载，或加 `--watch` 后改完文件保存即生效
  （重载会清理当时的改键状态，建议先松开按键）
- Windows 无第三方平台依赖：后端直接用 Win32 FFI（`WH_KEYBOARD_LL` + `SendInput`）

---

## 1. 进度

| 平台 | 输入捕获 | 输出注入 | 状态 |
| --- | --- | --- | --- |
| Windows | `WH_KEYBOARD_LL` 低级键盘钩子 | `SendInput` | ✅ 已完成（编译、单测、CLI、钩子安装均已实测） |
| Linux | evdev `EVIOCGRAB` | uinput 虚拟键盘 | ✅ 已接入：设备抓取、热插拔、退出清理、诊断与 stop；实机验证待完成 |
| macOS | `CGEventTap` | `CGEventPost` | ⏳ 未开始（API 方案已选型：core-graphics + 可访问性授权） |

macOS 等尚未支持的平台运行时会给出明确提示：`the <os> backend is not implemented in this build yet`。

---

## 2. 快速开始（Windows）

```powershell
# 构建（发布版）
cargo build --release

# 1) 写出示例配置：%APPDATA%\keyforge\config.toml
.\target\release\keyforge.exe init

# 2) 校验配置（会打印解析出来的每条映射）
.\target\release\keyforge.exe check

# 3) 环境自检：完整性级别、钩子可用性、注入能力、前台程序识别
.\target\release\keyforge.exe doctor

# 4) 前台运行（Ctrl+C 退出）；编辑配置后可从托盘手动重载，或加 --watch 使用热重载
.\target\release\keyforge.exe run -v
```

也可以直接运行守护进程二进制。Windows 下 `keyforged.exe` 不显示控制台日志窗口，
会在任务栏右侧通知区域显示托盘图标（可能收纳在「显示隐藏的图标」中）：

```powershell
.\target\release\keyforged.exe --config "$env:APPDATA\keyforge\config.toml" --log-level debug
```

右键托盘图标可选择：

- **Config: …**：显示当前实际使用的配置文件完整路径。
- **Open config folder**：在资源管理器中打开该配置文件所在文件夹。
- **Reload config**：立即重载配置，无需 `--watch`。配置无效时保留原配置，错误写入日志。
- **Pause / Resume**：暂停 / 恢复改键；暂停先释放仍按住的注入键，暂停期间键盘输入正常通过。
  暂停期间重载配置不会自动恢复改键。
- **Exit**：释放仍按住的注入键、移除键盘钩子并退出。

运行图标为绿色 **K**，暂停图标为红色 **S**，提示文字显示 running / paused 状态。
图标已嵌入 exe，无需额外复制；源文件位于 `crates/keyforge-platform/assets/`，
可运行 `scripts/generate-icons.ps1` 重新生成。`keyforged.exe` 默认将日志写入
`%APPDATA%\keyforge\keyforge.log`，可通过配置 `log_file` 或 `--log-file` 覆盖；
启动失败会弹出错误提示。需要查看前台日志时使用 `keyforge run -v`（也有托盘菜单）。
重载或暂停会清理当前按键状态，建议先松开键盘再使用这些菜单。

### 常用命令

| 命令 | 作用 |
| --- | --- |
| `keyforge run` | 前台运行守护进程 |
| `keyforge stop` | 优雅停止正在运行的实例（会先释放所有还按着的注入键）；`--force` 用 taskkill 兜底 |
| `keyforge check` | 只解析+校验配置并打印映射表，不接管键盘 |
| `keyforge doctor` | 诊断：配置、进程完整性级别、钩子/注入能力、前台程序 |
| `keyforge init` | 写出示例配置（`--force` 覆盖） |
| `keyforge keys` | 列出所有可用键名 |
| `keyforge service print` / `install` | 打印 / 生成开机自启定义（Windows 计划任务 XML） |
| `keyforge devices` | 列出输入设备（仅 Linux 有意义） |

通用参数：`-c/--config <PATH>`、`--log-level <error|warn|info|debug|trace>`、`--log-file <PATH>`、
`--watch`、`--no-app-probe`、`-v`（`--no-watch` 仍可用，是默认行为）。

配置路径优先级：`--config` > 环境变量 `KEYFORGE_CONFIG` >
`%APPDATA%\keyforge\config.toml`（Linux：`$XDG_CONFIG_HOME/keyforge/config.toml`，
macOS：`~/Library/Application Support/keyforge/config.toml`）。

---

## 2.1 快速开始（Linux）

```bash
cargo build --release
./target/release/keyforge init
./target/release/keyforge check
sudo modprobe uinput
```

推荐以普通用户运行。安装仓库中的 udev 规则并加入专用组：

```bash
sudo groupadd -f keyforge
sudo usermod -aG keyforge "$USER"
sudo install -m 644 packaging/linux/60-keyforge.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=input
sudo udevadm trigger --subsystem-match=misc
```

注销并重新登录以获得新组权限。该组可读取键盘事件并注入输入，只应加入可信用户。
如需重启后加载 uinput，可将 `uinput` 写入 `/etc/modules-load.d/keyforge.conf`。

```bash
./target/release/keyforge devices
./target/release/keyforge doctor
./target/release/keyforge run -v
# 另一终端（同一用户）：
./target/release/keyforge stop
```

默认配置为 `$XDG_CONFIG_HOME/keyforge/config.toml`，未设置时使用
`~/.config/keyforge/config.toml`。临时 root 测试需明确指定普通用户的配置：
`sudo ./target/release/keyforge run --config "$HOME/.config/keyforge/config.toml"`；
停止该实例也需使用 `sudo ./target/release/keyforge stop`。

- 输入通过 evdev 抓取并经 uinput 注入，适用于 X11、Wayland 和 Linux 控制台。
- `Ctrl+C`、`SIGTERM`、`keyforge stop` 均先释放注入键再退出；每个用户只允许一个实例。
  `stop --force` 在优雅退出超时后使用 SIGKILL，不能保证按键清理。
- 默认每 3 秒发现新键盘；拔出键盘会释放其持有的按键。多个键盘按住同一键时，最后一个松开才释放。
- `--watch` 可启用配置热重载；Linux 没有托盘菜单。启动、重载时建议先松开按键。
- X11 的前台应用过滤需要 `xdotool` 及有效的 DISPLAY/XAUTHORITY。
  Wayland 下前台应用为未知，`only_in` 不生效，`except_in` 按未知应用规则处理。
  全局映射不受影响；不需要应用过滤时可加 `--no-app-probe`。
- `doctor` 尝试创建并销毁虚拟键盘，不注入按键、不抓取物理键盘。

登录自启动（先完成组权限配置）：

```bash
./target/release/keyforge service install
mkdir -p ~/.config/systemd/user
cp "${XDG_CONFIG_HOME:-$HOME/.config}/keyforge/keyforge.service" ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now keyforge
journalctl --user -u keyforge -f
# 停止服务，避免 Restart=on-failure 在异常退出后重启：
systemctl --user stop keyforge
```

生成的 unit 使用当前构建目录的绝对路径；移动二进制后需要重新生成。
如果设置了自定义 XDG 配置目录，请相应调整 systemd 用户 unit 的安装目录。

---

## 3. 配置语法

```toml
[general]
default_profile = "default"   # 基础层（profile）名字
tap_hold_ms     = 200         # tap_hold 判定为“长按”的毫秒数
combo_mode      = "exact"     # exact：按键集合必须完全等于组合；subset：组合是子集即可
log_level       = "info"      # error | warn | info | debug | trace
log_max_size_mb  = 10          # 单个日志文件上限，单位 MiB，必须为正整数
log_retention_days = 7         # 归档保留天数，必须为正整数
# log_file      = 'C:\Users\me\AppData\Roaming\keyforge\keyforge.log'
# app_probe     = "auto"      # Linux 用 xdotool 探测前台程序；off 关闭

[profiles.default]            # 一个 profile 就是一层
description = "基础层"

[[profiles.default.maps]]     # 一条映射 = 一个触发器 + 一个动作
key = "capslock"
tap_hold = { tap = "escape", hold = "leftctrl" }
```

日志在下一条记录将超过大小上限时归档；单条超大记录会完整保留，可能单独超过上限。
持续运行时也会按 24 小时归档。归档保存在日志文件旁，格式为
`keyforge.log.archive-<Unix时间戳>-<序号>.log`。启动时和写日志时（最多每小时一次）
清理超过保留天数的归档，不删除其他文件。大小与保留天数支持 **Reload config** / `--watch`
生效；日志路径和日志级别的更改仍需重启。

### 3.1 触发器

```toml
key  = "capslock"             # 单键写法
keys = ["ctrl", "h"]          # 组合键写法（可多个）
```

- 键名不区分大小写、忽略 `-` `_` 等分隔符：`Page-Up` = `pgup` = `pageup`。
- **通用修饰键**：`ctrl` / `shift` / `alt` / `meta`（别名 `control` `cmd` `command` `win` `super` `option`）
  在触发器中表示「左右任一」。即 `keys = ["ctrl", "h"]` 左右 Ctrl 都能触发。
- 需要区分左右时用 `leftctrl` / `rightctrl` / `leftalt` / `rightalt` / `leftshift` / `rightshift` /
  `leftmeta` / `rightmeta`。
- 完整键名列表：`keyforge keys`。

### 3.2 动作（每条映射只能写一个）

| 写法 | 语义 |
| --- | --- |
| `remap = "leftctrl"` | **按住**期间输出该键；松开时释放。支持组合：`remap = "ctrl+shift+t"` |
| `tap = "escape"` | 激活时快速按一下，支持组合 |
| `sequence = ["shift+home", "backspace"]` | **按顺序执行多个和弦**（多步骤宏）：每步按下再松开。上例 = 从光标删除到行首 |
| `command = "..."` | 激活时执行命令（Windows 用 `cmd /C`，其他平台 `sh -c`），不阻塞钩子 |
| `layer = "nav"` | **按住**期间激活 nav 层，松开回到原层 |
| `toggle_layer = "nav"` | 按一下切换 nav 层（常驻，直到再次切换） |
| `block = true` | 吞掉这个键，什么都不输出 |
| `tap_hold = { tap = "...", hold = "..." }` | 轻按 = tap，长按/与其他键组合 = hold；可加 `timeout_ms` 覆盖 `general.tap_hold_ms` |

其它可选字段：

```toml
only_in   = ["code", "firefox"]   # 仅在（子串匹配）某个前台程序时生效
except_in = ["game.exe"]          # 在这些程序里不生效
enabled   = true                  # 置 false 可临时停用
```

应用名匹配规则：大小写不敏感、**子串**匹配。Windows 下取前台窗口所属进程的文件名
（如 `chrome.exe`），因此写 `chrome` 即可命中。拿不到前台程序信息时，`only_in` 映射会被跳过，
`except_in` 映射照常生效。

### 3.3 多步骤序列（sequence）

有些操作需要按顺序发几个和弦，例如「删除到行首」= 先 `Shift+Home` 选中，再 `Backspace` 删除：

```toml
[[profiles.default.maps]]
keys = ["capslock", "u"]                    # 你的布局里也可以写 ["ctrl", "u"]
sequence = ["shift+home", "backspace"]
```

- 每一步都是**按下再松开**，上一步完全松开后才执行下一步。
- 只有一个元素时等价于 `tap`（向后兼容：`sequence = "ctrl+c"` 仍然可用）。
- 激活时同样会"清理修饰键"：例如按住 Ctrl+U，引擎会先释放 Ctrl，让应用看到干净的
  `Shift+Home`（而不是 `Ctrl+Shift+Home`——那会选中到文首），执行完再把 Ctrl 按回去。
- 若某个应用需要步骤之间留间隔，可先用 `tap` 分两次触发；需要 `step_ms` 延时的话告诉我。

### 3.4 层（layer）语义

- 查找顺序：**瞬时层（最近激活的在前）→ 常驻层 → 基础层**。
- 层里没定义的键会**穿透**到下一层（例如 nav 层只改了 `h/j/k/l`，其它键行为不变）。
- 激活瞬时层的那个键本身不计入组合匹配，所以 `f1`（进 nav 层）+ `h` 能正确命中 nav 层的单键映射。

### 3.4 完整示例

见 `crates/keyforge-core/assets/default_config.toml`（即 `keyforge init` 写出的内容）。

---

## 4. 引擎语义（为什么这样设计）

物理按键事件进来，引擎回答两件事：**是否吞掉**、**要注入什么**。几个关键点：

1. **组合键的修饰键清理**：按下 `Ctrl` 时它已经发给系统了，之后 `H` 才凑成 `Ctrl+H`。
   引擎会在激活映射时把「已发出的」修饰键**先释放**，输出目标键，映射结束时再**恢复**
   （如果它仍被按住）。所以 `ctrl+h -> left` 得到的是干净的左方向键，而不是 `Ctrl+Left`。
2. **绝不重复输入（重要）**：引擎记录每个键是「系统自己发出去的（Physical）」还是
   「keyforge 注入的（Injected）」。Windows/macOS 钩子会把不拦截的事件放行给系统，因此**不再注入副本**；
   只有我们自己发出的按下才由我们负责释放。反之 Linux evdev 抓取设备后原始事件到不了系统，
   此时由 `set_reinject_passthrough(true)`（平台常量 `REINJECTS_PASSTHROUGH`）让引擎代发并吞掉原事件。
   这就是「按 a 出现 aa」的根因与防线，回归测试见
   `engine::tests::plain_key_passes_through_without_duplicating`。
3. **注入顺序**：映射输出**先于**触发它的那个物理按键交给系统（Windows 在钩子回调里同步注入），
   否则「长按 CapsLock = Ctrl」+ `c` 会变成先 `c` 后 `Ctrl`。
4. **自动重复**：被吞掉的键如果仍在按住并产生自动重复，会重复注入输出键，保持长按连续输入。
5. **自身注入不会被再次处理**：注入事件带魔数 `dwExtraInfo = "KFORGE"`，钩子直接放行。
6. **钩子内不跑命令**：`command` 只做入队，由消息循环执行，避免阻塞输入链（钩子超时会被系统摘除）。
7. **热重载（`--watch`，默认关闭）**：配置变更后重建引擎，并把当前按住的映射**先释放**；
   被"扣住"的 Win 按键状态会跨重载保留。因为重载必然丢弃"此刻正按住"的按键状态，
   默认使用托盘菜单手动重载或重启进程来生效。

---

## 5. 项目结构

```
keyforge/
├─ Cargo.toml                     工作区
├─ crates/
│  ├─ keyforge-core/              平台无关：键码表、配置模型与编译、引擎状态机、日志、路径
│  │  ├─ src/keycode.rs           一张宏表驱动：枚举 + 键名解析 + Win32 VK / evdev / macOS 键码
│  │  ├─ src/config.rs            TOML 模型、校验（一次报出所有错误）、编译为可匹配结构
│  │  ├─ src/engine.rs            组合键匹配、层、tap-hold、修饰键清理、自动重复
│  │  └─ assets/default_config.toml  示例配置（`keyforge init` 的模板）
│  ├─ keyforge-platform/          平台后端
│  │  ├─ src/windows.rs           WH_KEYBOARD_LL + SendInput + 前台程序 + doctor 探针
│  │  ├─ src/windows/tray.rs      通知区域图标、重载 / 暂停 / 恢复 / 退出菜单
│  │  └─ src/linux.rs             evdev/uinput、热插拔与退出清理
│  └─ keyforge-daemon/            `keyforge` / `keyforged` 两个二进制
│     ├─ src/runtime.rs           Host 实现：热重载、命令执行、前台程序轮询、日志
│     ├─ src/cli.rs               参数解析与子命令
│     └─ src/service.rs           开机自启定义（计划任务 XML / systemd unit / launchd plist）
└─ crates/.../tests/              集成测试
```

## 6. 测试

```bash
cargo test                         # 单元测试 + CLI 测试（端到端测试被标记为 ignore）
cargo test -p keyforge-daemon --test windows_e2e -- --ignored   # 真实按键端到端
cargo test -p keyforge-daemon --test windows_tray -- --ignored  # 托盘生命周期（需交互桌面且无现有实例）
cargo test -p keyforge-core                                     # 只看引擎/配置/键码
cargo test -p keyforge-platform control_is_exclusive -- --ignored # Linux IPC 集成测试（需允许 Unix socket）
```

端到端测试（`tests/windows_e2e.rs`）会：启动真实的 `keyforged`、等钩子安装完成、
用 `SendInput` 注入一个真实的 `F13`、断言日志里出现 `f13 -> remap f14` 与注入的 `+f14`。

> ⚠️ 该测试需要**普通交互会话（完整性级别 Medium 及以上）**。在受限/低完整性
> （Low Mandatory Level，如部分沙箱）环境中，UIPI 会拦截注入输入，钩子收不到事件，
> 测试会失败——用 `keyforge doctor` 可以明确看到
> `hook round trip: no event observed … (low integrity session)`。
> 此时 `KEYFORGE_SKIP_E2E=1` 可跳过（默认已 `#[ignore]`）。

## 7. Windows 注意事项

- 锁屏/会话断开与休眠时清理按键状态；解锁、返回桌面或唤醒时立即重新安装键盘钩子。
  安装暂时失败会每秒重试，用户手动暂停的状态保持不变。日志中可查找
  `keyboard hook reinstalled after desktop recovery` 确认恢复。
- 必须以**当前登录用户的交互会话**运行；不要做成 Windows 服务（Session 0 看不到键盘）。
  开机自启用 `keyforge service install` 生成计划任务（登录触发）。
- 不需要管理员权限；但若前台窗口属于**更高完整性级别**的进程（例如以管理员身份运行的程序），
  注入到该窗口的按键会被 Windows 的 UIPI 拦下。需要覆盖这类窗口时，请让 keyforge 也以管理员身份运行。
- `keyforge run` 日志默认写 stderr；`log_file` 或 `--log-file` 可同时落盘。
  `keyforged.exe` 默认写 `%APPDATA%\keyforge\keyforge.log`。
- 退出：托盘菜单 **Exit**、`keyforge stop`，或前台运行时 Ctrl+C（均会释放注入键并移除钩子）。

## 8. 已知限制 / 路线图

- Linux 后端已实现，真实硬件/桌面验证待完成。没有托盘；键盘 LED 状态尚未回写物理设备。
  仅抓取包含 A/Z/Enter/Space 且不含鼠标/触摸板轴的键盘节点，独立媒体键节点不接管。
- macOS 后端尚未实现，计划使用 `CGEventTap` / `CGEventPost` 和「辅助功能」授权。
- Windows 已提供托盘控制，尚未包含配置编辑图形界面；后续可加 GPUI 前端
  （读写同一份 TOML，通过 `--output`/配置热重载生效）。
- 输入法/Unicode 输入（发送任意字符串）未实现，目前只合成按键。
- 鼠标按键映射未实现。
