# LStock

自托管的 A股 / 美股盯盘快照服务。Rust + axum + SQLite，单二进制、Docker 部署，默认只读取公开行情源并按“每日三次快照 + 手动刷新当前页”工作。

## 功能范围

- 三种市场视图：**A股 / 美股 / 双屏**；双屏左右分栏，内容 tab 同步切换，窄屏自动上下堆叠。
- 内容 tab：自选、指数、情绪、策略榜单、板块、涨跌榜、新闻、宏观，全部已接入数据。
  - 板块：A股行业板块（东方财富），美股行业 ETF（腾讯）。
  - 涨跌榜：A股全市场涨幅/跌幅榜（东方财富），美股候选池涨跌排序（腾讯/新浪）。
  - 新闻：东方财富 7×24 财经快讯。
  - 宏观：A股 GDP/CPI/PPI/PMI/M2（东方财富/国家统计局）；美股 VIX 与主要指数（腾讯），10Y/DXY 在当前网络不可用时明确标注。
- 大盘情绪：A股涨跌家数、涨跌停、炸板率、连板高度、成交额、涨跌中位数、新高新低、主力资金；美股 VIX、涨跌家数、新高新低、PCR、10Y、美元指数；输出 0-100 情绪温度与分档。
- 三类策略榜单：短期投资、长期投资、股息投资价值，展示综合分与因子贡献，缺失数据单独列出。
- 快照模型：页面读取最近一次成功快照，不做秒级推送；左下角显示数据时效与 UTC 时间，右下角显示 UTC 时钟、手动刷新与下次刷新时间。
- 启动时在后台自动触发一次全局数据刷新（两个市场的自选、指数、情绪、板块、涨跌榜、新闻、宏观、三类榜单），不阻塞服务启动。
- 刷新任务并发执行，受 `MAX_CONCURRENCY` 限制；同一 scope 仍保持单飞、冷却与排队。
- 全站鉴权：单用户 Argon2 密码、会话 Cookie、CSRF、登录失败限流；支持 HTTPS 反向代理下 Secure Cookie。

## 环境变量

| 变量 | 必填 | 默认值 | 说明 |
|---|---|---|---|
| `AUTH_USER` | 是 | - | 登录用户名 |
| `AUTH_PASSWORD_HASH` | 是 | - | Argon2 PHC 哈希 |
| `DATA_DIR` | 否 | `./data` | 数据目录（SQLite / 快照 / 日志） |
| `DB_PATH` | 否 | `$DATA_DIR/l_stock.db` | SQLite 路径 |
| `BIND_ADDR` | 否 | `127.0.0.1:8080` | 监听地址；容器内用 `0.0.0.0:8080` |
| `REFRESH_TIMES_CN` | 否 | `09:30,13:00,14:45` | A股每日三个刷新时间（Asia/Shanghai） |
| `REFRESH_TIMES_US` | 否 | `09:30,12:00,15:45` | 美股每日三个刷新时间（America/New_York） |
| `REFRESH_OPEN_OFFSET_SECS` | 否 | `60` | 开盘第一个窗口的偏移秒数，避免集合竞价未定 |
| `REFRESH_COOLDOWN_SECS` | 否 | `60` | 同市场 + scope 手动刷新冷却 |
| `SNAPSHOT_RETENTION` | 否 | `30` | 每个 `(market, scope)` 保留的快照份数 |
| `STALE_AFTER_SECS` | 否 | `21600` | 超过该秒数标记“已过期”（默认 6 小时） |
| `MAX_CONCURRENCY` | 否 | `4` | 跨 scope 外部请求并发上限 |
| `HTTP_PROXY` / `HTTPS_PROXY` | 否 | - | 出站代理；容器只允许主动出站时使用 |
| `PROVIDER_TIMEOUT_SECS` | 否 | `10` | 行情源请求超时 |
| `PROVIDER_RETRIES` | 否 | `2` | 行情源有限重试次数 |
| `PROVIDER_USER_AGENT` | 否 | `l_stock/0.1 (+self-hosted)` | 行情请求 UA |
| `TENCENT_BASE_URL` | 否 | `https://qt.gtimg.cn` | 腾讯行情源 |
| `SINA_BASE_URL` | 否 | `https://hq.sinajs.cn` | 新浪行情源 |
| `EASTMONEY_BASE_URL` | 否 | `https://push2.eastmoney.com` | 东财情绪/估值源 |
| `EASTMONEY_DATACENTER_BASE_URL` | 否 | `https://datacenter-web.eastmoney.com` | 东财 F10 财务/分红数据 |
| `EASTMONEY_PUSH2EX_BASE_URL` | 否 | `https://push2ex.eastmoney.com` | 东财涨停/跌停/炸板池 |
| `EASTMONEY_FALLBACK_BASE_URLS` | 否 | `https://push2delay.eastmoney.com,https://push2his.eastmoney.com` | 东财 push2 备用域名（主域名不可达时自动切换） |
| `EASTMONEY_NEWS_BASE_URL` | 否 | `https://np-weblist.eastmoney.com` | 东财 7×24 快讯 |
| `YAHOO_BASE_URL` | 否 | `https://query1.finance.yahoo.com` | Yahoo 行情 / 指数 / 基本面 |
| `STOOQ_BASE_URL` | 否 | `https://stooq.com` | Stooq 美股指数备源 |

