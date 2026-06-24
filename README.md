# iperf

AI 推理后端性能基准测试工具。通过 OpenAI 兼容 API 对 vLLM、SGLang 等推理后端进行压力测试，测量吞吐量（prefill/decode）、延迟（TTFT、TPOT 百分位）和并发扩展能力。

## 功能特性

- **多后端支持** — vLLM、SGLang（OpenAI 兼容 API）
- **流式/非流式** — SSE 流式解析，逐 token 计时
- **并发工作池** — tokio 异步 worker，高并发低内存
- **配置灵活** — YAML 配置文件 + CLI 参数覆盖
- **多种输出** — Table / JSON / JSONL
- **模型管理** — HuggingFace 下载（断点续传）+ HTTP 文件服务
- **精确指标** — per-token TPOT 百分位，非 per-request 平均
- **Cache 命中率统计** — 自动统计 prompt cache 命中率（需后端支持）
- **进度条** — request-count 模式下显示可视化进度条

## 安装

```bash
cargo build --release
# 二进制文件在 target/release/iperf
```

构建时注入版本信息：

```bash
IPERF_COMMIT=$(git rev-parse --short HEAD) IPERF_BUILT=$(date -u) cargo build --release
```

## 快速开始

```bash
# 基本测试
iperf run -m "qwen/qwen2.5-7b-instruct" -c 4 -d 60s http://localhost:8000/v1

# 使用配置文件
iperf run --conf config.yaml

# 按请求数停止（优先级高于 duration）
iperf run -m "model-name" --request-count 100 -c 2 http://localhost:8000/v1

# 使用代理
iperf run -m "model-name" --http-proxy "http://127.0.0.1:8080" http://remote:8000/v1

# JSON 输出
iperf run -m "model-name" -f json -c 4 -d 30s http://localhost:8000/v1
```

## 命令

### `iperf run` — 运行基准测试

```
Usage: iperf run [OPTIONS] [TARGET]

Options:
      --conf <CONF>                    配置文件路径 (config.yaml)
  -b, --backend <BACKEND>              后端类型 (vllm, sglang) [default: vllm]
  -m, --model <MODEL>                  模型名称
  -c, --concurrency <CONCURRENCY>      并发 worker 数
  -d, --duration <DURATION>            测试时长 (如 "60s", "5m", "1h")
      --request-count <COUNT>          最大请求数 (优先级高于 duration)
  -M, --mode <MODE>                    请求模式: single, stream [default: stream]
      --prompt-tokens <N>              输入 token 数 [default: 256]
      --output-tokens <N>              最大输出 token 数 [default: 256]
      --no-cache                       每个请求前加 UUID（禁用 KV cache）
      --num-prefix-prompts <N>         prompt 池大小 [default: 100]
      --cache-rate <N>                 cache 命中率百分比
      --seed <N>                       随机种子
      --prompt-tokens-stddev <N>       prompt 长度标准差
  -f, --format <FORMAT>                输出格式: table, json [default: table]
      --output-dir <DIR>               JSONL 输出目录 [default: 二进制同级 output/]
      --http-proxy <URL>               HTTP 代理
      --trace                          调试: 打印第一个请求/响应
      --tag <TAG>                      结果标签
      --warmup <N>                     预热请求数（不计入统计）
```

### `iperf config` — 生成默认配置

```bash
iperf config -o config.yaml
```

### `iperf hub download` — 下载模型

```bash
# 从 HuggingFace 下载
iperf hub download meta-llama/Llama-3-8B --local-dir ./models/llama3

# 从自定义 hub 下载
iperf hub download my-model --source http://my-hub:8080

# 下载模型文件,一般用于多台机器同时下载,每个机器下载不同的文件
iperf hub download meta-llama/Llama-3-8B --offset 5 --count 10
```

### `iperf hub serve` — 模型文件服务
多台机器下载模型文件后,启动模型文件服务,其他机器可以访问模型文件,用于合并模型文件。

```bash
iperf hub serve --local-dir ./models --addr 0.0.0.0:8080
```

API:
- `GET /` — 模型列表
- `GET /models/{id}` — 文件列表
- `GET /models/{id}/{file}` — 下载文件（支持 Range）

## 配置文件

