# Releasing the desktop app

The release pipeline is `.github/workflows/release.yml` (plan 010 phase 5).
Pushing a `v*` tag builds installers on GitHub's runners and attaches them
to a **draft** GitHub release — nothing goes public until the draft is
published by hand.

## What a tag build produces

| Runner | Artifacts |
|---|---|
| `windows-latest` | NSIS installer (`.exe`) + MSI |
| `ubuntu-22.04` | AppImage, `.deb`, `.rpm` |

Both jobs: pnpm install in `app/`, `vite build`, then `tauri-action` runs
the bundler and uploads to the draft release. Rust and pnpm caches are
keyed per runner, so the first build is slow (full workspace compile) and
later ones much faster.

## Cutting a release

1. Bump the version. The authoritative one is `app/src-tauri/tauri.conf.json`
   (`"version"`); it names the bundles and feeds the in-app display
   (`getVersion()` in the nav rail). Keep `app/package.json` and the two
   `Cargo.toml`s in step when convenient — nothing breaks if they lag, it's
   just tidier.
2. Commit, then tag and push:

   ```
   git tag v0.2.0
   git push origin v0.2.0
   ```

3. Watch the `release` workflow under the repo's Actions tab (two jobs, one
   per OS).
4. When both finish, a draft release named `tuners v0.2.0` appears under
   Releases with the installers attached. Smoke-test the Windows installer
   on a real machine (install, launch, hook up Data Out), then edit the
   notes and **publish** the draft.

A bad build costs nothing: delete the draft and the tag
(`git push origin :refs/tags/v0.2.0`), fix, re-tag.

## Status / caveats

- **The workflow is untested until the first tag is pushed** (written
  2026-07-27, never run). Expect first-tag debugging: runner deps and
  tauri-action config are the usual suspects.
- **The updater is deliberately not wired.** Installed apps won't
  self-update; users install new versions manually. Wiring it needs:
  `pnpm tauri signer generate` (keep the private key OUT of the repo; add
  it as the `TAURI_SIGNING_PRIVATE_KEY` Actions secret), the public key +
  endpoint in `tauri.conf.json`, the updater plugin in the shell, and
  `tauri-action` then emits `latest.json` alongside the installers. Do
  this once there are testers who'd benefit from auto-updates.
- **Installed-app data**: a packaged install anchors its data root at the
  OS app-data dir (`TUNERS_DATA` still overrides). A dev machine's live
  data needs a one-time copy there if you switch to the installed build.
- Windows code signing is not set up — installers will trip SmartScreen
  ("unrecognized app"). Fine for handing builds to friends; a signing
  cert is the fix if that ever matters.
