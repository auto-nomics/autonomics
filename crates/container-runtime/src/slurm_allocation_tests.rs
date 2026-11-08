use super::*;
use crate::{ContainerNetwork, GpuRequest, PullPolicy, WorkspaceRef};
use std::fs;

const JOB: &str = "/slurm/uid_42/job_123";
const LEAF: &str = "/slurm/uid_42/job_123/step_0/task_0";
const GIB: &str = "1073741824";

struct Fixture {
    temporary: tempfile::TempDir,
    cgroup: String,
    mountinfo: String,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = Self {
            temporary,
            cgroup: String::new(),
            mountinfo: String::new(),
        };
        for (id, controller) in [(7, "cpu"), (8, "cpuset"), (9, "memory")] {
            fixture
                .cgroup
                .push_str(&format!("{id}:{controller}:{LEAF}\n"));
            fixture.mountinfo.push_str(&format!(
                "{id} 1 0:{id} / {} rw - cgroup cgroup rw,{controller}\n",
                escape(&fixture.mount(controller))
            ));
            for suffix in ["", "/step_0", "/step_0/task_0"] {
                let logical = format!("{JOB}{suffix}");
                match controller {
                    "cpu" => {
                        fixture.set(controller, &logical, "cpu.cfs_quota_us", "-1");
                        fixture.set(controller, &logical, "cpu.cfs_period_us", "100000");
                    }
                    "cpuset" => fixture.set(controller, &logical, "cpuset.cpus", "0-3"),
                    "memory" => {
                        fixture.set(controller, &logical, "memory.use_hierarchy", "1");
                        fixture.set(controller, &logical, "memory.limit_in_bytes", GIB);
                        fixture.set(
                            controller,
                            &logical,
                            "memory.memsw.limit_in_bytes",
                            "2147483648",
                        );
                    }
                    _ => unreachable!(),
                }
            }
        }
        fixture
            .cgroup
            .push_str("10:pids:/system.slice/slurmd.service\n");
        fixture
    }
    fn mount(&self, controller: &str) -> PathBuf {
        self.temporary
            .path()
            .join(format!("{controller} mount\\point"))
    }
    fn path(&self, controller: &str, logical: &str, name: &str) -> PathBuf {
        self.mount(controller)
            .join(logical.trim_start_matches('/'))
            .join(name)
    }
    fn set(&self, controller: &str, logical: &str, name: &str, value: &str) {
        let path = self.path(controller, logical, name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{value}\n")).unwrap();
    }
    fn read(&self) -> Result<SlurmAllocationEvidence> {
        observe_inputs(
            "123",
            "0",
            42,
            &self.cgroup,
            &self.mountinfo,
            4096,
            &mut toy_read,
        )
    }
    fn pids(&mut self, maximum: &str) {
        self.cgroup = self.cgroup.replace(
            "10:pids:/system.slice/slurmd.service",
            &format!("10:pids:{LEAF}"),
        );
        self.mountinfo.push_str(&format!(
            "10 1 0:10 / {} rw - cgroup cgroup rw,pids\n",
            escape(&self.mount("pids"))
        ));
        for suffix in ["", "/step_0", "/step_0/task_0"] {
            self.set("pids", &format!("{JOB}{suffix}"), "pids.max", maximum);
        }
    }
    fn request(&self) -> ContainerRunRequest {
        ContainerRunRequest {
            image: "toy@sha256:fixture".into(),
            command: vec!["true".into()],
            workspace: WorkspaceRef {
                host_path: self.temporary.path().to_path_buf(),
                container_workdir: "/workspace".into(),
            },
            env: Vec::new(),
            panels: Vec::new(),
            input_mounts: Vec::new(),
            network: ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: PullPolicy::Never,
            cpus: Some(4.0),
            memory: Some("1Gi".into()),
            pids_limit: None,
            shm_size: None,
            gpus: GpuRequest::None,
            user: None,
            timeout_secs: 1,
            name: "allocation-toy".into(),
        }
    }
}

fn escape(path: &Path) -> String {
    path.to_str()
        .unwrap()
        .replace('\\', "\\134")
        .replace(' ', "\\040")
        .replace('\t', "\\011")
        .replace('\n', "\\012")
}

