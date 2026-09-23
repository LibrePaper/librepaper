# Isolated backup recovery drill

`drill.sh` exercises the existing `librepaper admin backup create` and
`restore` path against an explicitly named synthetic deployment. It seeds
seven days of generated activity so the source contains document updates and
assets, creates a backup, encrypts the bundle with the operator's age public
recipient, decrypts it with the operator-managed identity, restores into a
separate empty PostgreSQL database and data directory, and compares the
source-history signature and asset references. It also checks the database
dump checksum and every backup/restored object length and SHA-256. The backup
and decrypted material are confined to a mode-0700 temporary directory and
removed by a failure-safe trap.

Prerequisites are a release `librepaper` binary, `psql`/`pg_dump`/`pg_restore`,
`age`, `tar`, Node.js, and an age recipient plus its private identity. The
identity is supplied by path and is not copied or printed. The database
connections must be dedicated disposable databases named exactly
`frugal_fixture` and `frugal_restored`; the latter must have no public tables.
The script refuses other names and refuses a populated restore database. Do
not point these variables at deployment databases.

On Linux without PostgreSQL client binaries, the included Docker wrapper can
provide the three clients. It uses host networking and mounts `/tmp` for the
drill's files; it is intended for this local disposable workflow:

```sh
mkdir -p /tmp/frugal-pg-bin
for tool in psql pg_dump pg_restore; do
  ln -sf "$PWD/tools/frugal-recovery/postgres-tool.sh" "/tmp/frugal-pg-bin/$tool"
done
```

From the repository root, for example:

```sh
PATH="/tmp/frugal-pg-bin:$PATH" \
LIBREPAPER_BIN="$PWD/target/release/librepaper" \
LIBREPAPER_SOURCE_URL='postgresql://postgres:frugal-local@127.0.0.1:55439/frugal_fixture' \
LIBREPAPER_RESTORE_URL='postgresql://postgres:frugal-local@127.0.0.1:55439/frugal_restored' \
AGE_RECIPIENT='age1…' AGE_IDENTITY='/operator/managed/identity.txt' \
tools/frugal-recovery/drill.sh
```

Set `FRUGAL_KEEP_DRILL=1` to keep the synthetic source/restored object
directories under the printed `/tmp/librepaper-frugal-recovery.*` path for
follow-up application replay checks. The database contents remain in the two
dedicated fixture databases either way. With the flag set, the script deletes
the decrypted backup staging directory and tar files but retains the
encrypted `.age` bundle and the two data directories. Without it, the whole
temporary directory is removed at exit.

The drill's SQL signature compares document and update counts, total update
payload bytes, an ordered MD5 over every update payload, asset rows and their
keys/digests/lengths, snapshot count, and label count. Matching signatures
show that source-history rows and their payloads were restored byte-for-byte;
the object checks prove the referenced stored objects survived encryption,
decryption, and restore. This synthetic drill is a recovery-path check, not a
measurement of production recovery time, cost, or backup size.

The drill deletes the original plaintext backup before decrypting; restoration
therefore depends on the encrypted copy. A failure in any step exits nonzero.

After a preserved drill, verify source heads and every labeled historical
frontier through the application replay path:

```sh
FRUGAL_SOURCE_DATABASE_URL="$LIBREPAPER_SOURCE_URL" \
FRUGAL_RESTORE_DATABASE_URL="$LIBREPAPER_RESTORE_URL" \
FRUGAL_SOURCE_DATA=/tmp/librepaper-frugal-recovery.XXXXXX/source-data \
FRUGAL_RESTORE_DATA=/tmp/librepaper-frugal-recovery.XXXXXX/restored-data \
cargo test --release -p librepaper --test frugal_restore_verify -- --ignored --nocapture
```

Plaintext source and restored deployments remain readable by the operator.
For production backups, keep private staging on protected storage, copy only
the encrypted bundle off-host, keep recovery identities separately, and test
recovery with those identities. Deleting staging files is not secure erasure
on SSDs or snapshotting filesystems. Restoring a historical backup can restore
data that was subsequently deleted. This drill does not install a backup
scheduler, retention policy, or production key-management system.
