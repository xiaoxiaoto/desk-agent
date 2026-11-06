# RPA Operation Protocol — Draft v1

**Status:** Proposed specification. This document defines a target protocol and reference implementation requirements; it is not evidence that any feature is currently available. Implementation, public interfaces, schemas, capability catalog and verification MUST be migrated together before conformance is claimed. Examples are illustrative protocol messages, not claims about an existing deployment.

## 1. Scope and conventions

This protocol defines task submission, capability discovery, execution, approval, observation and recovery for a local-first desktop agent platform. WeCom is an application adapter, not the platform boundary. GUI automation, official APIs and browser automation are distinct channels. A channel MUST NOT be substituted silently. UI-visible content is not a complete archive and MUST NOT be represented as authorized enterprise or archive API data. Private protocols, bypassing login, CAPTCHA or platform permissions are prohibited.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, **SHOULD NOT**, and **MAY** are to be interpreted as described in RFC 2119 and RFC 8174 when capitalized. Normative requirements apply to conforming implementations, not necessarily to the current codebase.

The specification has two profiles: **Core**, mandatory for every conforming deployment, and **Durable**, mandatory for deployments advertising event replay or recovery. Sections 12 and 13 additionally specify the standard local implementation. A different implementation MAY claim Core or Durable conformance only if its externally observable behavior and safety guarantees meet the applicable requirements; it MUST NOT claim standard-local-implementation conformance.

## 2. Roles and trust boundaries

A **client** submits requests and consumes results. An **interface** authenticates the client, translates the transport and invokes the Runtime. The **Runtime** owns task lifecycle, policy, route selection, dispatch, verification and audit. An **agent** interprets natural language and proposes plans; it MUST NOT invoke OS input APIs or bypass Runtime. A **capability provider** implements a registered business operation using an explicitly declared **channel**. An **approver** authorizes a specific pending action; an **event consumer** reads authorized events.

All externally invoked controlled operations MUST go through the same Runtime capability invoker. Interfaces, plugins and agents MUST NOT dispatch directly to OS or application adapters. Plugins receive controlled context and MUST NOT own or bypass the lifecycle. Untrusted plugins require process or WASM isolation; in-process plugins MUST NOT be represented as isolated.

User instructions, application content, retrieved documents and model output have distinct trust levels. Application content, retrieved documents and model output MUST NOT override system policy, user authorization, exact instructions or approval scope. No unbounded public mouse or keyboard capability is permitted.

## 3. Versioning and encoding

The major protocol version is `1`. HTTP paths are rooted at `/api/v1`; schemas carry independent immutable versioned identifiers. JSON is UTF-8, with object member names encoded as strings; timestamps are RFC 3339 UTC strings with `Z`. Binary evidence is stored separately and represented by an opaque authorized reference. Identifiers are opaque strings generated using UUIDv7 or another collision-resistant scheme; clients MUST NOT infer authorization or ordering from them.

The request schema rejects unknown fields. Therefore adding a request field is not assumed compatible with older servers. An incompatible request, response meaning, enum or lifecycle change requires a new major API version and a documented migration. New optional response fields MAY be added within a major version; clients MUST ignore unknown response object fields. Clients MUST treat unknown status, event and error codes conservatively and MUST NOT execute or retry on the assumption that they mean success. Servers MUST NOT change the meaning of existing fields or reuse identifiers. Capabilities advertise their schema versions separately.

Every request carries `protocol_version: "1"` in its body where a body is defined. HTTP discovery and read operations use the path version. Unsupported versions fail before any side effect. IPC uses the same semantic request and response objects; its framing is defined in §11. CLI and desktop construct the same task request, dispatch through Runtime and expose the same semantic result and error information.

## 4. Identity and shared data rules

A `request_id` identifies one inbound interaction, an `operation_id` one accepted execution or recovery operation, and a `task_id` the logical task. A task contains ordered steps identified by `step_id` and zero-based `sequence`; an `attempt` is one invocation of a step and starts at 1. A retry retains the step ID and increments the attempt. `trace_id` correlates work and MUST NOT be used as an authorization credential. A new request receives a new request ID even when an idempotent replay returns an existing operation.

Objects below use JSON field names as the canonical wire representation. Unless specified otherwise, fields are required; optional fields are omitted rather than implicitly interpreted as defaults. `null` is accepted only where explicitly stated. Arrays preserve order. String and collection bounds are specified in §14. Schemas MUST reject malformed input before dispatch and MUST NOT reinterpret malformed exact input as natural language.

