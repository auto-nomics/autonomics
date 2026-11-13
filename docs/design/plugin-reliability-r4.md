# Plugin reliability and behavior admission (R4)

Implementation lives in the isolated evaluation worktree. Existing frozen R2/R3
sources and binaries are unchanged. The production checkout has separate prompt
architecture work; integrate the changes there only through a reviewed patch.

## Agent interfaces

- `plugin_status(plugin_path)` reads current files, returning content identity,
  active commit/digest, unactivated edits and behavior verification. Lifecycle
  `status` remains compatible. Content identity excludes host `.rsi`, `.git`,
  installation and lifecycle metadata; scripts, fixtures, docs and node parameters
  participate. Symlinks are rejected.
- `plugin_install` accepts optional `expected_development_fingerprint`. Candidate
  snapshots undergo the existing engineering gates again, then behavior checks,
  before source registration. Content changes invalidate admission. Source and
  live registration are restored on failure; restoration failure is explicitly
  `ACTIVATION_INCONSISTENT`. Installations through the same infrastructure are
  serialized. Use the PluginInfra facade, not low-level PluginStore, for admission.
  Synchronous install calls with behavior suites must run on a blocking worker
  when the embedding host is asynchronous; the agent tool already does this.
- `plugin_node_upsert` accepts the native NodeDefinition JSON schema. Existing
  TOML creation and replacement remain supported. Native compilation validates
  the entire manifest before writing it.
- `plugin_r_node_create` wraps an existing relative plugin script: labeled File
  input `input`, outputs results.tsv and warnings.txt, 120 seconds, 1 CPU, 1 GB.
  Its R script must follow AUTONOMICS_INPUT0/AUTONOMICS_OUTPUT0/1 conventions.
  Use upsert for parameters, different ports and explicit resource overrides.
- `plugin_create` supplies a minimal README with an explicit documentation TODO.

## Behavior suites

Trusted hosts call `configure_behavior_suite(plugin, suite)`. Bindings live beside
registry configuration, outside agent-writable development paths. No agent tool
can change these bindings. A first `fixtures/behavior.json` may register author
self-tests; its authority is forcibly `author`, never independent. The binding
persists, so deleting the fixture does not remove the requirement. Hosts can
replace bindings when deliberately updating the acceptance contract. Include old
cases when registering a new suite; all registered cases are required.

Suite JSON: `id`, `authority` (`author` or `independent`), nonempty `cases`.
Each case has unique `id`, node `kind`, labeled text `inputs`, JSON `params`, and
nonempty output `checks`. Each check has `path`; optional `exact`, `contains`,
`expected_tsv`, `group_columns`, `relative_tolerance`, `absolute_tolerance`.
Unquoted TSV supports exact schema/group matching, duplicate rejection, exact NA
and text, finite numeric comparison. Group labels remain strings. Unsupported
quoted TSV should use text comparison or a host-specific contract extension.

Expected values never enter the execution container. The existing native test
runner executes the exact candidate using the pinned image and port contract.
Tests cap node execution at 120 seconds (125-second outer deadline), reject
missing/truncated outputs and retain executions and failures alongside snapshots.
Reports bind suite digest, checker version, candidate content and execution.
Verification labels describe the tested contract, not universal correctness.
No suite permits activation as unverified; failed suites preserve the old source.
Changing a host contract makes the old verification require revalidation.

## Agent workflow

Inspect existing capability descriptions first, reuse when suitable, otherwise
write code. Packaging is optional unless the task explicitly requires evolution.
After editing: query status, install with its fingerprint, confirm no unactivated
changes, then invoke the active node. Installation and task file delivery are
separate obligations. No implicit activation on save or external publication.
Both production get_node_doc and the evaluation adapter attach status to registered-node descriptions; native
run source identities remain the authority for the code actually executed.

## R4 evaluation

Controller logs finish_reason, usage, remaining calls/tokens and length events.
Requests cap at 8192 output tokens; aggregate allowance remains 4096 times the
call budget. At most two explicit length recoveries share the original call and
wall budgets. Truncated tool calls never execute. Normal stop and unknown empty
responses are not retried. Missing usage terminates because budget accounting
cannot be guaranteed. Node indexes sort by address. Existing frozen runs retain
old behavior and grading; a new protocol never repairs or overwrites them.

`native_evolution_eval.py prepare ROOT --experiment choice|evolution` freezes a
new stream (default seed offset 40000). Each plan also freezes independent public development inputs/expected outputs, separate from hidden scoring inputs. Both arms receive them; plugin S2 admission requires both S1 and S2 cases. Choice makes packaging optional; evolution
requires immutable S1/S2 versions and reports development separately. Freeze code
before execution. Full comparative studies require frozen task/repetition/budget
specifications; small interface probes establish feasibility only.
