# iperf watch — 实时监控

周期性采集 GPU 和推理服务指标，输出 JSONL 文件。

## 用法

```bash
iperf watch [OPTIONS] [TARGET]
```

## 参数

```
Options:
  -i, --interval <SECONDS>             采集间隔 [default: 2]
      --nsys                           启用 nsys GPU profiling
  -d, --duration <DURATION>            运行时长 (如 "60s", "5m", "1h")
  -b, --backend <BACKEND>              后端类型 (vllm, sglang)
  -m, --model <MODEL>                  模型名称（用于 JSONL 文件名）
      --tag <TAG>                      标签（用于 JSONL 文件名）
      --output-dir <DIR>               JSONL 输出目录 [default: 二进制同级 output/]
```

## 示例

```bash
# 基本监控，2 秒间隔
iperf watch -m "qwen/qwen2.5-7b-instruct" http://localhost:8000/v1

# 启用 nsys profiling
iperf watch -m "model-name" --nsys -d 60s http://localhost:8000/v1

# 指定输出目录和标签
iperf watch -m "model-name" --tag "test1" --output-dir ./results -i 5
```

## 输出

每次 watch 会话生成唯一 UUID，指标写入 `{output_dir}/{model}-{tag}.jsonl`（tag 为空时 `{model}.jsonl`）。

JSONL 每行格式：

```json
{
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2026-06-26T10:30:00.123Z",
  "gpu": [
    {
      "gpu_index": 0,
      "gpu_name": "NVIDIA A100-SXM4-80GB",
      "gpu_utilization": 45,
      "memory_utilization": 23,
      "memory_used": 12345,
      "memory_total": 81920,
      "power_draw": 234.5,
      "temperature_gpu": 52
    }
  ],
  "inference": {
    "num_requests_running": 5,
    "num_requests_waiting": 2,
    "gpu_cache_usage_perc": 0.75,
    "ttft_avg_ms": 120.5,
    "tpot_avg_ms": 25.3,
    "generation_tokens_total": 10000
  },
  "derived": {
    "generation_tokens_per_sec": 150.0,
    "requests_per_sec": 2.5
  }
}
```

## 指标类型

| 优先级 | 来源 | 采集方式 | 说明 |
|--------|------|----------|------|
| P0 | 推理服务 | HTTP GET `/metrics`（Prometheus 格式） | 请求数、队列、cache、TTFT、TPOT、吞吐量 |
| P1 | GPU 硬件 | `nvidia-smi` CSV | 利用率、显存、功耗、温度、时钟频率 |
| P2 | 系统追踪 | `nsys profile`（`--nsys` 开关） | kernel 耗时、memcpy 摘要 |

### P0 推理指标

支持 vLLM（`vllm:`）和 SGLang（`sglang:`）前缀，通过 `--backend` 指定。

| 指标 | 说明 |
|------|------|
| `num_requests_running` | 正在处理的请求数 |
| `num_requests_waiting` | 等待中的请求数 |
| `gpu_cache_usage_perc` | GPU KV cache 使用率 |
| `ttft_avg_ms` | 首 token 平均延迟 |
| `tpot_avg_ms` | 每 token 平均生成时间 |
| `e2e_latency_avg_ms` | 端到端平均延迟 |
| `prompt_tokens_total` | 累计 prompt tokens |
| `generation_tokens_total` | 累计生成 tokens |

衍生指标（通过 counter delta / interval 计算）：
- `generation_tokens_per_sec` — 生成吞吐量
- `prompt_tokens_per_sec` — 输入吞吐量
- `requests_per_sec` — 请求吞吐
- `throughput_per_gpu` — 每 GPU 吞吐量

### P1 GPU 指标

通过 `nvidia-smi --query-gpu=... --format=csv` 采集。不可用时静默跳过。

| 指标 | 说明 |
|------|------|
| `gpu_utilization` | GPU 利用率 (%) |
| `memory_utilization` | 显存利用率 (%) |
| `memory_used` / `memory_total` / `memory_free` | 显存 (MiB) |
| `power_draw` / `power_limit` | 功耗 (W) |
| `temperature_gpu` | 温度 (°C) |
| `clock_sm` / `clock_memory` | 时钟频率 (MHz) |
| `pstate` | 性能状态 (P0-P12) |
| `throttle_reasons` | 降频原因 |

### P2 nsys 追踪

`--nsys` 启用，结束时自动停止 nsys 并解析 kernel/memcpy 摘要。

## 优雅降级

- `nvidia-smi` 不存在 → GPU 字段为 null，不报错
- `/metrics` 不可达 → inference 字段为 null，warning 到 stderr
- `nsys` 启动失败 → 降级为不追踪，warning 到 stderr

## 终端显示

每次采集后往 stderr 输出一行摘要：

```
[10:30:02] GPU0: 45% | Mem: 12345/81920 MiB | Power: 234W | Running: 5 | TTFT: 120ms | TPOT: 25ms | Gen: 150 tok/s
```
