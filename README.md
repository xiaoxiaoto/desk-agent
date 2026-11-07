# DeskAgent 架构设计

> **架构模型：分层架构 + 插件扩展架构 + 统一能力调用链**  
> 目标：构建跨 App、跨操作系统、跨模型、可通过 API/SDK 调用的通用桌面智能代理平台。

> 本文描述目标架构；[RPA 协议草案（中文）](docs/RPA_PROTOCOL.zh-CN.md) / [英文](docs/RPA_PROTOCOL.md) 定义目标任务协议和标准本地实现要求，**不代表现有实现已符合**。当前实现范围见文末；草案与现有接口的差异须在后续统一契约、接口、Schema 和验证时协调迁移。

项目名称为 **DeskAgent**，项目包和命令名称使用 `desk-agent`。从旧名称迁移时，配置环境变量改为 `DESK_AGENT_*`；默认数据库路径改为 `desk-agent.sqlite3`，已有数据库需通过 `DESK_AGENT_DB` 或 `DATABASE_URL` 显式指定，避免误认为旧数据已自动迁移。

---

## 1. 架构目标与设计原则

项目不应被设计成某个 App 的自动化脚本集合，而应定位为通用的 **Desktop Agent Platform / Runtime**。企业微信可以作为首个重点适配的应用，后续逐步扩展微信、钉钉、飞书、浏览器、邮件及其他桌面应用。

平台需要支持：

- **多种任务输入**：自然语言任务、精确能力指令、结构化工作流/API 调用。
- **多种执行通道**：官方 API、桌面 UI 自动化、浏览器自动化，以及后续接入的其他通道。
- **多种感知方式**：Accessibility、OCR、截图和支持视觉的大模型（VLM）。
- **多操作系统**：macOS、Windows、Linux。
- **多模型供应商**：LLM、VLM、OCR 服务可替换、可组合。
- **插件扩展**：App 插件、平台插件、模型 Provider 插件及第三方能力插件。
- **对外接口**：HTTP API、WebSocket、CLI、IPC，以及 Python / TypeScript SDK。

### 1.1 核心原则

1. Agent 负责理解、规划和选择能力，不直接调用 AppleScript、鼠标或键盘接口。
2. 所有可执行操作统一抽象为 Capability。
3. 所有 Capability 的执行必须经过 Runtime，不允许插件或 Agent 绕过 Runtime 直接执行受控操作。
4. App 业务语义与操作系统原语分离：`wecom.message.send` 不等于 `desktop.click`。
5. 插件通过明确的扩展点注册能力，不通过模块间随意互相引用实现扩展。
6. 通过 Ports / Traits 做依赖反转，核心业务不依赖具体 App、OS 或模型厂商。
7. 发送消息、删除数据、提交表单等有副作用的动作必须具备权限策略、必要的用户确认、审计和结果验证。
8. 初期优先采用模块化单体；接口稳定、确有独立部署或复用需求后再拆分 Crate 或进程。
9. **所有 Rust 源码统一放在项目根目录 `src/` 下。**

## 2. 总体架构：三种架构视角

```text
┌──────────────────────────────────────────────────────────────────┐
│                          外部接入层                               │
│             HTTP API / WebSocket / CLI / IPC / SDK                │
└───────────────────────────────┬──────────────────────────────────┘
                                │
                                ▼
┌──────────────────────────────────────────────────────────────────┐
│                         分层架构                                  │
│                                                                  │
│  接入层 → 应用用例层 → Agent 层 → 统一 Runtime 层                 │
│                                      │                           │
│                             Capability Registry                  │
│                                      │                           │
│                         Permission / Dispatcher                  │
│                                      │                           │
│                      Execute / Verify / Audit                    │
└──────────────────────────────────────┬───────────────────────────┘
                                       │
                                       ▼
┌──────────────────────────────────────────────────────────────────┐
│                       插件扩展架构                                │
│                                                                  │
│  App Plugins       Platform Plugins       Provider Plugins       │
│  WeCom             macOS                  LLM                     │
│  WeChat            Windows                VLM                     │
│  DingTalk          Linux                  OCR                     │
│  Feishu            Browser Engine         Third-party Plugins     │
└──────────────────────────────────────┬───────────────────────────┘
                                       │
                                       ▼
┌──────────────────────────────────────────────────────────────────┐
│                       统一能力调用链                              │
│                                                                  │
│  Request → Resolve → Authorize → Dispatch → Execute              │
│          → Verify → Audit → Response                              │
└──────────────────────────────────────────────────────────────────┘
```

这三种视角不是三套互相独立的系统：

- 分层架构定义代码组织和依赖方向。
- 插件架构定义扩展组件如何进入系统，以及如何注册能力。
- 统一能力调用链定义能力从被请求到执行完成的标准生命周期。

## 3. 一、分层架构

### 3.1 分层模型

建议使用以下逻辑层次。`kernel` 和 `domain` 为核心模型与契约，横切能力通过接口被各层使用，不构成允许任意依赖的“公共工具箱”。

```text
┌─────────────────────────────────────────────────────────────┐
│ L0 接入层 Interfaces                                         │
│ HTTP / WebSocket / CLI / IPC / SDK                           │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ L1 应用用例层 Application                                    │
│ TaskService / AgentService / CapabilityInvokeUseCase        │
│ AppManagement / Workflow                                     │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ L2 Agent 层 Agent                                            │
│ Parser / Planner / Reasoning / Memory / Selector             │
│ Natural Language Strategy / Exact Command Strategy          │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ L3 统一执行层 Runtime                                        │
│ Registry / Permission / Confirmation / Dispatcher           │
│ Retry / Timeout / Verification / Audit                       │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ L4 能力与适配器层                                            │
│ Generic Capabilities / App Adapters / Channel Adapters       │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ L5 基础设施与平台实现层                                      │
│ macOS / Windows / Linux / Browser / API Client / Database    │
│ LLM / VLM / OCR Providers / Filesystem / Network             │
└─────────────────────────────────────────────────────────────┘

横切关注点：Kernel Contracts、Domain Models、Config、Security、
Observability、Event Bus。
```

### 3.2 各层职责与边界

| 层 | 主要模块 | 负责什么 | 不负责什么 |
|---|---|---|---|
| L0 接入层 | `interfaces` | 接收外部请求、身份认证、协议转换、流式输出 | Agent 推理、直接操作桌面 |
| L1 应用用例层 | `application` | 创建任务、启动 Agent、查询能力、执行精确能力调用、取消任务 | 绑定某家模型或某个 OS 的具体实现 |
| L2 Agent 层 | `agent` | 意图理解、参数抽取、任务规划、能力选择、上下文管理 | 直接调用 AppleScript、鼠标或 App 私有 API |
| L3 统一执行层 | `runtime` | 能力解析、权限、确认、调度、执行控制、验证、审计 | 实现某个 App 的业务逻辑或 OS 原语 |
| L4 能力与适配器层 | `capabilities`、`applications` | 实现通用能力和 App 业务能力，选择具体执行通道 | 管理用户会话入口、承担总体任务规划 |
| L5 基础设施与平台层 | `platforms`、`providers`、`infrastructure` | 实现 OS 操作、模型调用、数据库、网络和文件等技术细节 | 决定业务任务应该如何规划 |

