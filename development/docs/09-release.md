# 09 — Releases

Same flow as Domus. A marker at the **start** of a commit subject on `master` asks for a release.

| Marker | From | To |
|---|---|---|
| `!alpha` | nothing released | `0.0.1-alpha.1` |
| `!alpha` | `0.0.1-alpha.1` | `0.0.1-alpha.2` |
| `!beta` | `0.0.1-alpha.3` | `0.0.1-beta.1` |
| `!stable` | `0.0.1-beta.2` | `0.0.1` |
| `!fix` / `!feat` / `!major` | `1.4.2` | `1.4.3` / `1.5.0` / `2.0.0` |
| `!patch` | `0.0.2` | rebuild `0.0.2` in place |

A scope of platform names limits the build: `!alpha(linux)`, `!fix(windows)`. Any other scope is an ordinary commit scope and builds everything. When one push carries several markers, the strongest wins.

## Steps

1. The marker opens a release PR (`chore/release-vX`): `scripts/set-version.mjs` writes the version into `Cargo.toml` and `Cargo.lock`, `.github/release-targets` records the scope, and a CHANGELOG entry is written. Nothing is built.
2. Merging that PR is the release: `scripts/package-linux.sh` builds `Findex-<v>-x86_64.AppImage` and `findex_<v>_amd64.deb` (logo at every hicolor size), the Linux job installs the AppImage with `scripts/install-linux.sh` and checks the menu entry and logo, the Windows job builds a zip, and `publish` creates the tag and GitHub Release (pre-release for alpha and beta).

Run the workflow by hand: mode `pr` (pick a marker), `release` (retry the version master carries) or `patch`.

## Repository settings

- Settings > Actions > General: allow GitHub Actions to create pull requests, or add a `RELEASE_TOKEN` secret.
- No other secrets are needed (no signing yet; code signing is M8).

## Installing

```
curl -fsSL https://raw.githubusercontent.com/aiyu-ayaan/Switchboard-Rust/master/scripts/install-linux.sh | sh
```

Options: `--channel beta|alpha`, `--version X`, `--from-file PATH`, `--set-default`, `--uninstall` (which first runs `findex --repair`, so folders go back to the previous file manager before the app is deleted).