## 5. Task request and instruction model

`POST /api/v1/rpa` is the only generic HTTP task-execution endpoint. All externally invoked task execution, cancellation, approval and recovery operations MUST use this endpoint, the shared task contracts and Runtime capability invoker. Their additional request and response shapes require a coordinated contract revision and are not specified by this draft. The existing read-only capability catalog remains a separate required discovery endpoint. The task-submission body is the shared `TaskRequest`:

```json
{
  "protocol_version": "1",
  "app": "wecom",
  "messages": ["Open WeCom"],
  "instructions": [{"action": "launch_wecom"}]
}
```

| Field | Rule |
|---|---|
| `protocol_version` | Required string `"1"`. |
| `app` | Required stable application identifier; never a model or provider identifier. |
| `messages` | Ordered array of nonblank strings, required when `instructions` is absent. When instructions are present it MAY be empty and is not used for planning. |
| `instructions` | Optional nonempty array of typed business instructions. Its presence selects exact mode; its absence selects natural-language mode. `null` and empty arrays are invalid. |

At least one input form is required. When both are present, instructions are authoritative and messages MUST NOT alter execution. Exact instructions preserve order, target, parameters and user text; no LLM rewrite, omission or substitution is permitted. An instruction is a tagged object with `action` and, where defined, `params`. The instruction vocabulary and parameter schemas are versioned public contracts; OS primitives and implementation details MUST NOT become public business instructions. The catalog maps each instruction to a stable namespaced capability ID. Natural-language interpretation MUST use a deterministic resolver or explicitly configured planner and MUST stop for clarification when intent, target or required data is ambiguous.

The exact instruction `{"action":"center_wecom_window"}` has no parameters and maps to `wecom.window.center`. Its macOS GUI channel moves an existing WeCom window to the center of the full screen containing it, without resizing the window. It MUST NOT silently launch WeCom or switch applications. The operation requires macOS Accessibility permission and a usable existing window; unsupported platforms or unmet prerequisites make the capability unavailable or result in an error. Completion does not itself constitute independent verification, and an error MUST NOT be reported as verified. This declaration does not claim full protocol conformance or validation on actual hardware.

The GUI route requires exactly one running WeCom instance. It selects its sole Accessibility window, or its Accessibility main window if multiple windows exist; a missing main window is an error. It selects the active display with the largest overlap with that window; a tie or no overlapping display produces an error. After setting the position, the adapter reads back Accessibility position and size and succeeds only if both are within one point of the target position and original size. A failed or mismatched readback returns an error even though the window may already have moved. This readback is not independent screenshot verification.

The parameterless exact instruction `{"action":"capture_app_window"}` and natural-language message `截取企业微信窗口` map to capability and required scope `desktop.window.capture` through Runtime's `macos_gui` route. The explicit local grant binds app `wecom`, target `main_or_single_app_window`, effect `capture`, and parameters `none:v1`; prior screen or window grants do not authorize it. macOS Screen Recording and Accessibility permissions, available authorization and audit storage, and exactly one running WeCom process are required. The sole Accessibility window, or the main window when multiple exist, must map uniquely by PID and geometry to a visible Core Graphics window; ambiguity fails. Capture uses that window, not a screen rectangle or occluding windows. It returns a sensitive PNG path in a private `0700` directory under `~/.desk-agent`; the unique file has mode `0600`. Image contents must not be logged. Completion does not imply independent verification.

An explicitly locally authorized capture request MAY invoke the macOS Screen Recording permission request when permission is absent. The operating system decides whether to show a prompt; the user must grant access and a Runtime restart may be required. Capability discovery remains read-only and MUST NOT request OS permission or show a prompt.

The exact instruction `{"action":"locate_app_window_objects","params":{"objects":["search button","chat list"]}}` accepts 1–16 ordered nonblank strings, each at most 256 UTF-8 bytes. No natural-language planner for this instruction is provided. It captures a fresh selected window image, then sends that sensitive image to the fixed local endpoint `http://127.0.0.1/v1/chat/completions`, model `wevisone-vl-4b`, with `wevisone_task` `referring`, once per object in order; there is no silent remote disclosure. In addition to the capture grant, a separate explicit local grant binds app `wecom`, capability and scope `vision.window.locate`, route `local_vision`, target `main_or_single_app_window`, effect `local_inference`, and parameters `objects-sha256:<digest>`. The digest is lowercase SHA-256 of the UTF-8 compact `serde_json` serialization of the exact ordered objects array. Runtime rechecks the applicable grant before each capture and each disclosure.

