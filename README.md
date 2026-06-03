<!--
这份 README 已按当前项目状态重写。
-->

<p align="center">
  <img src="pi_agent_rust_illustration.webp" alt="Pi Agent Rust" width="600"/>
</p>

<h1 align="center">pi_agent_rust</h1>

<p align="center">
  <strong>面向终端、RPC 和编辑器集成的 Rust 原生 AI Coding Agent CLI。</strong>
</p>

<p align="center">
  <a href="#当前状态">当前状态</a> •
  <a href="#快速开始">快速开始</a> •
  <a href="#功能">功能</a> •
  <a href="#架构">架构</a> •
  <a href="#配置">配置</a> •
  <a href="#开发">开发</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-2024%20edition-orange?logo=rust" alt="Rust 2024">
  <img src="https://img.shields.io/badge/runtime-tokio-blue" alt="Tokio runtime">
  <img src="https://img.shields.io/badge/unsafe-forbidden-brightgreen" alt="No Unsafe Code">
</p>

## 当前状态

`pi_agent_rust` 是 Pi coding-agent 工作流的 Rust 实现。当前代码库聚焦于原生 CLI 能力：
交互式 TUI、流式 LLM provider、内置本地工具、会话持久化、资源包、RPC/ACP 集成，以及语义化工作区上下文。

这个仓库已经和旧 README 描述的状态有明显差异：

- 主异步运行时已经切换为 `tokio`，并通过本地 `pi-runtime` crate 做统一封装。
- HTTP 栈已经切换为 `pi-http`，底层使用 `reqwest` 和 `rustls`。
- 旧的 JS/TS extension runtime 和 WASM runtime 不再是当前产品面。
- AWS Bedrock provider 已从当前 provider 面删除。
- 项目正在按功能拆分为多个 workspace crate，用于降低重编译成本并隔离依赖。
- README 不再发布没有当前证据支撑的 benchmark 数字。

## 快速开始

```bash
# 从源码构建
cargo build --release

# 启动交互式会话
target/release/pi

# 单次提问并直接输出结果
target/release/pi -p "Summarize this repository"

# 继续当前项目最近一次会话
target/release/pi --continue

# 从会话选择器恢复
target/release/pi --resume
```

