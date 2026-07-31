## Summary

<!-- Briefly describe what this PR does and why. Link to relevant issues. -->

## Test Plan

<!-- How was this change tested? Include relevant commands, scenarios, or screenshots. -->

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` is clean
- [ ] `cargo fmt --all -- --check` passes

## Checklist

- [ ] Code follows existing patterns and conventions
- [ ] New public APIs have documentation
- [ ] Tests are included for new functionality or bug fixes
- [ ] No `unsafe` code introduced
- [ ] Key invariants (INV-01 through INV-04) are preserved
- [ ] Commit messages follow the `<type>(<scope>): <summary>` convention
