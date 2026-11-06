# RPA 操作协议 — v1 草案

**状态：**提议中的规范。本文定义目标协议和标准本地实现要求，不代表任何功能已经实现。只有在实现、公开接口、Schema、能力目录和验证同步迁移后，才能声明符合本规范。示例为协议消息示意，不代表现有部署的实际行为。

## 1. 范围与约定

本协议定义本地优先桌面 Agent 平台的任务提交、能力发现、执行、审批、观察及恢复。企业微信是应用适配器，而非平台边界。GUI 自动化、官方 API 和浏览器自动化是不同通道，禁止静默切换。界面可见内容不是完整归档，不得将其描述为已授权企业 API 或会话存档数据。禁止私有协议及绕过登录、验证码或平台权限。

大写关键词 **MUST**、**MUST NOT**、**REQUIRED**、**SHOULD**、**SHOULD NOT**、**MAY** 按 RFC 2119 和 RFC 8174 解释，分别表示必须、禁止、必需、应当、不应当和可以。规范性要求约束符合本协议的实现，不表示现有代码已满足。

规范包含两种配置：所有符合实现必须满足的 **Core**，以及声明事件重放或恢复的部署必须满足的 **Durable**。第 12、13 节还规定标准本地实现。其他实现只有在外部可观察行为和安全保证满足相应要求时，才可声明符合 Core 或 Durable；不得声明符合标准本地实现。

## 2. 角色与信任边界

**客户端**提交请求并消费结果。**接口**认证客户端、转换传输并调用 Runtime。**Runtime** 负责任务生命周期、策略、路由选择、分发、验证和审计。**Agent** 解释自然语言并提出计划；禁止直接调用 OS 输入 API 或绕过 Runtime。**能力提供方**通过明确声明的**通道**实现注册的业务操作。**审批方**授权特定待执行操作；**事件消费者**读取获授权的事件。

所有外部调用的受控操作必须经过同一个 Runtime 能力调用器。接口、插件和 Agent 不得直接向 OS 或应用适配器分发。插件只接收受控上下文，不得拥有或绕过执行生命周期。不可信插件需要进程或 WASM 隔离；不得将进程内插件描述为已隔离。

用户指令、应用内容、检索文档和模型输出属于不同信任等级。应用内容、检索文档和模型输出不得覆盖系统策略、用户授权、精确指令或审批范围。禁止公开无约束的鼠标或键盘能力。

## 3. 版本与编码

协议主版本为 `1`。HTTP 路径以 `/api/v1` 为根；Schema 使用独立、不可变的版本化标识。JSON 使用 UTF-8，对象成员名为字符串；时间戳为带 `Z` 的 RFC 3339 UTC 字符串。二进制证据单独存储，使用经授权的不透明引用表示。标识是不透明字符串，使用 UUIDv7 或其他抗碰撞方案生成；客户端不得从标识推断权限或顺序。

请求 Schema 拒绝未知字段。因此，不得假定新增请求字段与旧服务端兼容。不兼容的请求、响应含义、枚举或生命周期变化需要新的 API 主版本和迁移文档。主版本内可以新增可选响应字段；客户端必须忽略未知响应对象字段。客户端必须保守处理未知状态、事件和错误码，不得假设其表示成功而执行或重试。服务端不得改变已有字段含义或复用标识。能力分别声明其 Schema 版本。

有请求体时，每个请求在请求体中携带 `protocol_version: "1"`。HTTP 发现与读取操作使用路径版本。不支持的版本必须在任何副作用前失败。IPC 使用相同语义的请求与响应对象，其帧格式见第 11 节。CLI 和桌面端构造相同的任务请求，经 Runtime 分发，并呈现相同语义的结果和错误信息。

## 4. 标识与共享数据规则

`request_id` 标识一次入站交互；`operation_id` 标识一次已接受的执行或恢复操作；`task_id` 标识逻辑任务。任务包含按序排列的步骤，每个步骤有 `step_id` 和从零开始的 `sequence`；`attempt` 是步骤的一次调用，从 1 开始。重试保留步骤 ID 并递增尝试次数。`trace_id` 关联工作，不得作为授权凭据。即使幂等重放返回已有操作，新请求也必须获得新的请求 ID。

