# Vitals → Meerkat：功能与 UI 梳理，以及 Windows 版落地方案

来源：<https://vitalsmac.com/> （含 `llms.txt`、`llms-full.txt`、`changelog.md`、`windows.md`，抓取于 2026-10-07，App 版本 1.3.5）
本文档是工作产物，不进仓库。

---

## 1. Vitals 是什么

> Activity Monitor 的替代品：把每个子进程归到它所属的 App 下，一台正在干活的 Mac 显示约 60 个 App 而不是约 1000 个进程，每个 App 一个内存、CPU、能耗、磁盘、网络数字。

三根支柱：
1. **App 而非进程** —— 归组引擎是产品身份。
2. **30 天历史 + Worth a Look** —— 不只回答「现在多忙」，还回答「刚才发生了什么、哪个 App 干的」。
3. **菜单栏常驻** —— 每个读数一个菜单栏项（值／图形／两者），背后是 dropdown 迷你版 App。

一次性买断 $9（发布期，原价 $39），无订阅、无试用、14 天退款。官方对 Windows 只有一个 waitlist（`/windows`），并明确说了三句关键话：
- **A count, not a promise** —— 还没有任何构建。
- **Built for Windows. Not a port.** —— Windows 有托盘，要按托盘重新设计。
- 会保留的内核：**apps instead of a wall of processes、insights、Projects、可自由排列的读数**。

---

## 2. Mac 版功能全景（逐项，来自官方文案 + changelog）

### 2.1 采集与归组
| 功能 | 细节 |
|---|---|
| 进程按 App 归组 | 每个 helper 进程加到启动它的 App 上；每个 App 给一个 memory/CPU/energy/disk/network 数字 |
| 每 App 的指标列 | 命名为 `AppCPU`、`AppMemory`、`AppWriting`、`AppDownloading`、`AppGPU`、`AppPower` |
| 浏览器内部拆分 | 点开 Chrome/Safari/Firefox/Edge/Brave/Arc/Dia → 内存与 CPU 按 tab、扩展、图形、浏览器本体拆开；Slack/VS Code 类按窗口拆 |
| 进程级操作 | Quit / Force Quit，永远先确认；可只退单个进程 |
| 右键项 | Copy Name / Copy Process Name / Copy Process Path；Pin（置顶，跨排序保持）；Turn Off App Nap |
| 系统进程可隐藏 | Show macOS in Lists 开关：关掉后列表只显示你自己的 App |

### 2.2 子系统 tab（窗口 + dropdown 都有）
Overview / CPU / Memory / Disk / Network / GPU / Battery / Sound / Bluetooth / Projects，外加 Docker 在跑时的 Containers。

各 tab 已知数字（官网文案原词）：
- **CPU**：`CPU load (%)`、user/system 拆分、每核分布、load averages、Uptime
- **Memory**：In Use、Normal、App、Wired、Compressed、Cached、Free、Swap，另有 memory pressure 百分比、磁盘占比
- **Disk**：`Reading/Writing speed` + 当日累计传输量
- **Network**：`Downloading/Uploading speed` + 一周、一月历史累计
- **GPU**：利用率百分比、峰值评级、独立显存占用
- **Battery**：充电百分比、预计可用时间、瓦数、循环次数、健康度（衰减）；Idle Wake Ups 列；「谁在阻止睡眠」列表（附它自己声明的原因和持续时长）
- **Temperature & Fans**：CPU/GPU 温度、风扇转速、可锁定风扇转速（退出时自动交还系统）、菜单栏温度可跟 CPU/GPU/最热传感器；无电池的 Mac mini/iMac 显示整机功耗
- **Sound**：输出设备与音量 + 每个发声 App 独立滑块（记住设置，跨重启）、麦克风设备与音量 + 哪些 App 在用
- **Bluetooth**：所有已连接设备，按电量从低到高
- **Projects**：按项目目录归组的进程 + 它们的监听端口；闲置多日的 dev server 被点名并提议停止；支持整个项目一次性 Quit/Force Quit
- **Containers**：每个 Docker 容器的内存/CPU/网络/端口 + Stop

