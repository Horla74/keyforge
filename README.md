# keyforge

跨平台键盘改键工具（Rust）。当前完成 **Windows** 后端；Linux / macOS 后端代码已写好骨架但
**暂停接入**（见下方「进度」）。

- 无 UI，作为后台服务/守护进程运行（`keyforged`），配套 CLI（`keyforge`）
- 配置为 TOML：支持**组合键**、单键替换、层（layer）、轻按/长按双角色（tap-hold）、
  执行命令、屏蔽按键、按前台程序过滤
- 配置热重载：**默认关闭**；加 `--watch` 后改完文件保存即生效（热重载会丢弃当时正按住的按键状态，所以默认要求重启）
- 零第三方平台依赖：Windows 后端直接用 Win32 FFI（`WH_KEYBOARD_LL` + `SendInput`）

---

## 1. 进度

| 平台 | 输入捕获 | 输出注入 | 状态 |
| --- | --- | --- | --- |
| Windows | `WH_KEYBOARD_LL` 低级键盘钩子 | `SendInput` | ✅ 已完成（编译、单测、CLI、钩子安装均已实测） |
| Linux | evdev `EVIOCGRAB` | uinput 虚拟键盘 | ⏸ 代码骨架已写（`crates/keyforge-platform/src/linux.rs`），未接入模块树、未实机验证 |
| macOS | `CGEventTap` | `CGEventPost` | ⏳ 未开始（API 方案已选型：core-graphics + 可访问性授权） |

非 Windows 平台运行时会给出明确提示：`the <os> backend is not implemented in this build yet`。

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

# 4) 前台运行（Ctrl+C 退出）；编辑配置后需重启，或加 --watch 使用热重载
.\target\release\keyforge.exe run -v
```

也可以直接用守护进程二进制（等价于 `keyforge run`）：

```powershell
.\target\release\keyforged.exe --config "$env:APPDATA\keyforge\config.toml" --log-level debug
```

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

## 3. 配置语法

```toml
[general]
default_profile = "default"   # 基础层（profile）名字
tap_hold_ms     = 200         # tap_hold 判定为“长按”的毫秒数
combo_mode      = "exact"     # exact：按键集合必须完全等于组合；subset：组合是子集即可
log_level       = "info"      # error | warn | info | debug | trace
# log_file      = 'C:\Users\me\AppData\Roaming\keyforge\keyforge.log'
# app_probe     = "auto"      # Linux 用 xdotool 探测前台程序；off 关闭

[profiles.default]            # 一个 profile 就是一层
description = "基础层"

[[profiles.default.maps]]     # 一条映射 = 一个触发器 + 一个动作
key = "capslock"
tap_hold = { tap = "escape", hold = "leftctrl" }
```

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
   默认要求重启进程来生效。

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
│  │  └─ src/linux.rs             ⏸ evdev/uinput 实现（未接入模块树）
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
cargo test -p keyforge-core                                     # 只看引擎/配置/键码
```

端到端测试（`tests/windows_e2e.rs`）会：启动真实的 `keyforged`、等钩子安装完成、
用 `SendInput` 注入一个真实的 `F13`、断言日志里出现 `f13 -> remap f14` 与注入的 `+f14`。

> ⚠️ 该测试需要**普通交互会话（完整性级别 Medium 及以上）**。在受限/低完整性
> （Low Mandatory Level，如部分沙箱）环境中，UIPI 会拦截注入输入，钩子收不到事件，
> 测试会失败——用 `keyforge doctor` 可以明确看到
> `hook round trip: no event observed … (low integrity session)`。
> 此时 `KEYFORGE_SKIP_E2E=1` 可跳过（默认已 `#[ignore]`）。

## 7. Windows 注意事项

- 必须以**当前登录用户的交互会话**运行；不要做成 Windows 服务（Session 0 看不到键盘）。
  开机自启用 `keyforge service install` 生成计划任务（登录触发）。
- 不需要管理员权限；但若前台窗口属于**更高完整性级别**的进程（例如以管理员身份运行的程序），
  注入到该窗口的按键会被 Windows 的 UIPI 拦下。需要覆盖这类窗口时，请让 keyforge 也以管理员身份运行。
- 日志默认写 stderr；`log_file` 或 `--log-file` 可同时落盘（默认路径
  `%APPDATA%\keyforge\keyforge.log`）。
- 退出：Ctrl+C（会移除钩子，不会残留吞键状态）。

## 8. 已知限制 / 路线图

- Linux / macOS 后端：代码骨架已完成，需接入模块树 + 依赖（`evdev` / `core-graphics`）并实机验证；
  Linux 启动需要 `sudo` 或 `input` 组 + uinput 权限，macOS 需要「辅助功能」授权。
- 未包含图形界面：本里程碑按需求以「无 UI 只运行服务 + CLI」交付；后续可加 GPUI 前端
  （读写同一份 TOML，通过 `--output`/配置热重载生效）。
- 输入法/Unicode 输入（发送任意字符串）未实现，目前只合成按键。
- 鼠标按键映射未实现。
