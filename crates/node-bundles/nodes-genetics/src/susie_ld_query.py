#!/usr/bin/env python3
"""Extract signed LD pairs for a SuSiE locus from a gsa-MiXeR panel."""
import argparse
import ctypes
import sys

import numpy as np


def read_bim_rsids(path):
    rsids = []
    with open(path) as handle:
        for line in handle:
            if line.strip():
                rsids.append(line.split()[1])
    return rsids


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine-home", required=True)
    parser.add_argument("--lib", required=True)
    parser.add_argument("--bim-file", required=True)
    parser.add_argument("--ld-file", required=True)
    parser.add_argument("--chrom", required=True)
    parser.add_argument("--trait1-file", required=True)
    parser.add_argument("--r2-min", type=float, default=0.0)
    args = parser.parse_args()

    sys.path.insert(0, args.engine_home)
    from precimed.common.libbgmg import LibBgmg

    chrom = int(args.chrom)
    bim_file = args.bim_file.replace("@", str(chrom))
    ld_file = args.ld_file.replace("@", str(chrom))
    rsids = read_bim_rsids(bim_file)
    rsid_set = set(rsids)
    query = set()
    with open(args.trait1_file) as handle:
        header = handle.readline().rstrip("\r\n").split("\t")
        snp_index = header.index("SNP")
        for line in handle:
            fields = line.rstrip("\r\n").split("\t")
            if fields:
                query.add(fields[snp_index])

    unknown_snps = query - rsid_set
    if unknown_snps:
        print(
            f"SNPs missing from BIM reference: {', '.join(sorted(unknown_snps))}",
            file=sys.stderr,
        )
        return 1

    lib = LibBgmg(lib_name=args.lib)
    lib.init(
        bim_file=bim_file,
        frq_file="",
        chr_labels=[chrom],
        trait1_file=args.trait1_file,
        trait2_file=None,
        exclude="",
        extract="",
        exclude_ranges="",
    )
    lib.set_ld_r2_coo_from_file(chrom, ld_file)
    lib.set_ld_r2_csr(chrom)

    num_snp = lib.num_snp
    num_tag = lib.num_tag
    if len(rsids) != num_snp:
        raise RuntimeError(
            f"BIM has {len(rsids)} variants but libbgmg loaded {num_snp}"
        )

    tag_to_snp = np.zeros(num_tag, dtype=np.int32)
    lib.cdll.bgmg_retrieve_tag_indices(
        lib._context_id, num_tag, tag_to_snp
    )
    for snp in query:
        print(f"#matched\t{snp}")

    count = lib.cdll.bgmg_num_ld_r_chr(lib._context_id, chrom)
    print("id_a\tid_b\tr", file=sys.stderr)
    if count <= 0:
        return

    tag_index = np.zeros(count, dtype=np.int32)
    snp_index = np.zeros(count, dtype=np.int32)
    values = np.zeros(count, dtype=np.float32)
    status = lib.cdll.bgmg_retrieve_ld_r_chr(
        lib._context_id,
        chrom,
        ctypes.c_longlong(count),
        tag_index,
        snp_index,
        values,
    )
    if status != 0:
        raise RuntimeError(lib.get_last_error())

    output = []
    for tag, snp, value in zip(tag_index, snp_index, values):
        id_a = rsids[tag_to_snp[tag]]
        id_b = rsids[snp]
        if id_a not in query or id_b not in query:
            continue
        if float(value) * float(value) < args.r2_min:
            continue
        output.append((id_a, id_b, float(value)))

    for id_a, id_b, value in output:
        print(f"{id_a}\t{id_b}\t{value:.9g}")


if __name__ == "__main__":
    main()