### 2.3 历史与洞察
- 30 天历史：CPU、内存、能耗、磁盘、网络；区间 1 小时 / 12 小时 / 24 小时 / 7 天 / 30 天
- 「最忙 App」按小时排名，保留最近 2 天
- **Worth a Look**：独立面板，按天分组；同一件事被多次注意到合成一行带计数；每行说明「现在的情况」；可按类型过滤、可 pin
- 提醒：App 长期占用 CPU、内存持续增长、狂写磁盘或狂用网络时发通知

### 2.4 菜单栏 / 面板 / 窗口 / 设置
- 每个读数独立菜单栏项：CPU、内存、GPU、网络、磁盘、温度、电池；样式三选：**Value / Graph / Both**；可带或不带图标与标题；⌘-drag 排序；紧凑尺寸小屏可用；内存项可显示「空闲」而非「已用」；电池 ≤20% 变红、充电显示闪电
- 右键菜单栏项：Open Vitals / Check for Updates / Settings / Quit
- dropdown：所有指标 + 最忙 App + 每个指标一个 tab；Tiles 与 **List**（每指标一行）两种密度；可从 dropdown 直接 Quit App / Stop 容器；点 App 直接开窗口到那个 App；键盘 Tab 遍历、⌘1–⌘0 跳 tab、Esc 关闭；可毛玻璃可实底
- 窗口：tab 条 + 卡片 + 图表 + 列表；**Arrange everything**（每个 tab 上可显隐/排序 section、卡片、ring、App）；工具栏有 Chart style 与 Export；标题栏显示芯片/核数/内存；Over Time 独立面板按天分组；⌘F 任意 tab 搜索 App（无列表的 tab 会跳到 Memory 并带上过滤）；窗口可 Pin
- Settings 侧栏页：**General**（Open Window On、Show macOS in Lists、快捷键、单位、语言、Show in Dock）、**Layout**（顶部画出菜单栏，点项改样式、从图片里选外观、+ 添加；再画 dropdown 与窗口，拖拽排序 tabs/tiles/sections；dropdown 预览用真实数字）、**License**（激活/Deactivate This Mac/写评价）、**About**（Copy Report —— 机型+设置+近期日志，不含任何 App 名）
- 单位：摄氏/华氏、网络字节/比特、CPU 按核/按整机，每项旁边给实时示例
- 语言：11 种，用 macOS 自己的术语，跟随系统语言，可按 App 覆盖
- 快捷键：任意 App 里唤起 dropdown 或窗口；Shortcuts.app 动作（取一个读数 / 最忙 App / 打开 / 显示面板）
- 导出：1200×630 分享卡 或 仪表盘图片，保存或复制
- 隐私：无账号、无统计，只有 2 个网络请求（许可证校验 + 更新检查）；可选 helper 守护进程绕过沙箱读取全系统任务
- 显示在 Dock（可选）；开机自启时只待菜单栏

### 2.5 Roadmap（官方尚未实现，说明这些不是遗漏）
MCP server、Ask Vitals、Agents tab、存储地图、缓存清理、每盘磁盘活动、桌面小部件、Keep Awake、失控 dev 进程 watchdog。

---

## 3. UI 结构拆解（要复刻的骨架）

```
┌ 托盘区（Windows 通知区域）
│  [CPU ▁▂▅▃ 42%] [MEM 78%] [NET ↓1.2 ↑0.3] [BAT 61%]   ← 每个读数一个图标
│  点击 → 面板（无边框透明置顶窗，锚定在该图标下方）
│  右击 → 系统菜单：Open / Settings / Check for Updates / Quit
│
├ 面板 Panel (400×580)
│  头部：机器摘要（芯片 · 核数 · 内存 · 运行时间）+ 打开窗口按钮
│  读数区：Tiles（卡片网格，每卡 = 名称 + 数字 + 迷你图）｜List（每指标一行）
│  App 区：最忙 App 列表（图标 + 名 + 当前指标条 + hover 出现 ×）
│  tab 条：Overview CPU Memory Disk Network GPU Battery …（可显隐排序）
│
└ 主窗口 (1120×740，min 880×560)
   工具栏：tab 分段控件 ｜ 右侧 Chart style、Export、Arrange、⌘F 搜索
   标题栏信息条：芯片 · 核数 · 内存总量 · 运行时间
   内容区：section 流（卡片 + 大图 + 列表）
   点击 App → Inspector（图表让位而不是被盖住）
```

设计语言（从截图与文案推断）：卡片式分区、每个数字都有对应曲线、字重层级强但色相克制、图表用极细线与渐变面积、深浅两态跟随系统、留白紧、玻璃感背景。

