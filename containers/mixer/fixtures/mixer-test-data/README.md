# Official MiXeR chr21-22 fixture

The reference files and trait inputs in this directory come from the upstream
`precimed/gsa-mixer` v2.2.1 test data that is excluded from the clean Git
checkout. They are retained as the migration baseline.

`trait1.fit1.json` is the upstream fit1 result for the documented chr21-22
command. The current `fit2.json` baseline was regenerated with the official
image (`localhost/atc/mixer:2.2.1`, source commit
`ea2a445912f83e5767d67372b6075912ed5655d8`), Python 3.10, NumPy 1.23.3, and
SciPy 1.9.1; two repeated runs produced identical `params`. It replaces an
older dirty-checkout result created with NumPy 2.x.
