# Releasing

A release is a version tag. Pushing `vX.Y.Z` runs
[`.github/workflows/release.yml`](../.github/workflows/release.yml), which
builds every download and collects them in a **draft** GitHub release. A
maintainer checks the draft and publishes it. Until then nobody can see or
download it, and the website and `install.sh` keep pointing at the previous
release.

Every release has exactly these files. The website's download buttons and
`website/install.sh` look them up by name, so the names never change:

| File | Contents |
|---|---|
| `link2linux_amd64.deb` | Desktop app for Debian, Ubuntu and derivatives (x86_64) |
| `link2linux.x86_64.rpm` | Desktop app for Fedora, openSUSE and other RPM systems |
| `link2linux-x86_64.AppImage` | Desktop app for any distribution |
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

3. Store it as repository secrets. The workflow reads only these four:

   ```sh
   base64 -w0 link2linux-release.jks | gh secret set ANDROID_KEYSTORE_BASE64
   gh secret set ANDROID_KEYSTORE_PASSWORD   # prompts for the value
   gh secret set ANDROID_KEY_ALIAS --body link2linux
   gh secret set ANDROID_KEY_PASSWORD        # same as the keystore password unless you chose another
   ```

Without these secrets the Android job stops with an error, and nothing is
released.

## Each release

1. **Bump the versions** in one PR, all to the same `X.Y.Z`:
   - `Cargo.toml`: `[workspace.package] version`
   - `apps/android/app/build.gradle.kts`: `versionName = "X.Y.Z"`, and
     `versionCode` one higher than in the last release
2. **Merge it** after `make ci` passes.
3. **Tag the merged commit** on `main` and push the tag:

   ```sh
   git switch main && git pull --ff-only
   git tag -a vX.Y.Z -m "link2linux X.Y.Z"
   git push origin vX.Y.Z
   ```

   The workflow refuses to build a tag that doesn't match both versions.
4. **Check the draft** under Releases once the workflow is green:
   - all five files are there;
   - the notes read well;
   - the APK installs on a phone;
   - the `.deb` installs with `sudo apt install ./link2linux_amd64.deb`.
5. **Publish it.** The download buttons and `install.sh` pick it up at once.

If something is wrong, delete the draft and the tag, fix it, and tag again:
`git push origin :refs/tags/vX.Y.Z`.

## Cost

The workflow runs only when a version tag is pushed, never on pull requests.
On this public repository, GitHub's standard runners are free.