### 3.3 核心模型与 Ports

建议将稳定的抽象放在 `kernel`、`domain` 和 `application/ports`：

- `kernel`：能力协议、执行上下文、通用错误、事件、插件契约等基础抽象。
- `domain`：Task、AgentSession、Application、PermissionPolicy、ExecutionRecord 等领域对象。
- `application/ports`：由用例层定义对模型、能力注册表、App Adapter、平台、仓储和事件发布器的需求。

依赖方向遵循：**高层策略定义接口，底层实现接口；底层实现不得反向控制高层业务流程。**

### 3.4 依赖规则

| 模块 | 允许依赖 | 禁止或避免依赖 |
|---|---|---|
| `kernel` | 基础类型和通用库 | 任意具体 App、OS、Provider |
| `domain` | `kernel` | `interfaces`、具体 Adapter、Provider |
| `application` | `domain`、`kernel`、Ports | 具体 macOS / WeCom 实现 |
| `agent` | Agent 契约、Application 用例接口、模型 Port | 直接依赖平台鼠标键盘实现 |
| `runtime` | 能力契约、权限领域模型、执行 Port | 直接依赖某个 App 的内部实现 |
| `capabilities` | Capability 契约、平台 Port | Agent 内部推理模块 |
| `applications` | Capability 契约、App Adapter Port、通道接口 | 绕过 Runtime 执行受控动作 |
| `platforms` | Platform Port、`kernel` | Agent、App 业务用例 |
| `providers` | Model Port、`kernel` | App 业务流程 |
| `plugins` | Plugin 契约、Registry 接口 | 直接控制任务执行生命周期 |
| `interfaces` | Application 用例 | 直接调用 OS 自动化实现 |
| `bootstrap` | 所有待装配模块 | 放入业务规则 |

## 4. 二、插件扩展架构

### 4.1 插件分类

插件是系统扩展单元。一个插件可以注册一个或多个扩展点，但应声明清楚自身类型和依赖。

| 插件类型 | 示例 | 可提供的扩展 |
|---|---|---|
| App Plugin | WeCom、WeChat、DingTalk、Feishu | Application Adapter、App 专属 Capabilities、API/Desktop/Browser Channels |
| Platform Plugin | macOS、Windows、Linux | DesktopPlatform 实现、窗口/截图/输入能力实现 |
| Provider Plugin | 托管或本地 LLM/VLM/OCR 服务 | Model Provider、模型元数据和健康检查 |
| Capability Plugin | 文件处理、通用浏览器操作 | 一个或多个独立 Capability |
| Integration Plugin | CRM、工单或内部系统 | 外部系统连接器和业务能力 |

建议首期内置插件与外部插件共用同一套注册契约。第一阶段使用静态编译的 Built-in Plugin，后续再增加 WASM 或独立进程插件。不要在第一版就引入 Rust 动态库热加载，避免 ABI、版本兼容和崩溃隔离成本。

### 4.2 插件扩展点

```text
Plugin
 ├── ApplicationAdapter
 ├── CapabilityProvider
 ├── PlatformProvider
 ├── ModelProvider
 ├── ChannelProvider
 └── EventHandler（按需）
```

插件只能通过受控的 `PluginContext` 获取注册服务和基础设施接口，不应获得绕过 Runtime 的任意执行权限。

### 4.3 插件 Manifest

示例：企业微信插件。

```toml
id = "com.example.desktop-agent.wecom"
name = "WeCom Adapter"
version = "0.1.0"
plugin_type = "application"
api_version = "1"

[compatibility]
platforms = ["macos"]

[permissions]
required = ["app.wecom.access"]
optional = ["desktop.screenshot", "desktop.input"]

[capabilities]
provides = [
  "wecom.contact.search",
  "wecom.message.search",
  "wecom.message.send"
]

[dependencies]
plugins = []
```

Manifest 至少应包含：插件 ID、名称、版本、插件 API 版本、类型、兼容平台、依赖、能力声明、权限需求和配置 Schema。加载前必须验证 Manifest、版本、权限和依赖。

### 4.4 插件生命周期

```text
Discovered
    │
    ▼
Manifest Validation
    │
    ▼
Dependency Resolution
    │
    ▼
Permission Validation
    │
    ▼
Create Plugin Instance
    │
    ▼
Initialize(PluginContext)
    │
    ▼
Register Adapters / Capabilities / Providers
    │
    ▼
Health Check
    │
    ▼
Enabled
    │
    ▼
Disable / Unregister / Shutdown
```

生命周期要求：

- 插件加载失败不能导致整个 Runtime 无法启动，除非该插件被标记为必需依赖。
- 插件卸载时应先停止接收新请求，再等待或取消运行中的任务，最后注销能力并释放资源。
- 插件注销后，Registry 不应继续向新任务返回该插件的能力。
- 正在执行的任务应记录插件版本，保证审计和问题排查可追溯。
- 对不可信第三方插件，应采用 WASM 或独立进程隔离，并实施资源限制。

### 4.5 插件如何注册能力

```text
Bootstrap
   │
   ▼
PluginManager
   │
   ▼
Load Built-in / External Plugin
   │
   ▼
Plugin::initialize(PluginContext)
   │
   ├── register_application_adapter(...)
   ├── register_capability(...)
   ├── register_model_provider(...)
   └── register_platform_provider(...)
   │
   ▼
Capability Registry
   │
   ▼
Agent / API Discovery
```

Registry 应为每个 Capability 保存：稳定 ID、版本、Schema、所属插件、风险等级、权限要求、执行器引用、启用状态和元数据。

## 5. 三、统一能力调用链

### 5.1 Capability 是统一执行单位

无论用户输入的是自然语言还是精确指令，最终都应转换为一个或多个 Capability 调用。

能力 ID 建议使用稳定、可读、带命名空间的格式：

```text
desktop.screenshot
desktop.click
desktop.type_text
desktop.keyboard.press
browser.navigate
browser.click
wecom.contact.search
wecom.message.send
wechat.message.send
feishu.document.create
```

命名约定：`<namespace>.<resource>.<action>`。能力 ID 一旦对外发布，应谨慎变更；不兼容变更通过版本字段或新能力 ID 表达。

不要把所有 App 能力写进一个大型 Rust enum。使用 `CapabilityDefinition + JSON Schema + JSON Value`，支持插件动态注册和 API 扩展。

### 5.2 统一请求与响应

所有外部任务执行入口使用共享任务契约，而不是直接暴露 `capability_id` 调用。以下是协议草案中的目标请求示例，尚不能作为现有实现的调用保证：

```json
{
  "protocol_version": "1",
  "app": "wecom",
  "instructions": [{"action": "launch_app"}]
}
```