Localization returns `capture_id`, `image_size: [width,height]` and ordered `objects: [{query, predictions: [{label, coords: [x1,y1,x2,y2]}]}]`; `coords` remain original captured-image pixels, with no automatic screen coordinates. Output also includes `capture_path`, `captured_at_unix_ms`, `target` and `coordinate_space`. Before inference the image is downscaled and JPEG-encoded, and returned boxes are scaled back to original pixels; small targets may be lost.

Capture also returns `capture_id`. The standalone exact instruction `{"action":"convert_app_window_coordinates","params":{"capture_id":"123e4567-e89b-12d3-a456-426614174000","boxes":[[10.0,20.0,30.0,40.0]]}}` converts 1–100 finite, strictly ordered boxes `[x1,y1,x2,y2]` within the original screenshot dimensions. The ID is a canonical lowercase UUID and resolves only a private capture record, never an arbitrary path. Records expire after 300 seconds; older screenshots have no record and cannot be converted. A changed target window, display or geometry fails closed. Conversion does not capture, invoke a model, request screen permission or click. CLI accepts `--instruction 'convert_app_window_coordinates={"capture_id":"123e4567-e89b-12d3-a456-426614174000","boxes":[[10.0,20.0,30.0,40.0]]}'`.

Conversion requires a separate explicit grant for app `wecom`, capability and scope `desktop.window.coordinates.convert`, route `macos_gui`, target `main_or_single_app_window`, effect `read_geometry`, and parameters `conversion-sha256:<digest>`. The digest is lowercase SHA-256 of compact UTF-8 `serde_json` serialization of a struct with fields `capture_id` then `boxes`; numbers use serde_json f64 normalization (e.g. `1.0`). Runtime rechecks the grant before dispatch.

The output contains `capture_id`, `window_bounds_points: [x,y,width,height]`, `screen_coordinate_space: "macos_global_top_left_points_at_capture"` and ordered `boxes: [{image_bounds_pixels, screen_bounds_points, screen_center_points}]`. Screen points use the capture-time macOS global top-left origin and logical points, possibly negative; each axis scales by the window point dimension divided by image pixel dimension, without assuming a Retina factor. Capture fails closed for windows spanning displays, ambiguous containment, changed capture target, or unmappable geometry.

These are snapshots, not current UI state or authorization to act. Unchanged geometry cannot exclude movement and return or changes after checking. Predictions and labels are untrusted; independently revalidate the target and current UI before clicking.

The standalone exact instruction `{"action":"activate_app_element","params":{"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}}` is a bounded WeCom business action, not arbitrary desktop input or natural-language planning. CLI accepts `--instruction 'activate_app_element={"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}'`. Supply a finite `[x,y]` in macOS global top-left logical points and exact nonempty AXRole and AXTitle strings, each at most 256 UTF-8 bytes. The point must be inside the live selected WeCom window. A screenshot can help derive coordinates, but activation does not capture one or require a capture record or Screen Recording permission. The caller must identify the intended target; model predictions and application content are untrusted and cannot authorize the action.
If AXRole or AXTitle is unavailable, the action fails closed; `expected_label` is AXTitle, not AXDescription. An arbitrary button is not guaranteed to support the required state transition.
Each coordinate must be exactly representable as f32 (integer points are an example); the operation does not round coordinates or guess pixel-to-point scaling.

The `macos_gui` route requires local macOS Accessibility permission. It independently hit-tests the live AX element and checks its exact role and title, membership in the selected WeCom window, and that the window is frontmost and unobstructed, including immediately before dispatch. Mismatch or uncertainty fails closed; it never silently launches or activates an application. A separate explicit, single-use local grant binds app `wecom`, capability and scope `wecom.element.activate`, route `macos_gui`, target `main_or_single_app_window`, effect `activate_element`, and `activate-sha256:<digest>` binding the exact point and expected role and title. The digest is lowercase SHA-256 of compact UTF-8 `serde_json` serialization of a struct whose fields are ordered `point`, `expected_role`, `expected_label`; point numbers use serde_json f64 formatting (e.g. `1.0`) and parameters must not be rewritten. The grant is atomically consumed and audited after preflight, immediately before dispatch; capture and coordinate grants cannot be reused.

