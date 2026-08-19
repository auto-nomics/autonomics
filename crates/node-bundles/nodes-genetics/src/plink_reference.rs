use std::path::Path;

const DEFAULT_PREFIX_ROOTS: [&str; 4] = [
    "/data/mixer/resources/g1000_eur/stage",
    "/mnt/data/mixer/resources/g1000_eur/stage",
    "/mnt/disk3/mixer/reference/mixer_data/stage",
    "/mnt/disk2/dataset/1000g_plink/eur",
];

fn configured_prefix(shared: Option<&str>, specific: Option<&str>) -> Option<String> {
    if let Some(value) = specific.filter(|value| !value.trim().is_empty()) {
        return Some(value.trim().to_string());
    }
    if let Some(value) = shared.filter(|value| !value.trim().is_empty()) {
        return Some(value.trim().to_string());
    }
    None
}

fn prefix_for_root(root: &str, chrom: i64) -> String {
    format!("{root}/chr{chrom}/1000G.EUR.chr{chrom}.qc")
}

fn prefix_template_for_root(root: &str) -> String {
    format!("{root}/chr{{N}}/1000G.EUR.chr{{N}}.qc")
}

fn complete_prefix_exists(root: &str, chrom: i64) -> bool {
    let prefix = prefix_for_root(root, chrom);
    ["bed", "bim", "fam"]
        .iter()
        .all(|extension| Path::new(&format!("{prefix}.{extension}")).is_file())
}

fn default_prefix_template(chroms: &[i64]) -> String {
    for root in DEFAULT_PREFIX_ROOTS {
        if chroms
            .iter()
            .all(|chrom| complete_prefix_exists(root, *chrom))
        {
            return prefix_template_for_root(root);
        }
    }
    prefix_template_for_root(DEFAULT_PREFIX_ROOTS[0])
}

pub(crate) fn lava_prefix_template(chroms: &[i64]) -> String {
    configured_prefix(
        std::env::var("PLINK_REF_PREFIX_TEMPLATE").ok().as_deref(),
        std::env::var("LAVA_PLINK_REF_PREFIX_TEMPLATE")
            .ok()
            .as_deref(),
    )
    .unwrap_or_else(|| default_prefix_template(chroms))
}

pub(crate) fn hdl_l_prefix_template(chrom: i64) -> String {
    configured_prefix(
        std::env::var("PLINK_REF_PREFIX_TEMPLATE").ok().as_deref(),
        std::env::var("HDL_L_PLINK_REF_PREFIX_TEMPLATE")
            .ok()
            .as_deref(),
    )
    .unwrap_or_else(|| default_prefix_template(&[chrom]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plink_specific_override_wins_over_shared_override() {
        assert_eq!(
            configured_prefix(
                Some(" /shared/chr{N}/panel "),
                Some(" /specific/chr{N}/panel "),
            ),
            Some("/specific/chr{N}/panel".to_string())
        );
    }

    #[test]
    fn plink_blank_overrides_fall_back_to_defaults() {
        assert_eq!(configured_prefix(Some("  "), Some("")), None);
    }
}
