#!/bin/sh
# Hold this CI job's cargo target dir until the job ends.
#
# On the self-hosted runners (ci/self-hosted-runner in georizon_next) each CI
# job kind keeps one target dir on the server's disk, shared by every runner
# slot, so a run starts from the last run's artifacts. Two runs of one job can
# overlap (two PRs labelled minutes apart). Cargo locks the dir for one cargo
# command at a time, but a job runs several, and every job's checkout sits at
# the same path in its own container. A second job building in between would
# leave artifacts newer than this job's sources, which cargo then takes as
# built from them. So the job takes the dir here, before its first cargo
# command, and keeps it to the end; a second job waits in this step.
#
# A background process holds the lock. The runner leaves it running between
# steps and kills it when the job ends, cancelled or not, and the job's
# container goes too, so a lock never outlives its job. gz-ci-sweep takes the
# same lock before it deletes a dir.
#
# The code is georizon_next's tools/ci-lock-target-dir.sh; keep the two alike.
#
# Usage: ci-lock-target-dir.sh    (locks $CARGO_TARGET_DIR)
set -eu

dir=${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is not set}
mkdir -p "$dir"
ready=$(mktemp -u "${RUNNER_TEMP:-/tmp}/target-lock.XXXXXX")
start=$(date +%s)

# shellcheck disable=SC2016 # $1 is the inner shell's.
nohup flock "$dir.lock" sh -c ': >"$1"; exec sleep infinity' sh "$ready" >/dev/null 2>&1 &
holder=$!
while [ ! -e "$ready" ]; do
    if ! kill -0 "$holder" 2>/dev/null; then
        echo "::error::could not lock $dir"
        exit 1
    fi
    sleep 1
done
echo "Holding $dir; waited $(($(date +%s) - start)) s for it."