`app` 标识目标应用；未提供 `instructions` 时通过非空 `messages` 进入自然语言模式，同时提供时精确指令优先。精确模式保留步骤顺序、目标、参数和原文；自然语言意图或目标不明确时必须停止并澄清。能力 ID 与参数 Schema 用于 Runtime 内部解析和能力目录，不能替代统一外部任务请求。

`TaskRequest.app` 是唯一目标，不在指令参数中重复；必填 ID 为 1..128 个 ASCII 字符，匹配 `^[a-z][a-z0-9_.-]{0,127}$`。内部两种 `TaskInput` 模式均必须携带 app，无企业微信默认值，响应保留原请求 app。语法错误为 `invalid_request`；语法有效但不支持的目标在能力解析、授权和原生分发前返回 `capability_unavailable`，绝不回退企业微信。当前目录仅支持 `wecom`，企业微信自然语言短语只在该目标下解析。

任务动作统一为 `launch_app`、`center_app_window`、`detection_app_window_objects`、`click_app_element`、`inspect_app_element`、`input_app_element`，协议版本为 `"1"`。具体企业微信适配器方法及能力 ID、scope、授权目标和摘要保持不变。

精确指令 `{"action":"center_app_window"}` 对应能力 `wecom.window.center`，通过 macOS GUI 通道将已有企业微信窗口移至其所在屏幕的完整屏幕中心，不改变窗口大小；不会静默启动或切换应用。该操作依赖 macOS 辅助功能权限和可操作的已有窗口；不支持的平台或前置条件不满足时不可用或返回错误。执行完成不代表结果已独立验证，失败亦不得声称已验证；此声明不代表完整协议符合性或真实硬件验证。

GUI 通道要求恰有一个运行中的企业微信实例；只有一个辅助功能窗口时选择该窗口，存在多个窗口时选择辅助功能主窗口，无法确定主窗口时返回错误。按窗口与活动显示器的最大重叠面积选屏；面积并列或没有重叠显示器时返回错误。设置位置后回读辅助功能位置和大小，只有与目标位置及原大小分别相差不超过 1 点才返回成功；回读失败或不符时返回错误，但窗口可能已经移动。该回读不是独立的截图验证。

精确指令 `{"action":"capture_app_window"}` 或自然语言 `截取企业微信窗口` 通过 Runtime 的 `macos_gui` 通道调用能力及必需范围 `desktop.window.capture`，仅截取目标企业微信窗口。需要 macOS 屏幕录制和辅助功能权限、本地明确授权及恰好一个运行中的企业微信进程。只有一个辅助功能窗口时选该窗口，多个时选主窗口；须按 PID 和几何信息唯一匹配可见 Core Graphics 窗口，否则失败。不截取屏幕矩形或遮挡它的其他窗口，旧屏幕或窗口授权不能复用。返回值是 `~/.desk-agent` 下私有 `0700` 目录中唯一 `0600` PNG 文件的本地路径。截图属于敏感信息，不将截图内容写入日志；执行完成不代表已独立验证。

精确指令 `{"action":"detection_app_window_objects","params":{"objects":["搜索按钮","会话列表"]}}` 接受 1–16 个有序非空白字符串，每项最多 256 个 UTF-8 字节；不支持自然语言规划。它先截取新的目标窗口图像，再按顺序逐项将敏感图像发送到固定本地端点 `http://127.0.0.1/v1/chat/completions`，模型 `wevisone-vl-4b`，`wevisone_task` 为 `referring`，不会静默向远程服务披露。除截图授权外，还需要分别明确授权能力及范围 `vision.window.locate`，通道 `local_vision`，目标 `main_or_single_app_window`，效果 `local_inference`，参数 `objects-sha256:<digest>`，应用仍为 `wecom`。摘要是完整有序对象数组经 `serde_json` 紧凑序列化所得 UTF-8 字节的 SHA-256 小写十六进制值；每次截图和披露前重新检查对应授权。

结果包含 `capture_id`、`image_size: [width,height]` 和按查询顺序排列的 `objects: [{query, predictions: [{label, coords: [x1,y1,x2,y2]}]}]`；`coords` 保持原始截图像素，不自动添加屏幕坐标。发送前图像会缩放并编码为 JPEG，模型坐标再换算回原图像像素；缩放可能影响小目标识别。结果同时包含 `capture_path`、`captured_at_unix_ms`、`target` 和 `coordinate_space`。

截图操作也返回 `capture_id`。可独立提交精确指令 `{"action":"convert_app_window_coordinates","params":{"capture_id":"123e4567-e89b-12d3-a456-426614174000","boxes":[[10.0,20.0,30.0,40.0]]}}`，或 CLI `--instruction 'convert_app_window_coordinates={"capture_id":"123e4567-e89b-12d3-a456-426614174000","boxes":[[10.0,20.0,30.0,40.0]]}'`。ID 必须是规范小写 UUID；框为 1–100 个有限的原始图像像素 `[x1,y1,x2,y2]`，坐标在图片范围内且左上严格小于右下。只能使用 ID 对应的私有记录，不接受任意路径；记录 300 秒后过期，旧截图没有记录不能转换。目标窗口、显示器或几何发生变化时拒绝执行；操作不会截图、调用模型、请求屏幕权限或点击。

转换需要单独明确授权：应用 `wecom`，能力及范围 `desktop.window.coordinates.convert`，通道 `macos_gui`，目标 `main_or_single_app_window`，效果 `read_geometry`，参数 `conversion-sha256:<digest>`。摘要为字段按 `capture_id`、`boxes` 顺序的结构经 `serde_json` 紧凑 UTF-8 序列化后的 SHA-256 小写十六进制值；数值采用 serde_json 的 f64 表示（如 `1.0`）。派发前重新检查授权。

例如，将下面的 `capture_id` 和框替换成新截图返回的真实值；数字须按 `f64` 的 `serde_json` 格式书写：

```sh
params='{"capture_id":"123e4567-e89b-12d3-a456-426614174000","boxes":[[10.0,20.0,30.0,40.0]]}'
digest=$(printf '%s' "$params" | shasum -a 256 | cut -d ' ' -f 1)
sqlite3 "$HOME/.desk-agent/security.sqlite3" "INSERT OR IGNORE INTO local_capability_grants VALUES ('wecom','desktop.window.coordinates.convert','macos_gui','main_or_single_app_window','read_geometry','conversion-sha256:$digest');"
desk-agent-cli rpa --app wecom --instruction "convert_app_window_coordinates=$params"
```

完成后可按相同摘要删除对应授权；不要把授权扩大到其他截图或检测框。

转换返回 `capture_id`、`window_bounds_points: [x,y,width,height]`、`screen_coordinate_space: "macos_global_top_left_points_at_capture"` 和按输入顺序的 `boxes: [{image_bounds_pixels, screen_bounds_points, screen_center_points}]`。屏幕坐标为截图时 macOS 全局左上角原点的逻辑点，可为负；各轴按窗口点数尺寸除以截图像素尺寸缩放，不假设 Retina 倍率。窗口跨多个显示器、显示器归属不明确或截图几何无法对应时失败。坐标只是快照，不保证当前界面：几何一致也不能排除移动后移回或检查后变化。预测不可信，转换结果不是行动授权；点击前必须独立重新验证目标和当前界面。