下文对象使用 JSON 字段名作为规范线格式。除非另有说明，字段均为必填；可选字段应省略，不隐式推断默认值。仅明确说明时接受 `null`。数组保持顺序。字符串和集合上限见第 14 节。Schema 必须在分发前拒绝格式错误的输入，不得将错误的精确输入重新解释为自然语言。

## 5. 任务请求与指令模型

`POST /api/v1/rpa` 是唯一通用 HTTP 任务执行入口。所有外部调用的任务执行、取消、审批与恢复操作必须使用此入口、共享任务契约及 Runtime 能力调用器；其新增请求和响应形式需要协调修订契约，本文不作既定定义。现有只读能力目录仍是单独的必需发现入口。任务提交请求体为共享 `TaskRequest`：

```json
{
  "protocol_version": "1",
  "app": "wecom",
  "messages": ["Open WeCom"],
  "instructions": [{"action": "launch_wecom"}]
}
```

| 字段 | 规则 |
|---|---|
| `protocol_version` | 必填字符串 `"1"`。 |
| `app` | 必填的稳定应用标识；不得用作模型或提供方标识。 |
| `messages` | 有序非空白字符串数组；缺少 `instructions` 时必填。存在指令时可为空，且不用于规划。 |
| `instructions` | 可选非空类型化业务指令数组。存在时选择精确模式，缺失时选择自然语言模式。`null` 和空数组无效。 |

至少提供一种输入形式。同时提供时，以指令为准，`messages` 不得改变执行。精确指令必须保持顺序、目标、参数和用户文本；禁止 LLM 改写、遗漏或替换。指令是包含 `action` 以及在适用时包含 `params` 的带标签对象。指令词汇表和参数 Schema 是版本化公共契约；OS 原语和实现细节不得成为公开业务指令。能力目录将每个指令映射到稳定的命名空间能力 ID。自然语言解释必须使用确定性解析器或明确配置的规划器；意图、目标或必要数据存在歧义时必须停止并请求澄清。

精确指令 `{"action":"center_wecom_window"}` 不带参数，对应能力 `wecom.window.center`。其 macOS GUI 通道将已有企业微信窗口移至窗口所在屏幕的完整屏幕中心，不改变窗口大小；不得静默启动企业微信或切换应用。该操作需要 macOS 辅助功能权限和可操作的已有窗口；平台不支持或前置条件不满足时，能力不可用或返回错误。完成本身不构成独立验证，错误不得报告为已验证。此声明不代表完整协议符合性或已在真实硬件上验证。

GUI 通道要求恰有一个运行中的企业微信实例；只有一个辅助功能窗口时选择该窗口，存在多个窗口时选择辅助功能主窗口，无法确定主窗口时返回错误。按窗口与活动显示器的最大重叠面积选屏；面积相同或没有重叠显示器时返回错误。设置位置后适配器回读辅助功能位置和大小，只有两者分别与目标位置及原大小相差不超过 1 点才返回成功；回读失败或不符时返回错误，但窗口可能已经移动。该回读不是独立的截图验证。

无参数精确指令 `{"action":"capture_app_window"}` 和自然语言消息 `截取企业微信窗口` 通过 Runtime 的 `macos_gui` 通道对应能力及必需范围 `desktop.window.capture`。本地明确授权必须绑定应用 `wecom`、目标 `main_or_single_app_window`、效果 `capture` 和参数 `none:v1`；先前的屏幕或窗口授权不能用于此操作。需要 macOS 屏幕录制及辅助功能权限、可用的授权和审计存储，以及恰好一个运行中的企业微信进程。只有一个辅助功能窗口时选该窗口，多个时选主窗口；必须按 PID 和几何信息唯一匹配可见 Core Graphics 窗口，存在歧义则失败。只捕获该窗口，不截取屏幕矩形或遮挡它的其他窗口。返回 `~/.desk-agent` 下私有 `0700` 目录中唯一 `0600` PNG 文件的路径；不得记录截图内容。完成不代表已独立验证。

