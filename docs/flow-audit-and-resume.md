# Flow 文件输入、审计与复用

DAG Shell 的 `from_path(id, pattern)` 枚举文件并向下游投递文件引用；
`channel_value(id, value)` 和 `from_list(id, values)` 传递所给的值，字符串不会自动变成文件。
节点需要文件输入时，用 `from_path` 的返回 channel 绑定 `process(..., inputs=[channel])`。
多个输出按节点注册表定义的命名输出选择后绑定下游；不根据文件名猜测上下游关系。

运行时默认启用 `enable_dag_history`，历史数据库使用 `dag_history_db`。
启用后若创建目录或打开数据库失败，启动会报出路径和错误，不再静默禁用历史功能。
显式关闭历史功能时，仍可以在同一存活会话中复用内存缓存，但跨重启恢复和审计查询不可用。

`run_flow` 返回 `run_id`、`audit_persisted`、按 process 汇总的状态及有限的产物路径预览。
`audit_persisted=false` 表示本次运行没有成功保存审计记录，不能据此声称历史记录已可查询。
用 `dag_runs_log` 的 `view=node_detail`、`view=execution` 或 `view=full` 查询完整记录。
持久化的 `nodes` 包含实例身份、文件产物、端口分配和执行器实际返回的 execution 证据，
包括可用的 workspace、镜像、stdout/stderr 引用；没有观测到的证据保持空值。
Arrow 行数据不会被收集到审计 JSON。

`resume=true` 仅复用能够验证身份和全部输出的文件任务。
VFS 文件通过授权存储接口计算内容 SHA256，本地不超过 64 KiB 的输出也会附加并校验内容 SHA256。
输入改变、输出改变或丢失都会导致重新执行；
因此同大小文件的内容变化也会使缓存失效。这个校验会读取文件内容，大文件会增加 I/O。
不支持验证的输入、流式输出及非文件输出仍重新执行；不能仅从 `reused=0` 推断历史存储故障。

`compute_readiness` 只检查请求的节点、身份和镜像配置，不运行任务，不检查历史数据库或产物读取。
空的镜像和身份列表不代表镜像或身份已通过验证；结果中的 `verification_gaps` 说明检查边界。
完整验收仍需真实执行、读取产物、重复运行验证复用及查询审计记录。

审计记录同时保存完整 flow 定义及 manifest hash；列表在没有旧式 snapshot 时显示 manifest 标识。
运行详情支持完整 run ID 或最近 1000 条记录中的唯一前缀；前缀冲突时须使用更长的前缀。
流程恢复按相同 agent 身份进行，不会把其他 agent 的流程自动接入当前会话。

旧的小文件缓存若没有内容哈希，会重新执行一次以生成内容证明，随后再复用。