After authorization, a failed activation preflight may return `diagnostic_code` in the `TaskError`: the error remains `capability_unavailable`, phase `validation`, outcome `not_dispatched`, and `retryable: false`, with its request ID. The optional fixed code describes only that pre-dispatch failure:

- `accessibility_unavailable`: Accessibility access is unavailable.
- `target_window_unavailable`: the target window cannot be obtained.
- `coordinate_precision_unsupported`: the point cannot be represented with the required precision.
- `point_outside_target_window`: the point is outside the target window.
- `target_not_frontmost`: the target window is not frontmost.
- `point_obscured`: the point is established to be obscured.
- `hit_test_unavailable`: the accessibility hit test is unavailable.
- `target_element_mismatch`: the hit element does not belong to the target window.
- `accessibility_role_unavailable`: the element's accessibility role is unavailable.
- `accessibility_role_mismatch`: the accessibility role differs from the expected role.
- `accessibility_title_unavailable`: the element's accessibility title is unavailable.
- `accessibility_title_mismatch`: the accessibility title differs from the expected title.
- `target_changed`: the target changed during preflight.
- `preflight_unavailable`: preflight could not otherwise establish a safe result.

These codes disclose no actual role, title, label, coordinates, screenshot or raw OS error. Uncertain failures use an appropriate unavailable code; obscuration is not inferred. The fixed code may also be recorded in the audit decision. It cannot bypass authorization or validation, does not diagnose prior records retroactively, and is not supplied for failures after dispatch.

The standalone exact `diagnose_app_element` instruction takes the same `point`, `expected_role` and `expected_label` as activation and performs read-only current preflight checks; it does not dispatch input or consume an activation grant. CLI accepts `--instruction 'diagnose_app_element={"point":[100.0,200.0],"expected_role":"AXButton","expected_label":"Search"}'`. It requires a separate explicit local grant for app `wecom`, capability and scope `wecom.element.diagnose`, route `macos_gui`, target `main_or_single_app_window`, effect `inspect_element`, and `diagnose-sha256:<digest>`, computed using the same ordered parameter serialization as activation. A match returns `{"diagnostic_code":"matched"}`; a failed check returns a TaskError with a fixed diagnostic code. This is a current observation, not a retrospective diagnosis or authorization for later activation.

After dispatch the adapter checks for a newly observed AX focus or selection transition; an already-focused or unchanged state is insufficient. Missing or unchanged evidence, a dispatch failure, or an audit failure after an effect leaves the outcome unknown, not verified. Never automatically retry a dispatched action. Success returns `{"activated":true}`; the separate protocol verification status remains `not_performed` and does not claim independent protocol-level verification.

Before execution Runtime MUST normalize, resolve, validate the schema, authorize scope and risk, obtain applicable confirmation, select an explicit route, dispatch through the capability invoker, verify, audit and return a standard result. A changed route requires renewed applicable policy and approval checks. Authorization and approval MUST bind the application, capability, target, effect, route and relevant parameter digest. The Runtime MUST recheck expiry and revocation before dispatch.

## 6. Capability discovery

`GET /api/v1/capabilities` returns `{ "protocol_version": "1", "capabilities": [...] }`. Each entry contains `capability_id`, `instruction_action`, `app`, `schema_version`, `input_schema_ref`, `available`, `required_scopes`, `channels`, `risk`, `confirmation`, `verification`, `prerequisites` and `unavailable_reason` (null only when available). `channels` identify explicit GUI, official API or browser routes; `risk` and confirmation and verification descriptions MUST reflect actual policy and channel support. Availability MUST reflect executable Runtime behavior and prerequisites, not merely a declared instruction or enum. Discovery is read-only and MUST NOT dispatch tasks.

A capability version and its parameter schema are frozen for a running attempt. If a capability, route, authorization or approval changes before dispatch, Runtime MUST revalidate and stop or request new approval if the prior decision no longer covers the operation. The catalog MUST identify unsupported or unavailable capabilities without claiming that future capabilities are executable.

## 7. Execution lifecycle and response

Task statuses are `created`, `validated`, `planned`, `awaiting_approval`, `running`, `verifying`, `completed`, `failed`, `cancelled`, and `unknown`. A task proceeds through validation, planning where applicable, approval where required, execution and verification; it MAY terminate early in `failed` or `cancelled` before dispatch. `unknown` is required when an external effect cannot be established after dispatch. `completed`, `failed`, `cancelled` and `unknown` terminate an execution operation; reconciliation or recovery is a separate linked operation and MUST NOT silently rewrite its historical outcome.