---

## 4. Windows 落地映射（含权限与降级）

技术栈：**Tauri 2 + Rust（`x86_64-pc-windows-gnu`）+ React 19 + Vite**。下表「结论」列已按本机实测修正，标 ❌ 的是被证据推翻的原判断。

| Mac 做法 | Windows 对应 | 权限 | 结论 |
|---|---|---|---|
| 多个菜单栏项（值/图形/两者，⌘-drag 排序） | 多个托盘图标，位图由 **Rust 侧 GDI 直接绘制**（CreateDIBSection + TextOutW + Polyline），按 `SM_CXSMICON` 取实际尺寸 | 普通用户 | ✅ 值/图形/两者都能做。⚠️ Win11 不让第三方控制托盘顺序（系统在「其他系统托盘图标」里管理）→ 顺序在 Meerkat 的 Layout 里定义。⚠️ 不能用 webview canvas 画：隐藏 webview 的定时器会被节流，托盘会停止刷新（实测踩到后改成 GDI） |
| dropdown 面板 | `decorations:false, transparent:true, alwaysOnTop, skipTaskbar` 窗口，按托盘事件的 position 定位 | 普通用户 | ✅ |
| 进程按 App 归组 | 安装目录 + 父进程链锚定；`svchost -k <group>` 按角色拆分；名称取版本信息 FileDescription | 普通用户 | ✅ 实测 424 进程 → 295 App，Chrome 29 进程并成 1 行；中文名走系统自己的措辞（任务管理器/微信输入法/Windows 资源管理器） |
| CPU load / 每核 / load average | sysinfo 每核 + GetSystemTimes 拆 user/system（除以 kernel+user 总时间） | 普通用户 | ✅ load average 在 Windows 无对应概念 → 改用「处理器队列长度」(PDH) |
| 内存六态 | 换成 Windows 术语：使用中 / 可用 / 已缓存 / 已提交 / 提交上限 / 分页文件 / 分页缓冲池 / 非分页缓冲池；压力取 GlobalMemoryStatusEx.dwMemoryLoad | 普通用户 | ✅ 语义映射而非逐字对齐 |
| 磁盘速率 + 当日累计 | sysinfo Disk::usage() 的 total_read/written_bytes 自算差值；累计由 Meerkat 按时间积分（Windows 无「今日总量」计数） | 普通用户 | ✅ 6 卷 + SSD 标识实测正常 |
| 网络速率 + 周/月累计 | sysinfo Networks 64 位计数器差值；累计自存 | 普通用户 | ✅ |
| **每 App 网络字节** | 只能靠 ETW Microsoft-Windows-Kernel-Network，内核会话需**管理员** | 管理员 | ❌ 降级为每 App **连接数 + 监听端口**（GetExtendedTcpTable，普通权限）；字节数留给可选提权 helper |
| 每 App GPU | PDH `\GPU Engine(*)\Utilization Percentage`，实例名形如 `pid_<n>_luid_..._engtype_3D` | 普通用户 | ✅ 实测 DeltaForce 25.9% / Qoder 5.0%。⚠️ 两个坑：`PDH_MORE_DATA` 在 windows 0.62 是 0x800007D2（不是 pdh.h 的 0x80000408）；值数组每项含 f64，缓冲区必须 8 字节对齐 |
| GPU 总体 + 显存 | DXGI IDXGIAdapter3::QueryVideoMemoryInfo | 普通用户 | ⚠️ NVIDIA 的 CurrentUsage 恒为 0 → 显示「不可用」而不是 0.00 B；DXGI 会把同一块卡按不同 LUID 枚举两次，要按名字+容量去重并过滤 Basic Render Driver |
| 温度 | MSAcpi_ThermalZoneTemperature 实测**拒绝访问**（这就是 sysinfo 返回 0 传感器的原因）；PDH `\Thermal Zone Information\Temperature` 可读但本机是主板**静态区域温度**（8 线程满载 12 秒不动）；**2026-10-18 追加实测：提权（管理员）后同一 WMI 类仍只返回同一个静态区域，40 线程满载 25 秒数值不变——提权打开的是访问同一块静态读数的门，不是 CPU 封装温度** | 无解 | ❌ CPU 温度在 Windows 上没有用户态 API，提权也拿不到，**绝不拿区域温度冒充**。✅ GPU 温度/风扇%/功耗走 **NVML**（nvml.dll 在，非管理员可读）。提权 helper 方案已证伪并从代码中移除（relaunch_elevated 已删） |
| 风扇转速 / 锁风扇 | Win32_Fan / CIM_Fan / CIM_AssociatedCooling 全 0 实例；PDH 全量扫描无 RPM 计数器；SuperIO/EC 需签名内核驱动 | — | ❌ CPU/机箱风扇彻底不做（保留「Windows 无公开 API」诚实态）；只有 GPU 风扇 % 可做 |
| 电池：电量/时间/瓦/循环/健康 | GetSystemPowerStatus + `powercfg /batteryreport /xml`（设计容量/满充容量/循环数） | 普通用户 | ✅ 台式机明确显示「这台机器没有电池」；BatteryLifeTime 常为 -1 → 显示「—」不猜 |
| 每 App 音量 | Core Audio IMMDeviceEnumerator → IAudioSessionManager2 → IAudioSessionControl2 + ISimpleAudioVolume | 普通用户 | ✅ 完整可做（含静音、电平柱）。⚠️ 0.62 里类型在 `Win32::Media::Audio`，`MMDeviceEnumerator` 的 GUID 常量**该 crate 已自带**（自己手写 CLSID 会撞 REGDB_E_CLASSNOTREG）、Activate 被 Com_StructuredStorage + Variant 两个 feature 门控、必须在调用线程上 CoInitializeEx（MTA，实测在采样线程可用）、会话指针不可缓存要每秒重举、写操作要按 instance id 重新定位（同一进程的会话在两次枚举间会换位，按位置匹配会把 A 的滑块写到 B 上）。⚠️ Windows 在**同一次登录内**会自己按 exe 名还原音量，所以 Meerkat 的存盘只在跨重启/跨音频服务重启时才真正起作用。⚠️ 每应用**输入**音量不可靠 → 只留设备级麦克风电平（IAudioMeterInformation 不开采集流，不点亮隐私指示灯） |
| 蓝牙电量 | WinRT BluetoothLEDevice + 连 GATT 读 Battery Service(0x180F) 的特征 0x2A19 | 普通用户 | ⚠️ 只对暴露标准 180F 的设备（实测 Xbox 手柄有该接口但**无现成电量属性**，必须真连 GATT）。AirPods、罗技 2.4G/Bolt 走私有协议 → 单独给「设备不支持」状态，不能显示 0% |
| Projects：目录/命令行/端口 | GetExtendedTcpTable(owner pid) + 读 PEB 取 cwd（需 PROCESS_QUERY_INFORMATION\|VM_READ = 0x410） | 普通用户（同用户） | ✅ 实测 438 进程中 201 个可读到 cwd，237 个失败**全部**是 OpenProcess 拒绝（系统/服务/session 0）。项目根从 cwd 向上找 package.json / Cargo.toml / .git，不要直接用 cwd |
| 「闲置很久的 dev server」 | 无「最后活动时间」API（GetPerTcpConnectionEStats 的 Set 实测需管理员） | — | ⚠️ 只能启发式：established 集合变化 + IO 计数 + 累计 CPU + 存活时长 + GetLastInputInfo |
| 浏览器内部按 tab 拆 | ~~EnumWindows 每标签一个隐藏顶层窗口~~ | 普通用户 | ❌ **原判断被实测推翻**：556 个顶层窗口里 Chrome 只有 1 个带标题（browser 主进程），23 个 renderer 零标题；Site Isolation 下 renderer≠tab。改为按 browser/renderer/utility/GPU 进程类型拆分，标签标题只能给「当前活动标签」 |
| 30 天历史 | 本地分桶（1min×2h / 5min×24h / 1h×30d），App 明细保 48h；JSONL + serde，不引入 SQLite C 依赖 | 普通用户 | ✅ 已实现并落盘 |
| Worth a Look / 提醒 | 规则引擎 + plugin-notification（winrt-notification Toast，AUMID 取 identifier） | 普通用户 | ✅ 重复项合成一行带计数；未安装时 dev 版能弹但显示名退化 |
| 谁在阻止睡眠 / Idle Wake Ups | powercfg /requests 需管理员；后者无公开等价计数 | 管理员 | ❌ 都不做 |
| 导出 1200×630 | 前端离屏 canvas → PNG | 普通用户 | ✅ |
| 全局快捷键 / 自启 / 任务栏 | plugin-global-shortcut（gnu 可编）；autostart 写 HKCU CurrentVersion\Run；set_skip_taskbar | 普通用户 | ✅ 均不需管理员 |
| 打包分发 | **Tauri 不产 MSIX**（只有 nsis / msi）；NSIS 的 makensis 由 CLI 自动下到 `%LOCALAPPDATA%\tauri\NSIS`（5–10MB），默认 currentUser 安装；gnu host 下资源编译只调 PATH 上的 **windres**（rc.exe 走不到，debug 构建已实证产出 .rsrc 段） | 普通用户 | ✅ 现实可交付：NSIS per-user 安装包 + 「exe + WebView2Loader.dll」便携 zip。实测 objdump -p **无** libgcc / libwinpthread / libstdc++ 依赖，用户机不需要 MinGW；+crt-static 可链但 269KB→5.1MB 纯亏，**不要用** |
| 许可证 / 自动更新 | v1 不做付费与激活；updater 需 pubkey + 静态 JSON 托管（GitHub Releases 即可），signer 免费 | — | 按你的决定延后；未签名 exe 会被 SmartScreen/MOTW 拦，文档要写清 |

