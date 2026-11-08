//! Read-only admission for containers inheriting a real Slurm cgroup-v1 job.
//!
//! This is separate from delegated cgroup-v2 Podman operation. Only limits at
//! the exact job anchor or below it provide admission; a shared host/service
//! controller or process affinity never supplies a job resource guarantee.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ContainerRunRequest, ContainerRuntimeError};

type Result<T> = std::result::Result<T, ContainerRuntimeError>;
const MAX_FILE: u64 = 64 * 1024;
const MAX_PROC: u64 = 1024 * 1024;
const MAX_LEVELS: usize = 64;
const CGROUP_V1_MAGIC: libc::c_long = 0x27e0eb;
const PROC_MAGIC: libc::c_long = 0x9fa0;

/// Swap policy for inherited Slurm limits, independent of the formal contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlurmSwapPolicy {
    /// Preserve the allocation's policy. Observed memsw is not a no-swap claim.
    #[default]
    Inherit,
    /// Require a finite job-specific memsw bound no larger than the RAM bound.
    Disabled,
}

#[derive(Debug, Clone, Serialize)]
struct LevelEvidence {
    logical_path: PathBuf,
    directory: PathBuf,
    raw: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, Serialize)]
struct ControllerEvidence {
    controller: String,
    membership: PathBuf,
    mount_root: PathBuf,
    mount_point: PathBuf,
    job_root: PathBuf,
    levels: Vec<LevelEvidence>,
}

#[derive(Debug, Clone, Serialize)]
struct ValidatedRequest {
    cpus: f64,
    memory_bytes: u64,
    pids_limit: Option<u64>,
    swap_policy: SlurmSwapPolicy,
    memsw_not_greater_than_ram: bool,
}

/// Immutable observed allocation. Serialize this as technical evidence.
/// All cap fields are enforced inside the exact job's hierarchy; stricter
/// shared ancestors are deliberately not used to admit a larger allocation.
#[derive(Debug, Clone, Serialize)]
pub struct SlurmAllocationEvidence {
    schema: &'static str,
    cgroup_version: u8,
    job_id: String,
    step_id: String,
    step_identity_source: &'static str,
    uid: u32,
    pid: u32,
    raw_environment: BTreeMap<String, Option<String>>,
    raw_cgroup: String,
    raw_mountinfo: String,
    raw_proc_uid: String,
    controllers: Vec<ControllerEvidence>,
    effective_cpuset: String,
    cpuset_cardinality: u64,
    cpu_cap_numerator: u64,
    cpu_cap_denominator: u64,
    cpu_cap: f64,
    memory_cap_bytes: u64,
    memsw_cap_bytes: Option<u64>,
    pids_cap: Option<u64>,
    pids_status: String,
    validated_request: Option<ValidatedRequest>,
}

impl SlurmAllocationEvidence {
    pub fn job_id(&self) -> &str {
        &self.job_id
    }
    pub fn step_id(&self) -> &str {
        &self.step_id
    }
    pub fn uid(&self) -> u32 {
        self.uid
    }
    pub fn cpu_cap(&self) -> f64 {
        self.cpu_cap
    }
    pub fn memory_cap_bytes(&self) -> u64 {
        self.memory_cap_bytes
    }
    pub fn pids_cap(&self) -> Option<u64> {
        self.pids_cap
    }
    pub fn pids_status(&self) -> &str {
        &self.pids_status
    }

    /// Check a declaration against this snapshot. This does not refresh it;
    /// use `observe_and_validate` immediately before each actual container run.
    pub fn validate_request(
        &self,
        request: &ContainerRunRequest,
        swap: SlurmSwapPolicy,
    ) -> Result<()> {
        self.checked_request(request, swap).map(|_| ())
    }

