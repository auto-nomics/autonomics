#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    EN,
    ZH,
}

impl Language {
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::EN => "EN",
            Language::ZH => "ZH",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "EN" => Language::EN,
            _ => Language::ZH,
        }
    }
}