**Windows 上反而比 Mac 做得更好的三处**：
1. 每应用音量是系统原生概念，可以比 Mac 更准（含实时电平柱），只差自己补持久化。
2. NVML 让「GPU 温度 + 风扇 % + 实时功耗」在**非管理员**下就能拿到 —— Mac 版只在部分机型给得出完整功耗数字。
3. 窗口标题与 UWP/MSIX 包身份能让归组比 Mac 更准。

## 5. v1 范围（你选的「核心监控对齐」）

**做**
- 托盘：多读数（CPU/内存/网络/磁盘/GPU/温度/电池）× 三样式（值/图/两者），Layout 里定义顺序与样式
- 面板：Tiles + List 两密度、读数区 + 最忙 App + tab 条 + 从面板 Quit
- 窗口：Overview、CPU、Memory、Disk、Network、GPU、Battery 七个 tab；App 归组列表 + 排序 + 搜索 + Pin + 右键复制/退出/强制退出
- 历史：分桶存储，区间 1h/12h/24h/7d/30d；最忙 App 小时排名 48h
- Worth a Look：规则 + Toast 提醒 + 面板里的独立列表
- 设置：General（单位/语言/区间/自启/任务栏显示/起始 tab）、Layout（菜单栏项、面板 tiles、窗口 tabs 与分区的显隐与排序 + 重置）
- i18n：en + zh-Hans，跟随系统显示语言，可手动覆盖
- 图标：真实进程图标（SHGetFileInfo）
- 低开销：窗口不可见时降采样频率（对齐 Mac 版「A lighter Vitals」）