    fn checked_request(
        &self,
        request: &ContainerRunRequest,
        swap: SlurmSwapPolicy,
    ) -> Result<ValidatedRequest> {
        let cpus = request
            .cpus
            .ok_or_else(|| invalid("Slurm inheritance requires explicit cpus"))?;
        if !cpus.is_finite()
            || cpus <= 0.0
            || !rational_at_most_f64(self.cpu_cap_numerator, self.cpu_cap_denominator, cpus)
        {
            return Err(invalid(
                "Slurm job CPU cap exceeds the exact declared CPU budget",
            ));
        }
        let memory = crate::resources::memory_quantity_bytes(
            request
                .memory
                .as_deref()
                .ok_or_else(|| invalid("Slurm inheritance requires explicit memory"))?,
        )?;
        if self.memory_cap_bytes == 0 || self.memory_cap_bytes > memory {
            return Err(invalid(
                "Slurm job RAM cap exceeds the exact declared memory budget",
            ));
        }
        let pids_limit = request
            .pids_limit
            .map(|limit| {
                let value = u64::try_from(limit)
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| invalid("positive declared PID budget required"))?;
                match self.pids_cap {
                    Some(cap) if cap > 0 && cap <= value => Ok(value),
                    _ => Err(invalid(
                        "no sufficiently bounded job-specific PIDs controller",
                    )),
                }
            })
            .transpose()?;
        let memsw_not_greater_than_ram = self
            .memsw_cap_bytes
            .is_some_and(|cap| cap > 0 && cap <= self.memory_cap_bytes);
        if swap == SlurmSwapPolicy::Disabled && !memsw_not_greater_than_ram {
            return Err(invalid(
                "disabled swap requires a job memsw cap no larger than its RAM cap",
            ));
        }
        Ok(ValidatedRequest {
            cpus,
            memory_bytes: memory,
            pids_limit,
            swap_policy: swap,
            memsw_not_greater_than_ram: swap == SlurmSwapPolicy::Disabled
                && memsw_not_greater_than_ram,
        })
    }
}

fn invalid(message: impl Into<String>) -> ContainerRuntimeError {
    ContainerRuntimeError::Invalid(format!("Slurm allocation: {}", message.into()))
}

/// Observe actual kernel/proc state and verify the declaration before a run.
pub fn observe_and_validate(
    request: &ContainerRunRequest,
    swap: SlurmSwapPolicy,
) -> Result<SlurmAllocationEvidence> {
    let mut evidence = observe()?;
    evidence.validated_request = Some(evidence.checked_request(request, swap)?);
    Ok(evidence)
}

/// Read actual procfs/cgroup-v1 controls, without changing any cgroup or task.
/// This early factory check does not claim a future request was admitted.
pub fn observe() -> Result<SlurmAllocationEvidence> {
    if !cfg!(target_os = "linux") || !cfg!(target_pointer_width = "64") {
        return Err(invalid("requires 64-bit Linux cgroup v1"));
    }
    let env_snapshot = environment()?;
    let job = env_snapshot["SLURM_JOB_ID"]
        .as_deref()
        .ok_or_else(|| invalid("SLURM_JOB_ID required"))?;
    // SAFETY: these functions only read the current kernel credentials.
    let uid = unsafe { libc::geteuid() };
    if unsafe { libc::getuid() } != uid {
        return Err(invalid("set-user-ID workers are unsupported"));
    }
    let cgroup = required_kernel_text(Path::new("/proc/self/cgroup"), PROC_MAGIC, MAX_PROC)?;
    let (step, step_identity_source) = step_identity(&env_snapshot, &cgroup, uid)?;
    let mountinfo = required_kernel_text(Path::new("/proc/self/mountinfo"), PROC_MAGIC, MAX_PROC)?;
    let status = required_kernel_text(Path::new("/proc/self/status"), PROC_MAGIC, MAX_PROC)?;
    let uid_line = proc_uid_line(&status, uid)?;
    // SAFETY: sysconf only reads the kernel's page-size setting.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = u64::try_from(page_size)
        .ok()
        .filter(|value| value.is_power_of_two())
        .ok_or_else(|| invalid("unknown kernel page size"))?;
    let mut read = |path: &Path| kernel_text(path, CGROUP_V1_MAGIC, MAX_FILE);
    let mut evidence = observe_inputs(job, &step, uid, &cgroup, &mountinfo, page_size, &mut read)?;
    if required_kernel_text(Path::new("/proc/self/cgroup"), PROC_MAGIC, MAX_PROC)? != cgroup
        || required_kernel_text(Path::new("/proc/self/mountinfo"), PROC_MAGIC, MAX_PROC)?
            != mountinfo
        || proc_uid_line(
            &required_kernel_text(Path::new("/proc/self/status"), PROC_MAGIC, MAX_PROC)?,
            uid,
        )? != uid_line
        || environment()? != env_snapshot
    {
        return Err(invalid(
            "kernel membership, mounts or Slurm environment changed during observation",
        ));
    }
    evidence.step_identity_source = step_identity_source;
    evidence.raw_environment = env_snapshot;
    evidence.raw_proc_uid = uid_line;
    evidence.pid = std::process::id();
    Ok(evidence)
}

