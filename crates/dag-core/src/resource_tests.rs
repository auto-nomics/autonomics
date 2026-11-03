use super::{
    MemorySample, effective_memory_limit_bytes_at, memory_capacity_bytes_at, sample_cgroup_v2_at,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct CgroupTree {
    directory: PathBuf,
}

impl CgroupTree {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "autonomics-resource-test-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        Self { directory }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.directory.join(relative)
    }

    fn controller(&self, relative: &str, current: &str, limit: &str, inactive: Option<&str>) {
        let directory = self.path(relative);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("memory.current"), current).unwrap();
        fs::write(directory.join("memory.max"), limit).unwrap();
        if let Some(inactive) = inactive {
            fs::write(
                directory.join("memory.stat"),
                format!("inactive_file {inactive}\n"),
            )
            .unwrap();
        }
    }

    fn sample(&self, process_cgroup: &str) -> Result<Option<MemorySample>, String> {
        sample_cgroup_v2_at(&self.directory, process_cgroup)
    }
}

impl Drop for CgroupTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn expect_sample(
    sample: Result<Option<MemorySample>, String>,
    charged: u64,
    usage: u64,
    limit: u64,
) {
    let sample = sample
        .unwrap()
        .expect("a finite visible controller must be sampled");
    assert_eq!(sample.charged_bytes, charged);
    assert_eq!(sample.usage_bytes, usage);
    assert_eq!(sample.limit_bytes, limit);
    assert_eq!(sample.source, "cgroup_v2");
    assert!((sample.ratio() - usage as f64 / limit as f64).abs() < 1e-14);
}

#[test]
fn absolute_unified_worker_path_uses_its_finite_service_ancestor() {
    let tree = CgroupTree::new();
    tree.controller("service", "400", "1000", Some("25"));
    fs::create_dir(tree.path("service/worker-native")).unwrap();
    expect_sample(tree.sample("0::/service/worker-native\n"), 400, 375, 1000);
}

#[test]
fn unified_root_path_samples_the_visible_root() {
    let tree = CgroupTree::new();
    tree.controller("", "700", "1000", Some("20"));
    expect_sample(tree.sample("0::/\n"), 700, 680, 1000);
}

#[test]
fn cgroup_v1_or_no_unified_record_has_no_v2_sample() {
    let tree = CgroupTree::new();
    tree.controller("", "900", "1000", None);
    for contents in [
        "2:memory:/service\n",
        "4:cpu,cpuacct:/service\n2:memory:/service\n",
    ] {
        assert_eq!(tree.sample(contents).unwrap(), None, "{contents:?}");
    }
}

#[test]
fn empty_process_metadata_or_empty_unified_path_is_rejected() {
    let tree = CgroupTree::new();
    for contents in ["", "\n", "0::\n"] {
        assert!(tree.sample(contents).is_err(), "{contents:?}");
    }
}

#[test]
fn relative_parent_dot_and_noncanonical_unified_paths_are_rejected() {
    let tree = CgroupTree::new();
    for path in [
        "service/worker",
        "/service/../worker",
        "/service/./worker",
        "/../worker",
        "./service",
        "/service//worker",
        "//service/worker",
    ] {
        assert!(tree.sample(&format!("0::{path}\n")).is_err(), "{path:?}");
    }
}

#[test]
fn duplicate_unified_records_are_rejected_even_when_identical() {
    let tree = CgroupTree::new();
    for contents in ["0::/service\n0::/other\n", "0::/service\n0::/service\n"] {
        assert!(tree.sample(contents).is_err(), "{contents:?}");
    }
}

#[test]
fn finite_leaf_is_selected_when_it_has_the_highest_working_ratio() {
    let tree = CgroupTree::new();
    tree.controller("service", "100", "1000", None);
    tree.controller("service/worker", "80", "100", None);
    expect_sample(tree.sample("0::/service/worker\n"), 80, 80, 100);
}

#[test]
fn finite_two_gib_parent_overrides_eight_gib_child_when_more_constrained() {
    let tree = CgroupTree::new();
    tree.controller("service", "1610612736", "2147483648", None);
    tree.controller("service/worker", "1073741824", "8589934592", None);
    expect_sample(
        tree.sample("0::/service/worker\n"),
        1610612736,
        1610612736,
        2147483648,
    );
}

#[test]
fn sibling_charge_already_in_the_parent_is_not_replaced_by_leaf_charge() {
    let tree = CgroupTree::new();
    tree.controller("service", "950", "1000", None);
    tree.controller("service/worker", "100", "1000", None);
    tree.controller("service/sibling", "850", "1000", None);
    expect_sample(tree.sample("0::/service/worker\n"), 950, 950, 1000);
}

#[test]
fn most_dangerous_ratio_is_selected_across_all_visible_finite_levels() {
    let tree = CgroupTree::new();
    tree.controller("", "7000", "10000", None);
    tree.controller("service", "1800", "2000", None);
    tree.controller("service/group", "400", "1000", None);
    tree.controller("service/group/worker", "50", "100", None);
    expect_sample(tree.sample("0::/service/group/worker\n"), 1800, 1800, 2000);
}