A step has the same terminal status meanings. Steps dispatch in exact input order. A later exact instruction MUST NOT dispatch if an earlier instruction failed, was cancelled or has unknown outcome. A `completed` step denotes completed execution, not proof of independent verification. Verification is separately `not_performed`, `verified`, `failed`, or `unknown`; `verified` requires independent evidence of the stated postcondition. Adapter submission alone is not verification. A failed verification does not prove that the side effect failed.

A successful submission returns HTTP `200` with `TaskResponse` if execution is terminal, or `202` if accepted but not terminal. The response is:

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

`output` is capability-schema-defined and MUST NOT include secrets by default. An omitted pending step MUST NOT be represented as completed. Task observation MUST be authorized; its interface and shared contract require a coordinated revision. An accepted asynchronous task MUST remain queryable for the advertised retention period. Approval or clarification requests MUST include an opaque reference and explicit scope, expiry and instructions for the client; no interaction implies approval.

## 8. Errors, deadlines and cancellation

All task-operation errors use `{ "error": { "code": "...", "message": "...", "request_id": "...", "task_id": null, "operation_id": null, "phase": "...", "outcome": "not_dispatched", "retryable": false } }`. Nullable IDs MUST be null when not allocated. `phase` identifies the failed lifecycle phase; `outcome` is `not_dispatched`, `known_failed`, `known_succeeded` or `unknown`. A sanitized message MUST NOT contain credentials, raw inputs, message content, screenshots or unfiltered stack traces. Stable error codes include `invalid_request`, `unsupported_version`, `unauthorized`, `forbidden`, `approval_required`, `ambiguous_target`, `not_found`, `conflict`, `idempotency_conflict`, `capability_unavailable`, `rate_limited`, `operation_failed`, `outcome_unknown` and `internal_error`.

HTTP maps malformed input to 400, unauthenticated to 401, forbidden to 403, missing resource to 404, conflicts to 409, ambiguity or invalid semantic input to 422, rate limiting to 429, unavailable capability to 503, and unexpected failures to 500. An accepted task with unknown external outcome is represented by its task status, not a fabricated known failure. Transport errors after dispatch do not prove cancellation or failure; clients MUST reconcile before retry. An error's `retryable` flag MUST be false unless the server has established capability-specific repeat safety, current policy and remaining retry budget.

Cancellation requests use the shared task contract through `POST /api/v1/rpa` and do not guarantee interruption. A dispatched operation may complete after the request. A task MAY be marked `cancelled` only when no in-flight side effect remains unresolved; otherwise it is `unknown`. Client deadlines limit waiting, not necessarily execution. Server deadlines and approval expiry use server time; monotonic elapsed time SHOULD be used for local timeouts. Clock skew MUST NOT by itself authorize dispatch or recovery.

## 9. Idempotency, approval and recovery

Clients MAY supply `Idempotency-Key` on task creation. The server scopes it by authenticated principal and operation, stores a canonical request digest and outcome, and rejects reuse with a different digest as `idempotency_conflict`. Identical repeats return the existing operation and a fresh request ID. Keys MUST remain valid for the published retention window; clients MUST NOT assume repeat safety after expiry. This deduplicates accepted protocol requests, not arbitrary external GUI effects.

Approval is a separate authorized interaction through `POST /api/v1/rpa` and the shared task contract. It carries an approval reference and a decision (`approve` or `deny`); the exact request shape requires a coordinated contract revision. The server MUST verify approver identity, bound operation and parameters, scope, expiry, revocation and single-use status. Denial or expiry MUST NOT cause automatic approval or retry.

Durable deployments MAY advertise recovery. Recovery uses `POST /api/v1/rpa` and the shared task contract; its coordinated revision MUST specify the action (`reconcile`, `resume`, `retry`, `rollback`, or `rollback_then_retry`), expected version, idempotency key and action-specific checkpoint or step references. Recovery is a new authorized operation linked to the original task. Stale versions, lost ownership or conflicting keys MUST fail before dispatch. An unknown attempt MUST be reconciled using independent evidence; acknowledgement alone is not proof of no effect. A retry requires known no-effect failure or capability-specific idempotency covering the previous attempt, plus fresh policy checks and a finite budget. Completed verified steps MUST NOT be rerun on resume. Compensation requires registered authorized capabilities, is itself a side effect, and MUST stop on failure or unknown; no message is assumed unsendable. `rollback_then_retry` requires successful verified compensation first.

