# MuSiC deconvolution fixture (residue)

The MuSiC image build tree (Dockerfile, `music_runner.R`, `.dockerignore`,
`test_smoke.sh`, and the original README) moved to the music manifest
plugin at `/mnt/projects/node-plugins/music/`, which is the single source
of truth for the tool.

Only the smoke fixture stays here — `fixtures/` and its generator —
because the still-live Rust test
`crates/node-bundles/nodes-io/tests/music_deconvolution_container.rs`
reads it from this path. That test is deleted together with the
`music_deconvolution_container` wrapper; when it goes, this directory can
move under the plugin (see that plugin's README) or be removed.

Regenerate the fixture with:

```bash
python3 containers/music-deconvolution/generate_fixtures.py
```
