---
title: "Security recommendations"
---

These are recommendations, not requirements. Following them does not guarantee that a deployment is secure or that its data can never be lost. They make data loss and takeover less likely, and they make recovery possible when something does go wrong.

## Accounts

Every account that controls part of the deployment is a way in.

- Turn on two-factor authentication for your hosting provider, your domain registrar and DNS host, the mail provider for your contact address, and GitHub. Prefer a hardware key where the service offers one.
- Turn on the registrar's transfer lock for every domain.
- Keep recovery codes offline, next to the keys described in [Keys and passwords](advanced.html#keys-and-passwords).
- If several people administer a GitHub organization, require two-factor authentication for all of its members.

## Mail and certificates

- If you publish a contact address on your domain, publish an SPF record that ends in `-all`, and sign outgoing mail with DKIM.
- Publish a DMARC record. Start with `p=quarantine` and a `rua` address, read the reports for a week or two, then move to `p=reject`.

```
_dmarc.paper.example TXT "v=DMARC1; p=quarantine; rua=mailto:admin@paper.example"
```

- If your DNS host supports CAA records, limit who may issue certificates. Caddy uses Let's Encrypt and falls back to ZeroSSL, which issues through Sectigo. Publish these records:

```
paper.example. CAA 0 issue "letsencrypt.org"
paper.example. CAA 0 issue "sectigo.com"
paper.example. CAA 0 issuewild ";"
```

- If your DNS host does not support CAA records, watch Certificate Transparency logs for your domain and its subdomains with a service such as Cert Spotter.
- The kit already sends `Strict-Transport-Security` with `includeSubDomains`. After a few weeks without problems, add `preload` and submit the domain to hstspreload.org.
- With `preload`, every subdomain must stay on HTTPS.

## The server

### SSH

- Allow key logins only. Some cloud images turn on password logins through cloud-init, so set the value explicitly in a file that sorts first. sshd keeps the first value it reads.

```sh
# /etc/ssh/sshd_config.d/00-local.conf sorts before 50-cloud-init.conf
printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\n' | sudo tee /etc/ssh/sshd_config.d/00-local.conf
sudo sshd -t && sudo systemctl restart ssh
sudo sshd -T | grep -E 'passwordauthentication|kbdinteractiveauthentication'   # both "no"
```

- Repeat this on every new or rebuilt server.

### Network firewall

Docker publishes ports past ufw and other host firewalls, so filter at your provider's network firewall instead.

| Priority | Action | Protocol | Source port | Destination port | TCP flags |
|---|---|---|---|---|---|
| 0 | Allow | ICMP | | | |
| 1 | Allow | TCP | | | established |
| 2 | Allow | TCP | | 22 | |
| 3 | Allow | TCP | | 80 | |
| 4 | Allow | TCP | | 443 | |
| 5 | Allow | UDP | 53 | | |
| 6 | Allow | UDP | 123 | | |
| 7 | Allow | UDP | | 41641 | |
| last | Deny | IPv4 | | | |

- A stateless firewall does not track connections. Rule 1 admits replies to connections that the server opens, such as backups, image pulls and certificate renewals.
- Rule 1 matches only packets with the ACK or RST flag, so it never admits a new connection.
- Rules 5 and 6 match the source port. They admit DNS and NTP replies.
- Rule 7 is for Tailscale. Leave it out if you do not use Tailscale.
- Drop rule 2 once SSH runs over Tailscale, as described in [Private access with Tailscale](#private-access-with-tailscale).
- Many provider firewalls filter IPv4 only. Check what listens on the server's IPv6 address.
- Test with a listener, not a port scan. A closed port can look the same with and without the firewall.

```sh
# on the server
timeout 30 python3 -m http.server 18080 --bind 0.0.0.0
# from another machine: this must fail
nc -z -w5 203.0.113.10 18080
```

- A change can take several minutes to apply.
- Run other apps on the same server without publishing host ports. Put them behind the kit's Caddy instead, as described in [Other apps on the same server](advanced.html#other-apps-on-the-same-server).

### Private access with Tailscale

Only you need SSH and the admin console, so they do not have to be open to the whole internet.

- **With a fixed IP address**, you can skip Tailscale. Allow SSH only from that address in the provider firewall. Limit the admin console to it with a `remote_ip` matcher like the one in step 5 below.
- **Without one**, an allow list breaks whenever your address changes. Home connections get new addresses from time to time, and a laptop changes address with every network it joins.

[Tailscale](https://tailscale.com) solves this. It is a hosted service that joins your devices into a private network, called a tailnet, over encrypted WireGuard connections. Each device keeps the same private address in `100.64.0.0/10` on any network. The server joins as one more device. SSH and the admin console then answer only tailnet addresses, while the app and documents stay public.

- Traffic between devices is end-to-end encrypted. Tailscale's coordination servers decide which devices belong to your tailnet, so you trust them with that.
- The free plan covers personal use. [Headscale](https://github.com/juanfont/headscale) is a self-hosted replacement for the coordination server.

The setup takes seven steps.

1. Tag the server in the tailnet policy. Let only your operators reach it, on ports 22 and 443. The server reaches none of your devices.

   ```json
   "groups": { "group:operators": ["you@example.com"] },
   "tagOwners": { "tag:server": ["group:operators"] },
   "grants": [
     { "src": ["group:operators"], "dst": ["tag:server"], "ip": ["tcp:22", "tcp:443"] }
   ]
   ```

   Keep your other rules, but make sure none of them lets `tag:server` reach anything. The default policy allows all traffic, including from the server to your devices.

2. Install Tailscale on the server and join it with the tag. Use plain `tailscale up`, not `--ssh`, so OpenSSH keys keep governing login. Tagged machines do not expire.
3. Turn off Tailscale's own netfilter rules. By default, Tailscale masquerades tailnet traffic that it forwards into a Docker container, so Caddy sees the Docker gateway address instead of the tailnet address.
4. Keep sshd on IPv4 when the provider firewall filters IPv4 only. Ubuntu starts sshd from `ssh.socket`, which ignores `AddressFamily`, so pin the socket instead.

   ```sh
   curl -fsSL https://tailscale.com/install.sh | sh
   sudo tailscale up --hostname paper-server --advertise-tags=tag:server
   sudo tailscale set --netfilter-mode=nodivert
   # sshd on IPv4 only
   sudo mkdir -p /etc/systemd/system/ssh.socket.d
   printf '[Socket]\nListenStream=\nListenStream=0.0.0.0:22\n' | sudo tee /etc/systemd/system/ssh.socket.d/ipv4-only.conf
   sudo systemctl daemon-reload && sudo systemctl restart ssh.socket
   ```

5. Serve the admin console only to tailnet addresses. Put this site block in `caddy/local.d/admin-tailnet.caddy`.

   ```
   admin.paper.example {
   	@outside not remote_ip 100.64.0.0/10
   	respond @outside 403
   	encode zstd gzip
   	header Strict-Transport-Security "max-age=31536000; includeSubDomains"
   	reverse_proxy librepaper:8080 {
   		lb_try_duration 30s
   		lb_try_interval 250ms
   	}
   }
   ```

   Validate the file, then reload Caddy.

   ```sh
   docker compose exec -T caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
   docker compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile
   ```

   If you run [your own Caddy](advanced.html#behind-your-own-caddy) in front of LibrePaper, put this block in that Caddy instead, with the upstream from that section. Its site block for LibrePaper must then leave out the admin name.

6. On your own machines, point the admin name at the server's tailnet address. Add a line such as `100.64.0.10 admin.paper.example` to `/etc/hosts`. A browser that uses DNS over HTTPS may ignore `/etc/hosts` and get a 403 from the public address. In Firefox, add the name to `ExcludedDomains` in the `DNSOverHTTPS` policy.
7. Once SSH works over the tailnet, remove the SSH rule from the provider firewall.

If Tailscale fails on the server, you can still get in through the public address. Practice this once before you need it.

1. Turn off the provider firewall.
2. Wait a few minutes until the change applies.
3. SSH to the server's public address and fix Tailscale.
4. Turn the firewall back on.

### Maintenance

- Turn on unattended security upgrades. Reboot when `/var/run/reboot-required` exists.
- Keep the `librepaper/` directory from the download at mode 0700 (`chmod 700 ~/librepaper`). Its TOML files hold your secrets but must stay readable for the containers, so the directory is what keeps other users out.
- Remove old release images now and then. The recipe is at the end of the [upgrade section](simple.html#upgrade).

## Backups

Backups are your way back from a takeover as well as from a failed disk. [Backups](advanced.html#backups) covers them in full. These parts matter most for security.

- Keep a copy of the restic password and your encryption keys off the server, as described in [Keys and passwords](advanced.html#keys-and-passwords).
- Give the app and the backups separate keys, each limited to its own bucket. See [Bucket keys and versioning](advanced.html#bucket-keys-and-versioning).
- Keep a copy of the backups that the server cannot delete. See [A copy the server cannot delete](advanced.html#a-copy-the-server-cannot-delete).
- Practice a restore on another machine at least once with the [restore drill](advanced.html#restore-drill).

## Upgrades

`./manage upgrade` checks the downloaded kit's attestation with cosign before it runs anything from it, so a replaced download stops the upgrade. The installed helper does the checking, so this protects every upgrade after the first. To check the first download yourself, or a kit you pass with `LIBREPAPER_KIT`, run the same check:

```sh
curl -fsSLO https://github.com/LibrePaper/librepaper/releases/download/vX.Y.Z/librepaper-deploy.tar.gz
curl -fsSLO https://github.com/LibrePaper/librepaper/releases/download/vX.Y.Z/librepaper-deploy.tar.gz.sigstore.json
docker run --rm --user "$(id -u):$(id -g)" --tmpfs /tmp -e HOME=/tmp -v "$PWD:/w:ro" -w /w ghcr.io/sigstore/cosign/cosign@sha256:b29487e48205d875c324c79583e2806d9d269c0fa299e0861bbec023d8430c8b \
  verify-blob-attestation --bundle librepaper-deploy.tar.gz.sigstore.json --type https://slsa.dev/provenance/v1 \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity https://github.com/LibrePaper/librepaper/.github/workflows/publish-docker.yml@refs/heads/main \
  librepaper-deploy.tar.gz
```

- `./manage upgrade` backs up first when backups are configured, and it stops when the backup fails. Do not skip the backup with `--no-backup`.
- Never run `docker compose down -v` on the server. It deletes the database and data volumes.