已获本地明确授权的截图请求在缺少屏幕录制权限时可以调用 macOS 权限请求接口。是否显示弹窗由操作系统决定；必须由用户授予权限，且可能需要重启 Runtime。能力发现保持只读，不得请求操作系统权限或弹窗。

精确指令 `{"action":"locate_app_window_objects","params":{"objects":["搜索按钮","会话列表"]}}` 接受 1–16 个有序非空白字符串，每项最多 256 个 UTF-8 字节；不支持该指令的自然语言规划。操作先截取新的目标窗口图像，再按对象顺序逐项向固定本地端点 `http://127.0.0.1/v1/chat/completions`、模型 `wevisone-vl-4b` 发送敏感图像，`wevisone_task` 为 `referring`；不会静默向远程服务披露。除截图授权外，还需另一项本地明确授权：应用 `wecom`、能力及范围 `vision.window.locate`、通道 `local_vision`、目标 `main_or_single_app_window`、效果 `local_inference`、参数 `objects-sha256:<digest>`。摘要是精确有序对象数组的 `serde_json` 紧凑 UTF-8 序列化的 SHA-256 小写十六进制值。Runtime 在每次截图和每次披露前重新检查相应授权。

定位输出包含 `capture_id`、`image_size: [width,height]` 和按查询顺序的 `objects: [{query, predictions: [{label, coords: [x1,y1,x2,y2]}]}]`；`coords` 保持原始截图像素，不自动添加屏幕坐标。输出还包含 `capture_path`、`captured_at_unix_ms`、`target` 和 `coordinate_space`。本地推理前图像缩放并编码为 JPEG，返回的框换算回原图像像素；小目标可能因此丢失。

截图也返回 `capture_id`。独立精确指令 `{"action":"convert_app_window_coordinates","params":{"capture_id":"123e4567-e89b-12d3-a456-426614174000","boxes":[[10.0,20.0,30.0,40.0]]}}` 接受 1–100 个有限的原图像像素框 `[x1,y1,x2,y2]`，须位于图片范围内且左上严格小于右下。ID 为规范小写 UUID，只能解析对应私有截图记录，不能传入任意路径。记录 300 秒后过期；旧截图没有记录，无法转换。目标窗口、显示器或几何发生变化时拒绝执行；转换不会截图、调用模型、请求屏幕权限或点击。CLI 可用 `--instruction 'convert_app_window_coordinates={"capture_id":"123e4567-e89b-12d3-a456-426614174000","boxes":[[10.0,20.0,30.0,40.0]]}'`。

转换需单独明确授权：应用 `wecom`，能力及范围 `desktop.window.coordinates.convert`，通道 `macos_gui`，目标 `main_or_single_app_window`，效果 `read_geometry`，参数 `conversion-sha256:<digest>`。摘要是字段依次为 `capture_id`、`boxes` 的结构经 `serde_json` 紧凑 UTF-8 序列化后的 SHA-256 小写十六进制值；数字使用 serde_json 的 f64 表示（如 `1.0`）。Runtime 在派发前重新检查授权。

输出包含 `capture_id`、`window_bounds_points: [x,y,width,height]`、`screen_coordinate_space: "macos_global_top_left_points_at_capture"` 和按输入顺序的 `boxes: [{image_bounds_pixels, screen_bounds_points, screen_center_points}]`。屏幕坐标为截图时 macOS 全局桌面左上角原点的逻辑点，可能为负；各轴按窗口点数尺寸与图片像素尺寸之比缩放，不假设 Retina 倍率。窗口跨多个显示器、显示器归属不明确、截图目标改变或截图几何无法对应时拒绝执行。

这些坐标只是快照，不代表当前界面或行动授权。几何未变也不能排除移动后移回或检查后变化。预测和标签不可信；点击前必须独立重新验证目标和当前界面。