**明确不做（v1 之后）**
Sound、Bluetooth、Projects、Containers、浏览器 tab 拆分、导出图片、风扇控制、睡眠阻止列表、每 App 网络字节、许可证与更新。

> 不做的方式：tab 注册表与分区系统是数据驱动的，新增 tab 只加一个采集模块 + 一个前端组件 + 一条注册项，不改窗口骨架。

---

## 6. 架构

```
Meerkat/
  src-tauri/src/
    main.rs              应用装配：插件、窗口、托盘、采样循环
    state.rs             Snapshot 共享状态（RwLock<Arc<Snapshot>>）+ 设置
    metrics/
      mod.rs             Sampler 循环、tick 策略（AC/电池、窗口可见性）
      cpu.rs             每核、总体、进程 CPU、队列长度
      memory.rs          使用中/可用/待机/已修改/映射/提交 + 压力
      disk.rs            PDH 读写速率 + 每卷空间 + 累计积分
      network.rs         每适配器收发 + 累计 + 端口归属表
      gpu.rs             PDH GPU Engine（含 pid）+ DXGI 显存
      battery.rs         电源状态 + WMI + powercfg 报告
      sensors.rs         温度（尽力）+ 不可用时的显式状态
      process.rs         进程快照（含 IO 累计、创建时间、命令行）
    group/
      mod.rs             进程 → App 归组引擎
      icons.rs           图标提取与缓存
    history/
      mod.rs             分桶滚降存储 + 按天摘要 + 小时排名
    alerts/
      mod.rs             Worth a Look 规则 + 去重 + Toast
    ipc.rs               commands / events 契约
    tray.rs              多读数托盘管理
  src/
    i18n/{en,zh-Hans}.ts 资源表
    styles/              tokens.css（色/字重/圆角/动效）+ 布局
    components/          Sparkline / AreaChart / BarSet / Gauge / AppRow / Card / TabBar
    tabs/                Overview / Cpu / Memory / Disk / Network / Gpu / Battery
    panel/               面板 App
    settings/            General / Layout
```