独立精确指令 `{"action":"click_app_element","params":{"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}}` 是受限的企业微信业务操作，不是任意桌面点击，也不支持自然语言规划。CLI 可用 `--instruction 'click_app_element={"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}'`。必须提供 macOS 全局左上原点逻辑点表示的有限坐标 `[x,y]`，以及由调用方识别的准确非空白 AXRole（不超过 256 UTF-8 字节）。`expected_role` 须精确匹配实际角色，不会把 `AXCell` 当成 `AXButton`。`expected_label` 可选：不超过 256 UTF-8 字节的非空白字符串选择语义名称验证；省略或 `null` 明确选择坐标加角色身份模式。空串或纯空白字符串无效，不会切换模式。
名称模式中，`expected_label` 按确定顺序匹配可访问名称：优先使用元素自身非空 `AXTitle`；只有自身标题缺失或为空时，才通过标准 `AXTitleUIElement` 关系读取同一 PID、同一窗口内的 `AXStaticText` 标题元素，优先其非空 `AXTitle`，否则使用其非空字符串 `AXValue`。自身非空标题不匹配时立即失败，不改用关系标签。不会使用 `AXDescription`、任意子节点、未读数、图标描述或猜测的文字；角色或名称不可用时拒绝执行，绝不静默退回坐标模式。
仅坐标模式示例：`{"action":"click_app_element","params":{"point":[100.0,200.0],"expected_role":"AXCell"}}`；CLI 使用 `--instruction 'click_app_element={"point":[100.0,200.0],"expected_role":"AXCell"}'`，显式 `"expected_label":null` 等价。此模式不读取标题或名称，也不声称该点是“消息”、正确的消息或任何语义目标。调用方须独立审阅截图/当前界面并明确批准精确点位和角色；模型预测与应用内容不可信，不能授权。已观察到的无名称“消息” `AXCell` 仍不符合名称模式；坐标模式仅授权批准的点位与角色，不推断消息身份，也不证明已完成真实“消息”点击。不能保证任意元素都支持所需状态变化。操作本身不截图、不需要截图记录或屏幕录制权限。
每个坐标必须能精确表示为 f32（例如整数点）；操作不会自动舍入坐标，也不会猜测像素到点的比例。

需要本机 macOS 辅助功能权限。两种模式均要求坐标位于实时唯一选定、可见、不透明且处于前台的企业微信窗口内。`macos_gui` 使用系统全局 AX 命中测试精确确认目标 PID、窗口和角色；名称模式另核对可访问名称。在派发原生 CGEvent 鼠标按下/抬起之前，立即重复确认同一元素与未变化的窗口几何。不会仅因其他 Core Graphics 窗口的矩形覆盖该点就认定遮挡，也不会按进程名豁免；命中其他进程或窗口仍以 `point_obscured` 或 `target_element_mismatch` 拒绝。不提供辅助功能命中信息的覆盖层可能无法被此检查发现，因此这不是对所有可视遮挡的保证。无法完成所需检查时拒绝执行，不静默启动或激活应用、不点击目标应用外部、不发送文本，也不绕过派发前验证。
前台应用身份通过 NSWorkspace 查询，并与该进程的 AXFocusedWindow 和所选窗口独立核对；任何查询失败仍停止。CGEvent 派发后最多观察 500 毫秒的焦点/选中状态变化，只读观察不会再次点击。
另需本地明确的一次性授权，绑定应用 `wecom`、能力及范围 `wecom.element.activate`、通道 `macos_gui`、目标 `main_or_single_app_window`、效果 `activate_element`、精确坐标和预期角色及身份模式/名称的参数 `activate-v3-sha256:<digest>`。摘要为字段按 `point`、`expected_role`、`expected_label` 顺序的结构经 `serde_json` 紧凑 UTF-8 序列化后的 SHA-256 小写十六进制值；最后一个字段始终保留，省略或 `null` 标签均写为 `null`。例如坐标模式规范参数为 `{"point":[100.0,200.0],"expected_role":"AXCell","expected_label":null}`。坐标使用 serde_json f64 表示（如 `1.0`），参数不改写。`v3` 绑定明确身份模式策略；所有旧授权（包括 `activate-v2-sha256:` 和 `activate-sha256:`）均拒绝。原有语义名称请求仍可使用，但须重新明确授予 v3 授权。预检后、派发前立即原子消费授权并审计，截图或坐标转换授权不可复用。

授权后的激活预检失败可在 `capability_unavailable` 错误中给出可选固定 `diagnostic_code`，阶段仍为 `validation`，结果为 `not_dispatched`，`retryable` 为 false。代码不包含实际角色、标题、标签、坐标、截图或原始系统错误；不会推断遮挡，不适用于派发后的失败，也不能追溯诊断过去的记录。完整代码及含义见 [RPA 协议](docs/RPA_PROTOCOL.zh-CN.md)。

独立精确指令 `inspect_app_element` 使用与点击相同的坐标、预期角色和可选名称，按选定模式只读检查当前状态，不派发输入或消费点击授权。CLI 名称模式使用 `--instruction 'inspect_app_element={"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}'`，坐标模式使用 `--instruction 'inspect_app_element={"point":[100.0,200.0],"expected_role":"AXCell"}'`（显式 `null` 等价）。它需要独立本地明确授权，绑定应用 `wecom`、能力和范围 `wecom.element.diagnose`、通道 `macos_gui`、目标 `main_or_single_app_window`、效果 `inspect_element` 和 `diagnose-v3-sha256:<digest>`，摘要按点击相同的参数顺序和序列化生成，始终包含 `expected_label`（含 `null`）。所有旧授权（包括 `diagnose-v2-sha256:` 和 `diagnose-sha256:`）均拒绝，原有名称请求须重新授予 v3 授权。匹配时返回 `{"diagnostic_code":"matched"}`，检查失败时返回固定诊断代码；当前观察不证明消息身份，也不构成后续点击授权。

派发后须在同一目标上观察到真实 `AXFocused` 或 `AXSelected` 从 false 变为 true；原本已聚焦/选中或状态不变不能证明成功。证据缺失或不变、派发失败、效果发生后审计失败均为结果未知，不报告已验证，也绝不自动重试已派发操作。成功返回 `{"clicked":true}`；独立协议验证状态仍为 `not_performed`，不声称识别了正确消息。

独立精确指令 `{"action":"input_app_element","params":{"point":[492.0,203.0],"text":"张三"}}` 先获取企业微信页头搜索框焦点，再把该字段的整个值替换为文本；CLI 为 `--instruction 'input_app_element={"point":[492.0,203.0],"text":"张三"}'`，不支持自然语言规划。`point` 为有限、可精确表示为 f32 的 macOS 全局左上原点逻辑屏幕点，不是图片像素；敏感 `text` 必须为 1..256 UTF-8 字节、非空白、不含 CR/LF 或 Unicode 控制字符，原文不裁剪、不改写、不追加。不会输入聊天编辑器、按 Enter、发送内容或产生鼠标/键盘事件；`SearchContact` / `search_contact` 仍不可用，不声称匹配到联系人。

