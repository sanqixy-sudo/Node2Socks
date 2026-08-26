# 12 — Cloud Docker 与反代部署要求

从仓库根目录部署：

```bash
cp examples/cloud/.env.example examples/cloud/.env
# 将 NODE2SOCKS_CLOUD_JWT_SECRET 替换为至少 32 字符的随机值
docker compose --env-file examples/cloud/.env -f examples/cloud/docker-compose.yml up -d --build
```

健康检查：

```bash
curl http://127.0.0.1:18080/healthz
```

停止：

```bash
docker compose --env-file examples/cloud/.env -f examples/cloud/docker-compose.yml down
```

默认容器：

```text
node2socks-cloud
  listen: 0.0.0.0:8080
  data: /data
```

宿主建议只绑定本机：

```yaml
ports:
  - "127.0.0.1:18080:8080"
```

然后用户自己 Nginx：

```nginx
server {
    listen 443 ssl http2;
    server_name sync.example.com;

    location / {
        proxy_pass http://127.0.0.1:18080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Node2Socks 客户端只保存：

```text
https://sync.example.com
```

不能要求固定二级路径；如果支持 path base，应通过 `server-info` 正确拼接。