数据流：`Sampler(1 个后台线程) → Snapshot → ①emit("metrics") 给可见窗口 ②按需 command snapshot() 拉取 ③写 history 桶`。
采样频率：面板/窗口可见 1Hz，全隐藏时 2s，电池供电再减半（对齐 Mac 版做法）。
托盘图标重绘：由面板 webview 离屏 canvas 生成 PNG 字节，经 `set_tray_bitmap` 下发给 `TrayIcon::set_icon`。

---

## 7. 真机验证结果（2026-10-08，TZZS-HOMEPC：Ultra 7 265K / 20 核 / 31.4 GiB / RTX 5060 Ti + Intel 核显 / Win11 build 28000）

跑通并核对到真实数值：

| 项 | 实测 |
|---|---|
| 进程→App 归组 | 424 进程 → 295 个 App；Chrome 29 个进程并成 1 行、Qoder CN 13 个进程 1 行 |
| App 名称 | 取版本信息 FileDescription，跟随系统语言：`任务管理器`、`微信输入法`、`Windows 资源管理器` |
| CPU | 总 33%、20 核逐核、user/priv 按整机归一（GetSystemTimes）、处理器队列长度 0 |
| 内存 | 使用中 26.7/31.4 GiB、压力 86%、已提交 90%、提交上限 59.6 GB、缓存、分页/非分页缓冲池 |
| 磁盘 | 读/写速率 + 今日累计（自算积分）、6 个卷、SSD 标识 |
| 网络 | 下载/上传速率、今日累计、133 条连接、3 个适配器（回环已排除） |
| GPU | 49% 利用率 + 每进程归因（DeltaForce 25.9% / Qoder 5.0% / Edge WebView2 0.55%）、2 块真实显卡 |
| 电池 | 台式机 → 明确显示「这台机器没有电池」，不伪造数据 |
| 温度 | 0 个传感器 → 显示「不可用」，绝不拿静态热区冒充（提权也拿不到，见上） |
| 开销 | 约 51 ms/tick（1 Hz）≈ 单核 5%；主进程 RSS 75 MB |

修掉的真实缺陷（都是验证时发现的，不是猜的）：
1. `PDH_MORE_DATA` 在 windows 0.62 元数据里是 `0x800007D2`，不是 pdh.h 的 `0x80000408` —— 用错常量导致 GPU 每进程利用率恒为 0。
2. `PdhGetFormattedCounterArrayW` 的取尺寸调用会返回实例数，但按 `items > 0` 提前返回空会让整张表读不出来。
3. PDH 值数组里每项含 f64，缓冲区必须 8 字节对齐；用 `Vec<u16>` 会读成垃圾。
4. `GlobalMemoryStatusEx` 的 user/priv 拆分要除以 `kernel + user` 总时间（kernel 已含 idle），否则 user+system 恒等于 100%、空闲段永远不出现。
5. DXGI 会把同一块物理显卡按不同 LUID 枚举两次，并按名字去重；`Microsoft Basic Render Driver`（vendor 0x1414）要过滤。
6. `ProductName` 在 Windows 11 上仍写「Windows 10 Pro」，按 build ≥ 22000 映射成 11。

未验证 / 已知限制：
- **托盘图标的实际像素**：托盘图标对象已创建（2 个读数 → 2 个 `tray_icon_app` 窗口），PNG 下发链路无报错，但任务栏区域截图取不到内容，**没有肉眼确认渲染效果**。
- 温度、风扇、每 App 网络字节、睡眠阻止列表：都需要管理员 helper，v1 明确不做。
- Sound / Bluetooth / Projects / Containers / 浏览器 tab 拆分 / 导出图片：按约定留到 v1 之后。