独立精确指令 `{"action":"activate_app_element","params":{"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}}` 是受限的企业微信业务操作，不是任意桌面输入，也不支持自然语言规划。CLI 使用 `--instruction 'activate_app_element={"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}'`。调用方须给出 macOS 全局左上原点逻辑点表示的有限坐标 `[x,y]`，以及准确的非空 AXRole 和 AXTitle，均不超过 256 UTF-8 字节。坐标必须在实时选定的企业微信窗口内。截图可用于推导坐标，但操作不截图、不需要截图记录或屏幕录制权限。调用方须识别预期目标；模型预测和应用内容不可信，不能授权操作。
AXRole 或 AXTitle 不可用时拒绝执行；`expected_label` 是 AXTitle，不是 AXDescription。不能保证任意按钮都支持所需的状态变化。
每个坐标必须能精确表示为 f32（例如整数点）；操作不会自动舍入坐标，也不会猜测像素到点的比例。

`macos_gui` 通道要求本机 macOS 辅助功能权限。它独立命中测试实时 AX 元素，检查精确角色和标题、元素属于所选企业微信窗口，且窗口位于前台、未被遮挡，并在派发前立即重新验证；不匹配或无法确认时拒绝执行，不静默启动或激活应用。必须有单独的本地明确一次性授权，绑定应用 `wecom`、能力及范围 `wecom.element.activate`、通道 `macos_gui`、目标 `main_or_single_app_window`、效果 `activate_element` 和包含精确坐标、预期角色与标题的参数 `activate-sha256:<digest>`。摘要是字段依次为 `point`、`expected_role`、`expected_label` 的结构经 `serde_json` 紧凑 UTF-8 序列化后的 SHA-256 小写十六进制值；坐标数字使用 serde_json f64 格式（如 `1.0`），参数不得改写。预检后、派发前立即原子消费授权并审计；截图或坐标授权不能复用。

授权后，激活预检失败的 `TaskError` 可包含可选固定 `diagnostic_code`；错误仍为 `capability_unavailable`，阶段为 `validation`、结果为 `not_dispatched`、`retryable: false`，并保留请求 ID。代码只描述本次派发前失败：

- `accessibility_unavailable`：辅助功能访问不可用。
- `target_window_unavailable`：无法获取目标窗口。
- `coordinate_precision_unsupported`：坐标不能按所需精度表示。
- `point_outside_target_window`：坐标位于目标窗口外。
- `target_not_frontmost`：目标窗口不在最前方。
- `point_obscured`：已确定该点被遮挡。
- `hit_test_unavailable`：辅助功能命中测试不可用。
- `target_element_mismatch`：命中元素不属于目标窗口。
- `accessibility_role_unavailable`：元素辅助功能角色不可用。
- `accessibility_role_mismatch`：辅助功能角色与预期不符。
- `accessibility_title_unavailable`：元素辅助功能标题不可用。
- `accessibility_title_mismatch`：辅助功能标题与预期不符。
- `target_changed`：预检期间目标发生变化。
- `preflight_unavailable`：预检因其他原因无法确定安全结果。

代码不泄露实际角色、标题、标签、坐标、截图或原始系统错误。不确定时使用适当的不可用代码，不推断遮挡；固定代码也可记录在审计决策中。诊断不能绕过授权或验证，不追溯诊断旧记录，也不用于派发后的失败。

独立精确指令 `diagnose_app_element` 使用与激活相同的 `point`、`expected_role` 和 `expected_label`，只读检查当前预检状态，不派发输入，也不消费激活授权。CLI 可用 `--instruction 'diagnose_app_element={"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}'`。它需要独立本地明确授权，绑定应用 `wecom`、能力及范围 `wecom.element.diagnose`、通道 `macos_gui`、目标 `main_or_single_app_window`、效果 `inspect_element` 和 `diagnose-sha256:<digest>`；摘要采用与激活相同的有序参数序列化。匹配时返回 `{"diagnostic_code":"matched"}`，检查失败时返回带固定诊断代码的 TaskError。这只是当前观察，不是对过去失败的追溯诊断，也不是后续激活的授权。