fn environment() -> Result<BTreeMap<String, Option<String>>> {
    [
        "SLURM_JOB_ID",
        "SLURM_JOBID",
        "SLURM_STEP_ID",
        "SLURM_STEPID",
    ]
    .into_iter()
    .map(|key| {
        let value = match std::env::var(key) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => return Err(invalid("Slurm environment must be UTF-8")),
        };
        Ok((key.to_string(), value))
    })
    .collect()
}

fn numeric(value: &str, allow_zero: bool) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
        && (allow_zero || value != "0")
        && value.parse::<u64>().is_ok()
}

fn job_environment(environment: &BTreeMap<String, Option<String>>) -> Result<&str> {
    let job = environment["SLURM_JOB_ID"]
        .as_deref()
        .ok_or_else(|| invalid("SLURM_JOB_ID required"))?;
    if !numeric(job, false)
        || environment["SLURM_JOBID"]
            .as_deref()
            .is_some_and(|other| other != job)
    {
        return Err(invalid(
            "Slurm job environment is nonnumeric or inconsistent",
        ));
    }
    Ok(job)
}

fn step_environment(environment: &BTreeMap<String, Option<String>>) -> Result<String> {
    job_environment(environment)?;
    let step = environment["SLURM_STEP_ID"]
        .as_deref()
        .or(environment["SLURM_STEPID"].as_deref())
        .ok_or_else(|| invalid("explicit SLURM_STEP_ID or SLURM_STEPID required"))?;
    if !(step == "batch" || numeric(step, true))
        || environment["SLURM_STEP_ID"]
            .as_deref()
            .is_some_and(|other| other != step)
        || environment["SLURM_STEPID"]
            .as_deref()
            .is_some_and(|other| other != step)
    {
        return Err(invalid(
            "Slurm step environment is unsupported or inconsistent",
        ));
    }
    Ok(step.to_string())
}

/// An absent batch step may be derived only from real procfs membership.
/// Numeric steps always require explicit Slurm environment identity.
fn step_identity(
    environment: &BTreeMap<String, Option<String>>,
    cgroup: &str,
    uid: u32,
) -> Result<(String, &'static str)> {
    let job = job_environment(environment)?;
    let missing = environment["SLURM_STEP_ID"].is_none() && environment["SLURM_STEPID"].is_none();
    let (step, source) = if missing {
        ("batch".to_string(), "kernel_batch_membership")
    } else {
        (step_environment(environment)?, "slurm_environment")
    };
    let memberships = memberships(cgroup)?;
    for name in ["cpu", "cpuset", "memory"] {
        let path = memberships
            .get(name)
            .ok_or_else(|| invalid(format!("missing required v1 {name} step identity")))?;
        job_root(path, job, &step, uid)?;
    }
    Ok((step, source))
}

fn proc_uid_line(status: &str, uid: u32) -> Result<String> {
    let lines = status
        .lines()
        .filter(|line| line.starts_with("Uid:"))
        .collect::<Vec<_>>();
    if lines.len() != 1 {
        return Err(invalid("missing or duplicate proc UID record"));
    }
    let values = lines[0].split_whitespace().skip(1).collect::<Vec<_>>();
    if values.len() != 4
        || values
            .iter()
            .any(|value| value.parse::<u32>().ok() != Some(uid))
    {
        return Err(invalid("proc credentials differ from the Slurm worker UID"));
    }
    Ok(lines[0].to_string())
}