## 10. Events, audit and evidence

Durable event records contain `schema_version`, `event_id`, `task_id`, `operation_id`, `request_id`, `trace_id`, `type`, `sequence`, `occurred_at`, and optional `step_id`, `attempt`, `causation_id`, `evidence_ref` or `error_code`. Payloads are typed and bounded per event type. Events are immutable; corrections are new linked records. `sequence` is assigned atomically on durable append and orders events within one task, not across tasks; wall-clock time is not an ordering key. Delivery MAY be at least once; consumers deduplicate by `event_id`. Pre-task rejections are correlated by request ID in the audit stream.

Durable deployments provide authorized per-task event access; its interface requires a coordinated contract revision. The response contains `events`, `next_cursor` and `retention_boundary`. Invalid or expired cursors and replay gaps MUST be reported, never silently treated as complete history. A transition event MUST reflect an actual transition, not be synthesized from a final result and presented as a durable journal. Audit records policy and effect decisions separately from diagnostic logs; protected evidence uses opaque, access-controlled references. Logs, events and ordinary audit records MUST NOT contain secrets or message or screenshot content by default.

## 11. Transport and authentication

HTTP uses JSON over a loopback-bound listener by default. Loopback alone is not authentication: state-changing and task data endpoints require a per-installation secret stored in the OS credential store and passed as a bearer credential. Credentials MUST NOT appear in URLs or logs. Cross-origin browser requests are denied by default. Remote exposure requires explicitly configured TLS and authentication; it MUST NOT be enabled by default. Authorization is evaluated per principal and task; identifiers are not credentials.

Local IPC uses one request and one response per connection: a four-byte unsigned big-endian payload length followed by UTF-8 JSON, maximum 1 MiB. IPC request and response objects use the same semantic contracts and field names as HTTP. The socket MUST be accessible only to the owning OS user and peer identity MUST be checked where supported. Invalid or oversized frames are rejected before dispatch. A transport disconnect or timeout after dispatch does not cancel the task or prove its outcome.

CLI and desktop use the same Runtime and shared task contracts. Human-readable CLI presentation MAY differ from JSON, but MUST preserve task ID, status, verification and error meaning. Desktop UI actions MUST construct the same request model. Health and capability discovery are read-only and MUST NOT execute tasks.

## 12. Standard local implementation architecture

The standard deployment is a single-user local Rust Runtime process plus CLI and desktop clients over authenticated local IPC; HTTP is an optional loopback interface on the same Runtime. The standard HTTP framework is Axum and the asynchronous runtime is Tokio. Internal domain contracts MUST NOT depend on Axum, SQLite or concrete OS adapters. Bootstrap assembles dependencies without business policy. Static built-in plugins are the default; no Rust dynamic-library hot loading is required. Multi-machine coordination is outside this profile and MUST NOT be claimed based on local locks.

The canonical Rust entities are `TaskRequest`, `TaskInstruction`, `TaskInput`, `TaskResponse`, `TaskExecution`, `TaskStepExecution`, `ExecutionStatus` and `TaskErrorResponse`; public schema versioning remains independent of Rust layout. IDs and timestamps are validated newtypes, status and action are tagged enums, and optional nullable fields are represented explicitly in serialization. Capability-specific parameters are defined by versioned schemas and adapters, not OS primitive variants in `TaskInstruction`.

## 13. Standard durable storage and concurrency

The standard durable store is SQLite on the local filesystem, in WAL mode with foreign keys enabled and durable commits configured for the required crash guarantees. The database and evidence directory MUST be accessible only to the owning user. Secrets are held in the OS credential store, not ordinary configuration or database logs. The logical schema is:

