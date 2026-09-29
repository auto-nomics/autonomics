---
name: skill-system
description: How to discover and use installed skills in autonomics. Consult before starting multi-step work that an existing skill may already cover, and whenever unsure how to find or load a skill.
tags: [meta, skills]
---

# Using skills

Skills are reusable operational guides bundled by the user, the community,
or the system. Each one condenses a repeated procedure into a short
document you load on demand.

## Discovery

- `skill_list` — every installed skill with its tier (`builtin`, `global`,
  `workspace`) and one-line description.
- `skill_search` — find skills by free-text query over names, tags, and
  descriptions.

## Usage

1. Before starting a multi-step task, search for coverage:
   `skill_search` with the task's domain keywords (e.g. `gwas`,
   `imputation`, `literature`).
2. If a skill matches, fetch it with `skill_get <name>` and read the
   full SKILL.md. Large documents are paginated — pass `offset`/`limit`
   to page through them.
3. Follow the skill's instructions instead of improvising an equivalent
   procedure. The skill encodes corrections the user already paid for.
4. When a skill turns out to be wrong or incomplete, say so in your
   final answer; the user maintains the library.

## Workflows

Skills marked `[workflow]` bundle parameterized DAG templates —
reusable node/edge graphs over the registered node kinds.

1. `skill_workflows <name>` lists the templates and their parameter
   schemas (type, required, default).
2. `skill_run_workflow <skill> <workflow> <params>` instantiates a
   template into the session DAG (nodes created, edges wired). It does
   not run the DAG by default — review with `view_dag`, then `run_dag`.
3. Prefer a bundled workflow over hand-assembling an equivalent DAG:
   it encodes proven port wiring and defaults.

## Growing the library (observations)

When you learn something a future session would otherwise rediscover
the hard way, record it with `skill_observe`: a failure and its fix
(`--kind failure`, anchored with the node kind and error text), a
verified recipe (`recipe`), or a caveat (`caveat`). Repeated anchored
failures cluster deterministically and surface as skill proposals the
user reviews — your observations are the raw material of new skills.
Failing evals feed the same channel automatically.

`skill_evolve` runs one evolution cycle immediately instead of
waiting for the background sweep. It is propose-only by design: it
writes and revises proposals for the user to review, and never
approves them itself.

## When not to use a skill

- The task is a single trivial step no skill covers — proceed directly.
- The skill's preconditions clearly do not hold (wrong data format,
   wrong tool version). State the mismatch rather than forcing it.