该选择器仅适用于当前支持的企业微信 GUI，不是通用文本框支持。点位须重复命中同一前台可见不透明唯一选定窗口内的启用 `AXTextField`；其边界完全位于窗口顶部 80 逻辑点页头带内，宽度大于零且高度不超过 60 点。严格祖先链为 `AXTextField → AXSplitGroup → AXSplitGroup → 选定 AXWindow`，并核对各级直接子节点归属；页头带内如存在其他 `AXTextField`（例如聊天输入框），必须由直接子级的垂直 `AXSplitter` 把目标与它们分到不同列，且目标必须是该分割线左侧唯一页头文本框；缺失分割线或同侧歧义均拒绝。相关直接子节点列表超过 2048 项时拒绝，不遍历整个窗口树；没有标题、标识符、描述、子角色或占位文字的伪造/猜测回退。需要辅助功能权限，不依赖截图或屏幕录制权限。

授权独立绑定应用 `wecom`、能力及范围 `wecom.search.query.set`、通道 `macos_gui`、目标 `wecom_search_field`、效果 `set_search_query` 和 `search-query-sha256:<digest>`。摘要是字段严格按 **`point` 然后 `text`** 顺序的紧凑 UTF-8 `serde_json` 结构体序列化的小写 SHA-256；f64 使用 serde_json 归一化，例如 `{"point":[492.0,203.0],"text":"张三"}`，不包含 action 外壳。以下仅使用非敏感示例；真实文本不得留在 shell 历史、跟踪或普通日志中：

```sh
digest=$(printf '%s' '{"point":[492.0,203.0],"text":"张三"}' | shasum -a 256 | cut -d ' ' -f 1)
sqlite3 "$HOME/.desk-agent/security.sqlite3" "INSERT OR IGNORE INTO local_capability_grants VALUES ('wecom','wecom.search.query.set','macos_gui','wecom_search_field','set_search_query','search-query-sha256:$digest');"
desk-agent-cli rpa --app wecom --instruction 'input_app_element={"point":[492.0,203.0],"text":"张三"}'
```