`.env.example` 与上表一致，可直接复制为 `.env`。

生成密码哈希：

```bash
cargo run -- hash-password 'change-me'
```

哈希包含 `$`，写进 Compose YAML 时要把每个 `$` 写成 `$$`，或直接生成转义值：

```bash
cargo run -- hash-password 'change-me' | sed 's/\$/$$/g'
```

## 本地运行

```bash
cargo build
DATA_DIR=./data \
AUTH_USER=trader \
AUTH_PASSWORD_HASH='<上一步输出的哈希>' \
BIND_ADDR=127.0.0.1:8080 \
cargo run
```

打开 `http://127.0.0.1:8080` 登录。首次没有快照时页面显示占位提示，点击“手动刷新”抓取当前页数据。

手动刷新命令行：

```bash
DATA_DIR=./data AUTH_USER=trader AUTH_PASSWORD_HASH='<hash>' \
  cargo run -- refresh --market both --tab index
```

## 数据源与延迟口径

| 市场 | 数据种类 | 主源 → 备源 | 延迟口径 |
|---|---|---|---|
| A股 | 自选 / 指数报价 | 腾讯 → 新浪 | 实时（源自身延迟） |
| A股 | 情绪 / 涨跌家数 / 涨跌停 / 资金 | 东方财富 | 实时 |
| 美股 | 自选报价 | 腾讯 → 新浪 → Yahoo | 腾讯/新浪延迟，Yahoo 延迟 |
| 美股 | 指数 | 腾讯 → 新浪 → Stooq → Yahoo | 腾讯/新浪延迟，Stooq 日频 |
| 美股 | 情绪 | 腾讯 VIX → Yahoo（VIX / 10Y / DXY） | 延迟；缺失指标明确标注 |
| A股 | 基本面 / 分红 | 东方财富 F10 | 报告期数据 |
| 美股 | 基本面 | Yahoo quoteSummary | 日频 |
| A股 | 板块 / 涨跌榜 | 东方财富（push2 + push2delay/push2his 备用） | 实时 |
| 美股 | 板块 / 涨跌榜 | 腾讯 → 新浪 | 延迟 |
| 两市 | 新闻 | 东方财富 7×24 快讯 | 实时 |
| A股 | 宏观 | 东方财富 / 国家统计局（GDP/CPI/PPI/PMI/M2） | 月/季度 |
| 美股 | 宏观 | 腾讯 VIX + 主要指数；10Y/DXY 不可用时标注 | 延迟 |

所有请求统一设置超时、有限重试、UA 与可选代理；主源失败会自动降级并在页面标注实际来源。Yahoo 在部分网络会返回 403/429、Stooq 可能返回 404，因此美股报价与指数优先使用腾讯/新浪，Yahoo/Stooq 仅作为兜底。A股基本面与分红使用东方财富 F10；东财 push2 主域名不可达时会自动切换 `push2delay` / `push2his`，并可用腾讯行情补齐 PE/股本/价格。美股基本面仍依赖 Yahoo，若 Yahoo 不可达，美股长期/股息榜单会显示“缺少基本面”。替换数据源只需改 `*_BASE_URL` 或在 `src/providers/` 增加实现，页面与榜单不感知具体来源。

## 快照、刷新与榜单

- 每个市场按本地时区在开盘、盘中、尾盘前 15 分钟各执行一次刷新；交易时段与夏令时由 `chrono-tz` 处理。
- 周末与节假日不发起外部请求，只记录 `skipped`，页面继续展示上一份快照并提示休市。
- 启动刷新、定时刷新和手动刷新都会并发入队，跨 scope 并发受 `MAX_CONCURRENCY` 限制；每个 `(market, scope)` 同时最多一个任务。
- 手动刷新只刷新当前可见市场与当前 tab（榜单包含当前榜单类型）。
- 同一 scope 冷却期内重复刷新返回“刚刚刷新过”，不会重复请求外部源。
- 刷新失败保留旧快照，刷新记录与页面角标显示失败状态。
- 快照按 `SNAPSHOT_RETENTION` 滚动保留；自选、权重与配置不属于快照，不受清理影响。
- 策略榜单候选池默认包含一组主要指数成分股代表（见 `src/rankings.rs`），并自动并入自选；刷新时写入 `ranking_snapshots` 与快照。因子权重可在榜单页调整，保存后下次刷新生效。

## 数据目录与备份

