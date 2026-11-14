# visualization-assets

共享画布的素材读取与直接编辑层。palette/parts/geometry/image_info 只读取指定正式版本；recolor/edit_parts/inset/refresh_inset 在校验 expected_revision 后直接保存新版本，返回 Visualization 元数据。

SVG 操作保留原文件、逐素材作者与许可、编辑记录和其他实例。路径编辑使用文件 sha256 与路径编号校验。`svg` 模块提供统一检查与直接修改入口，不再包含 candidate 模块。

验证：`cargo test -p visualization-assets`。
