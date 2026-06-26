# iperf hub — 模型文件服务

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
