#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
fixture_dir="$root/containers/deseq2/fixtures"
counts="$fixture_dir/pasilla_gene_counts.tsv"
metadata="$fixture_dir/pasilla_sample_metadata.tsv"

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && {
  cat >&2 <<'EOF'
Usage: test_deseq2_fixture.sh

Validates the local pasilla fixture without requiring R or Podman.
EOF
  exit 0
}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

need sha256sum
need awk

[[ -f "$counts" ]] || {
  echo "missing fixture: $counts" >&2
  exit 1
}
[[ -f "$metadata" ]] || {
  echo "missing fixture: $metadata" >&2
  exit 1
}

actual_counts_sha=$(sha256sum "$counts" | awk '{print $1}')
actual_metadata_sha=$(sha256sum "$metadata" | awk '{print $1}')
[[ "$actual_counts_sha" == ea0dafbfcc600559644cfe7dd5cc8de809d631eb64ba3089aaa25c2fa0954dad ]] || {
  echo "pasilla counts checksum mismatch: $actual_counts_sha" >&2
  exit 1
}
[[ "$actual_metadata_sha" == a49842eabda1f163a795dfcdc1e4b74ed7b44ceb5cc27f736ecaf8f8de5c3cff ]] || {
  echo "pasilla metadata checksum mismatch: $actual_metadata_sha" >&2
  exit 1
}

header=$(head -n 1 "$counts")
read -r gene_column first_sample _ <<<"$header"
[[ "$gene_column" == gene_id && -n "$first_sample" ]] || {
  echo "counts header must start with gene_id and sample columns" >&2
  exit 1
}

count_samples=$(awk -F '\t' 'NR == 1 {print NF - 1}' "$counts")
metadata_samples=$(awk 'END {print NR - 1}' "$metadata")
[[ "$count_samples" -eq "$metadata_samples" ]] || {
  echo "sample count mismatch: counts=$count_samples metadata=$metadata_samples" >&2
  exit 1
}

paste \
  <(head -n 1 "$counts" | cut -f 2- | tr '\t' '\n') \
  <(tail -n +2 "$metadata" | cut -f 1) |
  awk -F '\t' '
    $1 != $2 {
      printf "sample order mismatch at position %d: counts=%s metadata=%s\n", NR, $1, $2 > "/dev/stderr"
      exit 1
    }
  '

count_lines=$(wc -l <"$counts")
[[ "$count_lines" -eq 14600 ]] || {
  echo "unexpected pasilla count rows: $((count_lines - 1))" >&2
  exit 1
}

awk -F '\t' '
  NR > 1 {
    if ($1 == "") { print "empty gene_id at line " NR > "/dev/stderr"; exit 1 }
    for (i = 2; i <= NF; i++) {
      if ($i !~ /^(0|[1-9][0-9]*)$/) {
        print "non-integer count at line " NR ", column " i > "/dev/stderr"
        exit 1
      }
    }
  }
' "$counts"

awk -F '\t' '
  NR > 1 && seen[$1]++ {
    print "duplicate gene_id: " $1 > "/dev/stderr"
    exit 1
  }
' "$counts"

awk -F '\t' '
  NR > 1 {
    if (NF != 3 || $1 == "" || $2 == "" || $3 == "") {
      print "malformed metadata row at line " NR > "/dev/stderr"
      exit 1
    }
    if (seen[$1]++) {
      print "duplicate sample_id: " $1 > "/dev/stderr"
      exit 1
    }
  }
' "$metadata"

echo "DESeq2 pasilla fixture validated successfully."
