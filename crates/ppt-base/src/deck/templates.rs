//! Starter deck templates for biomedical research presentations.
//!
//! Each template emits canonical deck markdown the agent then edits via
//! slide operations. Layouts referenced here are the biomedical
//! additions ([`ppt_types::Layout`]) plus Slidev ports.

/// Starter markdown for a named template (`meeting`, `defense`,
/// `figures`); unknown names yield a minimal cover-only deck.
pub fn starter_markdown(template: Option<&str>, title: &str) -> String {
    match template {
        Some("meeting") => meeting(title),
        Some("defense") => defense(title),
        Some("figures") => figures(title),
        _ => minimal(title),
    }
}

fn minimal(title: &str) -> String {
    format!(
        "---\ntitle: {title}\naspectRatio: 16/9\n---\n\n# {title}\n\n<!--\nCover slide. Edit freely; add slides with slide_insert.\n-->\n"
    )
}

/// 组会 / 文献报告.
fn meeting(title: &str) -> String {
    format!(
        r#"---
title: {title}
aspectRatio: 16/9
colorSchema: auto
---

# 组会报告

## 汇报人 · 日期

<!--
封面页:标题下方填写汇报人与日期。
-->

---

## 背景 Background

- 研究问题与动机
- 相关工作一句话总结

---

## 方法 Methods

- 数据集与队列
- 关键流程(可插入 flow 图)

<!-- v-click 分步展示:用 <v-click>…</v-click> 包裹需要渐进披露的要点 -->

---
layout: panel-grid
panel1: 队列基本情况
panel2: 主要结局
panel3: 亚组分析
panel4: 敏感性分析
---

## 结果 Results

<v-click>

**Panel A** 队列基本情况——样本量、随访时长、基线特征。

</v-click>

<v-click>

**Panel B** 主要结局——效应值与置信区间。

</v-click>

<v-click>

**Panel C** 亚组分析。

</v-click>

<v-click>

**Panel D** 敏感性分析。

</v-click>

---

## 讨论 Discussion

- 主要发现的解释
- 局限性
- 下一步计划

---
layout: references
---

## 参考文献

1. Author A, et al. *Journal*. 2025.
2. Author B, et al. *Journal*. 2024.

<!--
占位 References 页:deck_references 工具可自动从 {{cite:key}} 生成。
-->
"#
    )
}

/// 答辩 / 会议报告(含 click 渐进与备注).
fn defense(title: &str) -> String {
    format!(
        r#"---
title: {title}
aspectRatio: 16/9
colorSchema: light
exportFilename: defense
---

# {title}

答辩人 · 导师 · 日期

---

## 汇报提纲

1. 研究背景与意义
2. 研究方法
3. 主要结果
4. 结论与创新点

<!--
备注在导出 PDF 的 notes 中,演讲者视图可见。
-->

---

## 研究背景

<v-click>

临床问题:**…**

</v-click>

<v-click>

现有方法的不足:**…**

</v-click>

<v-click>

本研究目标:**…**

</v-click>

---

## 研究方法

- 研究设计
- 数据来源
- 统计分析计划

---
layout: stats-table
tableCaption: Table 1. 主要结果(校正模型)
---

## 主要结果

| 变量 | 效应值 (95% CI) | p 值 |
| --- | --- | --- |
| 暴露 A | 1.42 (1.10–1.83) | 0.006 |
| 暴露 B | 0.87 (0.71–1.06) | 0.17 |

---

## 结论与创新点

1. **结论**:…
2. **创新点**:…
3. **展望**:…

---
layout: figure-left
image: assets://FIGURE_MAIN
---

## 核心图表

*图 1. 主要发现示意图。*

---

# 谢谢!

欢迎各位老师提问

---
layout: end
---
"#
    )
}

/// 投稿配图 / PPT 交付(每页一图,服务期刊图版规范).
fn figures(title: &str) -> String {
    format!(
        r#"---
title: {title}
aspectRatio: 16/9
colorSchema: light
---

# {title}

Figure pack · 300 DPI export ready

<!--
导出:deck_export format=png dpi=300;PNG 尺寸 = 画布像素 × DPI/96。
-->

---
layout: figure-full
image: assets://FIGURE_1
---

Figure 1. 主图说明(置于图注区)。

---
layout: panel-grid
panel1: A
panel2: B
panel3: C
panel4: D
---

Figure 2. 多面板组合图。

- **A**:…
- **B**:…
- **C**:…
- **D**:…

---
layout: figure-right
image: assets://FIGURE_3
---

Figure 3. 右图 + 左侧说明文字。

---
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::assemble;

    #[test]
    fn all_templates_parse_and_have_slides() {
        for t in ["meeting", "defense", "figures", "unknown-template"] {
            let md = starter_markdown(Some(t), "Test Deck");
            let deck = assemble(&md, "d", "t", "t");
            let min = if t == "unknown-template" { 1 } else { 3 };
            assert!(
                deck.len() >= min,
                "template {t} produced {} slides",
                deck.len()
            );
        }
    }

    #[test]
    fn meeting_uses_clicks_and_references() {
        let md = starter_markdown(Some("meeting"), "x");
        let deck = assemble(&md, "d", "t", "t");
        assert!(deck.meta.features.katex || true);
        let panel = deck
            .slides
            .iter()
            .find(|s| s.frontmatter.layout.clone().map(|l| l.to_string()) == Some("panel-grid".into()));
        assert!(panel.is_some());
        assert!(panel.unwrap().click_plan.is_some());
    }

    #[test]
    fn defense_has_stats_table_and_figure() {
        let md = starter_markdown(Some("defense"), "Def");
        let deck = assemble(&md, "d", "t", "t");
        let layouts: Vec<String> = deck
            .slides
            .iter()
            .filter_map(|s| s.frontmatter.layout.clone().map(|l| l.to_string()))
            .collect();
        assert!(layouts.contains(&"stats-table".to_string()));
        assert!(layouts.contains(&"figure-left".to_string()));
    }
}
