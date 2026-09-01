//! Registry-agnostic image references for official tool containers.
//!
//! Tool nodes pin the repository and manifest digest, while deployment is
//! free to select the ACR endpoint through the process environment. Production
//! uses the configured Aliyun registry when no override is present.

pub const ACR_ENDPOINT_ENV: &str = "ACR_ENDPOINT";
pub const ACR_NAMESPACE: &str = "autonomics";
pub const DEFAULT_ACR_ENDPOINT: &str = "crpi-isjkczwpadlvr9i3.cn-hongkong.personal.cr.aliyuncs.com";

pub fn acr_image(repository: &str, digest: &str) -> Result<String, String> {
    let endpoint = endpoint_from_env(std::env::var_os(ACR_ENDPOINT_ENV));
    if endpoint.is_empty() {
        return Err(format!("`{ACR_ENDPOINT_ENV}` cannot be empty"));
    }
    acr_image_for_endpoint(&endpoint, repository, digest)
}

fn endpoint_from_env(value: Option<std::ffi::OsString>) -> String {
    match value {
        Some(value) => value.to_string_lossy().trim().to_string(),
        None => DEFAULT_ACR_ENDPOINT.into(),
    }
}

fn acr_image_for_endpoint(
    endpoint: &str,
    repository: &str,
    digest: &str,
) -> Result<String, String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(format!("`{ACR_ENDPOINT_ENV}` cannot be empty"));
    }
    if endpoint.contains("://")
        || endpoint.contains('/')
        || endpoint.contains('\\')
        || endpoint.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "`{ACR_ENDPOINT_ENV}` must be a registry host without scheme or path, got `{endpoint}`"
        ));
    }

    Ok(format!("{endpoint}/{ACR_NAMESPACE}/{repository}@{digest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_an_immutable_acr_reference() {
        let image = acr_image_for_endpoint(
            "registry.example.invalid",
            "lava",
            "sha256:b7dd1d3a3493cc32af2dc286f2aace77036b1fd806bee4f14a4ff67509268be7",
        )
        .unwrap();
        assert_eq!(
            image,
            "registry.example.invalid/autonomics/lava@sha256:b7dd1d3a3493cc32af2dc286f2aace77036b1fd806bee4f14a4ff67509268be7"
        );
    }

    #[test]
    fn falls_back_to_the_default_acr_endpoint() {
        let endpoint = endpoint_from_env(None);
        assert_eq!(endpoint, DEFAULT_ACR_ENDPOINT);
        let image = acr_image_for_endpoint(
            &endpoint,
            "pyradiomics",
            "sha256:31994246efb2426aa82db1c8c31a451aa040800489429c8a4637dcb5a03d0b74",
        )
        .unwrap();
        assert_eq!(
            image,
            format!("{DEFAULT_ACR_ENDPOINT}/autonomics/pyradiomics@sha256:31994246efb2426aa82db1c8c31a451aa040800489429c8a4637dcb5a03d0b74")
        );
    }

    #[test]
    fn rejects_registry_endpoints_with_a_scheme_or_path() {
        let error = acr_image_for_endpoint(
            "https://acr.example",
            "lava",
            "sha256:91d11747476967b0131571308f2a64bb12aa459205f282103f6bed4fb69ca0bf",
        )
        .unwrap_err();
        assert!(error.contains("without scheme or path"));

        let error = acr_image_for_endpoint(
            "acr.example/autonomics",
            "lava",
            "sha256:91d11747476967b0131571308f2a64bb12aa459205f282103f6bed4fb69ca0bf",
        )
        .unwrap_err();
        assert!(error.contains("without scheme or path"));

        let error = acr_image_for_endpoint(
            "",
            "lava",
            "sha256:91d11747476967b0131571308f2a64bb12aa459205f282103f6bed4fb69ca0bf",
        )
        .unwrap_err();
        assert!(error.contains("cannot be empty"));
    }
}