Provider 凭证通常通过环境变量提供：

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export GOOGLE_API_KEY="..."
export COHERE_API_KEY="..."
```

需要时可以显式选择 provider 和 model：

```bash
pi --provider anthropic --model claude-opus-4 "Review this diff"
pi --provider openai --model gpt-4.1 -p "Explain this error"
pi --list-providers
pi --list-models
```

## 功能

### 交互模式

| 模式 | 命令 | 用途 |
|---|---|---|
| 交互式 TUI | `pi` | 终端对话界面，支持流式输出、工具渲染、模型/会话控制和键盘导航。 |
| Print mode | `pi -p "..."` | 非交互单次提问，适合脚本和快速检查。 |
| JSON 输出 | `pi -p --mode json "..."` | 机器可读的响应输出。 |
| RPC mode | `pi --rpc` 或 `pi --mode rpc` | 基于 stdin/stdout 的逐行 JSON 协议，供外部客户端接入。 |
| ACP mode | `pi --acp` | 面向编辑器集成的 Agent Client Protocol 接口。 |

### 内置工具

Pi 向模型暴露 8 个 Rust 原生工具：

| 工具 | 作用 |
|---|---|
| `read` | 读取文本文件，带行号/元数据；启用图片特性时支持图片输入。 |
| `write` | 创建或覆盖文件。 |
| `edit` | 对已有文件执行精确文本替换。 |
| `bash` | 执行 shell 命令，包含进程处理和超时控制。 |
| `grep` | 搜索文件内容并返回上下文。 |
| `find` | 按路径或 glob 查找文件。 |
| `ls` | 列出目录内容。 |
| `hashline_edit` | 使用 `read`/`grep` 输出中的 line/hash 标记进行精确编辑。 |

这些工具都在 Rust 中实现，并通过 `ToolRegistry` 注册。Agent loop 会把 JSON Schema 工具定义发送给 provider，并把工具结果写回会话。

### Provider 支持

当前原生 provider 模块包括：

- Anthropic Messages API
- OpenAI Chat Completions
- OpenAI Responses API
- OpenAI Codex Responses route
- Google Gemini
- Google Gemini CLI / Antigravity 兼容 route
- Google Vertex AI
- Cohere Chat
- Azure OpenAI
- GitHub Copilot
- GitLab Duo

模型注册表还支持通过 `models.json` 和 provider metadata 接入 OpenAI-compatible provider。运行 `pi --list-providers` 可以查看当前构建支持的 canonical ID、alias 和凭证环境变量。

### 流式输出

Provider streaming 由 Rust HTTP/SSE 栈处理：

- `pi-http::http::client` 封装 `reqwest`。
- TLS 通过 `reqwest` 使用 `rustls`。
- `pi-http::sse` 解析 server-sent events。
- 各 provider adapter 把上游流转换为 Pi 的 `StreamEvent`。

请求超时可通过 `--request-timeout`、`PI_HTTP_REQUEST_TIMEOUT_SECS` 或 settings 中的 `requestTimeoutSecs` 控制。Ollama 这类本地 provider 默认超时时间比云端 provider 更长，以适配模型冷启动。

### 会话

会话保存对话历史、模型切换、工具调用、压缩事件，以及分支/树结构元数据。

当前支持的存储形态：

- JSONL session 文件。
- 启用 `sqlite-sessions` feature 时支持 SQLite session 文件。
- Session index 元数据，用于快速 resume 和 session picker。
- v2 sidecar store 路径的 session migration 命令。

常用命令：

```bash
pi --continue
pi --resume
pi --session /path/to/session.jsonl
pi --export /path/to/session.html
pi migrate ~/.pi/sessions --dry-run
```

### 资源

Pi 可以发现和加载以下资源包：

- skills
- prompt templates
- themes

资源包可以全局安装，也可以按项目安装：

```bash
pi install npm:@scope/pi-resources
pi install ./local-resource-package --local
pi list
pi remove npm:@scope/pi-resources
pi update
```

解析顺序是命令行覆盖项、项目 `.pi/`，再到全局 Pi 配置目录。

### 主题和 TUI

终端 UI 使用 `rich_rust` 和 `charmed_rust` 相关栈：

- `rich_rust` 负责终端样式输出和 markdown 渲染。
- `charmed-bubbletea`、`charmed-lipgloss`、`charmed-bubbles`、`charmed-glamour` 负责交互式 UI 结构和渲染。
- `crossterm` 负责底层终端控制。

主题发现可以通过以下参数控制：

```bash
pi --theme default
pi --theme ./theme.json
pi --theme-path ./themes
pi --no-themes
```

### 语义化工作区上下文

`context-preview` 会为任务构建建议性的语义上下文包：

```bash
pi context-preview "why is provider streaming failing?"
pi context-preview --format json --changed-path src/providers/openai.rs
```

图构建器会索引代码、测试、README/docs、证据文件和 Beads 元数据。当前 Rust 符号/调用抽取正在迁移到 `tree-sitter`，并通过 language extractor 抽象接入；后续新增其他语言时不需要重写图核心。

这个功能只提供上下文建议，不替代测试、Beads、Git 或 evidence gate 作为事实来源。

## 架构

当前高层流程：

```text
CLI / TUI / RPC / ACP
        |
        v
startup + config + auth + model selection
        |
        v
AgentSession + Agent loop
        |
        +--> Provider trait implementations
        |        |
        |        +--> pi-http + reqwest + SSE parser
        |
        +--> ToolRegistry
        |        |
        |        +--> read/write/edit/bash/grep/find/ls/hashline_edit
        |
        +--> Session persistence
                 |
                 +--> JSONL / optional SQLite / session index
