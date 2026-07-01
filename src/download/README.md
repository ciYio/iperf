# iperf hub — 模型管理

## `iperf hub download` — 下载模型

```bash
# 从 HuggingFace 下载
iperf hub download meta-llama/Llama-3-8B --local-dir ./models/llama3

# 从自定义 hub 下载
iperf hub download my-model --source http://my-hub:8080

# 下载指定范围的文件（多机并行下载）
iperf hub download meta-llama/Llama-3-8B --offset 5 --count 10

# 检查文件完整性（不下载）
iperf hub download meta-llama/Llama-3-8B --check-only

# 检查特定文件
iperf hub download meta-llama/Llama-3-8B --check-only --target model-00001-of-00004.safetensors
```

### 参数

```
Usage: iperf hub download [OPTIONS] <MODEL_ID>

Arguments:
  <MODEL_ID>               HuggingFace 模型 ID (如 "meta-llama/Llama-3-8B")

Options:
      --local-dir <DIR>    本地目录 [default: ./models/<model_id>]
  -r, --revision <REV>     分支/版本 [default: main]
      --source <URL>       自定义 hub URL
      --http-proxy <URL>   HTTP 代理
      --offset <N>         跳过前 N 个文件 [default: 0]
      --count <N>          下载 N 个文件 (0 = 全部) [default: 0]
      --role <N/M>         分布式下载角色 (如 "1/4")
      --check-only         仅检查文件完整性，不下载
      --target <FILE>      指定检查的文件名（配合 --check-only 使用）
```

### SHA256 校验

- **HuggingFace 下载**：自动获取 LFS 文件的 SHA256，下载完成后校验
- **自定义 Hub 下载**：仅检查文件存在性（无 SHA256）
- **校验进度**：计算 SHA256 时显示进度条

```
[1/4] model-00001-of-00004.safetensors - COMPUTING [=====>           ] 1.2GB/4.5GB (120MB/s)
[1/4] model-00001-of-00004.safetensors - OK [sha256: a4cd1f1a04d90b75...]
```

### 失败重试

下载失败时自动重试，间隔指数增长（最大 5 分钟）：
- 第 1 次失败：等待 1 秒
- 第 2 次失败：等待 2 秒
- 第 3 次失败：等待 4 秒
- ...
- 最大间隔：5 分钟

### 多机并行下载

大模型文件多时，可在多台机器上分别下载不同范围的文件：

```bash
# 机器 1: 文件 0-9
iperf hub download meta-llama/Llama-3-8B --offset 0 --count 10 --local-dir ./models/llama3

# 机器 2: 文件 10-19
iperf hub download meta-llama/Llama-3-8B --offset 10 --count 10 --local-dir ./models/llama3
```

下载完成后，用 `iperf hub serve` 提供文件服务，其他机器可访问合并后的模型文件。

### 分布式下载（--role）

使用 `--role N/M` 参数可以更方便地在多台机器上并行下载：

```bash
# 4 台机器并行下载
iperf hub download model --role 1/4  # 机器 1
iperf hub download model --role 2/4  # 机器 2
iperf hub download model --role 3/4  # 机器 3
iperf hub download model --role 4/4  # 机器 4
```

**分布式下载逻辑**：
- 文件分为两类：`.safetensors`（大文件）和其他（小文件）
- `--role 1/N`：下载所有小文件 + 第 1 份 safetensors
- `--role 2/N, 3/N, ...`：只下载对应的 safetensors 分片
- 小文件只下载一次，避免重复

## `iperf hub serve` — 模型文件服务

多台机器下载模型文件后，启动文件服务供其他机器访问。

```bash
iperf hub serve --local-dir ./models --addr 0.0.0.0:8080
```

### 参数

```
Usage: iperf hub serve [OPTIONS] --local-dir <DIR>

Options:
      --local-dir <DIR>    模型目录
  -a, --addr <ADDR>        监听地址 [default: 0.0.0.0:8080]
```

### API

- `GET /` — 模型列表
- `GET /models/{id}` — 文件列表
- `GET /models/{id}/{file}` — 下载文件（支持 Range）
