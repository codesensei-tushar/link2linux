
## Install

**Linux desktop, 64-bit x86 or ARM**

Install this version in one line. It picks the .deb on Debian and Ubuntu, the
.rpm on Fedora and openSUSE, and the AppImage elsewhere, for your computer's
architecture:

```sh
curl -fsSL https://codesensei-tushar.github.io/link2linux/install.sh | L2L_VERSION=v{{VERSION}} sh
```

Or download a package below. For x86 PCs: `link2linux_amd64.deb`,
`link2linux.x86_64.rpm`, `link2linux-x86_64.AppImage` (which needs FUSE 2) or
`link2linux.x86_64.flatpak`. For ARM computers: `link2linux_arm64.deb`,
`link2linux.aarch64.rpm`, `link2linux-aarch64.AppImage` or
`link2linux.aarch64.flatpak`. Install a Flatpak bundle with
`flatpak install --user ./link2linux.x86_64.flatpak`; it takes the GNOME
runtime from Flathub.

**Android phone, 8.0 and newer**

Download `link2linux.apk` on the phone and open it. If Android asks, allow your
browser to install apps.

Then open link2linux on the desktop and scan the QR code it shows with the
phone.

## Verify

```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify link2linux_amd64.deb --repo codesensei-tushar/link2linux
```

