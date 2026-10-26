# Desktop packaging files

Files that the Linux packages install next to the app. `src-tauri/tauri.conf.json`
(`bundle.linux.deb.files` and `bundle.linux.rpm.files`) maps each one to its
installed path. The AppImage can't install system files, so the app shows the
commands below instead.

| File | Installed as | Purpose |
|---|---|---|
| `linux/firewalld/link2linux.xml` | `/usr/lib/firewalld/services/link2linux.xml` | firewalld service: mDNS and UDP 27381 |
| `linux/ufw/link2linux` | `/etc/ufw/applications.d/link2linux` | ufw application profile: UDP 5353 and 27381 |

Both open only what the desktop needs to hear its phone: mDNS
announcements and, on networks that filter multicast, the phone's beacons on
UDP 27381 (`docs/protocol.md`, "Firewalls"). The desktop never accepts
incoming TCP connections; it dials the phone. Neither file opens anything by
itself: the user allows the service once.

```sh
# firewalld (Fedora, openSUSE, and others where its zone blocks mDNS)
sudo firewall-cmd --permanent --add-service=link2linux && sudo firewall-cmd --reload

# ufw (when it is enabled; Ubuntu ships it disabled)
sudo ufw allow link2linux
```
