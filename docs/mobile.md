# Local phone access

The phone uses the regular Rust web app directly. Nothing is deployed publicly. By default, `hey-boss issue web` binds only to localhost.

Allow LAN access while the Mac uses your home's public IPv4 address:

```sh
hey-boss issue web --allow-public-ip YOUR_HOME_PUBLIC_IP
```

Replace the placeholder with your home public IPv4 address. The service never learns or updates this trusted address automatically. Both Wi-Fi bands work when they share that address; Ethernet works too. No macOS Location or Automation permission is needed for this check.

Open the printed `http://192.168.…:4781/` address on your phone. Add the same option to your service command for persistent access. If another service owns port 4781, stop it normally or use `--port 4782`. Desktop and phone share the database and web app.

The service checks `https://api.ipify.org` in the background at startup and every 30 seconds, bypassing HTTP proxies and binding to the current IPv4 route's address. Each check has a 3-second timeout and a bounded response. Requests never wait for an external lookup: they read cached verification and kernel network information. Access starts blocked, failed or mismatched checks clear verification, and successful checks expire after 45 seconds. Local interface/address/subnet changes block access immediately until checked again. An external IP change with identical local network settings can take up to 45 seconds to block.

Every LAN request also requires private IPv4 addresses on the checked interface's subnet, the Mac's exact local IP and primary port as Host, and a matching browser origin. IPv6, public peers, other subnets, and forwarded requests are denied. Event streams recheck before sending data. Localhost remains available; port 80 stays loopback-only. Existing CSRF protection applies to phone writes.

This grants access to devices on the approved subnet. A public IP can be shared by other households through carrier-grade NAT and is not a cryptographic network identity. Only use this on a trusted LAN, and do not forward the port or expose it through a reverse proxy. An ISP address change or VPN can block access; update the configured address deliberately after confirming you are home.

Local HTTP supports browsing and editing. iPhone Web Push and service-worker installation require trusted HTTPS and are unavailable over this HTTP connection. The retired relay server is restricted to loopback and has no deployment workflow. Old local mobile data is left intact; deleting deployment files does not destroy cloud resources.

See [local HTTP service setup](local-http.md) for launchd provisioning. Existing explicit private Tailscale access cannot be combined with `--allow-public-ip`.