fn required_kernel_text(path: &Path, magic: libc::c_long, maximum: u64) -> Result<String> {
    kernel_text(path, magic, maximum)?
        .ok_or_else(|| invalid(format!("missing kernel file {}", path.display())))
}

fn kernel_text(path: &Path, magic: libc::c_long, maximum: u64) -> Result<Option<String>> {
    if magic == CGROUP_V1_MAGIC {
        let parent = path
            .parent()
            .ok_or_else(|| invalid("missing cgroup parent"))?;
        if parent.canonicalize()? != parent {
            return Err(invalid("aliased cgroup directory"));
        }
        let directory = File::open(parent)?;
        filesystem(&directory, magic)?;
    }
    use std::os::unix::ffi::OsStrExt;
    let path_c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| invalid("NUL in kernel path"))?;
    // SAFETY: path_c is NUL terminated; the resulting owned fd is closed below.
    let fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error.into())
        };
    }
    // SAFETY: fd was just opened successfully and ownership is transferred once.
    let file = unsafe { File::from_raw_fd(fd) };
    filesystem(&file, magic)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("kernel control must be a regular virtual file"));
    }
    let mut bytes = Vec::new();
    (&file).take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(invalid("kernel control exceeds observation bound"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| invalid("kernel control is not UTF-8"))
}

fn filesystem(file: &File, expected: libc::c_long) -> Result<()> {
    // SAFETY: fstatfs writes a correctly allocated statfs for an owned fd.
    let mut value = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(file.as_raw_fd(), value.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let value = unsafe { value.assume_init() };
    if value.f_type != expected {
        return Err(invalid(
            "control path is not the expected real kernel filesystem",
        ));
    }
    Ok(())
}

fn absolute_path(value: &str) -> Result<PathBuf> {
    if !value.starts_with('/')
        || value.contains('\0')
        || (value != "/"
            && value
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || matches!(part, "." | "..")))
    {
        return Err(invalid("noncanonical cgroup or mount path"));
    }
    Ok(PathBuf::from(value))
}

fn decode_mount_path(value: &str) -> Result<PathBuf> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            if index + 3 >= bytes.len()
                || !bytes[index + 1..index + 4]
                    .iter()
                    .all(|byte| (b'0'..=b'7').contains(byte))
            {
                return Err(invalid("invalid mountinfo path escape"));
            }
            let number = (bytes[index + 1] - b'0') as u16 * 64
                + (bytes[index + 2] - b'0') as u16 * 8
                + (bytes[index + 3] - b'0') as u16;
            decoded.push(u8::try_from(number).map_err(|_| invalid("invalid mountinfo path byte"))?);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    absolute_path(&String::from_utf8(decoded).map_err(|_| invalid("mount path is not UTF-8"))?)
}

#[derive(Clone)]
struct Mount {
    root: PathBuf,
    point: PathBuf,
    controllers: Vec<String>,
}

fn mounts(raw: &str) -> Result<Vec<Mount>> {
    let mut output = Vec::new();
    for line in raw.lines().filter(|line| !line.is_empty()) {
        let (left, right) = line
            .split_once(" - ")
            .ok_or_else(|| invalid("malformed mountinfo separator"))?;
        let left = left.split_whitespace().collect::<Vec<_>>();
        let right = right.split_whitespace().collect::<Vec<_>>();
        if left.len() < 6 || right.len() < 3 {
            return Err(invalid("malformed mountinfo record"));
        }
        if right[0] != "cgroup" {
            continue;
        }
        let root = decode_mount_path(left[3])?;
        let point = decode_mount_path(left[4])?;
        let controllers = right[2].split(',').map(str::to_string).collect();
        output.push(Mount {
            root,
            point,
            controllers,
        });
    }
    Ok(output)
}

fn memberships(raw: &str) -> Result<BTreeMap<String, PathBuf>> {
    let mut output = BTreeMap::new();
    for line in raw.lines().filter(|line| !line.is_empty()) {
        let fields = line.splitn(3, ':').collect::<Vec<_>>();
        if fields.len() != 3 || !numeric(fields[0], true) {
            return Err(invalid("malformed proc cgroup record"));
        }
        let path = absolute_path(fields[2])?;
        if fields[1].is_empty() {
            continue;
        } // unified membership does not prove v1 limits
        if fields[0] == "0" {
            return Err(invalid("invalid v1 hierarchy ID"));
        }
        for controller in fields[1].split(',') {
            if controller.is_empty()
                || output
                    .insert(controller.to_string(), path.clone())
                    .is_some()
            {
                return Err(invalid("duplicate cgroup controller membership"));
            }
        }
    }
    Ok(output)
}