#[test]
fn charge_and_limit_are_never_taken_from_different_cgroup_levels() {
    let tree = CgroupTree::new();
    tree.controller("service", "400", "500", None);
    tree.controller("service/worker", "90", "100", None);
    expect_sample(tree.sample("0::/service/worker\n"), 90, 90, 100);
}

#[test]
fn inactive_file_is_subtracted_at_the_same_level_before_ratio_comparison() {
    let tree = CgroupTree::new();
    tree.controller("service", "900", "1000", Some("500"));
    tree.controller("service/worker", "300", "500", Some("50"));
    expect_sample(tree.sample("0::/service/worker\n"), 300, 250, 500);
}

#[test]
fn inactive_file_subtraction_saturates_without_underflow() {
    let tree = CgroupTree::new();
    tree.controller("service", "10", "100", Some("1000"));
    expect_sample(tree.sample("0::/service\n"), 10, 0, 100);
}

#[test]
fn missing_stat_file_conservatively_uses_raw_charge() {
    let tree = CgroupTree::new();
    tree.controller("service", "800", "1000", None);
    expect_sample(tree.sample("0::/service\n"), 800, 800, 1000);
}

#[test]
fn malformed_current_is_an_error_not_host_fallback() {
    for value in ["not-a-number", "-1", "18446744073709551616", ""] {
        let tree = CgroupTree::new();
        tree.controller("service", value, "1000", None);
        assert!(tree.sample("0::/service\n").is_err(), "{value:?}");
    }
}

#[test]
fn malformed_limit_is_an_error_not_unlimited_or_host_fallback() {
    for value in ["not-a-number", "-1", "18446744073709551616", ""] {
        let tree = CgroupTree::new();
        tree.controller("service", "800", value, None);
        assert!(tree.sample("0::/service\n").is_err(), "{value:?}");
    }
}

#[test]
fn malformed_present_inactive_file_is_an_error_not_zero_cache() {
    for value in ["not-a-number", "-1", "18446744073709551616", ""] {
        let tree = CgroupTree::new();
        tree.controller("service", "800", "1000", Some(value));
        assert!(tree.sample("0::/service\n").is_err(), "{value:?}");
    }
}

#[test]
fn present_stat_without_inactive_file_or_with_duplicate_key_is_rejected() {
    for contents in ["anon 10\n", "inactive_file 10\ninactive_file 20\n"] {
        let tree = CgroupTree::new();
        tree.controller("service", "800", "1000", None);
        fs::write(tree.path("service/memory.stat"), contents).unwrap();
        assert!(tree.sample("0::/service\n").is_err(), "{contents:?}");
    }
}

#[test]
fn zero_limit_is_an_error_not_unlimited_or_host_fallback() {
    let tree = CgroupTree::new();
    tree.controller("service", "800", "0", None);
    assert!(tree.sample("0::/service\n").is_err());
}

#[test]
fn bad_ancestor_data_cannot_be_hidden_by_a_valid_finite_leaf() {
    let tree = CgroupTree::new();
    tree.controller("service", "not-a-number", "1000", None);
    tree.controller("service/worker", "90", "100", None);
    assert!(tree.sample("0::/service/worker\n").is_err());
}

#[test]
fn controllerless_worker_is_skipped_but_not_its_finite_parent() {
    let tree = CgroupTree::new();
    tree.controller("service", "800", "1000", None);
    fs::create_dir(tree.path("service/worker")).unwrap();
    expect_sample(tree.sample("0::/service/worker\n"), 800, 800, 1000);
}

#[test]
fn partially_present_nonroot_controller_is_rejected() {
    for missing in ["memory.current", "memory.max"] {
        let tree = CgroupTree::new();
        tree.controller("service", "800", "1000", None);
        tree.controller("service/worker", "100", "500", None);
        fs::remove_file(tree.path("service/worker").join(missing)).unwrap();
        assert!(tree.sample("0::/service/worker\n").is_err(), "{missing}");
    }
}

#[test]
fn visible_root_without_memory_max_does_not_hide_finite_descendant() {
    let tree = CgroupTree::new();
    tree.controller("", "900", "1000", None);
    fs::remove_file(tree.path("memory.max")).unwrap();
    tree.controller("service/worker", "100", "500", None);
    expect_sample(tree.sample("0::/service/worker\n"), 100, 100, 500);
}

#[test]
fn visible_root_with_finite_limit_but_missing_current_is_rejected() {
    let tree = CgroupTree::new();
    tree.controller("", "100", "1000", None);
    fs::remove_file(tree.path("memory.current")).unwrap();
    assert!(tree.sample("0::/\n").is_err());
}