| Table | Key and required contents |
|---|---|
| `tasks` | `task_id` primary key; principal, app, mode, status, version, definition digest, timestamps. |
| `steps` | `(task_id, sequence)` primary key; unique `step_id`, instruction reference, status, verification and current attempt. |
| `attempts` | `(step_id, attempt)` primary key; operation, route/version, intent, dispatch marker, outcome and timestamps. |
| `events` | `(task_id, sequence)` primary key; unique `event_id`, correlation, type, bounded payload and timestamp. |
| `audit` | `audit_id` primary key; principal, decision, task/operation references and safe metadata. |
| `approvals` | `approval_id` primary key; scoped digest, principal, decision, expiry, revocation and consumption. |
| `idempotency` | `(principal, operation_kind, key)` primary key; request digest, operation ID, result reference and expiry. |
| `checkpoints` | `checkpoint_id` primary key; task/version, journal sequence, definition digest, state/evidence references and expiry. |
| `leases` | `task_id` primary key; owner, expiry and monotonically increasing fencing token. |

Indexes MUST support principal/task authorization, ordered task events and expiry cleanup. Foreign keys and uniqueness constraints MUST preserve identity. Store sensitive input and evidence separately from ordinary logs, encrypted at rest using keys protected by the OS credential store; references require the same authorization as the underlying task. Define configured retention and deletion periods, backup access, audit obligations and deletion exceptions; document them to clients. Migration is transactional where SQLite supports it, versioned and backed up before incompatible changes. A failed integrity check or migration MUST prevent dispatch; restore MUST reconcile unresolved attempts before resuming.

Before a side effect, atomically persist intent, attempt, route, policy and approval references and relevant idempotency state. If this fails, do not dispatch. Record dispatch and observed result as separate transitions. A database transaction cannot be atomic with an external GUI or API side effect: after a crash between dispatch and durable result, mark outcome unknown and reconcile, never claim exactly-once.

For task ownership, acquire a SQLite write transaction, compare expected task version and lease expiry, increment the fencing token and commit before dispatch. Each state update MUST compare the current token and expected version. A stale owner MUST stop; lease expiry alone does not prove its external operation stopped. If a prior owner may have dispatched an unresolved effect, the new owner MUST reconcile before another dispatch. Use monotonic local timing where possible and conservatively handle process restart and clock discontinuity. Journal and checkpoint changes for the same transition MUST commit atomically.

## 14. Limits and operational policy

The standard deployment accepts at most 1 MiB per request or IPC frame, 100 instructions per task, 64 KiB per individual string, and 100 events per page. A deployment MUST publish configured concurrency, queue, execution deadline, retention and retry limits through its metadata; limits MUST be finite. Overload is rejected before dispatch with `rate_limited` and an optional bounded `retry_after_ms`; this delay is not itself authorization to retry a dispatched effect. Task and evidence access, export and deletion require authorization. Expired audit records MAY be removed only in accordance with configured legal retention obligations.

## 15. Conformance scenarios

A conforming implementation MUST provide executable behavioral checks for:

1. Natural-language ambiguity stops before dispatch; exact instructions preserve target, text, parameters and order.
2. Invalid input, unavailable capabilities, expired authorization and denied approval produce no side effect.
3. Route changes and capability-version changes require renewed applicable policy checks.
4. Adapter submission alone is not reported as independently verified; verification failure does not erase a possible effect.
5. Client disconnect, timeout and cancellation during dispatch do not imply failure or safe retry.
6. Identical idempotency keys deduplicate; changed requests conflict; expired keys do not imply external repeat safety.
7. Concurrent recovery, stale versions and lost leases cannot dispatch duplicate work; unresolved prior effects require reconciliation.
8. Crashes before dispatch, after dispatch and before durable result preserve the specified outcome and recovery rules.
9. Event delivery duplicates, replay gaps, expired cursors and retention boundaries are observable and correctly handled.
10. Partial or unknown compensation stops rollback and forbids automatic retry; unauthorized users cannot access tasks or evidence.
11. HTTP, IPC, CLI and desktop preserve shared task semantics; schema examples validate against versioned schemas and actual Runtime behavior.

The Durable profile additionally requires these checks against real persistent storage and a real Runtime dispatch path, not solely serialized types or simulated event lists. A deployment MUST advertise only profiles and capabilities for which these checks pass.

## References

- [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119), [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174): normative language.
- [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110): HTTP semantics and idempotency.
- [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457): machine-readable error design.
- [MCP lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle): lifecycle and capability negotiation design reference.
- [CloudEvents specification](https://github.com/cloudevents/spec/blob/main/cloudevents/spec.md): event identity and delivery design reference.
- [W3C Trace Context](https://www.w3.org/TR/trace-context/): tracing context design reference.
- [Google AIP-180](https://google.aip.dev/180): compatibility design reference.