fn toy_read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[test]
fn exact_job_caps_and_shared_pids_are_explicit() {
    let fixture = Fixture::new();
    let evidence = fixture.read().unwrap();
    assert_eq!(evidence.job_id(), "123");
    assert_eq!(evidence.step_id(), "0");
    assert_eq!(evidence.uid(), 42);
    assert_eq!(evidence.cpu_cap(), 4.0);
    assert_eq!(evidence.memory_cap_bytes(), 1 << 30);
    assert_eq!(evidence.pids_cap(), None);
    assert_eq!(
        evidence.pids_status(),
        "not_proven_shared_controller_membership"
    );
    assert!(
        evidence
            .validate_request(&fixture.request(), SlurmSwapPolicy::Inherit)
            .is_ok()
    );
    let record = evidence
        .checked_request(&fixture.request(), SlurmSwapPolicy::Inherit)
        .unwrap();
    assert!(!record.memsw_not_greater_than_ram);
    let json = serde_json::to_value(evidence).unwrap();
    assert_eq!(json["controllers"][0]["job_root"], JOB);
    assert_eq!(json["cpu_cap_numerator"], 4);
    assert!(json["controllers"][0]["levels"][0]["raw"]["cpu.cfs_quota_us"].is_string());
}

#[test]
fn fractional_quota_is_compared_exactly_without_rounding_up() {
    let fixture = Fixture::new();
    fixture.set("cpu", JOB, "cpu.cfs_quota_us", "250000");
    let evidence = fixture.read().unwrap();
    let mut request = fixture.request();
    request.cpus = Some(2.5);
    assert!(
        evidence
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_ok()
    );
    request.cpus = Some(f64::from_bits(2.5_f64.to_bits() - 1));
    assert!(
        evidence
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_err()
    );
    assert!(!rational_at_most_f64(1, 3, 1.0 / 3.0));
    assert!(rational_at_most_f64(
        1,
        3,
        f64::from_bits((1.0_f64 / 3.0).to_bits() + 1)
    ));
    assert!(!rational_at_most_f64(1, 1, f64::NAN));
    assert!(!rational_at_most_f64(1, 1, f64::INFINITY));
    assert!(!rational_at_most_f64(1, 1, f64::from_bits(1)));
}

#[test]
fn cpuset_cap_is_not_replaced_by_process_affinity() {
    let fixture = Fixture::new();
    fixture.set("cpuset", LEAF, "cpuset.cpus", "1-2");
    let evidence = fixture.read().unwrap();
    assert_eq!(evidence.effective_cpuset, "1-2");
    let mut request = fixture.request();
    request.cpus = Some(1.0);
    assert!(
        evidence
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_err()
    );
    fixture.set("cpuset", LEAF, "cpuset.cpus", "4-5");
    assert!(fixture.read().is_err());
    fixture.set("cpuset", LEAF, "cpuset.cpus", "0-1,1-2");
    assert!(fixture.read().is_err());
}

#[test]
fn exact_uid_job_step_and_unambiguous_mounts_are_required() {
    for from_to in [
        ("job_123", "job_1234"),
        ("uid_42", "uid_43"),
        ("step_0", "step_batch"),
        ("step_0/task_0", "step_0/job_123/task_0"),
    ] {
        let mut fixture = Fixture::new();
        fixture.cgroup = fixture.cgroup.replace(from_to.0, from_to.1);
        assert!(fixture.read().is_err(), "accepted {}", from_to.1);
    }
    let mut fixture = Fixture::new();
    let first_line = fixture.mountinfo.lines().next().unwrap().to_owned();
    fixture.mountinfo.push_str(&first_line);
    fixture.mountinfo.push('\n');
    assert!(fixture.read().is_err());
    let mut fixture = Fixture::new();
    fixture.mountinfo = fixture
        .mountinfo
        .replace(" / ", " /slurm/uid_42/job_123/step_0 ");
    assert!(fixture.read().is_err(), "hidden job root admitted");
    let mut fixture = Fixture::new();
    fixture.cgroup = format!("0::{LEAF}\n");
    assert!(fixture.read().is_err(), "v2 membership admitted as v1");
}