派发后适配器检查新出现的 AX 焦点或选中状态变化；原本已聚焦或状态不变不足以证明成功。证据缺失或不变、派发失败、效果发生后审计失败均为结果未知，不能报告已验证；派发后绝不自动重试。成功返回 `{"activated":true}`；独立的协议验证状态仍为 `not_performed`，不声称协议层独立验证。

执行前，Runtime 必须依次进行规范化、解析、Schema 校验、范围和风险授权、适用确认、显式路由选择、通过能力调用器分发、验证、审计并返回标准结果。路由改变时必须重新执行适用的策略和审批检查。授权与审批必须绑定应用、能力、目标、副作用、路由和相关参数摘要。Runtime 必须在分发前重新检查过期与撤销状态。

## 6. 能力发现

`GET /api/v1/capabilities` 返回 `{ "protocol_version": "1", "capabilities": [...] }`。每项包含 `capability_id`、`instruction_action`、`app`、`schema_version`、`input_schema_ref`、`available`、`required_scopes`、`channels`、`risk`、`confirmation`、`verification`、`prerequisites` 和 `unavailable_reason`（仅可用时为 null）。`channels` 标识显式的 GUI、官方 API 或浏览器路由；风险、确认和验证描述必须反映实际策略与通道支持。可用性必须反映可执行的 Runtime 行为和前置条件，不能仅依据已声明的指令或枚举。发现操作只读，不得分发任务。

运行中的尝试必须固定能力版本及参数 Schema。如果分发前能力、路由、授权或审批发生变化，Runtime 必须重新校验；原决策不再覆盖操作时必须停止或请求新审批。目录必须标识不支持或不可用的能力，不得将未来能力声称为可执行。

## 7. 执行生命周期与响应

任务状态为 `created`、`validated`、`planned`、`awaiting_approval`、`running`、`verifying`、`completed`、`failed`、`cancelled` 和 `unknown`。任务依次经过校验、适用时的规划、必需时的审批、执行和验证；分发前可提前以 `failed` 或 `cancelled` 终止。分发后无法确定外部效果时必须使用 `unknown`。`completed`、`failed`、`cancelled` 和 `unknown` 是一次执行操作的终态；核对或恢复是独立关联操作，不得静默改写历史结果。

步骤使用相同的终态含义。步骤按精确输入顺序分发。前一条指令失败、取消或结果未知时，不得分发后续精确指令。步骤 `completed` 表示执行完成，并不证明经过独立验证。验证状态独立为 `not_performed`、`verified`、`failed` 或 `unknown`；`verified` 需要证明所述后置条件的独立证据。仅由适配器提交不能构成验证；验证失败不证明副作用失败。

执行已终结时，成功提交返回 HTTP `200` 和 `TaskResponse`；已接受但未终结时返回 `202`。响应示例：

```json
{
  "protocol_version": "1",
  "app": "wecom",
  "execution": {
    "request_id": "01900000-0000-7000-8000-000000000001",
    "task_id": "01900000-0000-7000-8000-000000000002",
    "operation_id": "01900000-0000-7000-8000-000000000003",
    "trace_id": "01900000-0000-7000-8000-000000000004",
    "mode": "exact_instruction",
    "status": "completed",
    "steps": [{"step_id": "01900000-0000-7000-8000-000000000005", "sequence": 0, "attempt": 1, "instruction": {"action": "launch_wecom"}, "status": "completed", "verification": {"status": "not_performed", "evidence_ref": null}, "output": {}, "error": null}]
  }
}
```

`output` 由能力 Schema 定义，默认不得包含秘密。省略的待执行步骤不得表示为已完成。任务观察必须经过授权，其接口与共享契约需要协调修订。已接受的异步任务必须在公布的保留期内可查询。审批或澄清请求必须包含不透明引用、明确范围、过期时间和客户端操作说明；没有交互不代表批准。

## 8. 错误、期限与取消

