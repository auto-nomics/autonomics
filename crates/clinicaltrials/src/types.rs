use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Study {
    pub protocol_section: ProtocolSection,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProtocolSection {
    pub identification_module: IdentificationModule,
    pub status_module: StatusModule,
    pub sponsor_collaborators_module: SponsorModule,
    pub description_module: DescriptionModule,
    pub conditions_module: ConditionsModule,
    pub design_module: DesignModule,
    pub arms_interventions_module: ArmsInterventionsModule,
    pub outcomes_module: OutcomesModule,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdentificationModule {
    pub nct_id: Option<String>,
    pub brief_title: Option<String>,
    pub official_title: Option<String>,
    pub organization: Option<Organization>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Organization {
    pub full_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusModule {
    pub overall_status: Option<String>,
    pub start_date_struct: Option<DateValue>,
    pub primary_completion_date_struct: Option<DateValue>,
    pub completion_date_struct: Option<DateValue>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DateValue {
    pub date: Option<String>,
    pub value_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SponsorModule {
    pub lead_sponsor: Option<NamedEntity>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NamedEntity {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DescriptionModule {
    pub brief_summary: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConditionsModule {
    pub conditions: Vec<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesignModule {
    pub study_type: Option<String>,
    pub phases: Vec<String>,
    pub enrollment_info: Option<EnrollmentInfo>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnrollmentInfo {
    pub count: Option<u64>,
    pub value_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArmsInterventionsModule {
    pub interventions: Vec<Intervention>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Intervention {
    pub intervention_type: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutcomesModule {
    pub primary_outcomes: Vec<Outcome>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Outcome {
    pub measure: Option<String>,
    pub time_frame: Option<String>,
}
