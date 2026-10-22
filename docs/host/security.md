---
title: "Security recommendations"
---

These are recommendations, not requirements. Following them does not guarantee that a deployment is secure or that its data can never be lost. They make data loss and takeover less likely, and they make recovery possible when something does go wrong.

## Accounts

Every account that controls part of the deployment is a way in.

- Turn on two-factor authentication for your hosting provider, your domain registrar and DNS host, the mail provider for your contact address, and GitHub. Prefer a hardware key where the service offers one.
- Turn on the registrar's transfer lock for every domain.
- Keep recovery codes offline, next to the keys described in [Keys and passwords](#keys-and-passwords).
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

1. Tag the server in the tailnet policy. Let your own devices reach it on ports 22 and 443 only. The server reaches none of your devices.

   ```json
   "tagOwners": { "tag:server": ["autogroup:admin"] },
   "grants": [
     { "src": ["autogroup:member"], "dst": ["autogroup:member"], "ip": ["*"] },
     { "src": ["autogroup:member"], "dst": ["tag:server"], "ip": ["tcp:22", "tcp:443"] }
   ]
   ```

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

The [backup setup](advanced.html#backups) covers the basics, and this section goes further.

### Keys and passwords

- The restic password is fixed for the life of the repository. Losing it loses every backup.
- Keep the restic password in a password manager, separate from any file it is also rendered into.
- If your secrets are encrypted with SOPS and a PGP key, keep an offline copy of the private key.
- Keep the two-factor recovery codes in the same place.

### An example profile

This `resticprofile.toml` runs hourly, alerts through Healthchecks, and refuses to create a new repository.

```toml
[resticprofile]
repository = "s3:s3.provider.example/your-backups-bucket/restic"
# Fail the run instead of starting an empty repository when the bucket or path is wrong.
initialize = false

[resticprofile.env]
RESTIC_PASSWORD = "..."
AWS_ACCESS_KEY_ID = "..."
AWS_SECRET_ACCESS_KEY = "..."

# A lost server costs at most an hour of edits.
[resticprofile.backup]
schedule = "hourly"

[[resticprofile.backup.send-before]]
url = "https://hc-ping.com/<uuid>/start"
method = "HEAD"
[[resticprofile.backup.send-after]]
url = "https://hc-ping.com/<uuid>"
method = "HEAD"
[[resticprofile.backup.send-after-fail]]
url = "https://hc-ping.com/<uuid>/fail"
method = "HEAD"
```

- The image's defaults keep every snapshot from the last 48 hours, then one a day for 14 days and one a week for 11 weeks. The example does not change these defaults.
- A weekly `restic check` reads 10% of the data.
- With an hourly schedule, set the Healthchecks check to a 1 hour period and a 2 hour grace.
- Every run reads all stored files. Revisit the hourly schedule once the data passes about 20 GiB.
- A stored file that is missing from object storage does not stop the backup. The backup leaves that file out, prints a `WARNING` line in the backup container's log, and lists the file under `missing` in the manifest. Look for those lines now and then.

### Bucket keys

- Give the app and the backups separate buckets and separate keys.
- Limit each key to its own bucket, with only the actions it needs.

```json
{
  "Statement": [{
    "Effect": "Allow",
    "Action": [
      "s3:GetObject", "s3:PutObject", "s3:DeleteObject",
      "s3:ListBucket", "s3:GetBucketLocation",
      "s3:ListBucketMultipartUploads", "s3:ListMultipartUploadParts", "s3:AbortMultipartUpload"
    ],
    "Resource": ["arn:aws:s3:::your-backups-bucket", "arn:aws:s3:::your-backups-bucket/*"]
  }]
}
```

- Turn on versioning for the backups bucket. Add a lifecycle rule that expires noncurrent versions after 30 days, removes expired delete markers, and aborts incomplete uploads after 7 days.

### A copy the server cannot delete

Versioning alone does not stop someone with root on the server, because on some providers `s3:DeleteObject` also deletes old versions.

- Replicate the backups bucket to a second bucket in your provider's object storage. The server's key has no rights on the replica, and it cannot change the replication rule.
- Turn on versioning on both buckets, and replicate delete markers. A delete then reaches the replica only as a delete marker, and the file stays there as an old version.
- Give the replica the same 30-day lifecycle rule. Without delete marker replication, the replica keeps every file forever.
- Check who owns the replica. On some providers, a bucket's owner has full rights whatever its policy says. Keep the owning user without any access keys, and create the read key under a separate user.
- Use a read-only key for checks and restores.

```json
{
  "Statement": [{
    "Effect": "Allow",
    "Action": ["s3:GetObject", "s3:ListBucket", "s3:GetBucketLocation"],
    "Resource": ["arn:aws:s3:::your-backups-replica", "arn:aws:s3:::your-backups-replica/*"]
  }]
}
```

- Replication is asynchronous, and nothing alerts when it stops. Check the replica now and then from your own machine with the read-only key.
- Run these checks with `--no-lock`, because the read-only key cannot write lock files.

```sh
export RESTIC_REPOSITORY=s3:s3.provider.example/your-backups-replica/restic
export RESTIC_PASSWORD=... AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
restic snapshots --no-lock --latest 1   # the newest snapshot should be about an hour old
restic check --no-lock
```

- To restore after damage, copy the replica as it was before the damage, for example with `rclone copy --s3-version-at`. Then restore from that copy.
- An append-only backup key does not work with restic for long runs. Restic deletes its own lock files, and a run that cannot refresh its lock for about 22 minutes cancels itself.
- A replica in the same provider account does not survive a takeover of that account. That is one more reason for the account security in [Accounts](#accounts).

### Practice a restore

Run the [restore drill](advanced.html#restore-drill) on another machine at least once.

## Upgrades

Verify each release kit before you install it.

```sh
gh release download vX.Y.Z --repo LibrePaper/librepaper --pattern librepaper-deploy.tar.gz
gh attestation verify librepaper-deploy.tar.gz --repo LibrePaper/librepaper \
  --signer-workflow LibrePaper/librepaper/.github/workflows/publish-docker.yml
```

- Release kits carry a build provenance attestation from the release workflow.
- `./manage upgrade` backs up first when backups are configured, and it stops when the backup fails. Do not skip the backup with `--no-backup`.
- Never run `docker compose down -v` on the server. It deletes the database and data volumes.
