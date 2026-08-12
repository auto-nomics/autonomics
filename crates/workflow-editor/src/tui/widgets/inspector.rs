//! Right-pane inspector — shows the currently selected node's details
//! (label, kind, params, SOP), or the workflow-level SOP if no node is
//! selected.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};

use crate::tui::state::AppState;

pub struct InspectorWidget<'a> {
    pub state: &'a AppState,
    pub focus: bool,
}

impl<'a> Widget for InspectorWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let _ = self.focus;
        let Some(manifest) = &self.state.manifest else {
            Paragraph::new("no workflow").render(area, buf);
            return;
        };

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            format!("Workflow: {}", manifest.name),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("  id       {}", manifest.id),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                "  nodes    {}  edges  {}",
                manifest.nodes.len(),
                manifest.edges.len()
            ),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));

        if let Some(sop) = &manifest.sop {
            lines.push(Line::from(Span::styled(
                "Workflow SOP:",
                Style::default().fg(Color::Magenta),
            )));
            for l in sop.lines() {
                lines.push(Line::from(format!("  {l}")));
            }
            lines.push(Line::from(""));
        }

        if let Some(nid) = self.state.selected_node {
            if let Some(node) = manifest.nodes.iter().find(|n| n.id == nid) {
                lines.push(Line::from(Span::styled(
                    format!("Node: {}", node.label),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(format!("  id      {}", node.id)));
                lines.push(Line::from(format!("  kind    {}", node.kind)));
                lines.push(Line::from(format!(
                    "  pos     ({}, {})",
                    node.position.0, node.position.1
                )));
                lines.push(Line::from(format!("  inputs  {:?}", node.inputs)));
                lines.push(Line::from(format!("  outputs {:?}", node.outputs)));
                lines.push(Line::from("  params:"));
                let params = serde_json::to_string_pretty(&node.params).unwrap_or_default();
                for l in params.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("    {l}"),
                        Style::default().fg(Color::Gray),
                    )));
                }
            }
        } else if let Some(eid) = self.state.selected_edge {
            if let Some(edge) = manifest.edges.iter().find(|e| e.id == eid) {
                lines.push(Line::from(Span::styled(
                    "Edge",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(format!("  id       {}", edge.id)));
                lines.push(Line::from(format!("  source   {}:{}", edge.source, edge.source_handle)));
                lines.push(Line::from(format!("  target   {}:{}", edge.target, edge.target_handle)));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "(nothing selected — [ / ] to cycle nodes)",
                Style::default().fg(Color::DarkGray),
            )));
        }

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}