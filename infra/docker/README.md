# Autonomics Garage Object Storage

本目录提供本地对象存储测试环境：Garage (S3 对象存储) + Garage Web UI。

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
    ├── ensure-secret.sh            # 生成缺失密钥
    ├── garage-warehouse-setup.sh   # 初始化 bucket/access key/layout
    └── render-garage-config.sh     # 渲染 garage.toml
```

## 服务

| 服务 | 镜像 | 端口 | 用途 |
|------|------|------|------|
| **garage** | `dxflrs/garage:v2.0.0` | 3900-3903 | S3 兼容对象存储 |
| **webui** | `khairul169/garage-webui:latest` | 3909 | Garage Web UI |

## 快速开始

```bash
cd infra/docker
cp ../../.env.example ../../.env
# 编辑 ../../.env；缺失的 Garage 密钥可由 make check-env 自动生成
make init    # 验证配置 + 拉取镜像
make up      # 启动
make health  # 健康检查
```

启动对象存储测试环境：

```bash
make garage-up
```

该目标只启动 `garage` 与 `garage-webui`。默认数据落在
`infra/docker/.data/`；如需指定其他路径，在
`.env` 中覆盖 `GARAGE_META_DIR`、`GARAGE_SNAPSHOTS_DIR` 和
`GARAGE_DATA_DIR_DISK1`。

## 密钥管理

真实密钥只存在于工作区根目录的 `.env`（gitignored）。由于 Garage 镜像没有
shell，`scripts/render-garage-config.sh` 会在宿主机渲染 `garage.toml` 并挂载进容器；
渲染结果已被 gitignore。

## 常用命令

```bash
make ps                              # 服务状态
make logs svc=garage                 # 单服务日志
make restart svc=garage              # 重启单服务
make health                          # Garage 健康检查
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
