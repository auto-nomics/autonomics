use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundIdentifierType {
    Name,
    Cid,
    InChIKey,
}

impl CompoundIdentifierType {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "name" => Some(Self::Name),
            "cid" => Some(Self::Cid),
            "inchikey" => Some(Self::InChIKey),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Cid => "cid",
            Self::InChIKey => "inchikey",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PropertyResponse {
    #[serde(default)]
    pub property_table: PropertyTable,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PropertyTable {
    pub properties: Vec<CompoundProperties>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct CompoundProperties {
    #[serde(rename = "CID")]
    pub cid: Option<u64>,
    pub molecular_formula: Option<String>,
    pub molecular_weight: Option<String>,
    #[serde(rename = "SMILES")]
    pub smiles: Option<String>,
    #[serde(rename = "ConnectivitySMILES")]
    pub connectivity_smiles: Option<String>,
    pub canonical_smiles: Option<String>,
    pub isomeric_smiles: Option<String>,
    #[serde(rename = "InChI")]
    pub inchi: Option<String>,
    #[serde(rename = "InChIKey")]
    pub inchikey: Option<String>,
    #[serde(rename = "IUPACName")]
    pub iupac_name: Option<String>,
    pub title: Option<String>,
}