所有任务操作错误使用 `{ "error": { "code": "...", "message": "...", "request_id": "...", "task_id": null, "operation_id": null, "phase": "...", "outcome": "not_dispatched", "retryable": false } }`。尚未分配的 ID 必须为 null。`phase` 标识失败阶段；`outcome` 为 `not_dispatched`、`known_failed`、`known_succeeded` 或 `unknown`。脱敏消息不得包含凭证、原始输入、消息内容、截图或未过滤的堆栈。稳定错误码包括 `invalid_request`、`unsupported_version`、`unauthorized`、`forbidden`、`approval_required`、`ambiguous_target`、`not_found`、`conflict`、`idempotency_conflict`、`capability_unavailable`、`rate_limited`、`operation_failed`、`outcome_unknown` 和 `internal_error`。

HTTP 将格式错误映射为 400，未认证为 401，禁止访问为 403，资源不存在为 404，冲突为 409，歧义或语义输入无效为 422，限流为 429，能力不可用为 503，意外失败为 500。已接受任务的外部结果未知时，以任务状态表示，不得伪造已知失败。分发后的传输错误不能证明取消或失败；客户端重试前必须核对。除非服务端已确定能力特定的重复安全性、当前策略和剩余重试预算，否则错误的 `retryable` 必须为 false。

取消请求通过 `POST /api/v1/rpa` 和共享任务契约提交，并不保证中断。已分发操作可能在请求后完成。只有不存在结果未决的在途副作用时，任务才可标记为 `cancelled`；否则为 `unknown`。客户端期限限制等待，不一定限制执行。服务端期限和审批到期以服务端时间为准；本地超时应使用单调经过时间。时钟偏差本身不得授权分发或恢复。

## 9. 幂等、审批与恢复

客户端可在创建任务时提供 `Idempotency-Key`。服务端以认证主体和操作为键的作用域，保存规范化请求摘要与结果；不同摘要复用同一键时拒绝并返回 `idempotency_conflict`。相同重试返回已有操作和新的请求 ID。键在公布的保留期内必须有效；到期后客户端不得假设重复安全。这只对已接受的协议请求去重，不能对任意外部 GUI 效果去重。

审批是通过 `POST /api/v1/rpa` 和共享任务契约进行的独立授权交互，携带审批引用和决策（`approve` 或 `deny`）；确切请求形式需要协调修订契约。服务端必须验证审批方身份、绑定的操作与参数、范围、过期、撤销和单次使用状态。拒绝或过期不得触发自动批准或重试。

Durable 部署可声明支持恢复。恢复使用 `POST /api/v1/rpa` 和共享任务契约；协调修订时必须定义动作（`reconcile`、`resume`、`retry`、`rollback` 或 `rollback_then_retry`）、预期版本、幂等键及对应动作所需的检查点或步骤引用。恢复是关联原任务的新授权操作。版本过期、所有权丢失或键冲突必须在分发前失败。未知尝试必须用独立证据核对；仅有确认不证明没有副作用。重试需要已知无副作用的失败，或覆盖前次尝试的能力特定幂等保证，同时需要新的策略检查和有限预算。恢复继续执行时不得重新运行已完成且已验证的步骤。补偿需要已注册、获授权的能力，本身也是副作用；失败或未知时必须停止，不能假设消息可以撤回。`rollback_then_retry` 必须先成功完成并验证补偿。

## 10. 事件、审计与证据

持久事件记录包含 `schema_version`、`event_id`、`task_id`、`operation_id`、`request_id`、`trace_id`、`type`、`sequence`、`occurred_at`，以及可选的 `step_id`、`attempt`、`causation_id`、`evidence_ref` 或 `error_code`。每种事件类型的载荷必须类型化且有界。事件不可变；修正使用新的关联记录。`sequence` 在持久追加时原子分配，只对单个任务内事件排序，不跨任务排序；墙上时钟不是排序依据。投递可以至少一次；消费者按 `event_id` 去重。任务创建前的拒绝在审计流中通过请求 ID 关联。