#[test]
fn mount_roots_are_mapped_componentwise_with_octal_escaping() {
    let fixture = Fixture::new();
    let membership = memberships(&fixture.cgroup).unwrap();
    let mut mounted = mounts(&fixture.mountinfo).unwrap();
    for mount in &mut mounted {
        mount.root = PathBuf::from("/slurm/uid_42");
    }
    let cpu = controller("cpu", &membership, &mounted, "123", "0", 42).unwrap();
    assert_eq!(
        cpu.levels[0].directory,
        fixture.mount("cpu").join("job_123")
    );
    assert_eq!(
        cpu.levels.last().unwrap().directory,
        fixture.mount("cpu").join("job_123/step_0/task_0")
    );
    mounted[0].root = PathBuf::from("/slurm/uid_4");
    assert!(controller("cpu", &membership, &mounted, "123", "0", 42).is_err());
    for invalid in [
        "/bad\\04",
        "/bad\\999",
        "/bad\\000",
        "/bad/../path",
        "/bad//path",
    ] {
        assert!(decode_mount_path(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn memory_requires_job_hierarchy_and_finite_exact_ram() {
    let fixture = Fixture::new();
    fixture.set("memory", JOB, "memory.use_hierarchy", "0");
    assert!(fixture.read().is_err());
    fixture.set("memory", JOB, "memory.use_hierarchy", "1");
    fs::remove_file(fixture.path("memory", LEAF, "memory.use_hierarchy")).unwrap();
    assert!(fixture.read().is_err());
    fixture.set("memory", LEAF, "memory.use_hierarchy", "1");
    let mut request = fixture.request();
    request.memory = Some("1GB".into());
    assert!(
        fixture
            .read()
            .unwrap()
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_err()
    );
    for suffix in ["", "/step_0", "/step_0/task_0"] {
        fixture.set(
            "memory",
            &format!("{JOB}{suffix}"),
            "memory.limit_in_bytes",
            "9223372036854771712",
        );
    }
    assert!(
        fixture.read().is_err(),
        "unlimited job admitted using an unseen shared ancestor"
    );
    fixture.set("memory", LEAF, "memory.limit_in_bytes", GIB);
    assert_eq!(fixture.read().unwrap().memory_cap_bytes(), 1 << 30);
    assert_eq!(finite_memory("9223372036854771712", 4096).unwrap(), None);
}

#[test]
fn pids_declaration_needs_a_finite_exact_job_controller() {
    let mut fixture = Fixture::new();
    let mut request = fixture.request();
    request.pids_limit = Some(128);
    assert!(
        fixture
            .read()
            .unwrap()
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_err()
    );
    fixture.pids("128");
    let evidence = fixture.read().unwrap();
    assert_eq!(evidence.pids_cap(), Some(128));
    assert!(
        evidence
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_ok()
    );
    request.pids_limit = Some(32);
    assert!(
        evidence
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_err()
    );
    for suffix in ["", "/step_0", "/step_0/task_0"] {
        fixture.set("pids", &format!("{JOB}{suffix}"), "pids.max", "max");
    }
    assert!(
        fixture
            .read()
            .unwrap()
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_err()
    );
    fixture.cgroup = fixture.cgroup.replace(
        &format!("10:pids:{LEAF}"),
        &format!("10:pids:{}", LEAF.replace("job_123", "job_999")),
    );
    assert!(
        fixture.read().is_err(),
        "another job's PIDs controller admitted"
    );
}

#[test]
fn swap_policy_never_claims_disabled_without_memsw_proof() {
    let fixture = Fixture::new();
    let request = fixture.request();
    assert!(
        fixture
            .read()
            .unwrap()
            .validate_request(&request, SlurmSwapPolicy::Disabled)
            .is_err()
    );
    fixture.set("memory", JOB, "memory.memsw.limit_in_bytes", GIB);
    assert!(
        fixture
            .read()
            .unwrap()
            .validate_request(&request, SlurmSwapPolicy::Disabled)
            .is_ok()
    );
    for suffix in ["", "/step_0", "/step_0/task_0"] {
        fs::remove_file(fixture.path(
            "memory",
            &format!("{JOB}{suffix}"),
            "memory.memsw.limit_in_bytes",
        ))
        .unwrap();
    }
    let evidence = fixture.read().unwrap();
    assert!(
        evidence
            .validate_request(&request, SlurmSwapPolicy::Inherit)
            .is_ok()
    );
    assert!(
        evidence
            .validate_request(&request, SlurmSwapPolicy::Disabled)
            .is_err()
    );
}

#[test]
fn control_changes_and_optional_control_appearance_are_rejected() {
    for name in ["cpu.cfs_quota_us", "memory.memsw.limit_in_bytes"] {
        let fixture = Fixture::new();
        let target = fixture.path(
            if name.starts_with("cpu.") {
                "cpu"
            } else {
                "memory"
            },
            JOB,
            name,
        );
        if name.starts_with("memory.") {
            fs::remove_file(&target).unwrap();
        }
        let mut count = 0;
        let result = observe_inputs(
            "123",
            "0",
            42,
            &fixture.cgroup,
            &fixture.mountinfo,
            4096,
            &mut |path| {
                if path == target {
                    count += 1;
                    if count == 2 {
                        return Ok(Some("1\n".into()));
                    }
                }
                toy_read(path)
            },
        );
        assert!(result.is_err(), "changed {name} admitted");
        assert_eq!(count, 2);
    }
}

#[test]
fn environment_uid_and_real_kernel_filesystem_are_not_toy_overrides() {
    let mut environment = BTreeMap::from([
        ("SLURM_JOB_ID".into(), Some("123".into())),
        ("SLURM_JOBID".into(), None),
        ("SLURM_STEP_ID".into(), Some("0".into())),
        ("SLURM_STEPID".into(), None),
    ]);
    assert_eq!(step_environment(&environment).unwrap(), "0");
    environment.insert("SLURM_JOBID".into(), Some("1234".into()));
    assert!(step_environment(&environment).is_err());
    environment.insert("SLURM_JOBID".into(), None);
    environment.insert("SLURM_STEPID".into(), Some("batch".into()));
    assert!(step_environment(&environment).is_err());
    assert!(proc_uid_line("Uid:\t42\t42\t42\t42\n", 42).is_ok());
    assert!(proc_uid_line("Uid:\t42\t43\t42\t42\n", 42).is_err());
    let fixture = Fixture::new();
    let path = fixture.path("cpu", JOB, "cpu.cfs_quota_us");
    assert!(
        kernel_text(&path, CGROUP_V1_MAGIC, MAX_FILE).is_err(),
        "ordinary file pretended to be cgroupfs"
    );
    let symlink = fixture.temporary.path().join("aliased");
    std::os::unix::fs::symlink(path, &symlink).unwrap();
    assert!(kernel_text(&symlink, PROC_MAGIC, MAX_FILE).is_err());
}

#[test]
fn missing_step_environment_only_admits_exact_kernel_batch_membership() {
    let environment = BTreeMap::from([
        ("SLURM_JOB_ID".into(), Some("123".into())),
        ("SLURM_JOBID".into(), Some("123".into())),
        ("SLURM_STEP_ID".into(), None),
        ("SLURM_STEPID".into(), None),
    ]);
    let fixture = Fixture::new();
    let batch = fixture.cgroup.replace("step_0", "step_batch");
    assert_eq!(
        step_identity(&environment, &batch, 42).unwrap(),
        ("batch".into(), "kernel_batch_membership")
    );
    assert!(environment["SLURM_STEP_ID"].is_none());
    assert!(environment["SLURM_STEPID"].is_none());
    assert!(
        step_identity(&environment, &fixture.cgroup, 42).is_err(),
        "numeric step inferred without environment"
    );
    for broken in [
        batch.replace("job_123", "job_1234"),
        batch.replace("uid_42", "uid_43"),
        batch.replacen("step_batch", "step_1", 1),
        format!("{batch}11:memory:/slurm/uid_42/job_123/step_batch\n"),
        batch.replace("step_batch/task_0", "step_batch/step_batch"),
        batch
            .lines()
            .filter(|line| !line.contains(":memory:"))
            .collect::<Vec<_>>()
            .join("\n"),
    ] {
        assert!(
            step_identity(&environment, &broken, 42).is_err(),
            "ambiguous/inconsistent batch identity admitted"
        );
    }
    let mut explicit = environment.clone();
    explicit.insert("SLURM_STEP_ID".into(), Some("0".into()));
    assert_eq!(
        step_identity(&explicit, &fixture.cgroup, 42).unwrap(),
        ("0".into(), "slurm_environment")
    );
    assert!(step_identity(&explicit, &batch, 42).is_err());
    explicit.insert("SLURM_STEP_ID".into(), Some(String::new()));
    assert!(
        step_identity(&explicit, &batch, 42).is_err(),
        "empty explicit step treated as absent"
    );
    explicit.insert("SLURM_STEP_ID".into(), None);
    explicit.insert("SLURM_JOBID".into(), Some("999".into()));
    assert!(step_identity(&explicit, &batch, 42).is_err());
}

#[test]
fn inferred_batch_observation_preserves_absent_raw_environment_and_source() {
    let fixture = Fixture::new();
    let batch = fixture.cgroup.replace("step_0", "step_batch");
    for name in ["cpu", "cpuset", "memory"] {
        fs::rename(
            fixture
                .mount(name)
                .join(JOB.trim_start_matches('/'))
                .join("step_0"),
            fixture
                .mount(name)
                .join(JOB.trim_start_matches('/'))
                .join("step_batch"),
        )
        .unwrap();
    }
    let environment = BTreeMap::from([
        ("SLURM_JOB_ID".into(), Some("123".into())),
        ("SLURM_JOBID".into(), None),
        ("SLURM_STEP_ID".into(), None),
        ("SLURM_STEPID".into(), None),
    ]);
    let (step, source) = step_identity(&environment, &batch, 42).unwrap();
    let mut observation = observe_inputs(
        "123",
        &step,
        42,
        &batch,
        &fixture.mountinfo,
        4096,
        &mut toy_read,
    )
    .unwrap();
    observation.step_identity_source = source;
    observation.raw_environment = environment;
    observation
        .validate_request(&fixture.request(), SlurmSwapPolicy::Inherit)
        .unwrap();
    let json = serde_json::to_value(observation).unwrap();
    assert_eq!(json["step_id"], "batch");
    assert_eq!(json["step_identity_source"], "kernel_batch_membership");
    assert!(json["raw_environment"]["SLURM_STEP_ID"].is_null());
    assert!(json["raw_environment"]["SLURM_STEPID"].is_null());
}
