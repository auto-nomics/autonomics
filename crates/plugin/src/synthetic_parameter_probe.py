"""Synthetic nuclei only; exercise the pinned analysis script inside its image."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile

from scipy import sparse
from scipy.io import mmwrite

with tempfile.TemporaryDirectory(prefix='parameter-probe-') as temporary:
    root = Path(temporary)
    names = ['A', 'A2', 'B', 'B2', 'Filler', 'mt-X', 'zz-X']
    rows = [[300, 300, 1, 600, 300, 300], [300, 0, 1, 0, 300, 300],
            [0, 300, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
            [100, 100, 0, 100, 100, 100], [0, 0, 0, 0, 700, 0],
            [0, 0, 0, 0, 0, 700]]
    barcodes = ['unique2', 'tie1', 'lowUMI', 'lowGenes', 'mtSentinel', 'customPrefixSentinel']
    inputs = []
    for library in (1, 2):
        matrix = root / ('matrix%d.mtx' % library)
        metadata = root / ('metadata%d.json' % library)
        mmwrite(str(matrix), sparse.coo_matrix(rows, dtype='int64'))
        metadata.write_text(json.dumps({'features': [
            {'id': n, 'name': n, 'feature_type': 'Gene Expression', 'genome': 'synthetic'}
            for n in names], 'barcodes': barcodes}))
        inputs.extend([matrix, metadata])
    markers, sources = root / 'markers.tsv', root / 'sources.tsv'
    markers.write_text('marker\tlabel\nA\tClassA\nA2\tClassA\nB\tClassB\nB2\tClassB\n')
    sources.write_text('program_id\tsource_species\tsource_symbol\thuman_symbol\thuman_entrez\tdirection\tmapping_status\tsource_file\tsource_rule\texpression_available\nSYN\tsynthetic\tA\tA\t1\tup\tmapped\tsynthetic\tsynthetic\tyes\n')
    inputs.extend([markers, sources])
    environment = dict(os.environ)
    for index, path in enumerate(inputs):
        environment['AUTONOMICS_INPUT%d' % index] = str(path)
    script = Path(os.environ['AUTONOMICS_FILES_DIR']) / 'analysis.sh'
    subprocess.run(['sh', str(script)], env=environment, check=True)
    with zipfile.ZipFile(os.environ['AUTONOMICS_OUTPUT0']) as bundle:
        observations = json.loads(bundle.read('barcodes_qc_marker_evidence.json'))
    parameters = {k: os.environ['AUTONOMICS_PARAM_' + k.upper()]
                  for k in ['min_markers', 'min_umi', 'min_genes', 'max_mito_fraction', 'mito_prefix']}
    unique = [o for o in observations if o['source_barcode'] == 'unique2']
    assert len(unique) == 2
    assert all(o['class'] == ('ClassA' if int(parameters['min_markers']) == 2 else 'Unknown') for o in unique)
    if int(parameters['min_umi']) == 999999 or int(parameters['min_genes']) == 999999:
        assert not any(o['qc_pass'] for o in observations)
    else:
        assert all(o['qc_pass'] for o in unique)
        assert not any(o['qc_pass'] for o in observations if o['source_barcode'] in ['lowUMI', 'lowGenes', 'mtSentinel'])
    Path(os.environ['AUTONOMICS_OUTPUT1']).write_text(json.dumps({
        'synthetic_only': True, 'parameters': parameters, 'observations': observations,
        'analysis_sha256': hashlib.sha256(script.read_bytes()).hexdigest(),
        'cgroup': Path('/proc/self/cgroup').read_text(), 'uid': os.getuid()}, indent=2))
