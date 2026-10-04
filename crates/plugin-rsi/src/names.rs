use crate::{Error, Result};

/// Validate the public plugin/install identity: lowercase kebab-case.
pub fn validate_plugin_name(name: &str) -> Result<&str> {
    let mut chars = name.chars();
    let valid = name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(name)
    } else {
        Err(Error::InvalidPluginName {
            name: name.to_string(),
            reason: "expected lowercase kebab-case, max 64 chars".to_string(),
        })
    }
}

/// Derive the unique runtime kind for the one-plugin-one-node MVP policy.
pub fn derive_node_kind(plugin_name: &str) -> Result<String> {
    validate_plugin_name(plugin_name)?;
    Ok(plugin_name.replace('-', "_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plugin_names() {
        assert_eq!(validate_plugin_name("demo-plugin").unwrap(), "demo-plugin");
        assert_eq!(validate_plugin_name("tool2").unwrap(), "tool2");
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_names() {
        for bad in ["", "-demo", "demo-", "Demo", "demo_tool", "../demo"] {
            assert!(validate_plugin_name(bad).is_err(), "{bad} must fail");
        }
    }

    #[test]
    fn derives_underscore_kind() {
        assert_eq!(
            derive_node_kind("clusterprofiler-bitr").unwrap(),
            "clusterprofiler_bitr"
        );
    }
}
