# Releasing

A release is a version tag. Pushing `vX.Y.Z` runs
[`.github/workflows/release.yml`](../.github/workflows/release.yml), which
builds every download and collects them in a **draft** GitHub release. A
maintainer checks the draft and publishes it. Until then nobody can see or
download it, and the website and `install.sh` keep pointing at the previous
release.

The draft's notes are the version's section of [`CHANGELOG.md`](../CHANGELOG.md),
then the install and verify instructions from
[`.github/release-notes.md`](../.github/release-notes.md), then GitHub's list of
the pull requests merged since the last release.

Every release has exactly these files. The website's download buttons and
`website/install.sh` look them up by name, so the names never change:

| File | Contents |
|---|---|
| `link2linux_amd64.deb`, `link2linux_arm64.deb` | Desktop app for Debian, Ubuntu and derivatives |
| `link2linux.x86_64.rpm`, `link2linux.aarch64.rpm` | Desktop app for Fedora, openSUSE and other RPM systems |
| `link2linux-x86_64.AppImage`, `link2linux-aarch64.AppImage` | Desktop app for any distribution |
| `link2linux.x86_64.flatpak`, `link2linux.aarch64.flatpak` | Desktop app as a Flatpak bundle, built from the `.deb` |
| `link2linux.apk` | Android app, signed with the release key |
| `SHA256SUMS` | SHA-256 of every file above |

Each file also gets a GitHub build attestation, so anyone can check that
it was built from this repository by this workflow:
`gh attestation verify link2linux_amd64.deb --repo codesensei-tushar/link2linux`.

Versions before 1.0 are published as pre-releases.

## One-time setup: the Android signing key

Android only installs an update when it is signed with the same key as the
installed app. **If the key is lost, nobody can update their installed app**:
they would have to uninstall it, which deletes its data. Create the key once
and keep an offline backup.

1. Create the keystore, choosing a strong password:

   ```sh
   keytool -genkeypair -v -keystore link2linux-release.jks -alias link2linux \
     -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=link2linux"
   ```

2. Back up `link2linux-release.jks` and its password somewhere offline, such
   as a password manager plus an encrypted USB drive. Never commit it.

3. Store it as repository secrets. The workflow reads only these. Give
   the keystore's full path: from any other directory `base64` finds no
   file, and the secret is set to an empty value.

   ```sh
   base64 -w0 ~/link2linux-release.jks | gh secret set ANDROID_KEYSTORE_BASE64
   gh secret set ANDROID_KEYSTORE_PASSWORD   # prompts for the value
   gh secret set ANDROID_KEY_ALIAS --body link2linux
   ```

   Don't set `ANDROID_KEY_PASSWORD` for a keystore made this way: keytool
   makes PKCS12 keystores, whose key is protected by the keystore password,
   and the workflow uses that password when the secret is unset. Set it only
   for an older JKS keystore whose key has a password of its own. A wrong
   key password fails signing with `Failed to obtain key with alias ...
   Wrong password?`.

Without these secrets the APK signing job stops with an error, and nothing is
released. A build-only run (below) is the quickest way to check that they
are right: it signs the APK the same way.

## Testing the pipeline: a build-only run

Before tagging, run the workflow by hand: **Actions → Release → Run
workflow** on `main`, or `gh workflow run release.yml`. It builds and signs
every file exactly as a release would, then keeps them, with `SHA256SUMS`, as
the run's `release-files` artifact instead of creating a release. It skips the
version checks, the attestations and the release notes. Download the artifact,
install the `.deb` and the APK, and pair them.

## Each release

1. **Prepare the release** in one PR:
   - bump the versions, all to the same `X.Y.Z`:
     - `Cargo.toml`: `[workspace.package] version` (the desktop app takes its
       version from here);
     - `apps/desktop/package.json`: `version`;
     - `apps/android/app/build.gradle.kts`: `versionName = "X.Y.Z"`, and
       `versionCode` one higher than in the last release (it stays `1` for
       the first one);
   - in `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`,
     drop anything that did not make it, add a new empty `## [Unreleased]`
     above it, and update the links at the bottom.
2. **Merge it** after `make ci` passes, and do a build-only run on the merged
   commit.
3. **Tag the merged commit** on `main` and push the tag:

   ```sh
   git switch main && git pull --ff-only
   git tag -a vX.Y.Z -m "link2linux X.Y.Z"
   git push origin vX.Y.Z
   ```

   The workflow refuses a tag that doesn't match all three versions, or
   that has no `CHANGELOG.md` section.
4. **Check the draft** under Releases once the workflow is green:
   - all 10 files are there: the four desktop formats for each of x86_64
     and ARM64, the APK, and `SHA256SUMS`;
   - the notes read well;
   - the APK installs on a phone;
   - the `.deb` installs with `sudo apt install ./link2linux_amd64.deb`.
5. **Publish it.** The download buttons and `install.sh` pick it up at once.
6. **Point the website at it** in a small PR: replace the previous version
   with `X.Y.Z` in `website/index.html` (the nine download links, the
   `Pre-release vX.Y.Z` release status pill, and the link of the "New" pill
   at the top of the page, whose text you rewrite to name what the release
   brings) and in the Android download link in `website/site.js`. The page's
   script switches the links to the newest release by itself, but the links
   have to work without it: each one downloads its file directly. Do this
   only after publishing, since a draft's files can't be downloaded yet.

If something is wrong, delete the draft and the tag, fix it, and tag again:
`git push origin :refs/tags/vX.Y.Z`.

## Cost

The workflow runs only when a version tag is pushed or someone starts it by
hand, never on pull requests. On this public repository, GitHub's standard
runners are free.
