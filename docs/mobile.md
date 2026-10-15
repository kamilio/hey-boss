# Local phone access

The phone uses the regular Rust web app, served directly from the Mac. Nothing is deployed publicly. The default `hey-boss issue web` binds only to localhost.

Allow LAN access on an explicit list of Wi-Fi names:

```sh
hey-boss issue web --allow-wifi 'Home' --allow-wifi 'Office'
```

Open the printed `http://192.168.…:4781/` address on a phone connected to the same Wi-Fi. If the usual web service already owns port 4781, stop that service normally or use a separate `--port 4782`. Add the same arguments to your service command for persistent access; an empty list keeps it localhost-only. The desktop and phone share the database, project switcher, issues, documents, mindmaps, and attachments.

LAN access requires all of the following on every request:

- The Mac's current Wi-Fi name exactly matches an allowed name.
- Both addresses are private IPv4 addresses on that Wi-Fi interface's subnet.
- The request uses the Mac's Wi-Fi IP and the primary web port, with a matching browser origin.

Public addresses, other subnets, IPv6, forwarded requests, unknown Wi-Fi names, and unavailable Wi-Fi information are denied. Switching networks takes effect on the next request. Localhost remains available. Existing CSRF protection applies to phone writes. This grants access to devices on the approved subnet: Wi-Fi names are not cryptographic identities, and the direct web app does not require phone pairing. Only allow networks whose other devices you trust. Do not forward this port through a router or reverse proxy.

Wi-Fi detection uses macOS CoreWLAN. macOS may require Location permission for the hosting application to reveal the network name. If the OS redacts it, LAN access stays blocked and startup reports the cause; it never falls back to trusting every private network. Other operating systems remain localhost-only.

Local HTTP works for browsing and editing. iPhone Web Push and service-worker installation require trusted HTTPS and are unavailable over this HTTP connection. The retired relay server is restricted to loopback and has no deployment or provisioning workflow. The supervisor no longer publishes phone notifications to it.

Existing local data, including any old `mobile.json` and `mobile-relay.json`, is left intact. Cloud resources from previous installations are not managed by this repository; removing deployment files does not destroy a running remote service.

See [local HTTP service setup](local-http.md) for launchd provisioning. Existing explicitly configured private Tailscale access remains separate and cannot be combined with `--allow-wifi`.
