# 公网访问方案

LStock 默认只建议在局域网使用。需要外网访问时，优先选择“主动出站隧道”，避免在路由器开放入站端口。无论哪种方式，都必须保留单用户登录、CSRF 校验与登录限流。

## 方案对比

| 方案 | 是否开放入站端口 | 身份层 | 维护成本 | 适用场景 |
|---|---|---|---|---|
| Tailscale | 否 | Tailscale 账号 + 应用登录 | 低 | 个人设备、手机与 NAS 同一 tailnet |
| Cloudflare Tunnel + Access | 否 | Cloudflare Access + 应用登录 | 中 | 已有域名，想用浏览器/手机直接访问 |
| 极空间远程访问 | 取决于产品 | 极空间账号 + 应用登录 | 低 | 已在用极空间远程访问且端口可映射 |
| FRP / rathole + VPS | 否（需 VPS） | 应用登录 | 中高 | 已有公网 VPS，愿意自行维护 |
| 路由器端口映射 + 反向代理 | 是 | 反向代理 TLS + 应用登录 | 中 | 不推荐直接裸奔 |

## Tailscale（推荐）

1. NAS 与手机/电脑安装 Tailscale 并登录同一 tailnet。
2. 在 NAS 上启动 LStock，宿主端口默认 `18084`。
3. 访问 `http://<nas-tailscale-ip>:18084`。

```bash
tailscale ip -4
curl -I http://<tailscale-ip>:18084/health
```

注意：

- Tailscale 地址仅在 tailnet 内可达；不要额外把端口映射到公网。
- 若通过 Tailscale Serve/Funnel 暴露，Funnel 属于公网方案，必须重新评估身份与 TLS。

## Cloudflare Tunnel + Access

1. 在 Cloudflare Zero Trust 创建 Tunnel，安装 `cloudflared` 到 NAS 或同网段机器。
2. 配置 Public Hostname：
   - Domain: `stock.example.com`
   - Service: `http://127.0.0.1:18084`
3. 在 Access 中为该域名添加策略，只允许指定邮箱或身份提供商。
4. 启动隧道：

```bash
cloudflared tunnel run l-stock
```

5. 浏览器访问 `https://stock.example.com`，先通过 Access，再输入 LStock 用户名密码。

关键点：

- Cloudflare 会在请求中带 `X-Forwarded-Proto: https`，LStock 会据此把会话 Cookie 标记为 `Secure`。
- 不需要在路由器开放 18084。
- Access 是额外身份层；LStock 自身的强密码与 CSRF 仍然必须保留。

## 极空间远程访问

如果极空间 Docker 应用能把容器端口映射到极空间远程访问域名：

1. 在极空间 Docker 中把 LStock 的 `8080` 映射到宿主端口。
2. 在极空间远程访问中配置该端口。
3. 用分配的远程域名访问，并确认 HTTPS 生效。

注意：不同极空间型号/固件的远程访问能力不同，不能假设任意端口都可直接暴露；若只能访问管理端口，优先改用 Tailscale 或 Cloudflare Tunnel。

## FRP / rathole + VPS

适合已有公网 VPS 的情况：

1. VPS 运行 frps/rathole 服务端，NAS 运行客户端，把本地 `18084` 映射到 VPS 端口。
2. VPS 用 Caddy/Nginx 终结 TLS，反向代理到本地映射端口。
3. 限制 VPS 防火墙只开放 443 与管理端口。

维护点：证书续期、VPS 安全更新、隧道断线重连。

## 路由器端口映射（不推荐）

如确需直接暴露：

1. 只映射反向代理端口（如 443），不要直接映射 LStock 的 8080。
2. 反向代理必须启用 HTTPS，并把 `X-Forwarded-Proto` 正确传给后端。
3. 使用强密码，确认登录限流与 CSRF 生效。
4. 定期查看 `/status` 的刷新记录，异常流量与异常登录尝试都应关注。

## 公网安全检查

- [ ] 不暴露数据库文件或数据目录。
- [ ] 不把 `AUTH_PASSWORD_HASH`、代理凭据写进仓库或镜像。
- [ ] HTTPS 下确认 Cookie 带 `Secure`。
- [ ] 连续登录失败会被短时锁定。
- [ ] 刷新、自选、权重、退出等 POST 操作均校验 CSRF。
- [ ] 优先使用隧道；端口映射只作为最后选项。
- [ ] 定期备份 `DATA_DIR`，并验证可恢复。
