# 极空间 Docker 部署运行手册

## 1. 准备数据目录

在极空间上创建数据目录，例如：

```text
/public/LStock-data
```

容器以 uid `10001` 运行，目录需要可写。若能在极空间终端执行：

```bash
mkdir -p /public/LStock-data
chown -R 10001:10001 /public/LStock-data
```

无法改属主时，可临时把 Compose 的 `user` 设为 `"0:0"` 完成初始化，再改回非 root。

## 2. 准备环境变量

在本地生成密码哈希：

```bash
cd l_stock
cargo run -- hash-password '你的强密码'
```

创建 `.env`（可复制 `.env.example`）：

```text
AUTH_USER=trader
AUTH_PASSWORD_HASH=<上一步输出>
LSTOCK_IMAGE=l-stock:latest
LSTOCK_PORT=18084
LSTOCK_DATA_DIR=/public/LStock-data
REFRESH_TIMES_CN=09:30,13:00,14:45
REFRESH_TIMES_US=09:30,12:00,15:45
HTTPS_PROXY=
```

> Argon2 哈希包含 `$`，直接粘贴到 Compose YAML 时需写成 `$$`。可生成转义值：
>
> ```bash
> cargo run -- hash-password '你的强密码' | sed 's/\$/$$/g'
> ```
>
> 使用 `.env` 时通常不需要手动转义，Compose 会从 env 文件读取原始值。

## 3. 构建并导入镜像

方式 A：GHCR 直接拉取（推荐）

GitHub Actions 会把镜像推送到 `ghcr.io/ggflyjean/l-stock:latest` 和 `ghcr.io/ggflyjean/l-stock:<sha>`：

```bash
# .env 中保持
LSTOCK_IMAGE=ghcr.io/ggflyjean/l-stock:latest

docker compose pull
docker compose up -d
```

如果 package 是私有的，先登录：

```bash
echo "$CR_PAT" | docker login ghcr.io -u ggflyjean --password-stdin
```

方式 B：从 GitHub Actions 下载镜像 tar（GHCR 不可达时）

1. 打开仓库 Actions → `docker` → 最近一次成功运行。
2. 页面底部 Artifacts 下载 `l-stock-image`。
3. 解压后得到 `l-stock-image.tar.gz`，再 `gunzip l-stock-image.tar.gz`。
4. 在极空间 Docker 界面导入 `l-stock-image.tar`（镜像名是 `l-stock:local`）。
5. 把 `.env` 中 `LSTOCK_IMAGE` 改为 `l-stock:local` 后启动。

方式 C：在开发机构建后导出（不需要 NAS 编译）

```bash
docker build -t l-stock:local .
docker save l-stock:local -o l-stock-local.tar
```

把 `l-stock-local.tar` 上传到极空间，在 Docker 界面导入镜像，然后把 `LSTOCK_IMAGE` 改为 `l-stock:local`。

方式 D：在极空间构建源码（不推荐；Rust release 编译消耗 CPU/内存）。

## 4. 启动

在 NAS 上准备 `docker-compose.yml` 与 `.env` 后：

```bash
docker compose up -d
docker compose ps
docker compose logs -f --tail=100
```

启动日志应包含监听地址、数据目录、刷新时间表、代理是否启用、行情源超时/重试；不会输出密码哈希或代理凭据。

## 5. 首次配置

1. 打开 `http://<nas>:18084`，用 `AUTH_USER` / 密码登录。
2. 默认视图为双屏，页面展示 A股与美股占位提示。
3. 在“自选”tab 的输入框添加代码，例如 `600519` / `AAPL`。
4. 点击右下角“手动刷新”，只刷新当前页面（当前市场 + 当前 tab）。
5. 打开“指数”“情绪”“策略榜单”页面，分别手动刷新一次，确认数据与数据源标注。
6. 打开 `/status` 查看刷新记录、运行中任务、数据源健康与下次刷新时间。

首次抓取依赖外网行情源；若 NAS 只允许代理出站，在 `.env` 设置 `HTTPS_PROXY` 后重启：

```bash
docker compose up -d
```

## 6. 结果核对

- 自选列表应显示代码、名称、价格、涨跌幅与成交额。
- 指数页应显示上证指数 / 深证成指 / 创业板指，以及道指 / 纳指 / 标普 500。
- 情绪页应显示 0-100 温度、分档与指标；缺失指标会明确列出。
- 策略榜单应显示综合分、排名与因子贡献；“数据不足”折叠区列出被排除的标的。
- 左下角显示两个市场的快照时间（本地时间 + UTC + 相对年龄）；右下角 UTC 时钟每秒更新。
- 页面超过 `STALE_AFTER_SECS` 未刷新时会提示“已过期”；最近一次刷新失败会提示“上次刷新失败”。

## 7. 备份

只需备份 `DATA_DIR`（默认 `/public/LStock-data`）：

```bash
tar -czf /public/LStock-data-$(date +%F).tar.gz -C /public LStock-data
```

备份包含 SQLite、历史快照、自选与权重。行情数据可从外部源重新抓取，但历史快照不能回补。

## 8. 升级

```bash
docker compose pull        # 使用仓库镜像时
docker compose up -d
```

或重新导入新的本地镜像后 `docker compose up -d`。启动时自动执行数据库迁移；升级前建议备份 `DATA_DIR`。

## 9. 回滚

```bash
docker compose down
# 如需完全重建索引与配置：
rm -rf /public/LStock-data
docker compose up -d
```

回滚到旧镜像时，若数据库已执行新迁移，请先恢复对应版本的 `DATA_DIR` 备份。

## 10. 刷新策略

- 默认每个市场每日三次：A股 `09:30 / 13:00 / 14:45`，美股 `09:30 / 12:00 / 15:45`（市场本地时间）。
- 开盘窗口默认延后 60 秒，避免集合竞价数据未定。
- 周末与节假日记录 `skipped`，不发起外部请求。
- 手动刷新仅当前页；同一 scope 默认 60 秒冷却，可调 `REFRESH_COOLDOWN_SECS`。
- 免费行情源可能限流或失效，页面会标注降级来源；长期使用建议关注 `/status` 的数据源健康。