```yaml
backend: vllm
base_url: http://localhost:8000/v1
model: qwen/qwen2.5-7b-instruct
concurrent: 4
duration_secs: 300
request_count: 0          # 0 = 不限制
mode: stream
prompt_tokens: 256
output_tokens: 256
no_cache: false
num_prefix_prompts: 100
cache_rate: 0
seed: 0
prompt_stddev: 0
format: table
output_dir: ./output
tag: ""
http_proxy: ""
trace: false
warmup: 0
```

CLI 参数优先级高于配置文件。只在 CLI 显式指定时才覆盖配置。

## 输出指标

### Table 输出

```
IPERF Benchmark Results

  Requests:        42/42 (success/total)
  Throughput:      1.40 req/sec

  Latency (TTFT)
    Mean:          261.5ms
    P50:           217.7ms
    P90:           386.9ms
    P95:           423.8ms
    Min:           177.3ms
    Max:           620.8ms

  Latency (TPOT)
    Mean:          18.5ms
    P50:           17.2ms
    P90:           29.7ms
    P95:           37.5ms
    Min:           6.6ms
    Max:           148.6ms

  Throughput (Tokens/sec)
    Prefill:       178.9 tok/sec
    Decode:        88.7 tok/sec
    Overall:       267.6 tok/sec
    TPM:           16.1K

  Prompt tokens:   5376
  Output tokens:   2688
  Cached tokens:   4800 (89.3%)
  Errors:          0
```

### 指标说明

| 指标 | 说明 |
|------|------|
| **TTFT** | Time To First Token — 首 token 延迟 |
| **TPOT** | Time Per Output Token — 每 token 生成时间 |
| **Prefill tok/s** | 输入吞吐量 = total_prompt_tokens / wall_clock |
| **Decode tok/s** | 输出吞吐量 = total_output_tokens / wall_clock |
| **TPM** | Tokens Per Minute |
| **Req/s** | 每秒请求数 |
| **Cached tokens** | 缓存命中的 prompt tokens 数及命中率百分比 |

### Cache 命中率

当后端支持 prompt caching（如 vLLM 的 `--enable-prefix-caching`）时，iperf 会自动统计 cache 命中率：

- **Stream 模式**：通过 `stream_options: {"include_usage": true}` 获取 usage 信息
- **非 Stream 模式**：直接从响应的 `usage.prompt_tokens_details.cached_tokens` 获取
- **统计方式**：累加所有请求的 `cached_tokens` 和 `prompt_tokens`，计算总命中率

测试 cache 命中率时，建议使用 `--num-prefix-prompts 1` 让所有请求使用相同 prompt：

```bash
iperf run -m "model-name" --request-count 100 --num-prefix-prompts 1 http://localhost:8000/v1
```

### 进度条

当使用 `--request-count` 时，会显示可视化进度条：

```
  [==================>         ] 15/20 requests, 0 errors
```

### JSONL 输出

结果自动追加到 `{output_dir}/{model_name}.jsonl`（或 `{model_name}-{tag}.jsonl`）。

## 架构

```
src/
├── main.rs          # tokio 入口 + 子命令分发
├── cli.rs           # clap CLI 定义
├── error.rs         # thiserror 结构化错误
├── config.rs        # Config + YAML + Default
├── cmd_run.rs       # run 子命令
├── cmd_config.rs    # config 子命令
├── backend/
│   ├── mod.rs       # Backend trait + 注册表
│   ├── openai.rs    # OpenAI HTTP + SSE 流解析
│   ├── vllm.rs      # vLLM 注册
│   └── sglang.rs    # SGLang 注册
├── benchmark/
│   ├── mod.rs       # PromptGenerator
│   └── runner.rs    # tokio worker 池
├── metrics/
│   ├── mod.rs       # Sample + Collector
│   └── stats.rs     # Stats + 百分位数
├── report/mod.rs    # Table/JSON/JSONL 渲染
├── download/mod.rs  # HuggingFace + Hub 下载器
└── hub/mod.rs       # axum HTTP 文件服务
```

## 停止条件

- `--request-count N` — 完成 N 个请求后停止（优先级最高）
- `--duration Ds` — 到达时长后停止
- 两者都不设 — 默认 duration=3600s
- `--request-count` 和 `--duration` 同时设置时，`--request-count` 优先，`--duration` 被忽略

## License

MIT