Durable 部署提供按任务授权的事件访问；其接口需要协调修订契约。响应包含 `events`、`next_cursor` 和 `retention_boundary`。无效或过期游标以及重放缺口必须报告，不得静默视为完整历史。状态转换事件必须反映实际发生的转换，不得从最终结果合成后冒充持久日志。审计与诊断日志分开记录策略和副作用决策；受保护证据使用不透明、受访问控制的引用。日志、事件和普通审计记录默认不得包含秘密、消息或截图内容。

## 11. 传输与认证

HTTP 使用 JSON，默认仅监听 loopback。仅绑定 loopback 不能替代认证：改变状态和访问任务数据的端点需要逐安装的秘密，保存在 OS 凭证存储中，并作为 bearer 凭证传递。凭证不得出现在 URL 或日志中。默认拒绝浏览器跨源请求。远程暴露必须显式配置 TLS 和认证；默认不得启用。授权按主体和任务评估；标识不是凭据。

本地 IPC 每个连接处理一组请求和响应：四字节无符号大端载荷长度，后接 UTF-8 JSON，最大 1 MiB。IPC 请求和响应对象与 HTTP 使用相同语义契约和字段名。Socket 必须仅允许所属 OS 用户访问，且在支持时检查对端身份。无效或超大帧在分发前拒绝。分发后的传输断连或超时不会取消任务，也不能证明结果。

CLI 和桌面端使用相同 Runtime 和共享任务契约。CLI 可提供不同于 JSON 的人类可读展示，但必须保留任务 ID、状态、验证和错误含义。桌面 UI 操作必须构造相同请求模型。健康检查和能力发现只读，不得执行任务。

## 12. 标准本地实现架构

标准部署采用单用户本地 Rust Runtime 进程，以及经认证本地 IPC 通信的 CLI 和桌面客户端；HTTP 是同一 Runtime 上的可选 loopback 接口。标准 HTTP 框架为 Axum，异步运行时为 Tokio。内部领域契约不得依赖 Axum、SQLite 或具体 OS 适配器。Bootstrap 只组装依赖，不包含业务策略。默认使用静态内置插件；不要求 Rust 动态库热加载。多机协调不属于此配置，不得依据本地锁声称支持多机。

规范 Rust 实体为 `TaskRequest`、`TaskInstruction`、`TaskInput`、`TaskResponse`、`TaskExecution`、`TaskStepExecution`、`ExecutionStatus` 和 `TaskErrorResponse`；公共 Schema 独立于 Rust 布局版本化。ID 和时间戳使用经过校验的新类型，状态和动作为带标签枚举，可选可空字段在序列化中显式表示。能力特定参数由版本化 Schema 和适配器定义，而不是将 OS 原语加入 `TaskInstruction`。

## 13. 标准持久存储与并发

标准持久存储是本地文件系统上的 SQLite，启用 WAL 模式和外键，并按要求的崩溃保证配置持久提交。数据库和证据目录必须仅允许所属用户访问。秘密保存在 OS 凭证存储中，不放在普通配置或数据库日志中。逻辑 Schema 如下：

| 表 | 键与必需内容 |
|---|---|
| `tasks` | `task_id` 主键；主体、应用、模式、状态、版本、定义摘要和时间戳。 |
| `steps` | `(task_id, sequence)` 主键；唯一 `step_id`、指令引用、状态、验证和当前尝试。 |
| `attempts` | `(step_id, attempt)` 主键；操作、路由及版本、意图、分发标记、结果和时间戳。 |
| `events` | `(task_id, sequence)` 主键；唯一 `event_id`、关联信息、类型、有界载荷和时间戳。 |
| `audit` | `audit_id` 主键；主体、决策、任务或操作引用及安全元数据。 |
| `approvals` | `approval_id` 主键；范围摘要、主体、决策、过期、撤销和使用情况。 |
| `idempotency` | `(principal, operation_kind, key)` 主键；请求摘要、操作 ID、结果引用和过期时间。 |
| `checkpoints` | `checkpoint_id` 主键；任务及版本、日志序号、定义摘要、状态及证据引用和过期时间。 |
| `leases` | `task_id` 主键；所有者、过期时间和单调递增的 fencing token。 |

