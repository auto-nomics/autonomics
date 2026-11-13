# 可交互科研绘图验收

目标沿用 [ROADMAP](ROADMAP.md) 中的开源 BioRender Leo 类工作流：围绕科学图意对话、草图与标记审阅、完整素材化、保持无关内容的局部修改、原生对象编辑，以及项目内保存和导出。以下将已有证据按覆盖范围整理；历史样例的通过不自动证明最新实现或完整新建流程通过。

| 要求 | 当前证据 | 结论与待补证据 |
| --- | --- | --- |
| 项目内保存图稿、版本与待审候选；Agent 修改不自动采用 | [v278整图审阅](../../target/visualization-evaluations/whole-final-review-gui-v278/result.json)、[v290局部修复](../../target/visualization-evaluations/crop-repair-model-v290/result.json)；Store 与保护链测试 | 有实际运行证据；多叶分支会被验收拒绝，v288失败已保留 |
| 从 GUI 传递当前图稿、版本、选中对象、批注与 Agent 草稿；核对后发送 | [批注交接测试](../../apps/gui/tests/helpers/canvas-annotation-handoff.mjs)、[正式画布上下文回放](../../target/visualization-evaluations/annotation-context-gui-v279/result.json)；对象批注与草图标记独立 | 正式画布和候选入口均有实现及回放；旧/已处理意见不能冒充当前授权 |
| 可编辑文字、关系、素材与组件；原生裁剪、移动、缩放及组合保持 | [v289裁剪闭环](../../target/visualization-evaluations/native-crop-gui-v289/result.json)、[v290真实Agent修复](../../target/visualization-evaluations/crop-repair-model-v290/result.json)；核心回归 | 裁剪/矩形贴合/缩放可用；矩形贴合包含透明边缘，不能称像素轮廓识别 |
| 科学范围澄清、来源读取、计划及原生图生成 | [新建TGFB流程](../../target/visualization-evaluations/tgfb-source-sketch-v211/result.json)、[新建凋亡流程](../../target/visualization-evaluations/fresh-apoptosis-context-v112/result.json) | 历史真实新建流程有证据，范围明确；这些结果未包含全部节点素材化 |
| 整图所有科学组件有明确素材归属、完整标签、来源与有效贴合；布局实际审阅 | [TGF整图审阅](../../target/visualization-evaluations/whole-final-review-gui-v278/result.json)、[裁剪后采样检查](../../target/visualization-evaluations/cropped-paint-gui-v290/result.json)、[紧凑布局Agent回放](../../target/visualization-evaluations/frame-pack-model-v285/result.json) | TGF与[凋亡八组件整图](../../target/visualization-evaluations/apoptosis-materialize-model-v291/result.json)均有素材化证据；凋亡使用4种Servier图元且168组合采样完整无交叠。TGF仍有混合画风与小图元问题，不能泛化为任意主题自动排版通过 |
| 局部修改保留其他对象、原科学文字/关系/证据、素材文件与历史 | [v285布局](../../target/visualization-evaluations/frame-pack-model-v285/result.json)、[v290单组件裁剪修复](../../target/visualization-evaluations/crop-repair-model-v290/result.json) | 指定范围逐字段检查通过；不提升为对任意模型请求的普遍保证 |
| 原生候选PNG审阅、人工采纳、可编辑源文件和逐素材来源导出 | [v278](../../target/visualization-evaluations/whole-final-review-gui-v278/result.json)、[v289](../../target/visualization-evaluations/native-crop-gui-v289/result.json)；裁剪显示区域保留于来源清单 | [凋亡整图导出](../../target/visualization-evaluations/apoptosis-materialize-export-v291/result.json)和[TGF裁剪缩放后导出](../../target/visualization-evaluations/tgfb-crop-export-v291/result.json)均已独立回放，保留逐图来源、裁剪及关系依据 |
| 新建科学图的连续完整体验 | [v292新建至素材化](../../target/visualization-evaluations/fresh-full-apoptosis-v292/fresh-evidence-stage.json)、[v293新建至素材化](../../target/visualization-evaluations/fresh-full-apoptosis-v293/fresh-evidence-stage.json)；两轮都从空项目开始，分别生成8/7个已确认组件并完成素材化。原运行失败结果保留 | **尚未完整证明**：v292新增说明标题前缀触发过严检查，v293固定8组件检查与实际7组件计划不符；已修复检查器。v293终态[独立回放](../../target/visualization-evaluations/fresh-materialized-export-v293/result.json)通过220组图片/文字及文字矩形无交叠、采用和源/来源导出，但不能替代原会话局部修订。v294在草图限定语检查处终止，明确句式识别已补正并通过正负例；v295正在新空项目验证完整连续流程 |
| 错误可恢复、诊断覆盖明确、最终报告与实际结果一致 | v288失败独立审计、原生crop数值防护、v290恢复成功；实际耗时和工具错误已记录 | 写入防护已改进；模型仍有路径上限/裁剪范围误用、长回复及个别虚报贴合记录的问题，需要保留独立核验 |

科学审阅的产品要求是：保留已确认的完整原文、关系和来源，明确通用素材、采样覆盖与未验证含义，并让研究者审阅。它不等于软件能够为任意机制、分子结构或化学计量出具正确性证明。已知图形角色错误或与文字明显矛盾的结构仍不能靠笼统免责声明放行。

新建流程已接入全组件素材化和局部审阅/采用/导出验证入口；下一步完成同一项目与会话内的全部阶段验收。v292素材化阶段为44次工具调用、4次错误、289271ms；不同运行之间任务与布局不同，不能据此宣称稳定加速。图面排版、工具错误恢复、实际延迟和最终报告一致性仍需验收。项目总体目标保持未完成。
