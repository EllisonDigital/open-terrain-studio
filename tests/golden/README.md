# Golden references

`node_hashes.json` pins the output of every node type: each one is evaluated
with default settings at 65² (inputs fed from a fixed fBm, see
`crates/terrain-nodes/tests/support/mod.rs`) and a hash of the result's bits is
stored here. `cargo test` fails if any node's output changes, since that would
change the terrain in every existing project that uses it.

After a deliberate change, regenerate the file and explain the change in the
pull request:

```sh
OTS_BLESS=1 cargo test -p terrain-nodes --test nodes golden
```

The basis noise functions also pin exact values in
`crates/terrain-nodes/src/noise/basis.rs` (`golden_values_never_change`).