索引必须支持主体/任务授权、有序任务事件和过期清理。外键及唯一约束必须维护标识完整性。敏感输入和证据与普通日志分开存储，使用 OS 凭证存储保护的密钥静态加密；引用需要与底层任务相同的授权。必须定义并向客户端说明配置的保留和删除期限、备份访问、审计义务与删除例外。SQLite 支持时迁移必须使用事务，维护版本，并在不兼容变更前备份。完整性检查或迁移失败必须阻止分发；恢复备份后继续执行前必须核对未决尝试。

副作用前，必须原子持久化意图、尝试、路由、策略及审批引用和相关幂等状态；失败时不得分发。分发与观察到的结果分别记录为独立转换。数据库事务不能与外部 GUI 或 API 副作用原子提交：若在分发后、结果持久化前崩溃，必须将结果标为未知并核对，不得声称 exactly-once。

任务所有权通过 SQLite 写事务获取：比较预期任务版本与租约到期时间，递增 fencing token，并在分发前提交。每次状态更新必须比较当前 token 和预期版本。过期所有者必须停止；仅租约到期不证明其外部操作已停止。若前一所有者可能已分发未决副作用，新所有者必须在再次分发前核对。尽可能使用单调本地计时，并保守处理进程重启与时钟跳变。同一转换的日志和检查点更新必须原子提交。

## 14. 限制与运行策略

标准部署每个请求或 IPC 帧最多 1 MiB，每个任务最多 100 条指令，每个字符串最多 64 KiB，每页最多 100 个事件。部署必须通过元数据公布配置的并发、队列、执行期限、保留期和重试限制；限制必须有限。过载时在分发前以 `rate_limited` 拒绝，可附有有界 `retry_after_ms`；该延迟本身不授权重试已分发的副作用。访问、导出和删除任务与证据需要授权。只有符合配置的法定保留义务时才可删除过期审计记录。

## 15. 一致性验证场景

符合本协议的实现必须提供下列可执行行为检查：

1. 自然语言歧义在分发前停止；精确指令保持目标、文本、参数和顺序。
2. 无效输入、不可用能力、授权过期及审批拒绝不产生副作用。
3. 路由和能力版本改变时重新执行适用的策略检查。
4. 仅有适配器提交不得报告为独立验证；验证失败不抹去可能已发生的效果。
5. 分发期间客户端断连、超时和取消不意味着失败或可以安全重试。
6. 相同幂等键去重；改变请求发生冲突；键过期不表示外部操作可安全重复。
7. 并发恢复、过期版本和租约丢失不得重复分发；此前未决效果需要核对。
8. 分发前、分发后以及结果持久化前崩溃时，保留规定的结果和恢复规则。
9. 可观察并正确处理重复事件、重放缺口、过期游标及保留边界。
10. 部分或未知补偿停止回滚并禁止自动重试；未授权用户不能访问任务或证据。
11. HTTP、IPC、CLI 和桌面端保持共享任务语义；Schema 示例通过版本化 Schema 和实际 Runtime 行为验证。

Durable 配置还必须针对真实持久存储和真实 Runtime 分发路径执行这些检查，不能只验证序列化类型或模拟事件列表。部署只能声明通过这些检查的配置和能力。

## 参考资料

- [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119)、[RFC 8174](https://www.rfc-editor.org/rfc/rfc8174)：规范性语言。
- [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110)：HTTP 语义和幂等性。
- [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457)：机器可读错误设计。
- [MCP 生命周期](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)：生命周期和能力协商设计参考。
- [CloudEvents 规范](https://github.com/cloudevents/spec/blob/main/cloudevents/spec.md)：事件身份及投递设计参考。
- [W3C Trace Context](https://www.w3.org/TR/trace-context/)：追踪上下文设计参考。
- [Google AIP-180](https://google.aip.dev/180)：兼容性设计参考。