```

Workspace crates：

| Crate | 作用 |
|---|---|
| `pi_agent_rust` | 根 binary 和 `pi` CLI 的主实现 crate。 |
| `pi-core` | 核心 model、provider、error 和共享数据类型。 |
| `pi-http` | HTTP client 封装、SSE parser 和 provider VCR 支持。 |
| `pi-runtime` | 基于 Tokio 的 runtime、channel、fs/io/time/sync 封装和请求上下文。 |
| `pi-theme` | 主题加载和 TUI 样式配置。 |
| `pi-observability` | 轻量 metrics 和 observability helper。 |
| `pi-platform` | 平台相关 helper。 |

公共 library surface 有意保持较窄。外部消费者应优先使用 `pi::sdk`；其他多数模块都是实现细节。

## 配置

配置会从全局 settings、项目 settings、环境变量和 CLI 参数中加载。CLI 参数优先级最高。

常见路径：

| 路径 | 用途 |
|---|---|
| `~/.pi/settings.json` 或平台对应配置路径 | 全局 settings。 |
| `.pi/settings.json` | 项目级 settings。 |
| `~/.pi/models.json` | Model registry 覆盖项。 |
| `~/.pi/sessions/` | 默认 session 目录。 |
| `~/.pi/packages/` | 已安装资源包目录。 |

常用环境变量：

| 变量 | 用途 |
|---|---|
| `PI_PROVIDER` | 默认 provider ID。 |
| `PI_MODEL` | 默认 model ID。 |
| `PI_CONFIG_PATH` | 覆盖 settings 路径。 |
| `PI_CODING_AGENT_DIR` | 覆盖全局 Pi 数据/配置根目录。 |
| `PI_SESSIONS_DIR` | 覆盖 session 存储目录。 |
| `PI_PACKAGE_DIR` | 覆盖 package 存储目录。 |
| `PI_HTTP_REQUEST_TIMEOUT_SECS` | 覆盖 provider 请求超时；`0` 表示不设超时。 |
| `PI_MAX_TOOL_ITERATIONS` | 覆盖每轮 prompt 最大工具调用次数。 |

`settings.json` 示例：

```json
{
  "provider": "anthropic",
  "model": "claude-opus-4",
  "requestTimeoutSecs": 60,
  "sessionStore": "jsonl",
  "theme": "default"
}
```

OpenAI-compatible provider 的 `models.json` 示例：

```json
{
  "models": [
    {
      "id": "acme-large",
      "provider": "acme",
      "api": "openai-completions",
      "baseUrl": "https://api.acme.example/v1",
      "apiKey": "$ENV:ACME_API_KEY",
      "contextWindow": 128000,
      "maxTokens": 8192
    }
  ]
}
```

## 安装

本地构建：

```bash
cargo build --release
cp target/release/pi ~/.local/bin/pi
```

通过 Cargo 安装：

```bash
cargo install --path . --locked
```

可选 features：

| Feature | 作用 |
|---|---|
| `sqlite-sessions` | 启用 SQLite session 存储，默认开启。 |
| `image-resize` | 启用图片 resize 支持。 |
| `clipboard` | 启用剪贴板集成。 |
| `syntax-highlighting` | 通过 `rich_rust` 启用语法高亮。 |
| `full` | 启用打包的可选 feature 集合。 |

## 开发

项目使用 Rust 2024，并禁止 unsafe code。

常用检查：

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

在多 agent 环境中执行较重的本地构建时，建议隔离构建产物和临时目录：

```bash
export CARGO_TARGET_DIR="/data/tmp/pi_agent_rust_cargo/${USER:-agent}/target"
export TMPDIR="/data/tmp/pi_agent_rust_cargo/${USER:-agent}/tmp"
mkdir -p "$CARGO_TARGET_DIR" "$TMPDIR"
```

项目结构：

```text
src/
  main.rs                    CLI 入口
  cli.rs                     Clap 参数模型
  agent.rs                   Agent loop 和工具迭代
  providers/                 原生 provider 实现
  tools.rs                   内置工具实现
  session.rs                 Session 模型和持久化路由
  session_index.rs           Session index 元数据
  session_sqlite.rs          可选 SQLite session backend
  interactive/               TUI 状态、渲染、命令和输入处理
  rpc.rs                     RPC 协议实现
  acp.rs                     ACP 协议实现
  resources.rs               skills、prompts、themes 和 package 资源加载
  semantic_workspace_graph.rs 语义上下文图构建器

crates/
  pi-core/
  pi-http/
  pi-runtime/
  pi-theme/
  pi-observability/
  pi-platform/
```

## 已移除或非当前功能面

下面这些旧 README 主题不再是当前产品承诺：

- JS/TS extension runtime
- WASM extension runtime
- extension hostcall policy、extension catalog 和 extension conformance claims
- AWS Bedrock provider 支持
- 以 `asupersync` 作为主 runtime 或 HTTP 栈
- 没有新证据文件支撑的 release-facing benchmark 数字

仓库清理过程中，部分历史 docs、tests 或文件名可能仍会提到旧功能面。判断当前实现面时，以 `Cargo.toml`、`src/lib.rs`、`src/main.rs` 和 workspace crates 实际接线为准。

## 许可证

MIT License with project rider。详见 [LICENSE](LICENSE)。
