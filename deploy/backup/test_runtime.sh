#!/bin/sh
# Disposable image-level checks for the backup runtime. Opt in with
# BACKUP_TEST_IMAGE pointing to the locally built backup target.
set -eu

: "${BACKUP_TEST_IMAGE:?set BACKUP_TEST_IMAGE to the locally built backup image}"
docker image inspect "$BACKUP_TEST_IMAGE" >/dev/null

suffix="$(date +%s)-$$"
while docker volume inspect "librepaper-backup-test-$suffix" >/dev/null 2>&1 \
	|| docker volume inspect "librepaper-backup-repo-test-$suffix" >/dev/null 2>&1 \
	|| docker container inspect "librepaper-backup-test-$suffix" >/dev/null 2>&1; do
	suffix="${suffix}x"
done

container="librepaper-backup-test-$suffix"
backup_volume="librepaper-backup-test-$suffix"
repo_volume="librepaper-backup-repo-test-$suffix"
workdir="$(mktemp -d "${TMPDIR:-/tmp}/librepaper-backup-accept.XXXXXX")"
backup_volume_created=0
repo_volume_created=0
container_created=0
preserve_logs=1

fail() {
	echo "FAIL: $*" >&2
	exit 1
}

cleanup() {
	if [ "$preserve_logs" -eq 1 ] && [ "$container_created" -eq 1 ]; then
		docker logs "$container" >"$workdir/container.log" 2>&1 || true
	fi
	if [ "$container_created" -eq 1 ]; then
		docker rm -f -v "$container" >/dev/null 2>&1 || true
	fi
	if [ "$backup_volume_created" -eq 1 ]; then
		docker volume rm "$backup_volume" >/dev/null 2>&1 || true
	fi
	if [ "$repo_volume_created" -eq 1 ]; then
		docker volume rm "$repo_volume" >/dev/null 2>&1 || true
	fi
	if [ "$preserve_logs" -eq 1 ]; then
		failure_dir="${TMPDIR:-/tmp}/librepaper-backup-accept-failed-$suffix"
		mv "$workdir" "$failure_dir"
		echo "Failure artifacts preserved at $failure_dir" >&2
	else
		rm -rf "$workdir"
	fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

wait_for_file() {
	path=$1
	tries=0
	while [ "$tries" -lt 30 ]; do
		if docker exec "$container" sh -c "test -s '$path'" >/dev/null 2>&1; then
			return 0
		fi
		if [ "$(docker inspect --format '{{.State.Running}}' "$container" 2>/dev/null || true)" != true ]; then
			docker logs "$container" >&2 || true
			fail "backup container exited before creating $path"
		fi
		sleep 1
		tries=$((tries + 1))
	done
	docker logs "$container" >&2 || true
	fail "timed out waiting for $path"
}

metric() {
	name=$1
	task=$2
	docker exec "$container" awk -v name="$name" -v task="$task" \
		'$1 ~ ("^" name "\\{task=\"" task "\"\\}$") { print $2 }' \
		/var/backups/librepaper/metrics/librepaper_backup.prom
}

wait_for_metric_change() {
	name=$1
	task=$2
	old=$3
	tries=0
	while [ "$tries" -lt 90 ]; do
		new="$(metric "$name" "$task" 2>/dev/null || true)"
		if [ -n "$new" ] && [ "$new" != "$old" ]; then
			return 0
		fi
		sleep 1
		tries=$((tries + 1))
	done
	fail "timed out waiting for $name for $task to change"
}

docker volume create "$backup_volume" >/dev/null
backup_volume_created=1
docker volume create "$repo_volume" >/dev/null
repo_volume_created=1

# Only the disposable local-repository volume needs ownership setup.
docker run --rm --user 0 \
	--mount "type=volume,source=$repo_volume,target=/var/backups/restic-repository" \
	--entrypoint /bin/sh "$BACKUP_TEST_IMAGE" \
	-c 'chown 10001:65534 /var/backups/restic-repository'

cat >"$workdir/config.toml" <<'TOML'
[global]
restic-binary = "/tmp/librepaper-backup-test-restic"

[resticprofile]
repository = "/var/backups/restic-repository"

[resticprofile.backup]
schedule-lock-wait = "1s"
source = ["/var/backups/librepaper/current"]
run-before = """set -eu; librepaper-backup-status start backup; rm -rf /var/backups/librepaper/current; mkdir -p /var/backups/librepaper/current; printf synthetic-backup-data > /var/backups/librepaper/current/payload; if [ -e /run/librepaper-backup/block ]; then while [ ! -e /run/librepaper-backup/release ]; do sleep 1; done; fi"""

[[resticprofile.backup.send-before]]
method = "HEAD"
url = "http://127.0.0.1:8765/backup/start"

[[resticprofile.backup.send-after]]
method = "HEAD"
url = "http://127.0.0.1:8765/backup/success"

[[resticprofile.backup.send-after-fail]]
method = "HEAD"
url = "http://127.0.0.1:8765/backup/failure"

[resticprofile.check]
schedule-lock-wait = "1s"

[[resticprofile.check.send-before]]
method = "HEAD"
url = "http://127.0.0.1:8765/check/start"

[[resticprofile.check.send-after]]
method = "HEAD"
url = "http://127.0.0.1:8765/check/success"

[[resticprofile.check.send-after-fail]]
method = "HEAD"
url = "http://127.0.0.1:8765/check/failure"
TOML

cat >"$workdir/malformed.toml" <<'TOML'
token = "synthetic-secret-must-not-be-logged"
[resticprofile
TOML

cat >"$workdir/restic-wrapper" <<'SH'
#!/bin/sh
set -eu
case "${BACKUP_TEST_RESTIC_FAIL:-}" in
	forget|check)
		for argument in "$@"; do
			if [ "$argument" = "${BACKUP_TEST_RESTIC_FAIL}" ]; then
				[ "$argument" != forget ] || exit 81
				exit 82
			fi
		done
		;;
esac
real_restic="$(command -v restic)"
exec "$real_restic" "$@"
SH
chmod 0755 "$workdir/restic-wrapper"

cat >"$workdir/receiver.py" <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer


class Receiver(BaseHTTPRequestHandler):
    def do_HEAD(self):
        with open("/var/backups/librepaper/deadman-events.log", "a", encoding="utf-8") as log:
            log.write(self.path + "\n")
        self.send_response(200)
        self.end_headers()

    do_GET = do_HEAD

    def log_message(self, *_args):
        pass


HTTPServer(("127.0.0.1", 8765), Receiver).serve_forever()
PY

# Malformed configuration must fail without echoing values and leave an
# independently visible config-error sentinel.
if docker run --rm \
	--mount "type=volume,source=$backup_volume,target=/var/backups/librepaper" \
	--mount "type=bind,source=$workdir/malformed.toml,target=/etc/librepaper/config.toml,readonly" \
	--entrypoint /usr/local/bin/librepaper-backup-entrypoint \
	"$BACKUP_TEST_IMAGE" >"$workdir/malformed.log" 2>&1; then
	fail "malformed config unexpectedly started"
fi
if grep -F "synthetic-secret-must-not-be-logged" "$workdir/malformed.log" >/dev/null; then
	fail "malformed config leaked a value to logs"
fi
docker run --rm \
	--mount "type=volume,source=$backup_volume,target=/var/backups/librepaper" \
	--entrypoint /bin/sh "$BACKUP_TEST_IMAGE" \
	-c 'grep -F "librepaper_backup_config_error 1" /var/backups/librepaper/metrics/librepaper_backup.prom >/dev/null' \
	|| fail "malformed config did not publish the config-error metric"

health="$(docker image inspect --format '{{if or (not .Config.Healthcheck) (eq (index .Config.Healthcheck.Test 0) "NONE")}}none{{else}}present{{end}}' "$BACKUP_TEST_IMAGE")"
[ "$health" = none ] || fail "backup image inherited a healthcheck"

docker run -d --name "$container" \
	--mount "type=volume,source=$backup_volume,target=/var/backups/librepaper" \
	--mount "type=volume,source=$repo_volume,target=/var/backups/restic-repository" \
	--mount "type=bind,source=$workdir/config.toml,target=/etc/librepaper/config.toml,readonly" \
	--mount "type=bind,source=$workdir/restic-wrapper,target=/tmp/librepaper-backup-test-restic,readonly" \
	--mount "type=bind,source=$workdir/receiver.py,target=/tmp/librepaper-backup-test-receiver.py,readonly" \
	--tmpfs /run/librepaper-backup:rw,uid=10001,gid=65534,mode=0700 \
	--env RESTIC_PASSWORD=synthetic-backup-password \
	--entrypoint /usr/local/bin/librepaper-backup-entrypoint \
	"$BACKUP_TEST_IMAGE" >/dev/null
container_created=1

[ "$(docker exec "$container" id -u)" = 10001 ] || fail "sidecar does not run as UID 10001"
[ "$(docker exec "$container" stat -c '%u:%g:%a' /run/librepaper-backup)" = 10001:65534:700 ] \
	|| fail "scheduler tmpfs is not private and correctly owned"
wait_for_file /run/librepaper-backup/crontab
docker exec "$container" test -s /run/librepaper-backup/crontab \
	|| fail "resticprofile did not create a scheduler file"
[ "$(docker exec "$container" stat -c '%a' /var/backups/librepaper/.status-state.json)" = 600 ] \
	|| fail "persistent status state is not private"
[ "$(docker exec "$container" stat -c '%a' /var/backups/librepaper/metrics/librepaper_backup.prom)" = 644 ] \
	|| fail "node-exporter metrics are not readable"

docker exec -d "$container" python3 /tmp/librepaper-backup-test-receiver.py >/dev/null
sleep 1

docker exec "$container" resticprofile version >"$workdir/version.txt" 2>&1
grep -F "0.33.1" "$workdir/version.txt" >/dev/null \
	|| fail "image does not contain pinned resticprofile 0.33.1"
docker exec "$container" resticprofile -c /etc/resticprofile/profiles.toml \
	-n resticprofile show >"$workdir/merged-profile.txt" 2>&1 \
	|| fail "resticprofile could not show the merged profile"
grep -F "/var/backups/librepaper/current" "$workdir/merged-profile.txt" >/dev/null \
	|| fail "operator source list was not merged"
grep -F "max-unused" "$workdir/merged-profile.txt" >/dev/null \
	|| fail "base retention policy was not included"
if grep -F "/etc/librepaper/config.toml" "$workdir/merged-profile.txt" >/dev/null \
	|| grep -F "/var/lib/librepaper/secrets" "$workdir/merged-profile.txt" >/dev/null; then
	fail "operator source list did not replace the base source list"
fi

docker exec "$container" resticprofile -c /etc/resticprofile/profiles.toml \
	-n resticprofile backup >"$workdir/backup-first.log" 2>&1 \
	|| fail "synthetic backup and after-backup retention failed"
backup_success="$(metric librepaper_backup_job_last_success_timestamp_seconds backup)"
[ "$backup_success" -gt 0 ] || fail "successful backup did not publish whole-job status"
docker exec "$container" grep -F /backup/start /var/backups/librepaper/deadman-events.log >/dev/null \
	|| fail "backup dead-man start hook did not reach local receiver"
docker exec "$container" grep -F /backup/success /var/backups/librepaper/deadman-events.log >/dev/null \
	|| fail "backup dead-man success hook did not reach local receiver"

# Hold a scheduled backup inside its export hook. The competing schedule should
# time out on the shared lock without changing the last-success timestamp.
sleep 2
docker exec "$container" touch /run/librepaper-backup/block
started_before="$(metric librepaper_backup_job_started_timestamp_seconds backup)"
docker exec -d "$container" resticprofile -c /etc/resticprofile/profiles.toml \
	run-schedule backup@resticprofile >/dev/null
wait_for_metric_change librepaper_backup_job_started_timestamp_seconds backup "$started_before"
sleep 2
if docker exec "$container" resticprofile -c /etc/resticprofile/profiles.toml \
	run-schedule backup@resticprofile >"$workdir/lock-timeout.log" 2>&1; then
	fail "second scheduled backup unexpectedly passed the shared lock"
fi
[ "$(metric librepaper_backup_job_last_success_timestamp_seconds backup)" = "$backup_success" ] \
	|| fail "lock timeout changed last successful backup"
docker exec "$container" touch /run/librepaper-backup/release
wait_for_metric_change librepaper_backup_job_last_success_timestamp_seconds backup "$backup_success"
docker exec "$container" rm -f /run/librepaper-backup/block /run/librepaper-backup/release

# A failed repository check or retention command must reach run-after-fail and
# leave the last-success value unchanged.
check_success="$(metric librepaper_backup_job_last_success_timestamp_seconds check)"
if docker exec -e BACKUP_TEST_RESTIC_FAIL=check "$container" \
	resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile check \
	>"$workdir/check-failure.log" 2>&1; then
	fail "synthetic failed repository check unexpectedly succeeded"
fi
[ "$(metric librepaper_backup_job_last_success_timestamp_seconds check)" = "$check_success" ] \
	|| fail "failed repository check published success"
[ "$(metric librepaper_backup_job_last_result_success check)" = 0 ] \
	|| fail "failed repository check did not publish failure"
docker exec "$container" grep -F /check/failure /var/backups/librepaper/deadman-events.log >/dev/null \
	|| fail "check dead-man failure hook did not reach local receiver"

backup_success="$(metric librepaper_backup_job_last_success_timestamp_seconds backup)"
if docker exec -e BACKUP_TEST_RESTIC_FAIL=forget "$container" \
	resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile backup \
	>"$workdir/retention-failure.log" 2>&1; then
	fail "synthetic retention failure unexpectedly succeeded"
fi
[ "$(metric librepaper_backup_job_last_success_timestamp_seconds backup)" = "$backup_success" ] \
	|| fail "failed retention published backup success"
[ "$(metric librepaper_backup_job_last_result_success backup)" = 0 ] \
	|| fail "failed retention did not publish backup failure"
docker exec "$container" grep -F /backup/failure /var/backups/librepaper/deadman-events.log >/dev/null \
	|| fail "backup dead-man failure hook did not reach local receiver"

# Kill the container while a synthetic export is staged. Status history is
# durable; the next run must remove the stale current directory before backup.
sleep 2
docker exec "$container" touch /run/librepaper-backup/block
docker exec "$container" rm -f /run/librepaper-backup/release
started_before="$(metric librepaper_backup_job_started_timestamp_seconds backup)"
docker exec -d "$container" resticprofile -c /etc/resticprofile/profiles.toml \
	-n resticprofile backup >/dev/null
wait_for_metric_change librepaper_backup_job_started_timestamp_seconds backup "$started_before"
wait_for_file /var/backups/librepaper/current/payload
docker kill --signal KILL "$container" >/dev/null
docker start "$container" >/dev/null
wait_for_file /run/librepaper-backup/crontab
docker exec -d "$container" python3 /tmp/librepaper-backup-test-receiver.py >/dev/null
sleep 1
[ "$(metric librepaper_backup_job_last_success_timestamp_seconds backup)" = "$backup_success" ] \
	|| fail "abrupt job termination erased successful backup history"
docker exec "$container" test -f /var/backups/librepaper/current/payload \
	|| fail "killed export did not leave staged data for recovery"
docker exec "$container" resticprofile -c /etc/resticprofile/profiles.toml \
	-n resticprofile backup >"$workdir/backup-recovery.log" 2>&1 \
	|| fail "backup did not recover after abrupt termination"
docker exec "$container" test ! -e /var/backups/librepaper/current/payload \
	|| fail "recovery did not remove stale staged export data"

preserve_logs=0
echo "PASS: backup image startup, scheduler, status hooks, locks, retention/check failures, and recovery"