fn job_root(path: &Path, job: &str, step: &str, uid: u32) -> Result<PathBuf> {
    if !numeric(job, false) || !(step == "batch" || numeric(step, true)) {
        return Err(invalid("invalid expected job/step"));
    }
    let components = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected_uid = format!("uid_{uid}");
    let expected_job = format!("job_{job}");
    let expected_step = format!("step_{step}");
    let anchors = components
        .iter()
        .enumerate()
        .filter(|(_, part)| part.starts_with("job_"))
        .collect::<Vec<_>>();
    if anchors.len() != 1 {
        return Err(invalid("missing or ambiguous exact Slurm job anchor"));
    }
    let (index, anchor) = anchors[0];
    if *anchor != expected_job
        || index == 0
        || components[index - 1] != expected_uid
        || components.get(index + 1).copied() != Some(expected_step.as_str())
        || components
            .iter()
            .filter(|part| part.starts_with("uid_"))
            .count()
            != 1
        || components
            .iter()
            .filter(|part| part.starts_with("step_"))
            .count()
            != 1
    {
        return Err(invalid("cgroup does not match exact Slurm UID/job/step"));
    }
    let mut root = PathBuf::from("/");
    for part in &components[..=index] {
        root.push(part);
    }
    Ok(root)
}

fn controller(
    name: &str,
    membership: &BTreeMap<String, PathBuf>,
    mounts: &[Mount],
    job: &str,
    step: &str,
    uid: u32,
) -> Result<ControllerEvidence> {
    let path = membership
        .get(name)
        .ok_or_else(|| invalid(format!("missing required v1 {name} controller")))?;
    let root = job_root(path, job, step, uid)?;
    let candidates = mounts
        .iter()
        .filter(|mount| {
            mount.controllers.iter().any(|item| item == name) && root.starts_with(&mount.root)
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err(invalid(format!(
            "missing or ambiguous mounted {name} hierarchy exposing job root"
        )));
    }
    let mount = candidates[0];
    let mut logical = path
        .ancestors()
        .take_while(|level| level.starts_with(&root))
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    logical.reverse();
    if logical.is_empty() || logical.len() > MAX_LEVELS {
        return Err(invalid("invalid cgroup job hierarchy depth"));
    }
    let levels = logical
        .into_iter()
        .map(|level| {
            let relative = level
                .strip_prefix(&mount.root)
                .map_err(|_| invalid("mount root does not contain job hierarchy"))?;
            Ok(LevelEvidence {
                directory: mount.point.join(relative),
                logical_path: level,
                raw: BTreeMap::new(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ControllerEvidence {
        controller: name.into(),
        membership: path.clone(),
        mount_root: mount.root.clone(),
        mount_point: mount.point.clone(),
        job_root: root,
        levels,
    })
}

#[derive(Clone, Debug)]
struct CpuSet(Vec<(u32, u32)>);
impl CpuSet {
    fn parse(raw: &str) -> Result<Self> {
        let mut intervals = raw
            .trim()
            .split(',')
            .map(|part| {
                let (start, end) = part.split_once('-').unwrap_or((part, part));
                let start = start
                    .parse::<u32>()
                    .map_err(|_| invalid("invalid cpuset CPU number"))?;
                let end = end
                    .parse::<u32>()
                    .map_err(|_| invalid("invalid cpuset CPU range"))?;
                if start > end {
                    return Err(invalid("descending cpuset CPU range"));
                }
                Ok((start, end))
            })
            .collect::<Result<Vec<_>>>()?;
        intervals.sort_unstable();
        if intervals.windows(2).any(|pair| pair[0].1 >= pair[1].0) {
            return Err(invalid("duplicate or overlapping cpuset CPUs"));
        }
        Ok(Self(intervals))
    }
    fn intersection(&self, other: &Self) -> Self {
        let mut result = Vec::new();
        let (mut left, mut right) = (0, 0);
        while left < self.0.len() && right < other.0.len() {
            let (a, b) = self.0[left];
            let (c, d) = other.0[right];
            if a.max(c) <= b.min(d) {
                result.push((a.max(c), b.min(d)));
            }
            if b < d {
                left += 1;
            } else {
                right += 1;
            }
        }
        Self(result)
    }
    fn count(&self) -> u64 {
        self.0
            .iter()
            .map(|(start, end)| u64::from(*end) - u64::from(*start) + 1)
            .sum()
    }
    fn display(&self) -> String {
        self.0
            .iter()
            .map(|(a, b)| {
                if a == b {
                    a.to_string()
                } else {
                    format!("{a}-{b}")
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn positive(raw: &str, label: &str) -> Result<u64> {
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid(format!("invalid positive {label}")))
}

fn finite_memory(raw: &str, page_size: u64) -> Result<Option<u64>> {
    let value = positive(raw, "memory control")?;
    let unlimited = i64::MAX as u64 / page_size * page_size;
    Ok((value < unlimited).then_some(value))
}

fn rational_at_most_f64(numerator: u64, denominator: u64, declared: f64) -> bool {
    if numerator == 0 || denominator == 0 || !declared.is_finite() || declared <= 0.0 {
        return false;
    }
    let bits = declared.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = (bits & ((1_u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1_u64 << 52 };
    let power = if exponent == 0 {
        -1074
    } else {
        exponent - 1023 - 52
    };
    if power >= 0 {
        if power >= 64 {
            return true;
        }
        let integer = u128::from(mantissa) << power;
        if integer >= u128::from(u64::MAX) {
            return true;
        }
        u128::from(numerator) <= integer * u128::from(denominator)
    } else {
        let shift = (-power) as u32;
        if shift >= 128 {
            return false;
        }
        u128::from(numerator)
            .checked_mul(1_u128 << shift)
            .is_some_and(|left| left <= u128::from(mantissa) * u128::from(denominator))
    }
}

fn observe_inputs<F>(
    job: &str,
    step: &str,
    uid: u32,
    cgroup: &str,
    mountinfo: &str,
    page_size: u64,
    read: &mut F,
) -> Result<SlurmAllocationEvidence>
where
    F: FnMut(&Path) -> Result<Option<String>>,
{
    if !page_size.is_power_of_two() {
        return Err(invalid("invalid page size"));
    }
    let membership = memberships(cgroup)?;
    let mounted = mounts(mountinfo)?;
    let mut controllers = Vec::new();
    let mut snapshots = BTreeMap::<PathBuf, Option<String>>::new();
    let mut get =
        |level: &mut LevelEvidence, name: &str, required: bool| -> Result<Option<String>> {
            let path = level.directory.join(name);
            let value = read(&path)?;
            if required && value.is_none() {
                return Err(invalid(format!(
                    "missing required control {}",
                    path.display()
                )));
            }
            if let Some(previous) = snapshots.insert(path, value.clone())
                && previous != value
            {
                return Err(invalid("inconsistent repeated control observation"));
            }
            level.raw.insert(name.into(), value.clone());
            Ok(value)
        };
    let mut cpu = controller("cpu", &membership, &mounted, job, step, uid)?;
    let mut cap: Option<(u64, u64)> = None;
    for level in &mut cpu.levels {
        let quota = get(level, "cpu.cfs_quota_us", true)?.unwrap();
        let quota = quota
            .trim()
            .parse::<i64>()
            .map_err(|_| invalid("invalid CFS quota"))?;
        let period = positive(
            &get(level, "cpu.cfs_period_us", true)?.unwrap(),
            "CFS period",
        )?;
        if quota == -1 {
            continue;
        }
        if quota <= 0 {
            return Err(invalid("invalid nonpositive CFS quota"));
        }
        let value = (quota as u64, period);
        if cap.is_none_or(|old| {
            u128::from(value.0) * u128::from(old.1) < u128::from(old.0) * u128::from(value.1)
        }) {
            cap = Some(value);
        }
    }
    controllers.push(cpu);
    let mut cpuset = controller("cpuset", &membership, &mounted, job, step, uid)?;
    let mut allowed: Option<CpuSet> = None;
    for level in &mut cpuset.levels {
        let set = CpuSet::parse(&get(level, "cpuset.cpus", true)?.unwrap())?;
        allowed = Some(allowed.map_or(set.clone(), |old| old.intersection(&set)));
    }
    let allowed = allowed.ok_or_else(|| invalid("missing job cpuset"))?;
    let cpus = allowed.count();
    if cpus == 0 {
        return Err(invalid("empty effective job cpuset"));
    }
    if cap.is_none_or(|old| u128::from(cpus) * u128::from(old.1) < u128::from(old.0)) {
        cap = Some((cpus, 1));
    }
    controllers.push(cpuset);
    let (cpu_numerator, cpu_denominator) =
        cap.ok_or_else(|| invalid("unbounded CPU allocation"))?;
    let mut memory = controller("memory", &membership, &mounted, job, step, uid)?;
    let mut ram = None::<u64>;
    let mut memsw = None::<u64>;
    for level in &mut memory.levels {
        if get(level, "memory.use_hierarchy", true)?.unwrap().trim() != "1" {
            return Err(invalid(
                "every job memory ancestor must use hierarchical accounting",
            ));
        }
        if let Some(value) = finite_memory(
            &get(level, "memory.limit_in_bytes", true)?.unwrap(),
            page_size,
        )? {
            ram = Some(ram.map_or(value, |old| old.min(value)));
        }
        if let Some(raw) = get(level, "memory.memsw.limit_in_bytes", false)?
            && let Some(value) = finite_memory(&raw, page_size)?
        {
            memsw = Some(memsw.map_or(value, |old| old.min(value)));
        }
    }
    let ram = ram.ok_or_else(|| invalid("no finite job-specific RAM cap"))?;
    controllers.push(memory);
    let mut pids_cap = None;
    let mut pids_status = "not_proven_missing_controller".to_string();
    if let Some(path) = membership.get("pids") {
        match job_root(path, job, step, uid) {
            Ok(_) => {
                let mut pids = controller("pids", &membership, &mounted, job, step, uid)?;
                for level in &mut pids.levels {
                    let raw = get(level, "pids.max", true)?.unwrap();
                    if raw.trim() != "max" {
                        let value = positive(&raw, "PIDs control")?;
                        pids_cap = Some(pids_cap.map_or(value, |old: u64| old.min(value)));
                    }
                }
                pids_status = if pids_cap.is_some() {
                    "proven_job_specific_limit"
                } else {
                    "not_proven_unlimited"
                }
                .into();
                controllers.push(pids);
            }
            Err(error) => {
                if path.components().any(|part| matches!(part, Component::Normal(value) if value.to_str().is_some_and(|name| name.starts_with("job_") || name.starts_with("step_") || name.starts_with("uid_")))) { return Err(error); }
                pids_status = "not_proven_shared_controller_membership".into();
            }
        }
    }
    for (path, value) in &snapshots {
        if &read(path)? != value {
            return Err(invalid(format!(
                "resource control changed during observation: {}",
                path.display()
            )));
        }
    }
    Ok(SlurmAllocationEvidence {
        schema: "SLURM_ALLOCATION_V1",
        cgroup_version: 1,
        job_id: job.into(),
        step_id: step.into(),
        step_identity_source: "slurm_environment",
        uid,
        pid: 0,
        raw_environment: BTreeMap::new(),
        raw_cgroup: cgroup.into(),
        raw_mountinfo: mountinfo.into(),
        raw_proc_uid: String::new(),
        controllers,
        effective_cpuset: allowed.display(),
        cpuset_cardinality: cpus,
        cpu_cap_numerator: cpu_numerator,
        cpu_cap_denominator: cpu_denominator,
        cpu_cap: cpu_numerator as f64 / cpu_denominator as f64,
        memory_cap_bytes: ram,
        memsw_cap_bytes: memsw,
        pids_cap,
        pids_status,
        validated_request: None,
    })
}

#[cfg(test)]
#[path = "slurm_allocation_tests.rs"]
mod tests;
