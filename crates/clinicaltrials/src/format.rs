use crate::types::Study;

pub fn format_study(study: &Study) -> String {
    let protocol = &study.protocol_section;
    let identification = &protocol.identification_module;
    let status = &protocol.status_module;
    let title = identification
        .brief_title
        .as_deref()
        .unwrap_or("Clinical trial");
    let nct_id = identification.nct_id.as_deref().unwrap_or("unknown");

    let mut output = format!("## {title}\n\n- **NCT ID:** {nct_id}\n");
    output.push_str(&format!(
        "- **Overall status:** {}\n",
        status.overall_status.as_deref().unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "- **Start date:** {}\n",
        status
            .start_date_struct
            .as_ref()
            .and_then(|date| date.date.clone())
            .unwrap_or_else(|| "unknown".into())
    ));
    output.push_str(&format!(
        "- **Completion date:** {}\n",
        status
            .completion_date_struct
            .as_ref()
            .and_then(|date| date.date.clone())
            .unwrap_or_else(|| "unknown".into())
    ));
    output.push_str(&format!(
        "- **Lead sponsor:** {}\n",
        protocol
            .sponsor_collaborators_module
            .lead_sponsor
            .as_ref()
            .and_then(|sponsor| sponsor.name.clone())
            .unwrap_or_else(|| "unknown".into())
    ));
    output.push_str(&format!(
        "- **Study type:** {}\n",
        protocol
            .design_module
            .study_type
            .as_deref()
            .unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "- **Phases:** {}\n",
        if protocol.design_module.phases.is_empty() {
            "none".to_owned()
        } else {
            protocol.design_module.phases.join(", ")
        }
    ));
    output.push_str(&format!(
        "- **Enrollment:** {}\n",
        protocol
            .design_module
            .enrollment_info
            .as_ref()
            .and_then(|enrollment| enrollment.count)
            .map_or("unknown".into(), |count| count.to_string())
    ));
    output.push_str(&format!(
        "- **Conditions:** {}\n",
        if protocol.conditions_module.conditions.is_empty() {
            "unknown".to_owned()
        } else {
            protocol.conditions_module.conditions.join("; ")
        }
    ));
    if !protocol.arms_interventions_module.interventions.is_empty() {
        output.push_str("\n### Interventions\n\n");
        for intervention in &protocol.arms_interventions_module.interventions {
            output.push_str(&format!(
                "- {} {}\n",
                intervention
                    .intervention_type
                    .as_deref()
                    .unwrap_or("INTERVENTION"),
                intervention.name.as_deref().unwrap_or("unknown")
            ));
        }
    }
    if !protocol.outcomes_module.primary_outcomes.is_empty() {
        output.push_str("\n### Primary outcomes\n\n");
        for outcome in &protocol.outcomes_module.primary_outcomes {
            output.push_str(&format!(
                "- {}\n",
                outcome.measure.as_deref().unwrap_or("unknown")
            ));
        }
    }
    if let Some(summary) = protocol.description_module.brief_summary.as_deref() {
        output.push_str(&format!("\n### Brief summary\n\n{summary}\n"));
    }
    output
}
