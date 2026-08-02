# Releasing the desktop app

The release pipeline is `.github/workflows/release.yml`.
Pushing a `v*` tag builds installers on GitHub's runners and attaches them
to a **draft** GitHub release; the draft publishes automatically once every
platform job (including the flatpak repackage) succeeds, so the updater
feed never sees a half-uploaded release. `.github/workflows/ci.yml` runs
the same release-profile build on every push to main, so tag-time build
breakage (dependency downgrades, contract drift) is caught early.

## What a tag build produces

| Runner | Artifacts |
|---|---|
| `windows-latest` | NSIS installer (`.exe`) + MSI, `.sig` updater files |
| `ubuntu-22.04` | AppImage, `.deb`, `.rpm`, `.sig` updater files |
| flatpak job | `.flatpak` bundle repackaged from the deb |

Plus `latest.json` — the signed updater feed the installed apps poll — and
`SHA256SUMS.txt`, generated at publish time over every attached asset; the
downloads table gets a SHA-256 column from the same pass.

## Cutting a release

1. Bump the version: the ONLY authoritative field is `[workspace.package]`
   `version` in the root `Cargo.toml` (both crates inherit it and
   `tauri.conf.json` has no version key, so bundles and the in-app display
   follow). The workflow refuses a tag that doesn't match it.
   `app/package.json`'s version is inert.
2. Update the flatpak manifest's deb filename pin
   (`flatpak/io.github.whompratt.tuners.yml`) if building locally — CI
   sed-patches it automatically.
3. Commit, then tag and push:

   ```
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. Watch the `release` workflow under the repo's Actions tab. When all
   jobs finish the release publishes itself with a downloads table
   (including per-file SHA-256) and a `SHA256SUMS.txt` asset.
   Smoke-test the Windows installer on a real machine when the change
   warrants it. Running the installer through VirusTotal before
   announcing a release catches new antivirus false positives before
   users do (v0.1.6 was flagged by Defender's ML heuristic and reported
   to Microsoft as a false positive, 2026-08-03).

A bad build costs nothing while still drafted: delete the draft and the
tag (`git push origin :refs/tags/v0.2.0`), fix, re-tag. After publication
prefer shipping a fix version — installed apps may already have seen the
feed.

## Status / caveats

- **Proven**: the full pipeline (both platforms + flatpak + auto-publish +
  signed updater artifacts) ran green on v0.1.4 (2026-07-28).
- **The updater is live**: `createUpdaterArtifacts` + pubkey are in
  `tauri.conf.json`, `latest.json` is public (repo went public), and
  installs self-update except under flatpak (gated: `/app` is immutable —
  flatpak users update via the bundle/repo). The
  `TAURI_SIGNING_PRIVATE_KEY` secret signs the artifacts; losing the
  keypair (`~/.tauri/tuners.key` on the dev machine) orphans shipped
  installs.
- `flatpak/io.github.whompratt.tuners.release.yml` rebuilds any shipped
  version from the published deb asset (url + sha256) without a
  toolchain — update url + hash together when pointing it at a new
  release.
- **Installed-app data**: a packaged install anchors its data root at the
  OS app-data dir (`TUNERS_DATA` still overrides). A dev machine's live
  data needs a one-time copy there if you switch to the installed build.
- Windows code signing is not set up, so installers will trip SmartScreen
  ("unrecognized app"). Fine for handing builds to friends; a signing
  cert is the fix if that ever matters.