#[test]
fn unlimited_leaf_and_intermediate_levels_do_not_hide_finite_service() {
    let tree = CgroupTree::new();
    tree.controller("service", "800", "1000", None);
    tree.controller("service/group", "700", "max", None);
    tree.controller("service/group/worker", "100", "max", None);
    expect_sample(tree.sample("0::/service/group/worker\n"), 800, 800, 1000);
}

#[test]
fn all_visible_limits_unlimited_returns_none_for_explicit_host_fallback() {
    let tree = CgroupTree::new();
    tree.controller("", "900", "max", None);
    tree.controller("service", "800", "max", None);
    tree.controller("service/worker", "100", "max", None);
    assert_eq!(tree.sample("0::/service/worker\n").unwrap(), None);
}

#[test]
fn traversal_stops_at_visible_root_without_sampling_its_host_parent() {
    let tree = CgroupTree::new();
    tree.controller("", "999", "1000", None);
    tree.controller("visible", "100", "1000", None);
    tree.controller("visible/service", "200", "500", None);
    tree.controller("visible/service/worker", "100", "1000", None);
    expect_sample(
        sample_cgroup_v2_at(&tree.path("visible"), "0::/service/worker\n"),
        200,
        200,
        500,
    );
}

#[test]
fn visible_root_can_be_the_only_finite_limit_for_a_nested_worker() {
    let tree = CgroupTree::new();
    tree.controller("", "900", "1000", Some("50"));
    tree.controller("service/worker", "100", "max", None);
    expect_sample(tree.sample("0::/service/worker\n"), 900, 850, 1000);
}

#[test]
fn fixture_root_is_an_absolute_existing_directory() {
    let tree = CgroupTree::new();
    assert!(tree.directory.is_absolute());
    assert!(Path::new(&tree.directory).is_dir());
}

#[test]
fn capacity_uses_minimum_limit_not_the_highest_pressure_samples_limit() {
    let tree = CgroupTree::new();
    tree.controller("", "9000", "10000", None);
    tree.controller("service", "1000", "2000", None);
    tree.controller("service/worker", "100", "5000", None);
    expect_sample(tree.sample("0::/service/worker\n"), 9000, 9000, 10000);
    assert_eq!(
        effective_memory_limit_bytes_at(&tree.directory, "0::/service/worker\n").unwrap(),
        Some(2000)
    );
}

#[test]
fn capacity_reads_limits_without_requiring_current_or_stat_to_be_usable() {
    let tree = CgroupTree::new();
    tree.controller("service", "not-a-number", "2000", Some("invalid"));
    tree.controller("service/worker", "also-invalid", "max", None);
    assert_eq!(
        effective_memory_limit_bytes_at(&tree.directory, "0::/service/worker\n").unwrap(),
        Some(2000)
    );
    assert!(tree.sample("0::/service/worker\n").is_err());
}

#[test]
fn capacity_preserves_unlimited_fallback_and_rejects_malformed_limit() {
    let tree = CgroupTree::new();
    tree.controller("", "900", "max", None);
    tree.controller("service/worker", "100", "max", None);
    assert_eq!(
        effective_memory_limit_bytes_at(&tree.directory, "0::/service/worker\n").unwrap(),
        None
    );
    fs::write(tree.path("service/worker/memory.max"), "invalid").unwrap();
    assert!(effective_memory_limit_bytes_at(&tree.directory, "0::/service/worker\n").is_err());
}

#[test]
fn capacity_uses_finite_cgroup_limit_without_consulting_invalid_host_metadata() {
    let tree = CgroupTree::new();
    tree.controller("service", "1000", "2147483648", None);
    tree.controller("service/worker", "100", "8589934592", None);
    assert_eq!(
        memory_capacity_bytes_at(&tree.directory, "0::/service/worker\n", "").unwrap(),
        2147483648
    );
}

#[test]
fn capacity_falls_back_to_positive_host_kibibytes_only_when_no_finite_limit_exists() {
    let tree = CgroupTree::new();
    tree.controller("", "1000", "max", None);
    tree.controller("service/worker", "100", "max", None);
    assert_eq!(
        memory_capacity_bytes_at(
            &tree.directory,
            "0::/service/worker\n",
            "MemTotal: 1000 kB\nMemAvailable: 200 kB\n"
        )
        .unwrap(),
        1024000
    );
}

#[test]
fn host_capacity_rejects_missing_duplicate_zero_bad_unit_and_overflow_total() {
    let tree = CgroupTree::new();
    tree.controller("", "1000", "max", None);
    for contents in [
        "",
        "MemAvailable: 100 kB\n",
        "MemTotal: 0 kB\n",
        "MemTotal: bad kB\n",
        "MemTotal: -1 kB\n",
        "MemTotal: 1 MB\n",
        "MemTotal: 1 kB\nMemTotal: 2 kB\n",
        "MemTotal: 18446744073709551615 kB\n",
    ] {
        assert!(
            memory_capacity_bytes_at(&tree.directory, "0::/\n", contents).is_err(),
            "{contents:?}"
        );
    }
}