这是单次授权，不复用截图或点击授权；普通日志及审计仅记录摘要，不记录 text、旧值或回读内容。目录独立检查授权和审计存储，原生前置条件仅为建议性状态。先授权再预检，预检成功后在分发前复查授权，并原子消费授权及提交意图审计。适配器通过可写 `AXFocused` 聚焦，确认字段已聚焦且应用 `AXFocusedUIElement` 精确为目标，写入前立即复查焦点、窗口及几何，再以可写 `AXValue` 完整替换。之后仅做最长 500ms 的有界只读精确回读并确认同一活动应用/窗口。成功的能力 `output` 仅为 `{"text_input":true}`，不回显 text；共享 HTTP/IPC `TaskResponse` 仍按精确契约在步骤的原始 `instruction` 中保留 point/text，授权调用方及 CLI 可看到自己提交的参数，不应将完整响应写入普通日志。独立验证为 `not_performed`，AX 回读不是独立截图证据。任何已派发失败（含可能聚焦/写入后或结果审计失败）均为 `unknown`，立即停止，绝不自动重试。详细契约见[协议](docs/RPA_PROTOCOL.zh-CN.md#受控的企业微信搜索字段文本输入)。


IPC 客户端对该精确视觉指令最多等待 1500 秒；等待超时或连接中断不代表服务端已停止，不能据此盲目重试。

响应使用共享 `TaskResponse` 和 `TaskExecution`，记录请求、任务、操作与追踪关联，以及有序步骤、执行状态和独立验证状态。`completed` 不等于 `verified`；适配器成功提交不能作为独立验证证据。结果不确定时报告 `unknown`，不得盲目重试。错误使用共享 `TaskErrorResponse`，包含稳定错误码和请求关联，不泄露敏感信息。具体字段、错误和版本规则以协议草案为准；草案示例并非现有实现承诺。

### 5.3 统一执行生命周期

```text
Capability Request
       │
       ▼
1. Normalize Request
   规范化请求、身份和执行上下文
       │
       ▼
2. Resolve Capability
   Registry 根据 capability_id 查找定义和执行器
       │
       ▼
3. Validate Arguments
   JSON Schema 校验、类型检查、引用解析
       │
       ▼
4. Permission & Risk Check
   用户权限、插件权限、风险等级和确认策略
       │
       ▼
5. Select Execution Route
   API / Desktop / Browser 等通道选择
       │
       ▼
6. Dispatch & Execute
   超时、取消、并发限制、重试策略
       │
       ▼
7. Verify Result
   检查返回值和实际状态是否达到目标
       │
       ▼
8. Audit & Emit Events
   记录请求摘要、执行者、插件版本、结果和耗时
       │
       ▼
9. Return Standard Response
```

Runtime 是该链路的唯一入口。Agent、HTTP API、SDK、工作流引擎和插件内部触发的受控动作，均应调用同一 `CapabilityInvoker`。

### 5.4 通道选择与降级

App Capability 与具体执行通道分离。例如 `wecom.message.send` 可以由不同 Channel 实现：

```text
wecom.message.send
       │
       ▼
WeCom Adapter
       │
       ▼
Channel Selector
       ├── Official API Channel
       ├── Desktop UI Channel
       └── Browser Channel（仅在确实支持时）
```

通道选择应考虑：

- 当前 App 是否已登录、连接是否健康。
- 当前能力是否由该通道支持。
- 用户是否授权该通道及相关权限。
- 通道稳定性、速率限制和成本。
- 操作风险及用户指定的执行约束。

降级必须显式、可审计。不能因为 API 调用失败就无条件改用桌面 UI 执行敏感动作；若执行通道发生变化，应重新校验权限与风险策略。

### 5.5 三种输入如何汇入同一调用链

#### A. 自然语言任务

```text
用户自然语言
    → Parser
    → Planner
    → Capability Selector
    → 结构化 Action Plan
    → Runtime::CapabilityInvoker
    → Capability 执行
    → 结果验证
    → Agent 汇总任务结果
```

示例：“在企业微信里给张三发送下午三点开会。”

Agent 可以将任务规划为：

```json
{
  "steps": [
    {
      "id": "step-1",
      "capability_id": "wecom.contact.search",
      "arguments": {"keyword": "张三"}
    },
    {
      "id": "step-2",
      "capability_id": "wecom.message.send",
      "arguments": {
        "recipient_id": "$ref:step-1.data.contact_id",
        "content": "下午三点开会"
      },
      "depends_on": ["step-1"]
    }
  ]
}
```

上述计划仅为 Agent 内部规划示意，不是对外请求或已实现的工作流格式。Runtime 负责校验计划和执行顺序；联系人等目标有歧义时必须先澄清，不得擅自发送。

#### B. 精确指令任务

```text
HTTP / SDK / CLI / Desktop
    → 共享 TaskRequest
    → Application 用例 → Runtime::CapabilityInvoker
    → Capability Registry
    → 权限检查
    → 执行器
    → 验证 / 审计
    → 标准响应
```

精确指令表达稳定的用户业务操作，不将 `desktop.screenshot`、鼠标或键盘等 OS 原语作为公开指令。精确模式可以跳过自然语言解析，但不能跳过 Runtime 的校验、权限、确认、验证和审计。

#### C. 视觉自动化任务

```text
Screenshot / Accessibility Tree
    → Perception（OCR + VLM + Accessibility）
    → UI State
    → Agent Planner
    → desktop.click / desktop.type_text 等 Capability
    → Runtime
    → Platform Adapter
    → 新截图 / 新 UI State
    → Verification
    → 成功返回或继续下一轮观察
```

视觉任务采用 Observe → Understand → Plan → Act → Verify 循环。VLM 负责理解界面，不应直接拥有系统输入权限。坐标、控件标识和窗口上下文应携带来源与时间信息，避免使用过期视觉状态执行动作。

## 6. 统一接口草案

以下代码用于表达边界，正式实现时需要统一项目错误类型、异步运行时、取消令牌和序列化约定。

### 6.1 Capability

```rust
#[async_trait::async_trait]
pub trait Capability: Send + Sync {
    fn definition(&self) -> CapabilityDefinition;

    async fn execute(
        &self,
        ctx: &ExecutionContext,
        args: serde_json::Value,
    ) -> Result<CapabilityOutput, CapabilityError>;
}
```

### 6.2 CapabilityInvoker

```rust
#[async_trait::async_trait]
pub trait CapabilityInvoker: Send + Sync {
    async fn invoke(
        &self,
        request: CapabilityRequest,
    ) -> Result<CapabilityResponse, InvocationError>;
}
```

### 6.3 Application Adapter

```rust
#[async_trait::async_trait]
pub trait ApplicationAdapter: Send + Sync {
    fn app_id(&self) -> &'static str;

    async fn initialize(
        &self,
        ctx: &PluginContext,
    ) -> Result<(), PluginError>;

    async fn health_check(&self) -> Result<HealthStatus, PluginError>;

    async fn shutdown(&self) -> Result<(), PluginError>;
}
```

### 6.4 Platform Port

```rust
#[async_trait::async_trait]
pub trait DesktopPlatform: Send + Sync {
    async fn screenshot(
        &self,
        request: ScreenshotRequest,
    ) -> Result<Screenshot, PlatformError>;

    async fn click(
        &self,
        request: ClickRequest,
    ) -> Result<(), PlatformError>;

    async fn type_text(
        &self,
        text: &str,
    ) -> Result<(), PlatformError>;

    async fn press_key(
        &self,
        key: Key,
    ) -> Result<(), PlatformError>;
}
```

### 6.5 Plugin

```rust
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;

    async fn initialize(
        &mut self,
        ctx: PluginContext,
    ) -> Result<(), PluginError>;

    async fn shutdown(&mut self) -> Result<(), PluginError>;
}
```

### 6.6 Model Provider

```rust
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn generate(
        &self,
        request: LlmRequest,
    ) -> Result<LlmResponse, ModelError>;
}

#[async_trait::async_trait]
pub trait VlmProvider: Send + Sync {
    async fn understand_image(
        &self,
        request: VlmRequest,
    ) -> Result<VlmResponse, ModelError>;
}
```

## 7. 推荐 Rust 项目目录

所有 Rust 源码位于根目录 `src/`。用户要求按此目录图预建目录；目录建立不代表对应模块已实现，源码文件和功能仍按实施阶段增加。

```text
desk-agent/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── README.md
├── .env.example
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── kernel/
│   │   ├── mod.rs
│   │   ├── capability.rs
│   │   ├── context.rs
│   │   ├── error.rs
│   │   ├── event.rs
│   │   ├── plugin.rs
│   │   └── types.rs
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── task.rs
│   │   ├── agent_session.rs
│   │   ├── application.rs
│   │   ├── permission.rs
│   │   └── execution.rs
│   ├── application/
│   │   ├── mod.rs
│   │   ├── task/
│   │   ├── agent/
│   │   ├── capability/
│   │   ├── app_management/
│   │   └── ports/
│   ├── agent/
│   │   ├── mod.rs
│   │   ├── parser/
│   │   ├── planner/
│   │   ├── reasoning/
│   │   ├── selector/
│   │   ├── memory/
│   │   └── strategies/
│   ├── runtime/
│   │   ├── mod.rs
│   │   ├── invoker.rs
│   │   ├── registry.rs
│   │   ├── dispatcher.rs
│   │   ├── permission.rs
│   │   ├── confirmation.rs
│   │   ├── verification.rs
│   │   └── audit.rs
│   ├── perception/
│   │   ├── mod.rs
│   │   ├── screenshot.rs
│   │   ├── accessibility.rs
│   │   ├── ocr.rs
│   │   ├── vlm.rs
│   │   └── fusion.rs
│   ├── capabilities/
│   │   ├── mod.rs
│   │   ├── desktop/
│   │   ├── browser/
│   │   └── system/
│   ├── applications/
│   │   ├── mod.rs
│   │   ├── common/
│   │   ├── wecom/
│   │   │   ├── mod.rs
│   │   │   ├── plugin.rs
│   │   │   ├── adapter.rs
│   │   │   ├── capabilities/
│   │   │   └── channels/
│   │   ├── wechat/
│   │   ├── dingtalk/
│   │   ├── feishu/
│   │   ├── browser/
│   │   └── email/
│   ├── platforms/
│   │   ├── mod.rs
│   │   ├── common/
│   │   ├── macos/
│   │   ├── windows/
│   │   └── linux/
│   ├── providers/
│   │   ├── mod.rs
│   │   ├── llm/
│   │   ├── vlm/
│   │   └── ocr/
│   ├── plugins/
│   │   ├── mod.rs
│   │   ├── manager.rs
│   │   ├── loader.rs
│   │   ├── manifest.rs
│   │   ├── lifecycle.rs
│   │   └── builtin.rs
│   ├── infrastructure/
│   │   ├── mod.rs
│   │   ├── database/
│   │   ├── filesystem/
│   │   ├── network/
│   │   ├── cache/
│   │   └── event_bus/
│   ├── interfaces/
│   │   ├── mod.rs
│   │   ├── http/
│   │   ├── websocket/
│   │   ├── cli/
│   │   └── ipc/
│   ├── config/
│   ├── observability/
│   └── bootstrap/
│       ├── mod.rs
│       ├── container.rs
│       ├── register.rs
│       └── startup.rs
├── config/
├── plugins/
├── migrations/
├── sdk/
│   ├── python/
│   └── typescript/
├── tests/
│   ├── unit/
│   ├── integration/
│   └── e2e/
├── examples/
├── scripts/
└── docs/
    ├── architecture.md
    ├── plugin-development.md
    ├── capability-spec.md
    ├── call-flow.md
    └── api-reference.md
```

## 8. 对外 API 与协议

| HTTP 入口 | 目标职责 |
|---|---|
| `POST /api/v1/rpa` | 唯一任务操作入口；任务执行、取消、审批和恢复通过共享任务契约与 Runtime 调用链处理。后几类操作的具体请求形式仍待协调定义，尚不代表已实现。 |
| `GET /api/v1/capabilities` | 只读能力目录，展示稳定能力 ID、指令、Schema、通道、权限、实际可用性及不可用原因；不分发任务。 |

不为单个 App、Capability 或任务控制动作增加独立执行路由。任务查询和事件访问的接口契约仍待协调定义，不能把未来设计写成现有路由。CLI、桌面、IPC 和未来接口构造或接受共享任务契约，不引入各自的任务或状态词汇。

任务提交包含 `protocol_version`、目标 `app` 及 `messages` 或 `instructions`；当前同步任务路径已采用该请求形状。草案还要求逐任务授权等安全保障，当前实现尚未全部完成。新增指令或行为须同步共享 Rust 类型、Runtime、受影响接口、能力目录、OpenAPI、JSON Schema、协议和行为验证。

## 9. 安全与可靠性设计

### 9.1 权限分级

建议至少定义以下风险等级：

| 风险等级 | 示例 | 默认策略 |
|---|---|---|
| Read | 截图、读取联系人或消息（受数据权限约束） | 授权后允许，记录审计 |
| Low Risk Write | 输入草稿、创建未提交内容 | 按策略允许或确认 |
| High Risk Write | 发送消息、提交表单、创建外部事件 | 默认要求明确授权或用户确认 |
| Destructive | 删除记录、批量操作、不可逆变更 | 强确认、范围预览、审计；必要时禁止自动执行 |

风险等级不能只由模型输出决定，应由能力定义、参数、目标对象、用户策略和运行环境共同计算。

### 9.2 视觉操作安全

- 操作前确认目标窗口、应用身份和焦点状态。
- 点击前检查坐标或控件引用是否仍对应最新界面状态。
- 对发送、删除、支付、提交等操作增加二次验证或确认。
- 截图、OCR 和模型输入可能包含敏感信息，应做权限控制、日志脱敏和数据保留策略。
- 不把 VLM 识别结果当成绝对事实；关键操作应通过 UI 状态或 App 返回结果验证。

### 9.3 重试与幂等

超时、取消或断连不证明已分发的副作用未发生。`unknown` 必须先核对，不得盲目重试；幂等键只能按约定去重请求，不能单独保证外部效果不重复。重试须结合能力特定安全保证、证据、当前授权和有限预算；通道变化须重新检查策略和适用审批。

### 9.4 协议草案的持久性与安全边界

目标协议区分所有实现必须满足的 Core 与提供事件重放或恢复时要求的 Durable；通过验收前不得声明符合。标准本地实现目标为单用户 Rust Runtime、认证的本地 IPC、可选 loopback HTTP 和 SQLite WAL；跨机器协调不在该配置的保证范围内。草案规定任务、步骤、尝试、事件、审计、审批、幂等、检查点和租约等持久记录，以及版本比较与 fencing token。

副作用前需持久记录意图及策略信息；SQLite 事务不能与外部 GUI 或 API 效果原子提交。分发后、结果持久化前崩溃时必须核对未知结果，不能声称 exactly-once。事件重放需要按任务授权、去重、游标和缺口报告；普通日志和审计默认不保存消息、截图或凭证。敏感证据须单独保护并遵守访问、保留和删除策略。这些均为目标要求，非当前实现能力声明。

## 10. 分阶段实施路线

| 阶段 | 目标 | 交付物与验收标准 |
|---|---|---|
| P0 架构骨架 | 建立 Kernel、Domain、Ports、Bootstrap | 可编译；模块依赖方向明确；基础错误和上下文统一 |
| P1 统一能力 Runtime | 实现 Capability Definition、Registry、Invoker | 能注册、发现、校验并调用一个 Mock Capability |
| P2 精确指令 API | 实现共享任务契约和 HTTP / SDK 接入 | 业务指令经 `POST /api/v1/rpa` 和 Runtime 执行，返回共享任务响应与关联信息；不公开 OS 原语执行入口 |
| P3 macOS 平台适配 | 实现截图、窗口、键鼠、Accessibility 接口 | Runtime 能通过 Platform Port 获取真实截图并执行受控输入 |
| P4 感知模块 | 接入 OCR、VLM、Accessibility 融合 | 输出带坐标/控件信息和置信度的 UI State |
| P5 企业微信插件 | 实现 WeCom Adapter 和首批业务能力 | 联系人搜索、消息读取、发送能力可发现、可执行、可验证 |
| P6 Agent 自然语言 | 实现 Parser、Planner、Selector 和任务执行 | 自然语言任务能生成计划并通过 Runtime 执行 |
| P7 插件管理 | Manifest、依赖检查、生命周期和启停 | 插件可初始化、注册、健康检查、注销和审计 |
| P8 SDK 与事件流 | Python / TypeScript SDK、任务事件访问 | 外部程序可按统一契约提交任务；事件与取消能力只有完成协议、授权及验证后才能声明支持 |
| P9 多 App / 多 OS | 接入微信、飞书、钉钉、Windows、Linux | 新增平台或 App 时不修改 Agent 与 Runtime 核心流程 |
| P10 外部插件隔离 | WASM 或独立进程插件运行时 | 第三方扩展具备权限隔离、资源限制和版本兼容机制 |

### 10.1 第一条端到端验收链路

先以可用的应用业务指令验证平台骨架，不将截图或鼠标键盘原语暴露为任务指令：

```text
POST /api/v1/rpa
    → 共享 TaskRequest → Application 用例
    → Runtime::CapabilityInvoker → Schema 校验 / 授权 / 适用确认
    → 显式路由与 Capability 执行 → 验证 / 审计
    → 共享 TaskResponse 或 TaskErrorResponse
```

验收应覆盖精确指令原样传递、能力目录中的真实可用性、权限和确认、运行时分发、执行与验证状态分离，以及错误和关联 ID。涉及外部副作用时，必须覆盖分发后断连、结果未知和禁止盲重试；不以 Mock 调用或提交成功替代真实路径及验证证据。

## 11. 架构验收标准

最终架构应能通过以下问题验证：

- 新增一个 App（例如飞书），是否只需新增 App Plugin 和对应能力，而不修改 Agent / Runtime 核心代码？
- 新增一个操作系统（例如 Windows），是否只需实现 Platform Port，而不修改 App 业务能力？
- 更换 LLM 或 VLM 供应商，是否只需替换 Provider？
- 自然语言、HTTP 精确调用和工作流调用，是否都进入同一个 Runtime 执行入口？
- 任何有副作用的动作，是否都经过权限检查、确认策略、审计与结果验证？
- 插件禁用或卸载后，Registry 是否能阻止新的调用，并妥善处理正在运行的任务？
- 是否能从一次执行记录追踪请求、计划、能力、插件版本、执行通道和验证结果？

只要上述问题的答案稳定为“是”，系统就具备了可扩展的通用 Desktop Agent Platform 基础。

---

## 12. 最终架构结论

**分层架构**确定系统内部的职责与依赖；**插件扩展架构**让 App、OS、模型和第三方能力能够按统一契约接入；**统一能力调用链**让所有操作以 Capability 为单位，经由 Runtime 完成授权、调度、执行、验证和审计。

建议始终保持这条主路径：

```text
用户 / API / SDK
      │
      ▼
Interface → Application →（Agent：可选）
      │
      ▼
Runtime::CapabilityInvoker
      │
      ├── Registry
      ├── Permission / Confirmation
      ├── Dispatcher / Channel Selector
      ├── Execution / Retry / Timeout
      ├── Verification
      └── Audit / Events
      │
      ▼
Plugin Capability
      │
      ├── App Adapter
      ├── Platform Adapter
      └── Provider Adapter
      │
      ▼
外部 App / OS / Model / Service
```

**关键约束：任何执行入口都不能绕过 `Runtime::CapabilityInvoker`。** 这是整个产品保持统一权限、安全控制、执行记录和扩展能力的核心边界。

---

## Repository implementation status

The repository is a single Cargo package with daemon, CLI, and desktop entry points, health/system/capability discovery APIs, and a shared RPA request (`protocol_version`, `app`, `messages`, optional typed `instructions`) across HTTP, CLI, desktop, and Runtime IPC. WeCom launch and macOS window centering are implemented; centering requires a specific local grant, audit storage and GUI prerequisites. HTTP task submission requires the local macOS Keychain bearer credential; if the credential store is unavailable, HTTP task requests are rejected. IPC uses length-prefixed JSON over a user-restricted Unix socket.

唤醒企业微信可通过共享任务请求发送精确指令 `{"protocol_version":"1","app":"wecom","instructions":[{"action":"launch_app"}]}`，或使用自然语言消息 `唤醒企业微信`。例如先启动 `desk-agent`，再运行 `desk-agent-cli rpa --app wecom --instruction launch_app`。macOS 上该能力使用 `open -a 企业微信`；命令成功只表示启动请求已提交，不代表窗口已独立验证或用户已登录。

窗口居中指令为 `center_app_window`，自然语言为 `将企业微信窗口居中`。需要 macOS 辅助功能权限、可移动的企业微信主窗口或唯一窗口，以及本地明确授权。启动 Runtime 一次创建 `~/.desk-agent/security.sqlite3` 后，由当前用户授权：

```sh
sqlite3 "$HOME/.desk-agent/security.sqlite3" "INSERT OR IGNORE INTO local_capability_grants VALUES ('wecom','wecom.window.center','macos_gui','main_or_single_wecom_window','move_without_resize','none:v1');"
```

窗口截图需另行明确授权，范围为能力 `desktop.window.capture`、目标 `main_or_single_app_window`、效果 `capture`、参数 `none:v1`；先前屏幕或窗口授权不能复用：

```sh
sqlite3 "$HOME/.desk-agent/security.sqlite3" "INSERT OR IGNORE INTO local_capability_grants VALUES ('wecom','desktop.window.capture','macos_gui','main_or_single_app_window','capture','none:v1');"
```

可用 `desk-agent-cli rpa --app wecom --instruction capture_app_window` 或自然语言 `截取企业微信窗口` 截取目标窗口。成功时步骤的 `output.path` 指向 `~/.desk-agent/captures/` 中的私有 PNG；截图包含敏感内容，使用者负责保留和删除。运行进程需要 macOS「屏幕录制」和「辅助功能」权限及运行中的企业微信窗口。

已获本地截图授权的请求在缺少屏幕录制权限时会调用 macOS 授权接口；是否显示弹窗由系统决定，用户仍须手动授权，可能需要在系统设置中授权并重启 Runtime。能力目录查询不会触发授权弹窗。

对象定位除上述截图授权外还需按精确对象数组单独授权；数组顺序或内容变化即需新的摘要和授权。例如 ASCII 数组 `["search button","chat list"]` 的摘要可用以下命令计算，然后仅在同意将截图发送到指定本地视觉服务时授予：

```sh
digest=$(printf '%s' '["search button","chat list"]' | shasum -a 256 | cut -d ' ' -f 1)
sqlite3 "$HOME/.desk-agent/security.sqlite3" "INSERT OR IGNORE INTO local_capability_grants VALUES ('wecom','vision.window.locate','local_vision','main_or_single_app_window','local_inference','objects-sha256:$digest');"
desk-agent-cli rpa --app wecom --instruction 'detection_app_window_objects=["search button","chat list"]'
```

该示例输入恰好是对应数组的 `serde_json` 紧凑 UTF-8 序列化；其他输入必须按同一规则序列化后计算摘要。撤销该对象数组的视觉授权：

```sh
sqlite3 "$HOME/.desk-agent/security.sqlite3" "DELETE FROM local_capability_grants WHERE app='wecom' AND capability='vision.window.locate' AND route='local_vision' AND target='main_or_single_app_window' AND effect='local_inference' AND parameters='objects-sha256:$digest';"
```

撤销截图授权：

```sh
sqlite3 "$HOME/.desk-agent/security.sqlite3" "DELETE FROM local_capability_grants WHERE app='wecom' AND capability='desktop.window.capture' AND route='macos_gui' AND target='main_or_single_app_window' AND effect='capture' AND parameters='none:v1';"
```

需要时撤销授权：

```sh
sqlite3 "$HOME/.desk-agent/security.sqlite3" "DELETE FROM local_capability_grants WHERE app='wecom' AND capability='wecom.window.center' AND route='macos_gui' AND target='main_or_single_wecom_window' AND effect='move_without_resize' AND parameters='none:v1';"
```

`DESK_AGENT_SECURITY_DB` 可指定安全库；父目录须由当前用户拥有且权限为 `0700`，数据库文件须为 `0600`。授权与审计存储不可用时不会分发居中操作。

This is not a Core or Durable conformance claim: general scope and confirmation policies, durable task recovery, event replay, independent verification and task observation are not implemented. The `recovery` request field is rejected; the draft requires a coordinated contract revision before recovery can be supported. The protocol's security and reliability requirements remain the target, not evidence of implemented behavior.

The [RPA protocol draft](docs/RPA_PROTOCOL.md) ([中文](docs/RPA_PROTOCOL.zh-CN.md)) defines the target contract and Core and Durable profiles. Only the synchronous terminal task path is currently exposed; no asynchronous acceptance, task control, or durable event interface is advertised.

## Development checks

```sh
cargo fmt --all -- --check
cargo test
cargo build
```
