# Autonomics Data Lake — Iceberg Stack

Iceberg 数据湖的 Docker 基础设施：Garage (S3 对象存储) + Postgres (元数据) + Lakekeeper (REST Catalog)。

## 目录结构

```
infra/docker/
├── docker-compose.yml              # 主 compose 文件
├── .env.example                    # 环境变量模板（安全提交）
├── .env                            # 真实密钥（gitignored）
├── Makefile                        # 便捷命令
├── garage/
│   └── garage.toml.template        # Garage 配置模板（密钥从 .env 注入）
└── scripts/
    └── entrypoint-garage.sh        # 启动时渲染 garage.toml
```

## 服务

| 服务 | 镜像 | 端口 | 用途 |
|------|------|------|------|
| **iceberg-catalog-db** | `postgres:16` | — | Lakekeeper 元数据存储 |
| **garage** | `dxflrs/garage:v2.0.0` | 3900-3903 | S3 兼容对象存储 |
| **lakekeeper-migrate** | `quay.io/lakekeeper/catalog` | — | 一次性数据库迁移 |
| **lakekeeper** | `quay.io/lakekeeper/catalog` | 8181 | Iceberg REST Catalog |

## 快速开始

```bash
cd infra/docker
cp .env.example .env
# 编辑 .env，填入真实密钥
make init    # 验证配置 + 拉取镜像
make up      # 启动
make health  # 健康检查
```

## 密钥管理

真实密钥只存在于 `.env`（gitignored）。`scripts/entrypoint-garage.sh` 在容器启动时将密钥从环境变量渲染到 `garage.toml`，确保配置文件可追踪而密钥不进 git。

## 常用命令

```bash
make ps                              # 服务状态
make logs svc=garage                 # 单服务日志
make restart svc=lakekeeper          # 重启单服务
make shell svc=iceberg-catalog-db    # 进入容器
make nuke                            # ⚠️ 删除所有数据卷
```

## 从旧位置迁移

```bash
# 停止旧服务
cd /var/lib/datalake/docker && docker compose down

# 复制密钥
cp /var/lib/datalake/docker/.env <project>/infra/docker/.env

# 新位置启动
cd <project>/infra/docker && make up
```

Garage 使用 bind mount，迁移后确保 `.env` 中 `GARAGE_*_DIR` 路径指向相同数据目录。
