# iperf hub — 模型管理

## `iperf hub download` — 下载模型

```bash
# 从 HuggingFace 下载
iperf hub download meta-llama/Llama-3-8B --local-dir ./models/llama3

# 从自定义 hub 下载
iperf hub download my-model --source http://my-hub:8080

# 下载指定范围的文件（多机并行下载）
iperf hub download meta-llama/Llama-3-8B --offset 5 --count 10
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
```

### 多机并行下载

大模型文件多时，可在多台机器上分别下载不同范围的文件：

```bash
# 机器 1: 文件 0-9
iperf hub download meta-llama/Llama-3-8B --offset 0 --count 10 --local-dir ./models/llama3

# 机器 2: 文件 10-19
iperf hub download meta-llama/Llama-3-8B --offset 10 --count 10 --local-dir ./models/llama3
```

下载完成后，用 `iperf hub serve` 提供文件服务，其他机器可访问合并后的模型文件。

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