- 数据库、快照、日志都在 `DATA_DIR`；不保存任何外部行情原始文件。
- 备份 `DATA_DIR` 即可保留自选、权重、刷新记录与历史快照。
- 恢复：停止服务，替换 `DATA_DIR`，再启动；迁移会在启动时自动执行。
- 数据库损坏时可删除后重建，但会丢失自选与权重；历史快照无法从外部源回补。

## Docker

镜像通过 GitHub Actions 构建并推送到 GHCR：`ghcr.io/ggflyjean/l-stock:latest`（同时保留每个提交的 `:<sha>` 标签）。工作流见 `.github/workflows/docker.yml`，触发条件为推送 `master`/`main`、打 `v*` 标签或手动运行。

Compose 直接拉取 GHCR 镜像：

```bash
cp .env.example .env    # 填入 AUTH_USER / AUTH_PASSWORD_HASH
docker compose pull
docker compose up -d
```

本机构建并推送到 GHCR（需要 `write:packages` 权限的 PAT）：

```bash
echo "$CR_PAT" | docker login ghcr.io -u ggflyjean --password-stdin
docker buildx build --platform linux/amd64 \
  -t ghcr.io/ggflyjean/l-stock:latest \
  -t ghcr.io/ggflyjean/l-stock:$(git rev-parse --short HEAD) \
  --push .
```

仅在本地构建/运行（不推送）：

```bash
docker build -t l-stock .
docker run --rm -p 18085:8080 \
  -e AUTH_USER=trader \
  -e AUTH_PASSWORD_HASH='<hash>' \
  -e DATA_DIR=/data \
  -v "$PWD/data:/data" \
  l-stock
```

GitHub Actions 每次成功构建还会导出 `l-stock-image.tar.gz` 供离线导入极空间。

默认容器内监听 `0.0.0.0:8080`，以 uid `10001` 非 root 运行，宿主端口默认 `18084`。极空间部署步骤见 [docs/deployment.md](docs/deployment.md)。

## 公网访问与安全清单

推荐 Cloudflare Tunnel 或 Tailscale 主动出站，不要求开放入站端口；详细步骤与对比见 [docs/remote-access.md](docs/remote-access.md)。

上线前必须确认：

- [ ] 使用强密码，并只保存 Argon2 哈希；不要在任何日志或仓库中出现明文。
- [ ] 通过 HTTPS 暴露（隧道或反向代理）；确认 `X-Forwarded-Proto: https` 时会话 Cookie 带 `Secure`。
- [ ] 保留 CSRF 校验；刷新、自选、权重、退出等 POST 操作都必须携带有效令牌。
- [ ] 保留登录失败限流（默认同一来源连续 5 次失败锁定 60 秒）。
- [ ] 优先使用隧道而不是路由器端口映射；确需端口映射时由反向代理终结 TLS。
- [ ] 出站代理只配置在运行环境，不写进镜像；启动日志只输出代理是否启用，不输出凭据。
- [ ] 定期备份 `DATA_DIR`，并确认容器以非 root 用户运行。

## 健康检查与状态

- `GET /health`：数据库可打开时返回 `ok`。
- `GET /status`：最近刷新记录、运行中任务、数据源健康、生效配置与下次刷新时间。
- `GET /api/status`：上述状态的 JSON 版本。
- `POST /api/refresh`：需要登录与 CSRF，表单字段 `market=cn|us|both`、`tab`、`board`。

## 日志与调试

- 默认日志级别为 `info`，会输出每个 HTTP 请求、手动刷新范围、刷新任务开始/完成、每个行情源的尝试与失败原因、快照写入结果。
- 需要更详细的外部请求日志时，在环境变量中设置：

```text
RUST_LOG=l_stock=debug,tower_http=debug
```

- 查看容器日志：

```bash
docker logs -f --tail=200 l-stock-fixed
```

- `/status` 页面每 15 秒自动刷新，也可以点“立即刷新”。
- 手动刷新失败时，页面与日志都会显示具体 scope、数据源和错误信息；刷新记录保存在 `refresh_runs`。

## 测试

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

覆盖：配置、鉴权/CSRF/限流、SQLite 迁移与快照、市场时区/夏令时/节假日、刷新时间计算、单飞/冷却、Provider 解析与降级（wiremock）、GBK 解码、情绪温度、榜单因子、Web 登录/双屏/刷新/自选/权重、端到端“抓取 → 快照 → 渲染”。

## 目录结构

```text
src/
  auth.rs         会话、CSRF、登录限流
  config.rs       环境配置
  db.rs           SQLite 迁移与查询
  market.rs       市场时区、交易时段、节假日、刷新时间
  models.rs       Market/Tab/Board、行情、情绪、榜单模型
  providers/      腾讯/新浪/东财/Yahoo/Stooq
  rankings.rs     候选池、因子、权重、打分
  sentiment.rs    情绪温度
  service.rs      刷新调度、单飞、冷却、快照
  web.rs          Axum 路由、页面、API
  main.rs         CLI 与启动
templates/        Askama 模板
static/           CSS / 前端脚本
tests/            集成测试
docs/             部署与公网访问文档
```
